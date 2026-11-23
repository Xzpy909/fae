//! Filter Perceptual Saturation (APPROXIMATION): ps_grid / ps_finish in fastedit/edit.py.
//!
//! Pass A: chain up to the Filter -> `fin` (filter input) and `fmid` (Warmth / Saturation,
//!         rounded), plus per-band sums of `fmid` on the 45-row grid (integers: exact, so the
//!         merged sums don't depend on how the image was split).
//! Reduce: grid means, separable Gaussian with mirrored edges.
//! Pass B: bilinear surround per pixel, push away from it, opacity, Film, LUTs.

use crate::edit::{film_px, filter_fl, pre_filter, round_clip, Bw, Edit};

pub const PS_W: [f64; 9] = [0.331, 0.0256, -0.8911, -0.2478, 3.1506, -0.0857, 0.1625, -0.3969, 0.597];
pub const PS_GH: usize = 45;

/// _ps_reflect: mirror without repeating the edge sample
fn reflect(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let p = 2 * n as isize - 2;
    let i = i.abs() % p;
    (if i >= n as isize { p - i } else { i }) as usize
}

/// Grid size (gh, gw) for a W x H image.
pub fn grid_size(w: usize, h: usize) -> (usize, usize) {
    let gw = ((PS_GH * w) as f64 / h as f64 + 0.5).floor() as usize;
    (PS_GH, gw.max(1))
}

/// Means of `fmid` per grid cell from the merged sums, then the Gaussian (rows, then columns).
fn blurred_grid(sums: &[u64], cnt: &[u64], gh: usize, gw: usize, k: &[f64]) -> Vec<f64> {
    let mut g = vec![0f64; gh * gw * 3];
    for cell in 0..gh * gw {
        let d = if cnt[cell] > 1 { cnt[cell] as f64 } else { 1.0 };
        for c in 0..3 {
            g[cell * 3 + c] = sums[cell * 3 + c] as f64 / d;
        }
    }
    let r = (k.len() as isize - 1) / 2;
    for axis in 0..2 {
        let n = if axis == 0 { gh } else { gw };
        let mut o = vec![0f64; g.len()];
        for t in -r..=r {
            let kt = k[(t + r) as usize];
            for y in 0..gh {
                for x in 0..gw {
                    let (sy, sx) = if axis == 0 { (reflect(y as isize + t, n), x) } else { (y, reflect(x as isize + t, n)) };
                    for c in 0..3 {
                        o[(y * gw + x) * 3 + c] += kt * g[(sy * gw + sx) * 3 + c];
                    }
                }
            }
        }
        g = o;
    }
    g
}

/// Bilinear position on the grid for pixel index i of n: (lower cell, upper cell, weight).
fn axis_taps(n: usize, cells: usize) -> Vec<(usize, usize, f64)> {
    (0..n)
        .map(|i| {
            let g = (((i as f64 + 0.5) * cells as f64) / n as f64 - 0.5).clamp(0.0, (cells - 1) as f64);
            if cells > 1 {
                let i0 = g.floor().min((cells - 2) as f64);
                let i1 = (i0 as usize + 1).min(cells - 1);
                (i0 as usize, i1, g - i0)
            } else {
                (0, 0, 0.0)
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply(src: &[u8], dst: &mut [u8], w: usize, h: usize, ob: usize, e: &Edit, bw: &Bw,
                    luts: &[[u8; 256]; 3], want_hist: bool) -> Option<crate::Hist> {
    let (t, kernel) = bw.ps.expect("ps");
    let (gh, gw) = grid_size(w, h);
    let cx: Vec<usize> = (0..w).map(|x| x * gw / w).collect();
    let row3 = w * 3;

    // pass A: fin | fmid per pixel (6 bytes), grid sums of fmid
    let mut fa = vec![0u8; w * h * 6];
    let parts = crate::map_bands(&mut fa, w * 6, |band, row0| {
        let mut sums = vec![0u64; gh * gw * 3];
        let mut cnt = vec![0u64; gh * gw];
        for (i, out) in band.chunks_exact_mut(w * 6).enumerate() {
            let y = row0 + i;
            let cy = y * gh / h * gw;
            let s = &src[y * row3..(y + 1) * row3];
            for x in 0..w {
                let mut px = [s[x * 3], s[x * 3 + 1], s[x * 3 + 2]];
                if let Some((a, b)) = e.md {
                    px = crate::edit::md_px(px, a, b[y * w + x]);
                }
                let fin = pre_filter(px, e);
                let fl = filter_fl(fin, bw);
                let mid = [round_clip(fl[0]) as u8, round_clip(fl[1]) as u8, round_clip(fl[2]) as u8];
                out[x * 6..x * 6 + 3].copy_from_slice(&fin);
                out[x * 6 + 3..x * 6 + 6].copy_from_slice(&mid);
                let cell = cy + cx[x];
                cnt[cell] += 1;
                for c in 0..3 {
                    sums[cell * 3 + c] += mid[c] as u64;
                }
            }
        }
        (sums, cnt)
    });
    let mut sums = vec![0u64; gh * gw * 3];
    let mut cnt = vec![0u64; gh * gw];
    for (s, c) in &parts {
        sums.iter_mut().zip(s).for_each(|(a, b)| *a += b);
        cnt.iter_mut().zip(c).for_each(|(a, b)| *a += b);
    }
    let g = blurred_grid(&sums, &cnt, gh, gw, kernel);

    // pass B
    let xt = axis_taps(w, gw);
    let yt = axis_taps(h, gh);
    let (op, pw) = (bw.op, PS_W);
    let fa = &fa;
    crate::Hist::merge(crate::map_bands(dst, w * ob, |band, row0| {
        let mut hist = want_hist.then(crate::Hist::default);
        let mut rowv = vec![0f64; gw * 3];
        for (i, out) in band.chunks_exact_mut(w * ob).enumerate() {
            let y = row0 + i;
            let (y0, y1, fy) = yt[y];
            for xc in 0..gw {
                for c in 0..3 {
                    rowv[xc * 3 + c] = g[(y0 * gw + xc) * 3 + c] * (1.0 - fy) + g[(y1 * gw + xc) * 3 + c] * fy;
                }
            }
            let a6 = &fa[y * w * 6..(y + 1) * w * 6];
            for x in 0..w {
                let (x0, x1, fx) = xt[x];
                let mut sm = [0f64; 3];
                for c in 0..3 {
                    sm[c] = rowv[x0 * 3 + c] * (1.0 - fx) + rowv[x1 * 3 + c] * fx;
                }
                let p = &a6[x * 6..x * 6 + 6];
                let f = [p[3] as f64, p[4] as f64, p[5] as f64];
                let yy = (f[0] + 2.0 * f[1] + f[2]) / 4.0;
                let ys = (sm[0] + 2.0 * sm[1] + sm[2]) / 4.0;
                let cc = [f[0] - yy, f[1] - yy, f[2] - yy];
                let d = [sm[0] - ys, sm[1] - ys, sm[2] - ys];
                let m = yy / 255.0 - 0.5;
                let nn = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / 128.0;
                let nc = (cc[0] * cc[0] + cc[1] * cc[1] + cc[2] * cc[2]).sqrt() / 128.0;
                let ka = pw[0] + pw[1] * m + pw[6] * nn + pw[7] * nc;
                let kb = pw[2] + pw[3] * m + pw[4] * m * m + pw[8] * nc;
                let ee = pw[5] * (yy - ys);
                let mut idx = [0u8; 3];
                for c in 0..3 {
                    let q = round_clip(f[c] + t * (cc[c] * ka + d[c] * kb + ee));
                    let a = p[c] as f64;
                    idx[c] = round_clip(a + (q - a) * op) as u8;
                }
                if let Some(fm) = &e.film {
                    idx = film_px(idx, fm);
                }
                crate::write_px(&mut out[x * ob..x * ob + ob], idx, luts, &mut hist);
            }
        }
        hist
    }))
}

/// Perceptual Saturation as a chain step, in place on the RGB image `buf` (w x h): the two passes of
/// `apply`, with the image as it arrives at this step as the filter input (`fin`); Film and the
/// curves are steps of their own in the chain.
pub(crate) fn step(buf: &mut [u8], w: usize, h: usize, bw: &Bw) {
    let (t, kernel) = bw.ps.expect("ps");
    let (gh, gw) = grid_size(w, h);
    let cx: Vec<usize> = (0..w).map(|x| x * gw / w).collect();
    let row3 = w * 3;

    // pass A: fmid per pixel (Warmth / Saturation, rounded), grid sums of fmid
    let mut fm = vec![0u8; w * h * 3];
    let src: &[u8] = buf;
    let parts = crate::map_bands(&mut fm, row3, |band, row0| {
        let mut sums = vec![0u64; gh * gw * 3];
        let mut cnt = vec![0u64; gh * gw];
        for (i, out) in band.chunks_exact_mut(row3).enumerate() {
            let y = row0 + i;
            let cy = y * gh / h * gw;
            let s = &src[y * row3..(y + 1) * row3];
            for x in 0..w {
                let fin = [s[x * 3], s[x * 3 + 1], s[x * 3 + 2]];
                let fl = filter_fl(fin, bw);
                let mid = [round_clip(fl[0]) as u8, round_clip(fl[1]) as u8, round_clip(fl[2]) as u8];
                out[x * 3..x * 3 + 3].copy_from_slice(&mid);
                let cell = cy + cx[x];
                cnt[cell] += 1;
                for c in 0..3 {
                    sums[cell * 3 + c] += mid[c] as u64;
                }
            }
        }
        (sums, cnt)
    });
    let mut sums = vec![0u64; gh * gw * 3];
    let mut cnt = vec![0u64; gh * gw];
    for (s, c) in &parts {
        sums.iter_mut().zip(s).for_each(|(a, b)| *a += b);
        cnt.iter_mut().zip(c).for_each(|(a, b)| *a += b);
    }
    let g = blurred_grid(&sums, &cnt, gh, gw, kernel);

    // pass B, in place
    let xt = axis_taps(w, gw);
    let yt = axis_taps(h, gh);
    let (op, pw) = (bw.op, PS_W);
    let fm = &fm;
    crate::for_each_band(buf, row3, |band, row0| {
        let mut rowv = vec![0f64; gw * 3];
        for (i, out) in band.chunks_exact_mut(row3).enumerate() {
            let y = row0 + i;
            let (y0, y1, fy) = yt[y];
            for xc in 0..gw {
                for c in 0..3 {
                    rowv[xc * 3 + c] = g[(y0 * gw + xc) * 3 + c] * (1.0 - fy) + g[(y1 * gw + xc) * 3 + c] * fy;
                }
            }
            let m3 = &fm[y * row3..(y + 1) * row3];
            for x in 0..w {
                let (x0, x1, fx) = xt[x];
                let mut sm = [0f64; 3];
                for c in 0..3 {
                    sm[c] = rowv[x0 * 3 + c] * (1.0 - fx) + rowv[x1 * 3 + c] * fx;
                }
                let f = [m3[x * 3] as f64, m3[x * 3 + 1] as f64, m3[x * 3 + 2] as f64];
                let yy = (f[0] + 2.0 * f[1] + f[2]) / 4.0;
                let ys = (sm[0] + 2.0 * sm[1] + sm[2]) / 4.0;
                let cc = [f[0] - yy, f[1] - yy, f[2] - yy];
                let d = [sm[0] - ys, sm[1] - ys, sm[2] - ys];
                let m = yy / 255.0 - 0.5;
                let nn = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / 128.0;
                let nc = (cc[0] * cc[0] + cc[1] * cc[1] + cc[2] * cc[2]).sqrt() / 128.0;
                let ka = pw[0] + pw[1] * m + pw[6] * nn + pw[7] * nc;
                let kb = pw[2] + pw[3] * m + pw[4] * m * m + pw[8] * nc;
                let ee = pw[5] * (yy - ys);
                let p = &mut out[x * 3..x * 3 + 3];
                for c in 0..3 {
                    let q = round_clip(f[c] + t * (cc[c] * ka + d[c] * kb + ee));
                    let a = p[c] as f64;
                    p[c] = round_clip(a + (q - a) * op) as u8;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflect_matches_numpy() {
        // _ps_reflect(np.arange(-3, 8), 5) -> [3 2 1 0 1 2 3 4 3 2 1]
        let got: Vec<usize> = (-3..8).map(|i| reflect(i, 5)).collect();
        assert_eq!(got, [3, 2, 1, 0, 1, 2, 3, 4, 3, 2, 1]);
        assert_eq!(reflect(-4, 1), 0);
    }

    #[test]
    fn grid_size_rounds() {
        assert_eq!(grid_size(640, 427), (45, 67)); // 45*640/427 = 67.45
        assert_eq!(grid_size(3, 1000), (45, 1));
    }
}
