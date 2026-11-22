//! Sharpen tool, the last step of the edit (after the curves, on the output pixels).
//!
//! Structure ≈ (APPROXIMATION, fitted to reference renders at -100 / -50 / 50 / 100, not pixel exact):
//!   detail = 255 * (ST_C1 * (y - guided(y, 1)) + ST_C2 * (y - guided(y, R)))
//!   g      = s * ST_POS_GAIN for s > 0, s for s < 0           (s = slider / 100)
//!   d      = A * tanh(g * detail / A)                          (A = ST_LIM_POS / ST_LIM_NEG)
//!   out_c  = c + d                                             (same offset on R, G, B)
//! y = luma 0..1 of the output, guided = self-guided filter with eps ST_EPS (the coarse one as a
//! fast guided filter on a shrunk copy, md::guided_fast):
//! edges stay, texture goes. Positive values boost fine texture and local structure (soft-limited),
//! negative values smooth it away. R scales with the image height (ST_R_1440 at 1440 px), the fine
//! radius stays 1 px, so the preview (smaller image) shows the fine part a bit stronger than Process.

use crate::md::guided_fast;

pub const ST_EPS: f32 = 3e-3;
pub const ST_R_1440: f64 = 29.0;
/// the coarse filter runs on a copy shrunk so that its radius is about this many pixels
pub const ST_FAST_R: usize = 12;
pub const ST_C1: f32 = 1.1;
pub const ST_C2: f32 = 1.5;
pub const ST_POS_GAIN: f64 = 2.0;
pub const ST_LIM_POS: f64 = 40.0;
pub const ST_LIM_NEG: f64 = 22.0;
/// tanh table: TQ steps per level over detail -TN/TQ .. TN/TQ (beyond that d is constant)
const TQ: f64 = 4.0;
const TN: usize = 4096;

/// Sharpen settings (all -1..1, 0 = off).
#[derive(Clone, Copy, Default)]
pub struct Sharp {
    pub structure: f64,
}

impl Sharp {
    pub fn active(&self) -> bool {
        self.structure != 0.0
    }
}

/// Coarse Structure radius for an image `height` pixels high.
pub fn st_radius(height: usize) -> usize {
    ((ST_R_1440 * height as f64 / 1440.0).round_ties_even() as usize).max(1)
}

/// reflect-101 index for the 3x3 window
#[inline(always)]
fn rf(i: isize, n: usize) -> usize {
    if n == 1 {
        0
    } else if i < 0 {
        1
    } else if i as usize >= n {
        n - 2
    } else {
        i as usize
    }
}

/// Horizontal 3-sums of a row and of its squares (reflect-101 at the ends)
fn sum3(row: &[f32], s: &mut [f32], q: &mut [f32]) {
    let w = row.len();
    for x in 0..w {
        let (l, c, r) = if x >= 1 && x + 1 < w {
            (row[x - 1], row[x], row[x + 1])
        } else {
            (row[rf(x as isize - 1, w)], row[x], row[rf(x as isize + 1, w)])
        };
        s[x] = l + c + r;
        q[x] = l * l + c * c + r * r;
    }
}

/// Self-guided filter with a 3x3 window (radius 1), fused band by band: the fine scale of Structure.
/// Same formula as md::guided_eps(.., 1, eps), with f32 sums.
pub fn guided3(y: &[f32], w: usize, h: usize, eps: f32) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    const B: usize = 64;
    crate::for_each_band_t(&mut out, w, B, |band, r0| {
        let rows = band.len() / w;
        // a, b for rows r0-1 ..= r0+rows (mirrored), 3x3 means of y and y*y first
        let n = rows + 2;
        let (mut a, mut b) = (vec![0f32; n * w], vec![0f32; n * w]);
        let (mut hs, mut hq) = (vec![0f32; 3 * w], vec![0f32; 3 * w]);
        let (mut va, mut vb) = (vec![0f32; w], vec![0f32; w]);
        for k in 0..n {
            let yr = rf(r0 as isize + k as isize - 1, h);
            for (t, dy) in (-1isize..=1).enumerate() {
                let row = &y[rf(yr as isize + dy, h) * w..][..w];
                let (s3, q3) = (&mut hs[t * w..][..w], &mut hq[t * w..][..w]);
                sum3(row, s3, q3);
            }
            for x in 0..w {
                let m = (hs[x] + hs[w + x] + hs[2 * w + x]) / 9.0;
                let q = (hq[x] + hq[w + x] + hq[2 * w + x]) / 9.0;
                let v = q - m * m;
                let ai = v / (v + eps);
                a[k * w + x] = ai;
                b[k * w + x] = m - ai * m;
            }
        }
        for (i, o) in band.chunks_exact_mut(w).enumerate() {
            let yrow = &y[(r0 + i) * w..][..w];
            // vertical 3-row sums of a and b, then the horizontal 3-sum
            for x in 0..w {
                va[x] = a[i * w + x] + a[(i + 1) * w + x] + a[(i + 2) * w + x];
                vb[x] = b[i * w + x] + b[(i + 1) * w + x] + b[(i + 2) * w + x];
            }
            let h3 = |v: &[f32], x: usize| -> f32 {
                if x >= 1 && x + 1 < w { v[x - 1] + v[x] + v[x + 1] } else { v[rf(x as isize - 1, w)] + v[x] + v[rf(x as isize + 1, w)] }
            };
            for x in 0..w {
                o[x] = h3(&va, x) / 9.0 * yrow[x] + h3(&vb, x) / 9.0;
            }
        }
    });
    out
}

#[inline(always)]
fn u8r(v: f64) -> u8 {
    (v + 0.5).floor().clamp(0.0, 255.0) as u8
}

/// In place on `buf` (`ob` = 3 or 4 bytes per pixel, alpha left alone), `width` pixels per row.
pub fn apply(buf: &mut [u8], width: usize, height: usize, ob: usize, sp: &Sharp) {
    if !sp.active() || width == 0 || height == 0 {
        return;
    }
    let mut y = vec![0f32; width * height];
    crate::for_each_band_t(&mut y, width, crate::BAND_ROWS, |band, r0| {
        let src = &buf[r0 * width * ob..][..band.len() * ob];
        for (o, p) in band.iter_mut().zip(src.chunks_exact(ob)) {
            *o = (0.2126f32 * p[0] as f32 + 0.7152f32 * p[1] as f32 + 0.0722f32 * p[2] as f32) / 255.0f32;
        }
    });
    let s = sp.structure;
    let (g, lim) = if s > 0.0 { (s * ST_POS_GAIN, ST_LIM_POS) } else { (s, ST_LIM_NEG) };
    let fine = guided3(&y, width, height, ST_EPS);
    let r = st_radius(height);
    let coarse = guided_fast(&y, width, height, r, ST_EPS, (r / ST_FAST_R).max(1));
    // d(det) = lim * tanh(det * g / lim) as a table over det (TQ steps per level, linear in between)
    let k = g / lim;
    let tab: Vec<f64> = (0..=2 * TN).map(|i| lim * libm::tanh(k * ((i as f64 - TN as f64) / TQ))).collect();
    crate::for_each_band(buf, width * ob, |band, row0| {
        let p0 = row0 * width;
        for (i, px) in band.chunks_exact_mut(ob).enumerate() {
            let j = p0 + i;
            let det = 255.0 * (ST_C1 * (y[j] - fine[j]) + ST_C2 * (y[j] - coarse[j])) as f64;
            let t = (det * TQ).clamp(-(TN as f64) + 1.0, TN as f64 - 1.0) + TN as f64;
            let ti = t as usize;
            let d = tab[ti] + (tab[ti + 1] - tab[ti]) * (t - ti as f64);
            px[0] = u8r(px[0] as f64 + d);
            px[1] = u8r(px[1] as f64 + d);
            px[2] = u8r(px[2] as f64 + d);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_and_flat_are_identity() {
        let (w, h) = (9, 7);
        let mut a: Vec<u8> = (0..w * h * 3).map(|i| (i * 37 % 256) as u8).collect();
        let orig = a.clone();
        apply(&mut a, w, h, 3, &Sharp { structure: 0.0 });
        assert_eq!(a, orig);
        let yv: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 251) as f32 / 251.0).collect();
        let g1 = guided3(&yv, w, h, ST_EPS);
        let g2 = crate::md::guided_eps(&yv, w, h, 1, ST_EPS);
        assert!(g1.iter().zip(&g2).all(|(a, b)| (a - b).abs() < 1e-5));
        let mut flat = vec![120u8; w * h * 4];
        apply(&mut flat, w, h, 4, &Sharp { structure: 1.0 });
        assert!(flat.iter().all(|&v| v == 120));
    }

    #[test]
    fn sign_boosts_or_smooths_texture() {
        let (w, h) = (40, 40);
        let src: Vec<u8> = (0..w * h).flat_map(|i| { let v = if (i % w + i / w) % 2 == 0 { 110 } else { 130 }; [v, v, v] }).collect();
        let spread = |a: &[u8]| a.iter().map(|&v| (v as i32 - 120).abs()).sum::<i32>();
        let mut up = src.clone();
        apply(&mut up, w, h, 3, &Sharp { structure: 1.0 });
        let mut dn = src.clone();
        apply(&mut dn, w, h, 3, &Sharp { structure: -1.0 });
        assert!(spread(&up) > spread(&src));
        assert!(spread(&dn) < spread(&src));
    }
}
