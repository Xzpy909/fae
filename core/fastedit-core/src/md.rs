//! Mid/Detail smoothing: self-guided filter of the luma (was md_base in fastedit/edit.py, OpenCV).
//!
//!   m = box(y), v = box(y*y) - m*m, a = v / (v + eps), b = m - a*m, base = box(a)*y + box(b)
//!
//! box = normalized (2r+1)^2 mean with mirrored edges (reflect-101, OpenCV's default border).
//! Sums run in f64 (sliding window), values are stored as f32 like OpenCV's float images.
//! The window is split into bands of BAND rows, each starting its sums fresh, so the result is
//! the same for any number of threads.

pub const MD_RADIUS_1440: f64 = 26.0;
pub const MD_EPS: f32 = 0.01;
const BAND: usize = 128;

/// Window radius for an image `height` pixels high (26 px at 1440p), Python's round (half to even).
pub fn md_radius(height: usize) -> usize {
    ((MD_RADIUS_1440 * height as f64 / 1440.0).round_ties_even() as usize).max(1)
}

/// reflect-101: ... 2 1 | 0 1 2 ... n-1 | n-2 ...
#[inline(always)]
fn refl(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let p = 2 * n as isize - 2;
    let i = i.abs() % p;
    (if i >= n as isize { p - i } else { i }) as usize
}

/// Normalized box filter with radius r, reflect-101 borders.
pub fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    assert_eq!(src.len(), w * h);
    let ri = r as isize;
    // horizontal: sliding sums per row
    let mut hor = vec![0f32; w * h];
    crate::for_each_band_t(&mut hor, w, BAND, |band, row0| {
        for (i, out) in band.chunks_exact_mut(w).enumerate() {
            let row = &src[(row0 + i) * w..(row0 + i + 1) * w];
            let mut s = 0f64;
            for t in -ri..=ri {
                s += row[refl(t, w)] as f64;
            }
            // mirrored indices only near the edges; same sums in the same order as refl() everywhere
            for (x, o) in out.iter_mut().enumerate() {
                *o = s as f32;
                let (add, sub) = if x >= r && x + r + 1 < w {
                    (x + r + 1, x - r)
                } else {
                    let xi = x as isize;
                    (refl(xi + ri + 1, w), refl(xi - ri, w))
                };
                s += row[add] as f64 - row[sub] as f64;
            }
        }
    });
    // vertical: sliding column sums, fresh at the start of every band
    let k = (2 * r + 1) as f64;
    let scale = 1.0 / (k * k);
    let mut out = vec![0f32; w * h];
    crate::for_each_band_t(&mut out, w, BAND, |band, row0| {
        let mut col = vec![0f64; w];
        for t in -ri..=ri {
            let rr = &hor[refl(row0 as isize + t, h) * w..][..w];
            col.iter_mut().zip(rr).for_each(|(c, v)| *c += *v as f64);
        }
        for (i, o) in band.chunks_exact_mut(w).enumerate() {
            let y = (row0 + i) as isize;
            o.iter_mut().zip(&col).for_each(|(o, c)| *o = (c * scale) as f32);
            let add = &hor[refl(y + ri + 1, h) * w..][..w];
            let sub = &hor[refl(y - ri, h) * w..][..w];
            for ((c, a), s) in col.iter_mut().zip(add).zip(sub) {
                *c += *a as f64 - *s as f64;
            }
        }
    });
    out
}

/// The smoothed luma Mid/Detail subtracts: guided filter of `y` (luma 0..1) with itself.
pub fn guided(y: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    guided_eps(y, w, h, r, MD_EPS)
}

/// Self-guided filter with radius `r` and regularisation `eps` (also used by sharp.rs).
pub fn guided_eps(y: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let m = box_mean(y, w, h, r);
    let yy: Vec<f32> = y.iter().map(|v| v * v).collect();
    let mut a = box_mean(&yy, w, h, r);
    drop(yy);
    let mut b = vec![0f32; y.len()];
    for i in 0..y.len() {
        let v = a[i] - m[i] * m[i];
        let ai = v / (v + eps);
        a[i] = ai;
        b[i] = m[i] - ai * m[i];
    }
    drop(m);
    let ma = box_mean(&a, w, h, r);
    let mb = box_mean(&b, w, h, r);
    ma.iter().zip(&mb).zip(y).map(|((a, b), y)| a * y + b).collect()
}

/// Fast guided filter (He & Sun 2015): the coefficients a, b are computed on a copy shrunk by
/// `k` (k x k block means, radius r / k) and scaled back up bilinearly; out = a * y + b at full size.
/// Close to guided_eps for large radii at about 1/k^2 of the box-filter work. Used by sharp.rs.
pub fn guided_fast(y: &[f32], w: usize, h: usize, r: usize, eps: f32, k: usize) -> Vec<f32> {
    if k <= 1 {
        return guided_eps(y, w, h, r, eps);
    }
    let (w2, h2) = (w.div_ceil(k), h.div_ceil(k));
    // block means (partial blocks at the right / bottom edge average what they have)
    let mut sm = vec![0f32; w2 * h2];
    crate::for_each_band_t(&mut sm, w2, 16, |band, r0| {
        for (i, row) in band.chunks_exact_mut(w2).enumerate() {
            let by = r0 + i;
            let (y0, y1) = (by * k, ((by + 1) * k).min(h));
            for (bx, o) in row.iter_mut().enumerate() {
                let (x0, x1) = (bx * k, ((bx + 1) * k).min(w));
                let mut s = 0f32;
                for yy in y0..y1 {
                    s += y[yy * w + x0..yy * w + x1].iter().sum::<f32>();
                }
                *o = s / ((y1 - y0) * (x1 - x0)) as f32;
            }
        }
    });
    let r2 = ((r as f64 / k as f64).round() as usize).max(1);
    let m = box_mean(&sm, w2, h2, r2);
    let yy: Vec<f32> = sm.iter().map(|v| v * v).collect();
    let mut a = box_mean(&yy, w2, h2, r2);
    let mut b = vec![0f32; sm.len()];
    for i in 0..sm.len() {
        let v = a[i] - m[i] * m[i];
        let ai = v / (v + eps);
        a[i] = ai;
        b[i] = m[i] - ai * m[i];
    }
    let ma = box_mean(&a, w2, h2, r2);
    let mb = box_mean(&b, w2, h2, r2);
    // bilinear between block centres, clamped at the borders
    let pos = |p: usize, n2: usize| -> (usize, usize, f32) {
        let f = ((p as f32 + 0.5) / k as f32 - 0.5).max(0.0);
        let i0 = (f as usize).min(n2 - 1);
        let i1 = (i0 + 1).min(n2 - 1);
        (i0, i1, f - i0 as f32)
    };
    let xs: Vec<(usize, usize, f32)> = (0..w).map(|x| pos(x, w2)).collect();
    let mut out = vec![0f32; w * h];
    crate::for_each_band_t(&mut out, w, BAND, |band, row0| {
        for (i, row) in band.chunks_exact_mut(w).enumerate() {
            let yi = row0 + i;
            let (j0, j1, fy) = pos(yi, h2);
            for (x, o) in row.iter_mut().enumerate() {
                let (i0, i1, fx) = xs[x];
                let lerp = |t: &[f32]| {
                    let top = t[j0 * w2 + i0] + (t[j0 * w2 + i1] - t[j0 * w2 + i0]) * fx;
                    let bot = t[j1 * w2 + i0] + (t[j1 * w2 + i1] - t[j1 * w2 + i0]) * fx;
                    top + (bot - top) * fy
                };
                *o = lerp(&ma) * y[yi * w + x] + lerp(&mb);
            }
        }
    });
    out
}

/// Everything Mid/Detail needs from an RGB image: luma, then the guided filter at md_radius(h).
pub fn base(rgb: &[u8], w: usize, h: usize) -> Vec<f32> {
    let y = crate::edit::luma01(rgb);
    guided(&y, w, h, md_radius(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_rounds_like_python() {
        assert_eq!(md_radius(1440), 26);
        assert_eq!(md_radius(720), 13);
        assert_eq!(md_radius(1), 1);
        // 26 * h / 1440 = 12.5 exactly for h = 692.307..: not an integer height, so check a tie:
        // h = 360 -> 6.5 -> round half to even = 6
        assert_eq!(md_radius(360), 6);
    }

    #[test]
    fn box_of_constant_is_constant() {
        let (w, h) = (7, 5);
        let src = vec![0.25f32; w * h];
        for r in [1, 3, 9] {
            assert!(box_mean(&src, w, h, r).iter().all(|v| (v - 0.25).abs() < 1e-7));
        }
    }

    #[test]
    fn box_matches_naive() {
        let (w, h, r) = (13, 300, 4);
        let src: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 251) as f32 / 251.0).collect();
        let got = box_mean(&src, w, h, r);
        let ri = r as isize;
        for y in 0..h {
            for x in 0..w {
                let mut s = 0f64;
                for dy in -ri..=ri {
                    for dx in -ri..=ri {
                        s += src[refl(y as isize + dy, h) * w + refl(x as isize + dx, w)] as f64;
                    }
                }
                let want = s / ((2 * r + 1) * (2 * r + 1)) as f64;
                assert!((got[y * w + x] as f64 - want).abs() < 1e-6, "{x},{y}");
            }
        }
    }
}
