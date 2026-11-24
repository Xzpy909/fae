//! Colour balance (wave B3), as fastedit/colour.py: Percentile White-Patch and Gray-Edge in linear sRGB,
//! then the soft highlight protection and back to sRGB.
//!
//! Both end in a per-channel gain on linear values, so the output of a pixel's channel depends only on
//! its 8-bit input level: the result is three 256-entry tables. NumPy's float32 maths is followed step by
//! step (the sRGB curves, `np.percentile`'s linear interpolation, the gains) so the tables come out as
//! Python's; the percentiles are taken from level histograms (exact, no sort). Gray-Edge's gains come from
//! a Gaussian blur and Sobel gradients as OpenCV computes them (float32, reflect-101 borders, OpenCV's
//! Gaussian kernel); the image's own pixels never go through the blur, only the three gains do.

use crate::px::{Px, View};
use rayon::prelude::*;

/// colour.py `_srgb_to_linear` for every 8-bit level (float32 throughout, as NumPy).
pub fn srgb_to_linear_lut() -> [f32; 256] {
    let mut t = [0f32; 256];
    for (v, o) in t.iter_mut().enumerate() {
        let s = v as f32 / 255.0f32;
        *o = if s <= 0.04045f32 { s / 12.92f32 } else { ((s + 0.055f32) / 1.055f32).powf(2.4f32) };
    }
    t
}

/// colour.py `_linear_to_srgb8` for one value.
fn linear_to_srgb8(lin: f32) -> u8 {
    let lin = lin.clamp(0.0, 1.0);
    let s = if lin <= 0.0031308f32 {
        lin * 12.92f32
    } else {
        1.055f32 * lin.powf((1.0f64 / 2.4) as f32) - 0.055f32
    };
    (s * 255.0f32 + 0.5f32).clamp(0.0, 255.0) as u8
}

/// `np.percentile(values, pct)` (method "linear", `pct` a Python number) of float32 values given as
/// (value, count), sorted by value; `n` = the total count.
fn percentile(sorted: &[(f32, u64)], n: u64, pct: f64) -> f32 {
    if n == 0 {
        return f32::NAN;
    }
    let q = pct / 100.0;
    let vi = (n - 1) as f64 * q;
    let (prev, next) = if vi >= (n - 1) as f64 {
        (n - 1, n - 1)
    } else if vi < 0.0 {
        (0, 0)
    } else {
        let p = vi.floor() as u64;
        (p, p + 1)
    };
    let gamma = vi - vi.floor();
    let at = |k: u64| {
        let mut acc = 0u64;
        for &(v, c) in sorted {
            acc += c;
            if k < acc {
                return v;
            }
        }
        sorted.last().map(|s| s.0).unwrap_or(f32::NAN)
    };
    let (a, b) = (at(prev), at(next));
    // numpy _lerp: a + (b - a) * t, or b - (b - a) * (1 - t) where t >= 0.5 (float32, t weak)
    let d = b - a;
    if gamma >= 0.5 {
        b - d * ((1.0 - gamma) as f32)
    } else {
        a + d * (gamma as f32)
    }
}

/// Per-channel histograms of the colour channels.
fn histograms(img: View) -> [[u64; 256]; 3] {
    let c = img.c;
    (0..img.h)
        .into_par_iter()
        .fold(
            || [[0u64; 256]; 3],
            |mut h, y| {
                for p in img.row(y).chunks_exact(c) {
                    h[0][p[0] as usize] += 1;
                    h[1][p[1] as usize] += 1;
                    h[2][p[2] as usize] += 1;
                }
                h
            },
        )
        .reduce(
            || [[0u64; 256]; 3],
            |mut a, b| {
                for ch in 0..3 {
                    for v in 0..256 {
                        a[ch][v] += b[ch][v];
                    }
                }
                a
            },
        )
}

/// colour.py `_finish_linear` after `lin * gains`: the highlight protection (the 99.9th percentile of
/// every corrected value), back to sRGB, alpha re-attached. Returns the image mapped through the tables.
fn finish(img: View, hist: &[[u64; 256]; 3], lin: &[f32; 256], gains: [f32; 3]) -> Px {
    let mut vals: Vec<(f32, u64)> = Vec::with_capacity(768);
    for ch in 0..3 {
        for v in 0..256 {
            if hist[ch][v] > 0 {
                vals.push((lin[v] * gains[ch], hist[ch][v]));
            }
        }
    }
    vals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let n = (img.w * img.h * 3) as u64;
    let peak = percentile(&vals, n, 99.9) as f64;
    let scale = if peak > 0.98 { Some((0.98 / peak) as f32) } else { None };
    let mut lut = [[0u8; 256]; 3];
    for ch in 0..3 {
        for v in 0..256 {
            let mut x = lin[v] * gains[ch];
            if let Some(s) = scale {
                x *= s;
            }
            lut[ch][v] = linear_to_srgb8(x);
        }
    }
    let mut out = img.to_px();
    let c = img.c;
    out.data.par_chunks_mut(img.w * c).for_each(|line| {
        for p in line.chunks_exact_mut(c) {
            p[0] = lut[0][p[0] as usize];
            p[1] = lut[1][p[1] as usize];
            p[2] = lut[2][p[2] as usize];
        }
    });
    out
}

/// colour.py `white_patch_enhance`: each channel's `percentile` (linear light) is brought to the
/// brightest one's, by `strength` (0..1).
pub fn white_patch(img: View, percentile_: f64, strength: f64) -> Px {
    let strength = strength.clamp(0.0, 1.0);
    if strength == 0.0 {
        return img.to_px();
    }
    let lin = srgb_to_linear_lut();
    let hist = histograms(img);
    let n = (img.w * img.h) as u64;
    let mut r = [0f32; 3];
    for ch in 0..3 {
        let vals: Vec<(f32, u64)> = (0..256).filter(|&v| hist[ch][v] > 0).map(|v| (lin[v], hist[ch][v])).collect();
        r[ch] = percentile(&vals, n, percentile_).max(1e-6);
    }
    let m = r[0].max(r[1]).max(r[2]);
    let s = strength as f32;
    let gains = r.map(|v| 1.0f32 + s * (m / v - 1.0f32));
    finish(img, &hist, &lin, gains)
}

/// OpenCV `borderInterpolate` with BORDER_REFLECT_101.
#[inline]
fn reflect101(mut p: isize, len: usize) -> usize {
    let n = len as isize;
    if n == 1 {
        return 0;
    }
    while p < 0 || p >= n {
        p = if p < 0 { -p } else { 2 * n - 2 - p };
    }
    p as usize
}

/// OpenCV `getGaussianKernel(n, sigma, CV_32F)` (its bit-exact double version, rounded to float).
pub fn gaussian_kernel(n: usize, sigma: f64) -> Vec<f32> {
    let scale2x = -0.125 / (sigma * sigma);
    let n2 = (n - 1) / 2;
    let mut vals = vec![0f64; n2];
    let mut sum = 0f64;
    let mut x = 1 - n as i64;
    for v in vals.iter_mut() {
        let t = ((x * x) as f64 * scale2x).exp();
        *v = t;
        sum += t;
        x += 2;
    }
    sum *= 2.0;
    sum += 1.0;
    if n % 2 == 0 {
        sum += 1.0;
    }
    let mul = 1.0 / sum;
    let mut k = vec![0f32; n];
    for i in 0..n2 {
        let t = (vals[i] * mul) as f32;
        k[i] = t;
        k[n - 1 - i] = t;
    }
    k[n2] = mul as f32;
    k
}

/// A separable symmetric filter (OpenCV's float path): rows, then columns, reflect-101 borders.
fn blur(src: &[f32], w: usize, h: usize, k: &[f32]) -> Vec<f32> {
    let r = k.len() / 2;
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let row = &src[y * w..(y + 1) * w];
        for (x, o) in out.iter_mut().enumerate() {
            let mut s = k[r] * row[x];
            for j in 1..=r {
                let a = row[reflect101(x as isize - j as isize, w)];
                let b = row[reflect101(x as isize + j as isize, w)];
                s += k[r + j] * (a + b);
            }
            *o = s;
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
        let c = &tmp[y * w..(y + 1) * w];
        for x in 0..w {
            o[x] = k[r] * c[x];
        }
        for j in 1..=r {
            let ya = reflect101(y as isize - j as isize, h);
            let yb = reflect101(y as isize + j as isize, h);
            let (ra, rb) = (&tmp[ya * w..(ya + 1) * w], &tmp[yb * w..(yb + 1) * w]);
            for x in 0..w {
                o[x] += k[r + j] * (ra[x] + rb[x]);
            }
        }
    });
    out
}

/// Σ |∇| (or (Σ |∇|^p)^(1/p)) of one channel: OpenCV's 3 × 3 Sobel, float32, reflect-101.
fn edge_norm(ch: &[f32], w: usize, h: usize, p: i64) -> f64 {
    // row pass: d = x[+1] - x[-1], s = x[-1] + 2 x + x[+1]; column pass the other way round
    let mut d = vec![0f32; w * h];
    let mut s = vec![0f32; w * h];
    d.par_chunks_mut(w).zip(s.par_chunks_mut(w)).enumerate().for_each(|(y, (dr, sr))| {
        let row = &ch[y * w..(y + 1) * w];
        for x in 0..w {
            let a = row[reflect101(x as isize - 1, w)];
            let b = row[reflect101(x as isize + 1, w)];
            dr[x] = b - a;
            sr[x] = row[x] * 2.0 + (a + b);
        }
    });
    (0..h)
        .into_par_iter()
        .map(|y| {
            let (ya, yb) = (reflect101(y as isize - 1, h), reflect101(y as isize + 1, h));
            let mut acc = 0f64;
            for x in 0..w {
                let sx = d[y * w + x] * 2.0 + (d[ya * w + x] + d[yb * w + x]);
                let sy = s[yb * w + x] - s[ya * w + x];
                let m = (sx * sx + sy * sy).sqrt() as f64;
                acc += if p == 1 { m } else { m.powi(p as i32) };
            }
            acc
        })
        .sum()
}

/// colour.py `gray_edge_enhance`: the channels' mean edge strength (Minkowski norm `p`, after a
/// Gaussian of `sigma`) is evened out, by `strength` (0..1).
pub fn gray_edge(img: View, p: i64, sigma: f64, strength: f64) -> Px {
    let strength = strength.clamp(0.0, 1.0);
    if strength == 0.0 {
        return img.to_px();
    }
    let lin = srgb_to_linear_lut();
    let hist = histograms(img);
    let (w, h, c) = (img.w, img.h, img.c);
    let kernel = if sigma > 0.0 {
        let ksize = ((sigma * 4.0).round_ties_even() as i64 | 1) as usize;
        Some(gaussian_kernel(ksize, sigma))
    } else {
        None
    };
    let mut norms = [0f64; 3];
    for (ch, norm) in norms.iter_mut().enumerate() {
        let mut plane = vec![0f32; w * h];
        plane.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
            for (x, px) in img.row(y).chunks_exact(c).enumerate() {
                o[x] = lin[px[ch] as usize];
            }
        });
        if let Some(k) = &kernel {
            plane = blur(&plane, w, h, k);
        }
        let s = edge_norm(&plane, w, h, p);
        *norm = if p == 1 { s } else { s.powf(1.0 / p as f64) };
    }
    let g = norms.map(|v| v.max(1e-6));
    let m = g[0].max(g[1]).max(g[2]);
    let gains = g.map(|v| (1.0 + strength * (m / v - 1.0)) as f32);
    finish(img, &hist, &lin, gains)
}

/// The stage's colour balance step (engine.py `Engine.stage`): White-Patch, then Gray-Edge.
pub fn apply(img: Px, p: &crate::fit::StageParams) -> Px {
    let mut out = img;
    if p.wp_on {
        out = white_patch(out.view(), p.wp_pct, p.wp_strength);
    }
    if p.ge_on {
        out = gray_edge(out.view(), p.ge_p, p.ge_sigma, p.ge_strength);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves() {
        let l = srgb_to_linear_lut();
        assert_eq!(l[0], 0.0);
        assert_eq!(l[255], 1.0);
        for v in 0..256 {
            assert_eq!(linear_to_srgb8(l[v]) as usize, v);
        }
    }

    #[test]
    fn percentile_as_numpy() {
        // np.percentile([1, 2, 3, 4], 50) == 2.5; 99 of 0..100 -> 99.0
        let v: Vec<(f32, u64)> = vec![(1.0, 1), (2.0, 1), (3.0, 1), (4.0, 1)];
        assert_eq!(percentile(&v, 4, 50.0), 2.5);
        let v: Vec<(f32, u64)> = (0..101).map(|i| (i as f32, 1)).collect();
        assert_eq!(percentile(&v, 101, 99.0), 99.0);
        assert_eq!(percentile(&v, 101, 100.0), 100.0);
    }

    #[test]
    fn kernel_sums_to_one() {
        let k = gaussian_kernel(9, 2.0);
        assert_eq!(k.len(), 9);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert_eq!(k[0], k[8]);
    }

    #[test]
    fn neutral_stays() {
        let mut p = Px::new(16, 9, 3);
        for (i, v) in p.data.iter_mut().enumerate() {
            *v = ((i / 3) * 7 % 200) as u8;
        }
        assert_eq!(white_patch(p.view(), 99.0, 0.0), p);
        let g = gray_edge(p.view(), 1, 2.0, 1.0);
        assert_eq!((g.w, g.h, g.c), (16, 9, 3));
    }
}
