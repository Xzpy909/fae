//! Filter Super Contrast: sc_finish (and sc_luts, sc_contrast, dc_maps) in fastedit/edit.py.
//!
//! Works in place on the edited image (curves not applied yet), then applies the curves:
//!   1. channel histograms -> Correct Color Cast curves (sc_luts) and Correct Contrast (stretch
//!      from channel percentiles)
//!   2. Dynamic Contrast (APPROXIMATION, grid CLAHE): per-cell histograms of R+G+B of the
//!      corrected image -> clipped CDF per cell (dc_maps), Gaussian over cells, bilinear between cells
//!   3. Shadows / Highlights table, Opacity (mixed with the filter's input), curves.
//!
//! All statistics are integer counts merged per band, so the result doesn't depend on threading.

use crate::edit::{round_clip, Sc};

const CC_ALO: [f64; 11] = [0.0, 0.2727, 0.4497, 0.5386, 0.6571, 0.7771, 1.0, 1.0, 1.0, 1.0, 1.0];
const CC_BLO: [f64; 11] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.2556, 0.5307, 0.7731, 0.8910, 1.0];
const CC_AHI: [f64; 11] = [0.0, 0.2118, 0.4149, 0.5963, 0.7711, 0.9293, 1.0, 1.0, 1.0, 1.0, 1.0];
const CC_BHI: [f64; 11] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1951, 0.3982, 0.6014, 0.8045, 1.0];
const CC_K: [f64; 11] = [1.0, 0.9890, 0.9783, 0.9680, 0.9481, 0.9379, 0.9277, 0.9177, 0.8979, 0.8876, 0.8778];

const DC_GW: usize = 12;
const DC_NB: usize = 64;
const DC_CLIP: f64 = 4.0;
const DC_BETA: f64 = 0.40;
const DC_SAT: f64 = 1.2;

/// _cc_tab: table at strength s (0..100), linear between the 10 % steps
fn cc_tab(t: &[f64; 11], s: f64) -> f64 {
    let i = ((s / 10.0).floor() as i64).clamp(0, 9) as usize;
    let f = s / 10.0 - i as f64;
    t[i] + (t[i + 1] - t[i]) * f
}

/// Correct Color Cast curves from the channel means
fn sc_luts(cast: f64, means: [f64; 3]) -> [[u8; 256]; 3] {
    let g = (means[0] + means[1] + means[2]) / 3.0;
    let mut out = [[0u8; 256]; 3];
    for (c, m) in means.iter().enumerate() {
        let k = 4.0 * cast * (g - m);
        for (i, v) in out[c].iter_mut().enumerate() {
            let x = i as f64;
            let t = x / 255.0;
            *v = round_clip(x + k * t * (1.0 - t)) as u8;
        }
    }
    out
}

/// _hist_pct: percentile q of a 256-bin histogram of n samples
fn hist_pct(h: &[u64; 256], n: u64, q: u64) -> f64 {
    let pos = ((n - 1) * q) as f64 / 100.0;
    let i0 = pos.floor() as u64;
    let fr = pos - i0 as f64;
    // searchsorted(cumsum, v, side="right") = number of cumulative sums <= v
    let first_above = |v: u64| {
        let mut cum = 0u64;
        for (i, &c) in h.iter().enumerate() {
            cum += c;
            if cum > v {
                return i;
            }
        }
        256
    };
    let v0 = first_above(i0);
    let v1 = if i0 < n - 1 { first_above(i0 + 1) } else { v0 };
    v0 as f64 + (v1 as f64 - v0 as f64) * fr
}

/// sc_contrast: linear stretch (a, b) of the channel average and the colour gain ka
fn sc_contrast(cc: f64, hs: &[[u64; 256]; 3], n: u64) -> (f64, f64, f64) {
    let pc = |q| [hist_pct(&hs[0], n, q), hist_pct(&hs[1], n, q), hist_pct(&hs[2], n, q)];
    let (p2, p98, p10, p90) = (pc(2), pc(98), pc(10), pc(90));
    let min3 = |v: [f64; 3]| v[0].min(v[1]).min(v[2]);
    let max3 = |v: [f64; 3]| v[0].max(v[1]).max(v[2]);
    let (a_lo, a_hi) = (min3(p2), max3(p98));
    let (b_lo, b_hi) = (min3(p10) + 1.0, max3(p90) - 1.0);
    let lo = a_lo * cc_tab(&CC_ALO, cc) + (b_lo - a_lo) * cc_tab(&CC_BLO, cc);
    let mut hi = 255.0 - (255.0 - a_hi) * cc_tab(&CC_AHI, cc) + (b_hi - a_hi) * cc_tab(&CC_BHI, cc);
    if hi - lo < 1.0 {
        hi = lo + 1.0;
    }
    let a = 255.0 / (hi - lo);
    (a, -lo * a, cc_tab(&CC_K, cc) * a)
}

/// _dc_reflect: mirror that repeats the edge sample
fn dc_reflect(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let p = 2 * n as isize;
    let i = i.rem_euclid(p);
    (if i >= n as isize { p - 1 - i } else { i }) as usize
}

/// dc_maps: per cell, the clipped and redistributed CDF (gh x gw x NB, values 0..255)
fn dc_maps(hist: &[u64], cnt: &[u64], gh: usize, gw: usize, dk: &[f64]) -> Vec<f64> {
    let mut a = vec![0f64; gh * gw * DC_NB];
    for cell in 0..gh * gw {
        let d = if cnt[cell] > 0 { cnt[cell] as f64 } else { 1.0 };
        for b in 0..DC_NB {
            a[cell * DC_NB + b] = hist[cell * DC_NB + b] as f64 / d;
        }
    }
    let r = (dk.len() as isize - 1) / 2;
    for axis in 0..2 {
        let n = if axis == 0 { gh } else { gw };
        let mut o = vec![0f64; a.len()];
        for t in -r..=r {
            let kt = dk[(t + r) as usize];
            for y in 0..gh {
                for x in 0..gw {
                    let (sy, sx) = if axis == 0 { (dc_reflect(y as isize + t, n), x) } else { (y, dc_reflect(x as isize + t, n)) };
                    for b in 0..DC_NB {
                        o[(y * gw + x) * DC_NB + b] += kt * a[(sy * gw + sx) * DC_NB + b];
                    }
                }
            }
        }
        a = o;
    }
    let lim = DC_CLIP / DC_NB as f64;
    let mut mp = vec![0f64; a.len()];
    for cell in 0..gh * gw {
        let v = &a[cell * DC_NB..(cell + 1) * DC_NB];
        let mut ex = 0.0;
        for &x in v {
            if x > lim {
                ex += x - lim;
            }
        }
        let mut cum = 0.0;
        for b in 0..DC_NB {
            let w = (if v[b] > lim { lim } else { v[b] }) + ex / DC_NB as f64;
            cum += w;
            mp[cell * DC_NB + b] = 255.0 * (cum - w / 2.0);
        }
    }
    mp
}

/// Bilinear taps for Dynamic Contrast: f = ((i + 0.5) / n) * cells - 0.5 (note: not the PS order)
fn dc_taps(n: usize, cells: usize) -> Vec<(usize, usize, f64)> {
    (0..n)
        .map(|i| {
            let f = ((i as f64 + 0.5) / n as f64 * cells as f64 - 0.5).clamp(0.0, (cells - 1) as f64);
            let top = if cells > 1 { cells - 2 } else { 0 };
            let i0 = (f.floor() as usize).min(top);
            let i1 = (i0 + 1).min(cells - 1);
            (i0, i1, f - i0 as f64)
        })
        .collect()
}

/// Super Contrast's measurements of the whole image: the Color Cast tables, the Correct Contrast
/// line and the Dynamic Contrast curves per cell. Kept by the preview so a draft of part of the
/// image can apply the same ones (chain.rs).
pub struct ScStats {
    t: [[u8; 256]; 3],
    corr: Option<(f64, f64, f64)>,
    /// (cells high, cells wide, CT: the cell's curve at R + G + B)
    dc: Option<(usize, usize, Vec<f64>)>,
}

/// Color Cast + Correct Contrast for one pixel ("mid")
#[inline(always)]
fn mid_px(st: &ScStats, p: &[u8]) -> [u8; 3] {
    let t = &st.t;
    let m = [t[0][p[0] as usize], t[1][p[1] as usize], t[2][p[2] as usize]];
    match st.corr {
        Some((ca, cb, ck)) => {
            let f = [m[0] as f64, m[1] as f64, m[2] as f64];
            let mm = (f[0] + f[1] + f[2]) / 3.0;
            core::array::from_fn(|c| round_clip(ca * mm + cb + ck * (f[c] - mm)) as u8)
        }
        None => m,
    }
}

/// Passes 1 and 2: the measurements of the whole image (`ob` bytes per pixel).
pub(crate) fn measure(img: &[u8], w: usize, h: usize, ob: usize, sc: &Sc) -> ScStats {
    let row = w * ob;
    let img = img;
    let n = (w * h) as u64;

    // 1. channel histograms of the filter input
    let parts = crate::map_ref_bands(img, row, |band, _| {
        let mut hh = [[0u64; 256]; 3];
        for p in band.chunks_exact(ob) {
            hh[0][p[0] as usize] += 1;
            hh[1][p[1] as usize] += 1;
            hh[2][p[2] as usize] += 1;
        }
        hh
    });
    let mut hist = [[0u64; 256]; 3];
    for part in &parts {
        for c in 0..3 {
            for v in 0..256 {
                hist[c][v] += part[c][v];
            }
        }
    }
    let means: [f64; 3] = core::array::from_fn(|c| {
        let s: u64 = (0..256).map(|v| v as u64 * hist[c][v]).sum();
        s as f64 / n as f64
    });
    let t = if sc.cast > 0.0 { sc_luts(sc.cast, means) } else { crate::edit::identity_luts() };
    let corr = if sc.cc > 0.0 {
        let mut hs = [[0u64; 256]; 3];
        for c in 0..3 {
            for v in 0..256 {
                hs[c][t[c][v] as usize] += hist[c][v];
            }
        }
        Some(sc_contrast(sc.cc, &hs, n))
    } else {
        None
    };
    let mut st = ScStats { t, corr, dc: None };

    // 2. Dynamic Contrast: per-cell histograms of R+G+B of mid, then curves per cell (CT)
    let dc = if sc.dyn_ > 0.0 {
        let gw = DC_GW;
        let gh = (((h * gw) as f64 / w as f64 + 0.5).floor() as usize).max(1);
        let cx: Vec<usize> = (0..w).map(|x| x * gw / w).collect();
        let st = &st;
        let parts = crate::map_ref_bands(img, row, |band, row0| {
            let mut hh = vec![0u64; gh * gw * DC_NB];
            let mut cnt = vec![0u64; gh * gw];
            for (i, r) in band.chunks_exact(row).enumerate() {
                let cy = (row0 + i) * gh / h * gw;
                for x in 0..w {
                    let m = mid_px(st, &r[x * ob..x * ob + 3]);
                    let s = m[0] as usize + m[1] as usize + m[2] as usize;
                    let cell = cy + cx[x];
                    hh[cell * DC_NB + (s / 12).min(DC_NB - 1)] += 1;
                    cnt[cell] += 1;
                }
            }
            (hh, cnt)
        });
        let mut hh = vec![0u64; gh * gw * DC_NB];
        let mut cnt = vec![0u64; gh * gw];
        for (a, b) in &parts {
            hh.iter_mut().zip(a).for_each(|(x, y)| *x += y);
            cnt.iter_mut().zip(b).for_each(|(x, y)| *x += y);
        }
        let mp = dc_maps(&hh, &cnt, gh, gw, sc.dk);
        // CT[cell][S]: the cell's curve at the channel average S / 3, S = R + G + B (0..765)
        let mut ct = vec![0f64; gh * gw * 766];
        for s in 0..766 {
            let ms = s as f64 / 3.0;
            let fb = ((ms + 0.5) * DC_NB as f64 / 256.0 - 0.5).clamp(0.0, (DC_NB - 1) as f64);
            let b0 = (fb.floor() as usize).min(DC_NB - 2);
            let wb = fb - b0 as f64;
            for cell in 0..gh * gw {
                ct[cell * 766 + s] = mp[cell * DC_NB + b0] * (1.0 - wb) + mp[cell * DC_NB + b0 + 1] * wb;
            }
        }
        Some((gh, gw, ct))
    } else {
        None
    };
    st.dc = dc;
    st
}

/// Pass 3, in place on `img` (`w` x `h`, `ob` bytes per pixel), which is the part of the whole image
/// (`fw` x `fh`) whose top-left pixel is (`ox`, `oy`); returns the UI's histograms when asked.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply(img: &mut [u8], w: usize, h: usize, ob: usize, sc: &Sc, st: &ScStats, luts: &[[u8; 256]; 3],
                    want_hist: bool, (ox, oy): (usize, usize), (fw, fh): (usize, usize)) -> Option<crate::Hist> {
    let row = w * ob;
    debug_assert_eq!(img.len(), row * h);
    let dc = st.dc.as_ref().map(|(gh, gw, ct)| (*gh, *gw, ct, dc_taps(fh, *gh), dc_taps(fw, *gw)));
    let gs = 1.0 + sc.dyn_ * (DC_SAT - 1.0);
    let gb = sc.dyn_ * DC_BETA;

    // 3. final pass, in place
    crate::Hist::merge(crate::map_bands(img, row, |band, row0| {
        let mut hist = want_hist.then(crate::Hist::default);
        for (i, r) in band.chunks_exact_mut(row).enumerate() {
            let y = row0 + i;
            for x in 0..w {
                let p = &mut r[x * ob..x * ob + ob];
                let m = mid_px(st, p);
                let mut res = [m[0] as f64, m[1] as f64, m[2] as f64];
                if let Some((_, gw, ct, yt, xt)) = &dc {
                    let (y0, y1, wy) = yt[oy + y];
                    let (x0, x1, wx) = xt[ox + x];
                    let s = m[0] as usize + m[1] as usize + m[2] as usize;
                    let tap = |yy: usize, xx: usize| ct[(yy * gw + xx) * 766 + s];
                    let v = (1.0 - wy) * (1.0 - wx) * tap(y0, x0) + (1.0 - wy) * wx * tap(y0, x1)
                        + wy * (1.0 - wx) * tap(y1, x0) + wy * wx * tap(y1, x1);
                    let mm = s as f64 / 3.0;
                    let m2 = mm + gb * (v - mm);
                    for v in res.iter_mut() {
                        *v = round_clip(m2 + gs * (*v - mm));
                    }
                }
                if let Some(shl) = sc.shl {
                    for v in res.iter_mut() {
                        *v = shl[*v as usize];
                    }
                }
                if sc.op != 1.0 {
                    for c in 0..3 {
                        let xf = p[c] as f64;
                        res[c] = round_clip(xf + (res[c] - xf) * sc.op);
                    }
                }
                crate::write_px(p, [res[0] as u8, res[1] as u8, res[2] as u8], luts, &mut hist);
            }
        }
        hist
    }))
}

/// The whole step on the whole image (`ob` bytes per pixel); returns the measurements (kept by the
/// preview) and the UI's histograms when asked.
pub(crate) fn finish(img: &mut [u8], w: usize, h: usize, ob: usize, sc: &Sc, luts: &[[u8; 256]; 3],
                     want_hist: bool) -> (ScStats, Option<crate::Hist>) {
    let st = measure(img, w, h, ob, sc);
    let hist = apply(img, w, h, ob, sc, &st, luts, want_hist, (0, 0), (w, h));
    (st, hist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_reflect_matches_python() {
        // [_dc_reflect(i, 4) for i in range(-3, 9)] -> [2, 1, 0, 0, 1, 2, 3, 3, 2, 1, 0, 0]
        let got: Vec<usize> = (-3..9).map(|i| dc_reflect(i, 4)).collect();
        assert_eq!(got, [2, 1, 0, 0, 1, 2, 3, 3, 2, 1, 0, 0]);
    }

    #[test]
    fn hist_pct_simple() {
        let mut h = [0u64; 256];
        h[10] = 50;
        h[200] = 50;
        assert_eq!(hist_pct(&h, 100, 2), 10.0);
        assert_eq!(hist_pct(&h, 100, 98), 200.0);
    }
}
