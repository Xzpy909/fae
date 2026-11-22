//! Sharpen tool, the last step of the edit (after the curves, on the output pixels).
//!
//! Structure ≈ (APPROXIMATION, fitted to reference renders of a game screenshot at -100 / -50 / 50 / 100
//! and a photo at 100, not pixel exact):
//!   detail = 255 * (ST_C1A * (y - g3(y, ST_EPS_A)) + ST_C1B * (y - g3(y, ST_EPS_B))
//!                  + ST_CM * (y - guided(y, Rm)) + ST_C2 * (y - guided(y, R))) * exp(ST_TILT * (y - 0.5))
//! g3 = 3x3 guided filter. The two fine terms together boost fine edges but not low-amplitude fine detail
//! (sensor noise / grain stays as it is). The tilt gives darker areas a little more detail than bright ones.
//! Fitted on three images (two game screenshots, one photo).
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
/// same for the mid filter
pub const ST_FAST_RM: usize = 8;
pub const ST_EPS_A: f32 = 1e-3;
pub const ST_EPS_B: f32 = 1e-2;
pub const ST_C1A: f32 = -0.67;
pub const ST_C1B: f32 = 0.88;
pub const ST_CM: f32 = 0.52;
pub const ST_C2: f32 = 0.94;
pub const ST_TILT: f32 = -0.47;
/// mid radius at 1440 px height
pub const ST_RM_1440: f64 = 10.0;
pub const ST_POS_GAIN: f64 = 2.0;
pub const ST_LIM_POS: f64 = 38.0;
pub const ST_LIM_NEG: f64 = 20.0;
/// tanh table: TQ steps per level over detail -TN/TQ .. TN/TQ (beyond that d is constant)
const TQ: f64 = 4.0;
const TN: usize = 4096;

/// Sharpen settings (sliders -1..1, 0 = off).
#[derive(Clone, Copy)]
pub struct Sharp {
    /// Adaptive Sharpening 0..1
    pub adaptive: f64,
    /// Balance Sharpening -1 (areas) .. 1 (edges), shifts Adaptive Sharpening
    pub balance: f64,
    pub structure: f64,
    pub local_contrast: f64,
    pub focus: f64,
    /// pixels of this image per output pixel (1 for Process; the preview passes its own scale, so
    /// the fixed-size Local Contrast blur covers the same part of the picture)
    pub scale: f64,
}

impl Default for Sharp {
    fn default() -> Self {
        Sharp { adaptive: 0.0, balance: 0.0, structure: 0.0, local_contrast: 0.0, focus: 0.0, scale: 1.0 }
    }
}

impl Sharp {
    pub fn active(&self) -> bool {
        self.adaptive > 0.0 || self.structure != 0.0 || self.local_contrast != 0.0 || self.focus != 0.0
    }
}

/// Local Contrast (fitted to reference renders at -100 / -50 / 50 / 100 on three images, mean error
/// 0.25-0.52 levels): per channel, out_c = c + g * (c - gauss(c, LC_SIGMA)), g = LC_POS * s for
/// s > 0 (unsharp mask with a wide radius), LC_NEG * s for s < 0 (mix towards the blur: a glow).
/// The blur is a fixed size in output pixels.
pub const LC_SIGMA: f64 = 20.0;
pub const LC_POS: f64 = 0.51;
pub const LC_NEG: f64 = 0.75;
/// the blur runs on a copy shrunk so that sigma is about this many pixels
const LC_SMALL_SIGMA: f64 = 4.0;

/// Focus (fitted to reference renders at -100 / -50 / 50 / 100 on one image, +100 on two more):
///   s > 0: per channel unsharp mask, out_c = c + FO_POS * s * (c - gauss(c, FO_SIGMA_POS))
///   s < 0: per channel blur in a gamma-FO_GAMMA domain (bright parts bloom a little, like a lens),
///          sigma = FO_SIGMA_NEG * |s|^FO_SIGMA_EXP, mixed in at FO_MIX.
/// Sizes are fixed in output pixels.
pub const FO_POS: f32 = 1.0;
pub const FO_SIGMA_POS: f64 = 1.0;
pub const FO_GAMMA: f64 = 3.8;
pub const FO_SIGMA_NEG: f64 = 5.44;
pub const FO_SIGMA_EXP: f64 = 1.86;
pub const FO_MIX: f32 = 0.96;

/// Adaptive Sharpening, Type Display (fitted to reference renders at 25 / 50 / 75 / 100 on one image,
/// 100 on two more; mean error 0.4-3 levels): per channel unsharp mask whose radius grows with the
/// slider, sigma = AS_SIGMA * s px, gain = AS_GAIN * min(1, s / 0.5)^AS_RAMP. Fixed size in output px.
pub const AS_SIGMA: f64 = 1.93;
pub const AS_GAIN: f64 = 1.78;
pub const AS_RAMP: f64 = 0.85;

/// Balance Sharpening (shape taken from a second program's renders at balance -100 / 0 / 100, as a
/// ratio to its own balance 0): Adaptive Sharpening's gain is multiplied per pixel by
/// w = 1 + |b| * (W(t) - 1), t = log2(1 + |grad luma|) (Sobel / 8, 0..255 scale),
///   b > 0 (edges): W = BS_P0 + BS_P1 * t            (flat ~0.8, strong edges ~2.2)
///   b < 0 (areas): W = BS_M0 * exp(-BS_MK * t) + BS_MC + BS_MD * t   (flat ~2.2, strong edges < 0: softened)
pub const BS_P0: f32 = 0.634;
pub const BS_P1: f32 = 0.283;
pub const BS_M0: f32 = 4.942;
pub const BS_MK: f32 = 2.257;
pub const BS_MC: f32 = 1.102;
pub const BS_MD: f32 = -0.285;

/// Per pixel Balance weight (see BS_*), from the luma gradient of `buf`.
fn balance_weights(buf: &[u8], w: usize, h: usize, ob: usize, b: f32) -> Vec<f32> {
    let mut yl = vec![0f32; w * h];
    crate::for_each_band_t(&mut yl, w.max(1), crate::BAND_ROWS, |band, r0| {
        let src = &buf[r0 * w * ob..][..band.len() * ob];
        for (o, p) in band.iter_mut().zip(src.chunks_exact(ob)) {
            *o = 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        }
    });
    let lum = |x: usize, y: usize| -> f32 { yl[y * w + x] };
    let mut out = vec![1f32; w * h];
    crate::for_each_band_t(&mut out, w.max(1), crate::BAND_ROWS, |band, r0| {
        for (i, row) in band.chunks_exact_mut(w).enumerate() {
            let y = r0 + i;
            let (ym, yp) = (refl101(y as isize - 1, h), refl101(y as isize + 1, h));
            for (x, o) in row.iter_mut().enumerate() {
                let (xm, xp) = (refl101(x as isize - 1, w), refl101(x as isize + 1, w));
                let gx = (lum(xp, ym) + 2.0 * lum(xp, y) + lum(xp, yp) - lum(xm, ym) - 2.0 * lum(xm, y) - lum(xm, yp)) / 8.0;
                let gy = (lum(xm, yp) + 2.0 * lum(x, yp) + lum(xp, yp) - lum(xm, ym) - 2.0 * lum(x, ym) - lum(xp, ym)) / 8.0;
                let t = (1.0 + (gx * gx + gy * gy).sqrt()).log2();
                let wt = if b > 0.0 { BS_P0 + BS_P1 * t } else { BS_M0 * (-BS_MK * t).exp() + BS_MC + BS_MD * t };
                *o = 1.0 + b.abs() * (wt - 1.0);
            }
        }
    });
    out
}

/// Coarse Structure radius for an image `height` pixels high.
pub fn st_radius(height: usize) -> usize {
    rad(ST_R_1440, height)
}

fn rad(at_1440: f64, height: usize) -> usize {
    ((at_1440 * height as f64 / 1440.0).round_ties_even() as usize).max(1)
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
/// Same formula as md::guided_eps(.., 1, eps), with f32 sums. Returns
/// ca * (y - g3(y, eps_a)) + cb * (y - g3(y, eps_b)) (both filters share the window statistics).
pub fn fine3(y: &[f32], w: usize, h: usize, eps_a: f32, eps_b: f32, ca: f32, cb: f32) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    const B: usize = 64;
    crate::for_each_band_t(&mut out, w, B, |band, r0| {
        let rows = band.len() / w;
        // a, b for rows r0-1 ..= r0+rows (mirrored), 3x3 means of y and y*y first
        let n = rows + 2;
        let (mut a, mut b) = (vec![0f32; n * w], vec![0f32; n * w]);
        let (mut a2, mut b2) = (vec![0f32; n * w], vec![0f32; n * w]);
        let (mut hs, mut hq) = (vec![0f32; 3 * w], vec![0f32; 3 * w]);
        let (mut va, mut vb) = (vec![0f32; w], vec![0f32; w]);
        let (mut va2, mut vb2) = (vec![0f32; w], vec![0f32; w]);
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
                let ai = v / (v + eps_a);
                a[k * w + x] = ai;
                b[k * w + x] = m - ai * m;
                let ai = v / (v + eps_b);
                a2[k * w + x] = ai;
                b2[k * w + x] = m - ai * m;
            }
        }
        for (i, o) in band.chunks_exact_mut(w).enumerate() {
            let yrow = &y[(r0 + i) * w..][..w];
            // vertical 3-row sums of a and b, then the horizontal 3-sum
            for x in 0..w {
                va[x] = a[i * w + x] + a[(i + 1) * w + x] + a[(i + 2) * w + x];
                vb[x] = b[i * w + x] + b[(i + 1) * w + x] + b[(i + 2) * w + x];
                va2[x] = a2[i * w + x] + a2[(i + 1) * w + x] + a2[(i + 2) * w + x];
                vb2[x] = b2[i * w + x] + b2[(i + 1) * w + x] + b2[(i + 2) * w + x];
            }
            let h3 = |v: &[f32], x: usize| -> f32 {
                if x >= 1 && x + 1 < w { v[x - 1] + v[x] + v[x + 1] } else { v[rf(x as isize - 1, w)] + v[x] + v[rf(x as isize + 1, w)] }
            };
            for x in 0..w {
                let yv = yrow[x];
                let ga = h3(&va, x) / 9.0 * yv + h3(&vb, x) / 9.0;
                let gb = h3(&va2, x) / 9.0 * yv + h3(&vb2, x) / 9.0;
                o[x] = ca * (yv - ga) + cb * (yv - gb);
            }
        }
    });
    out
}

/// exp(ST_TILT * (y - 0.5)) for y in 0..1: |argument| <= 0.24, so 4 terms are within 1e-4
#[inline(always)]
fn tilt(y: f32) -> f32 {
    let u = ST_TILT * (y - 0.5);
    1.0 + u * (1.0 + u * (0.5 + u * (1.0 / 6.0)))
}

/// round half up into 0..255 (clamp first, then truncate: no floor() call)
#[inline(always)]
fn u8r(v: f32) -> u8 {
    (v.clamp(0.0, 255.0) + 0.5) as u8
}

/// In place on `buf` (`ob` = 3 or 4 bytes per pixel, alpha left alone), `width` pixels per row:
/// Adaptive Sharpening, Structure, Local Contrast, Focus.
pub fn apply(buf: &mut [u8], width: usize, height: usize, ob: usize, sp: &Sharp) {
    if !sp.active() || width == 0 || height == 0 {
        return;
    }
    if sp.adaptive > 0.0 {
        let a = sp.adaptive.min(1.0);
        let g = (AS_GAIN * (a / 0.5).min(1.0).powf(AS_RAMP)) as f32;
        let id: [f32; 256] = core::array::from_fn(|i| i as f32);
        let bal = sp.balance.clamp(-1.0, 1.0) as f32;
        let wts = if bal != 0.0 { balance_weights(buf, width, height, ob, bal) } else { Vec::new() };
        blur_map(buf, width, height, ob, AS_SIGMA * a * sp.scale.clamp(1e-3, 1.0e3), &id, |c, b, j| {
            let v = c as f32;
            let gw = if wts.is_empty() { g } else { g * wts[j] };
            u8r(v + gw * (v - b))
        });
    }
    if sp.structure != 0.0 {
        structure(buf, width, height, ob, sp.structure);
    }
    if sp.local_contrast != 0.0 {
        let s = sp.local_contrast;
        let g = if s > 0.0 { LC_POS * s } else { LC_NEG * s };
        local_contrast(buf, width, height, ob, g as f32, LC_SIGMA * sp.scale.clamp(1e-3, 1.0e3));
    }
    if sp.focus != 0.0 {
        focus(buf, width, height, ob, sp.focus, sp.scale.clamp(1e-3, 1.0e3));
    }
}

fn gauss_kernel(sigma: f64) -> (Vec<f32>, usize) {
    let rad = ((3.0 * sigma).ceil() as usize).max(1);
    let mut ker: Vec<f32> = (0..=2 * rad).map(|i| { let d = i as f64 - rad as f64; (-0.5 * d * d / (sigma * sigma)).exp() as f32 }).collect();
    let ks: f32 = ker.iter().sum();
    ker.iter_mut().for_each(|v| *v /= ks);
    (ker, rad)
}

#[inline(always)]
fn refl101(i: isize, n: usize) -> usize {
    if i >= 0 && (i as usize) < n {
        return i as usize;
    }
    if n == 1 {
        return 0;
    }
    let p = 2 * n as isize - 2;
    let i = i.abs() % p;
    (if i >= n as isize { p - i } else { i }) as usize
}

/// Per pixel and channel: out = f(c, blur_c), blur = Gaussian (sigma in pixels of this image,
/// reflect-101) of tab[c] (a 256-entry input map, e.g. a gamma). Works band by band from a copy of
/// the 8-bit pixels; large sigmas are blurred on a copy shrunk by block means and read back
/// bilinearly.
pub fn blur_map<F>(buf: &mut [u8], w: usize, h: usize, ob: usize, sigma: f64, tab: &[f32; 256], f: F)
where
    F: Fn(u8, f32, usize) -> u8 + Sync + Send,
{
    let src = buf.to_vec();
    let k = if sigma >= 3.0 { ((sigma / 1.5).round() as usize).max(1) } else { 1 };
    if k == 1 {
        let (ker, rad) = gauss_kernel(sigma);
        crate::for_each_band(buf, w * ob, |band, row0| {
            let rows = band.len() / (w * ob);
            let n = rows + 2 * rad;
            // horizontal pass for rows row0-rad .. row0+rows+rad (mirrored at the image edges)
            let mut hb = vec![0f32; n * w * 3];
            let mut line = vec![0f32; (w + 2 * rad) * 3];
            for t in 0..n {
                let yy = refl101(row0 as isize + t as isize - rad as isize, h);
                let srow = &src[yy * w * ob..][..w * ob];
                for xp in 0..w + 2 * rad {
                    let xx = refl101(xp as isize - rad as isize, w);
                    let p = &srow[xx * ob..];
                    line[xp * 3] = tab[p[0] as usize];
                    line[xp * 3 + 1] = tab[p[1] as usize];
                    line[xp * 3 + 2] = tab[p[2] as usize];
                }
                let out = &mut hb[t * w * 3..][..w * 3];
                for (j, kv) in ker.iter().enumerate() {
                    let l = &line[j * 3..][..w * 3];
                    out.iter_mut().zip(l).for_each(|(o, v)| *o += kv * v);
                }
            }
            // vertical pass + output
            let mut acc = vec![0f32; w * 3];
            for (i, orow) in band.chunks_exact_mut(w * ob).enumerate() {
                acc.fill(0.0);
                for (j, kv) in ker.iter().enumerate() {
                    let l = &hb[(i + j) * w * 3..][..w * 3];
                    acc.iter_mut().zip(l).for_each(|(o, v)| *o += kv * v);
                }
                let p0 = (row0 + i) * w;
                for (x, px) in orow.chunks_exact_mut(ob).enumerate() {
                    px[0] = f(px[0], acc[x * 3], p0 + x);
                    px[1] = f(px[1], acc[x * 3 + 1], p0 + x);
                    px[2] = f(px[2], acc[x * 3 + 2], p0 + x);
                }
            }
        });
        return;
    }
    // shrunk: block means of tab[c]
    let (w2, h2) = (w.div_ceil(k), h.div_ceil(k));
    let mut sm = vec![0f32; w2 * h2 * 3];
    crate::for_each_band_t(&mut sm, w2 * 3, 16, |band, r0| {
        for (i, row) in band.chunks_exact_mut(w2 * 3).enumerate() {
            let by = r0 + i;
            let (y0, y1) = (by * k, ((by + 1) * k).min(h));
            for bx in 0..w2 {
                let (x0, x1) = (bx * k, ((bx + 1) * k).min(w));
                let mut a = [0f32; 3];
                for yy in y0..y1 {
                    for p in src[(yy * w + x0) * ob..(yy * w + x1) * ob].chunks_exact(ob) {
                        a[0] += tab[p[0] as usize];
                        a[1] += tab[p[1] as usize];
                        a[2] += tab[p[2] as usize];
                    }
                }
                let nn = ((y1 - y0) * (x1 - x0)) as f32;
                row[bx * 3..bx * 3 + 3].copy_from_slice(&[a[0] / nn, a[1] / nn, a[2] / nn]);
            }
        }
    });
    drop(src);
    let s2 = ((sigma * sigma - ((k * k - 1) as f64) / 12.0).max(1e-6)).sqrt() / k as f64;
    let (ker, rad) = gauss_kernel(s2);
    let mut tmp = vec![0f32; w2 * h2 * 3];
    crate::for_each_band_t(&mut tmp, w2 * 3, 16, |band, r0| {
        for (i, row) in band.chunks_exact_mut(w2 * 3).enumerate() {
            let srow = &sm[(r0 + i) * w2 * 3..][..w2 * 3];
            // mirrored copy of the row, then plain weighted sums (no index checks in the loop)
            let mut line = vec![0f32; (w2 + 2 * rad) * 3];
            for xp in 0..w2 + 2 * rad {
                let xx = refl101(xp as isize - rad as isize, w2);
                line[xp * 3..xp * 3 + 3].copy_from_slice(&srow[xx * 3..xx * 3 + 3]);
            }
            for (t, kv) in ker.iter().enumerate() {
                let l = &line[t * 3..][..w2 * 3];
                row.iter_mut().zip(l).for_each(|(o, v)| *o += kv * v);
            }
        }
    });
    let mut bl = vec![0f32; w2 * h2 * 3];
    crate::for_each_band_t(&mut bl, w2 * 3, 16, |band, r0| {
        for (i, row) in band.chunks_exact_mut(w2 * 3).enumerate() {
            for (t, kv) in ker.iter().enumerate() {
                let yy = refl101((r0 + i) as isize + t as isize - rad as isize, h2);
                let srow = &tmp[yy * w2 * 3..][..w2 * 3];
                row.iter_mut().zip(srow).for_each(|(o, v)| *o += kv * v);
            }
        }
    });
    drop(tmp);
    let pos = |p: usize, n2: usize| -> (usize, usize, f32) {
        let fl = ((p as f32 + 0.5) / k as f32 - 0.5).max(0.0);
        let i0 = (fl as usize).min(n2 - 1);
        (i0, (i0 + 1).min(n2 - 1), fl - i0 as f32)
    };
    let xs: Vec<(usize, usize, f32)> = (0..w).map(|x| pos(x, w2)).collect();
    crate::for_each_band(buf, w * ob, |band, row0| {
        for (i, row) in band.chunks_exact_mut(w * ob).enumerate() {
            let (j0, j1, fy) = pos(row0 + i, h2);
            let (r0, r1) = (&bl[j0 * w2 * 3..][..w2 * 3], &bl[j1 * w2 * 3..][..w2 * 3]);
            let p0 = (row0 + i) * w;
            for (x, px) in row.chunks_exact_mut(ob).enumerate() {
                let (i0, i1, fx) = xs[x];
                for c in 0..3 {
                    let top = r0[i0 * 3 + c] + (r0[i1 * 3 + c] - r0[i0 * 3 + c]) * fx;
                    let bot = r1[i0 * 3 + c] + (r1[i1 * 3 + c] - r1[i0 * 3 + c]) * fx;
                    px[c] = f(px[c], top + (bot - top) * fy, p0 + x);
                }
            }
        }
    });
}

/// Focus (constants above), in place. `scale` = pixels of this image per output pixel.
pub fn focus(buf: &mut [u8], w: usize, h: usize, ob: usize, s: f64, scale: f64) {
    if s > 0.0 {
        let id: [f32; 256] = core::array::from_fn(|i| i as f32);
        let g = FO_POS * s as f32;
        blur_map(buf, w, h, ob, FO_SIGMA_POS * scale, &id, |c, b, _| {
            let v = c as f32;
            u8r(v + g * (v - b))
        });
        return;
    }
    // gamma domain: v^G (table), blur, back through a table over the 4th root (nearly linear there)
    let fwd: [f32; 256] = core::array::from_fn(|v| libm::pow(v as f64 / 255.0, FO_GAMMA) as f32);
    const NB: usize = 4096;
    let back: Vec<f32> = (0..=NB).map(|i| (255.0 * libm::pow(i as f64 / NB as f64, 4.0 / FO_GAMMA)) as f32).collect();
    blur_map(buf, w, h, ob, FO_SIGMA_NEG * (-s).powf(FO_SIGMA_EXP) * scale, &fwd, |c, b, _| {
        let q = b.max(0.0).sqrt().sqrt() * NB as f32;
        let qi = (q as usize).min(NB - 1);
        let bs = back[qi] + (back[qi + 1] - back[qi]) * (q - qi as f32);
        let v = c as f32;
        u8r(v + FO_MIX * (bs - v))
    });
}

/// Gaussian blur of each channel (sigma in pixels of this image) on a shrunk copy, scaled back up
/// bilinearly while it is used: out_c = c + g * (c - blur_c).
pub fn local_contrast(buf: &mut [u8], w: usize, h: usize, ob: usize, g: f32, sigma: f64) {
    let k = ((sigma / LC_SMALL_SIGMA).round() as usize).max(1);
    let (w2, h2) = (w.div_ceil(k), h.div_ceil(k));
    // block means, 3 planes
    let mut sm = vec![0f32; w2 * h2 * 3];
    crate::for_each_band_t(&mut sm, w2 * 3, 16, |band, r0| {
        for (i, row) in band.chunks_exact_mut(w2 * 3).enumerate() {
            let by = r0 + i;
            let (y0, y1) = (by * k, ((by + 1) * k).min(h));
            for bx in 0..w2 {
                let (x0, x1) = (bx * k, ((bx + 1) * k).min(w));
                let mut acc = [0f32; 3];
                for yy in y0..y1 {
                    for p in buf[(yy * w + x0) * ob..(yy * w + x1) * ob].chunks_exact(ob) {
                        acc[0] += p[0] as f32;
                        acc[1] += p[1] as f32;
                        acc[2] += p[2] as f32;
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as f32;
                row[bx * 3] = acc[0] / n;
                row[bx * 3 + 1] = acc[1] / n;
                row[bx * 3 + 2] = acc[2] / n;
            }
        }
    });
    // separable Gaussian on the small copy (the block mean already blurred by about sqrt((k^2-1)/12))
    let s2 = ((sigma * sigma - ((k * k - 1) as f64) / 12.0).max(1e-6)).sqrt() / k as f64;
    let rad = ((3.0 * s2).ceil() as usize).max(1);
    let mut ker: Vec<f32> = (0..=2 * rad).map(|i| { let d = i as f64 - rad as f64; (-0.5 * d * d / (s2 * s2)).exp() as f32 }).collect();
    let ks: f32 = ker.iter().sum();
    ker.iter_mut().for_each(|v| *v /= ks);
    let refl = |i: isize, n: usize| -> usize {
        if n == 1 { return 0; }
        let p = 2 * n as isize - 2;
        let i = i.abs() % p;
        (if i >= n as isize { p - i } else { i }) as usize
    };
    let mut tmp = vec![0f32; w2 * h2 * 3];
    crate::for_each_band_t(&mut tmp, w2 * 3, 16, |band, r0| {
        for (i, row) in band.chunks_exact_mut(w2 * 3).enumerate() {
            let src = &sm[(r0 + i) * w2 * 3..][..w2 * 3];
            for x in 0..w2 {
                let mut acc = [0f32; 3];
                for (t, kv) in ker.iter().enumerate() {
                    let xx = refl(x as isize + t as isize - rad as isize, w2);
                    acc[0] += kv * src[xx * 3];
                    acc[1] += kv * src[xx * 3 + 1];
                    acc[2] += kv * src[xx * 3 + 2];
                }
                row[x * 3..x * 3 + 3].copy_from_slice(&acc);
            }
        }
    });
    let mut bl = vec![0f32; w2 * h2 * 3];
    crate::for_each_band_t(&mut bl, w2 * 3, 16, |band, r0| {
        for (i, row) in band.chunks_exact_mut(w2 * 3).enumerate() {
            let y = r0 + i;
            for (t, kv) in ker.iter().enumerate() {
                let yy = refl(y as isize + t as isize - rad as isize, h2);
                let src = &tmp[yy * w2 * 3..][..w2 * 3];
                row.iter_mut().zip(src).for_each(|(o, v)| *o += kv * v);
            }
        }
    });
    drop(tmp);
    drop(sm);
    let pos = |p: usize, n2: usize| -> (usize, usize, f32) {
        let f = ((p as f32 + 0.5) / k as f32 - 0.5).max(0.0);
        let i0 = (f as usize).min(n2 - 1);
        (i0, (i0 + 1).min(n2 - 1), f - i0 as f32)
    };
    let xs: Vec<(usize, usize, f32)> = (0..w).map(|x| pos(x, w2)).collect();
    crate::for_each_band(buf, w * ob, |band, row0| {
        for (i, row) in band.chunks_exact_mut(w * ob).enumerate() {
            let (j0, j1, fy) = pos(row0 + i, h2);
            let (r0, r1) = (&bl[j0 * w2 * 3..][..w2 * 3], &bl[j1 * w2 * 3..][..w2 * 3]);
            for (x, px) in row.chunks_exact_mut(ob).enumerate() {
                let (i0, i1, fx) = xs[x];
                for c in 0..3 {
                    let top = r0[i0 * 3 + c] + (r0[i1 * 3 + c] - r0[i0 * 3 + c]) * fx;
                    let bot = r1[i0 * 3 + c] + (r1[i1 * 3 + c] - r1[i0 * 3 + c]) * fx;
                    let b = top + (bot - top) * fy;
                    let v = px[c] as f32;
                    px[c] = u8r(v + g * (v - b));
                }
            }
        }
    });
}

/// Structure (see the top of this file), in place.
fn structure(buf: &mut [u8], width: usize, height: usize, ob: usize, s: f64) {
    let mut y = vec![0f32; width * height];
    crate::for_each_band_t(&mut y, width, crate::BAND_ROWS, |band, r0| {
        let src = &buf[r0 * width * ob..][..band.len() * ob];
        for (o, p) in band.iter_mut().zip(src.chunks_exact(ob)) {
            *o = (0.2126f32 * p[0] as f32 + 0.7152f32 * p[1] as f32 + 0.0722f32 * p[2] as f32) / 255.0f32;
        }
    });
    let (g, lim) = if s > 0.0 { (s * ST_POS_GAIN, ST_LIM_POS) } else { (s, ST_LIM_NEG) };
    let fine = fine3(&y, width, height, ST_EPS_A, ST_EPS_B, ST_C1A, ST_C1B);
    let r = st_radius(height);
    let coarse = guided_fast(&y, width, height, r, ST_EPS, (r / ST_FAST_R).max(1));
    let rm = rad(ST_RM_1440, height);
    let mid = guided_fast(&y, width, height, rm, ST_EPS, (rm / ST_FAST_RM).max(1));
    // d(det) = lim * tanh(det * g / lim) as a table over det (TQ steps per level, linear in between)
    let k = g / lim;
    let tab: Vec<f32> = (0..=2 * TN).map(|i| (lim * libm::tanh(k * ((i as f64 - TN as f64) / TQ))) as f32).collect();
    let (tq, tn) = (TQ as f32, TN as f32);
    crate::for_each_band(buf, width * ob, |band, row0| {
        let p0 = row0 * width;
        for (i, px) in band.chunks_exact_mut(ob).enumerate() {
            let j = p0 + i;
            let yj = y[j];
            let det = 255.0 * (fine[j] + ST_CM * (yj - mid[j]) + ST_C2 * (yj - coarse[j])) * tilt(yj);
            let t = (det * tq).clamp(1.0 - tn, tn - 1.0) + tn;
            let ti = t as usize;
            let d = tab[ti] + (tab[ti + 1] - tab[ti]) * (t - ti as f32);
            px[0] = u8r(px[0] as f32 + d);
            px[1] = u8r(px[1] as f32 + d);
            px[2] = u8r(px[2] as f32 + d);
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
        apply(&mut a, w, h, 3, &Sharp { structure: 0.0, ..Default::default() });
        assert_eq!(a, orig);
        let yv: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 251) as f32 / 251.0).collect();
        let f = fine3(&yv, w, h, 1e-3, 1e-2, 0.7, -0.3);
        let ga = crate::md::guided_eps(&yv, w, h, 1, 1e-3);
        let gb = crate::md::guided_eps(&yv, w, h, 1, 1e-2);
        for i in 0..w * h {
            let want = 0.7 * (yv[i] - ga[i]) - 0.3 * (yv[i] - gb[i]);
            assert!((f[i] - want).abs() < 1e-5);
        }
        let mut flat = vec![120u8; w * h * 4];
        apply(&mut flat, w, h, 4, &Sharp { structure: 1.0, ..Default::default() });
        assert!(flat.iter().all(|&v| v == 120));
    }

    #[test]
    fn sign_boosts_or_smooths_texture() {
        let (w, h) = (40, 40);
        let src: Vec<u8> = (0..w * h).flat_map(|i| { let v = if (i % w + i / w) % 2 == 0 { 110 } else { 130 }; [v, v, v] }).collect();
        let spread = |a: &[u8]| a.iter().map(|&v| (v as i32 - 120).abs()).sum::<i32>();
        let mut up = src.clone();
        apply(&mut up, w, h, 3, &Sharp { structure: 1.0, ..Default::default() });
        let mut dn = src.clone();
        apply(&mut dn, w, h, 3, &Sharp { structure: -1.0, ..Default::default() });
        assert!(spread(&up) > spread(&src));
        assert!(spread(&dn) < spread(&src));
    }

    #[test]
    fn local_contrast_flat_and_sign() {
        let (w, h) = (120, 90);
        let mut flat = vec![90u8; w * h * 3];
        apply(&mut flat, w, h, 3, &Sharp { local_contrast: 1.0, ..Default::default() });
        assert!(flat.iter().all(|&v| v == 90));
        // left half dark, right half bright: + pushes them apart near the edge, - pulls them together
        let src: Vec<u8> = (0..w * h).flat_map(|i| { let v = if i % w < w / 2 { 80 } else { 160 }; [v, v, v] }).collect();
        let at = |a: &[u8], x: usize| a[(45 * w + x) * 3] as i32;
        let mut up = src.clone();
        apply(&mut up, w, h, 3, &Sharp { local_contrast: 1.0, ..Default::default() });
        let mut dn = src.clone();
        apply(&mut dn, w, h, 3, &Sharp { local_contrast: -1.0, ..Default::default() });
        assert!(at(&up, w / 2 - 3) < 80 && at(&up, w / 2 + 3) > 160);
        assert!(at(&dn, w / 2 - 3) > 80 && at(&dn, w / 2 + 3) < 160);
    }

    #[test]
    fn focus_flat_identity_and_blur3_matches_direct() {
        let (w, h) = (64, 48);
        let mut flat = vec![77u8; w * h * 3];
        apply(&mut flat, w, h, 3, &Sharp { focus: -1.0, ..Default::default() });
        assert!(flat.iter().all(|&v| (v as i32 - 77).abs() <= 1));
        let mut flat = vec![77u8; w * h * 3];
        apply(&mut flat, w, h, 3, &Sharp { focus: 1.0, ..Default::default() });
        assert!(flat.iter().all(|&v| v == 77));
        // the shrunk path (large sigma) and the direct one agree on a smooth ramp
        let ramp: Vec<u8> = (0..w * h).flat_map(|i| { let v = 40 + (i % w) as u8 * 2; [v, v, v] }).collect();
        let id: [f32; 256] = core::array::from_fn(|i| i as f32);
        for sigma in [3.0, 5.0] {
            let mut a = ramp.clone();
            blur_map(&mut a, w, h, 3, sigma, &id, |_, b, _| (b + 0.5) as u8);
            for y in 10..h - 10 {
                for x in 16..w - 16 {
                    let i = (y * w + x) * 3;
                    assert!((a[i] as i32 - ramp[i] as i32).abs() <= 1, "sigma {sigma} x {x}");
                }
            }
        }
    }
}
