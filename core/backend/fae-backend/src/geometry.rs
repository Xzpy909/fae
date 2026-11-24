//! Geometry (wave B2), as fastedit/geometry.py `apply_geometry`: flip → rotate (90° steps + fine
//! angle) → crop (matches ui/js/crop.js; positive angles turn clockwise on screen).
//! - flips, quarter turns and whole-pixel crops: Pillow's transpose / crop (exact);
//! - a fine angle: OpenCV's `warpAffine(INTER_LANCZOS4 | WARP_INVERSE_MAP)` re-implemented with the same
//!   fixed-point coordinates (1/32 pixel), coefficient tables and border rules: replicated edges for
//!   opaque images; for RGBA premultiplied floats with a transparent outside, back to straight alpha
//!   (no dark fringes);
//! - transparent images with `crop_free`: the crop may reach past the image (transparent there).

use crate::fit::{pyround, StageParams};
use crate::px::{Px, View};
use crate::resample::crop;
use rayon::prelude::*;
use std::sync::OnceLock;

/// Pillow's ImageOps.mirror
pub fn mirror(p: View) -> Px {
    match p.c {
        3 => mirror_c::<3>(p),
        _ => mirror_c::<4>(p),
    }
}

fn mirror_c<const C: usize>(p: View) -> Px {
    let mut o = Px::new(p.w, p.h, C);
    o.data.par_chunks_mut(p.w * C).enumerate().for_each(|(y, line)| {
        for (d, s) in line.chunks_exact_mut(C).zip(p.row(y).chunks_exact(C).rev()) {
            d.copy_from_slice(s);
        }
    });
    o
}

/// Quarter turns clockwise: 1 = Pillow's ROTATE_270, 2 = ROTATE_180, 3 = ROTATE_90.
pub fn quarter(p: View, q: i64) -> Px {
    match p.c {
        3 => quarter_c::<3>(p, q),
        _ => quarter_c::<4>(p, q),
    }
}

fn quarter_c<const C: usize>(p: View, q: i64) -> Px {
    let q = q.rem_euclid(4);
    let (w, h) = (p.w, p.h);
    if q == 0 {
        return p.to_px();
    }
    if q == 2 {
        let mut o = Px::new(w, h, C);
        o.data.par_chunks_mut(w * C).enumerate().for_each(|(y, line)| {
            for (d, s) in line.chunks_exact_mut(C).zip(p.row(h - 1 - y).chunks_exact(C).rev()) {
                d.copy_from_slice(s);
            }
        });
        return o;
    }
    // 90° steps: output rows in bands, source read in tiles (cache-friendly, as Pillow's ROTATE_CHUNK)
    const T: usize = 64;
    let (ow, oh) = (h, w);
    let mut o = Px::new(ow, oh, C);
    o.data.par_chunks_mut(ow * C * T).enumerate().for_each(|(band, chunk)| {
        let y0 = band * T;
        let rows = chunk.len() / (ow * C);
        for x0 in (0..ow).step_by(T) {
            for dy in 0..rows {
                let y = y0 + dy;
                let line = &mut chunk[dy * ow * C..(dy + 1) * ow * C];
                for x in x0..(x0 + T).min(ow) {
                    // the source pixel of output (x, y)
                    let (sx, sy) = if q == 1 { (y, h - 1 - x) } else { (w - 1 - y, x) };
                    let s = (sy * w + sx) * C;
                    line[x * C..x * C + C].copy_from_slice(&p.data[s..s + C]);
                }
            }
        }
    });
    o
}

/// apply_geometry
pub fn apply(img: View, p: &StageParams) -> Px {
    if !p.has_geom() {
        return img.to_px();
    }
    let flipped;
    let img = if p.flip {
        flipped = mirror(img);
        flipped.view()
    } else {
        img
    };
    let q = p.rot_q.rem_euclid(4);
    let (w, h) = (img.w as f64, img.h as f64);
    let (cx, cy, cw, ch) = if p.crop_w <= 0.0 || p.crop_h <= 0.0 {
        if p.angle == 0.0 {
            return quarter(img, q);
        }
        // a fine angle without a crop: the full (unrotated-size) frame
        let (wq, hq) = if q % 2 == 1 { (h, w) } else { (w, h) };
        (0.0, 0.0, wq, hq)
    } else {
        (p.crop_x, p.crop_y, p.crop_w, p.crop_h)
    };
    let ow = pyround(cw).max(1);
    let oh = pyround(ch).max(1);
    let alpha = img.alpha();
    let free = alpha && p.crop_free;
    if p.angle == 0.0 {
        // lossless: quarter turn + whole-pixel crop
        let t = quarter(img, q);
        let (wq, hq) = (t.w as i64, t.h as i64);
        let mut left = pyround(wq as f64 / 2.0 + cx - ow as f64 / 2.0);
        let mut top = pyround(hq as f64 / 2.0 + cy - oh as f64 / 2.0);
        if free {
            return crop(t.view(), left, top, left + ow, top + oh);
        }
        left = left.max(0).min((wq - ow).max(0));
        top = top.max(0).min((hq - oh).max(0));
        return crop(t.view(), left, top, left + ow.min(wq), top + oh.min(hq));
    }
    let th = (q as f64 * 90.0 + p.angle) * (std::f64::consts::PI / 180.0);
    let (c, s) = (th.cos(), th.sin());
    let x0 = cx - ow as f64 / 2.0 + 0.5;
    let y0 = cy - oh as f64 / 2.0 + 0.5;
    let m = [c, s, c * x0 + s * y0 + w / 2.0 - 0.5, -s, c, -s * x0 + c * y0 + h / 2.0 - 0.5];
    let (ow, oh) = (ow as usize, oh as usize);
    if alpha {
        warp_rgba(img, &m, ow, oh)
    } else {
        warp_u8(img, &m, ow, oh)
    }
}

// ---- OpenCV's warpAffine, INTER_LANCZOS4 ---------------------------------------------------------------

const TAB: usize = 32; // INTER_TAB_SIZE
const COEF_SCALE: i32 = 1 << 15; // INTER_REMAP_COEF_SCALE

/// interpolateLanczos4 (imgwarp.cpp)
fn lanczos4(x: f32) -> [f32; 8] {
    const S45: f64 = std::f64::consts::FRAC_1_SQRT_2; // OpenCV's literal, the same double
    const CS: [[f64; 2]; 8] =
        [[1.0, 0.0], [-S45, -S45], [0.0, 1.0], [S45, -S45], [-1.0, 0.0], [S45, S45], [0.0, -1.0], [-S45, S45]];
    let mut k = [0f32; 8];
    if x < f32::EPSILON {
        k[3] = 1.0;
        return k;
    }
    let pi = std::f64::consts::PI;
    let y0 = -((x + 3.0) as f64) * pi * 0.25;
    let (s0, c0) = (y0.sin(), y0.cos());
    let mut sum = 0f32;
    for (i, ki) in k.iter_mut().enumerate() {
        let y = -((x + 3.0 - i as f32) as f64) * pi * 0.25;
        *ki = ((CS[i][0] * s0 + CS[i][1] * c0) / (y * y)) as f32;
        sum += *ki;
    }
    let inv = 1.0f32 / sum;
    for ki in k.iter_mut() {
        *ki *= inv;
    }
    k
}

struct Tables {
    f: Vec<f32>,
    i: Vec<i32>,
}

/// initInterTab2D for Lanczos-4: per (fy, fx) in 1/32 steps, 8 × 8 weights (float, and fixed point
/// summing to exactly 1 << 15).
fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let scale = 1.0f32 / TAB as f32;
        let t1: Vec<[f32; 8]> = (0..TAB).map(|i| lanczos4(i as f32 * scale)).collect();
        let mut f = vec![0f32; TAB * TAB * 64];
        let mut it = vec![0i32; TAB * TAB * 64];
        for i in 0..TAB {
            for j in 0..TAB {
                let base = (i * TAB + j) * 64;
                let (ft, itab) = (&mut f[base..base + 64], &mut it[base..base + 64]);
                let mut isum = 0;
                for k1 in 0..8 {
                    let vy = t1[i][k1];
                    for k2 in 0..8 {
                        let v = vy * t1[j][k2];
                        ft[k1 * 8 + k2] = v;
                        let q = (v * COEF_SCALE as f32).round_ties_even().clamp(-32768.0, 32767.0) as i32;
                        itab[k1 * 8 + k2] = q;
                        isum += q;
                    }
                }
                if isum != COEF_SCALE {
                    let diff = isum - COEF_SCALE;
                    let (mut mk, mut mx) = ((4, 4), (4, 4));
                    for k1 in 4..6 {
                        for k2 in 4..6 {
                            let v = itab[k1 * 8 + k2];
                            if v < itab[mk.0 * 8 + mk.1] {
                                mk = (k1, k2);
                            } else if v > itab[mx.0 * 8 + mx.1] {
                                mx = (k1, k2);
                            }
                        }
                    }
                    if diff < 0 {
                        itab[mx.0 * 8 + mx.1] -= diff;
                    } else {
                        itab[mk.0 * 8 + mk.1] -= diff;
                    }
                }
            }
        }
        Tables { f, i: it }
    })
}

/// cvRound / saturate_cast<int>(double)
fn sat_round(v: f64) -> i32 {
    v.round_ties_even().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

/// The source position of every output pixel of row `y`: (integer x, integer y, 1/32 fraction index).
fn row_coords(m: &[f64; 6], y: usize, adelta: &[i32], bdelta: &[i32], out: &mut Vec<(i32, i32, usize)>) {
    const AB_SCALE: f64 = 1024.0;
    out.clear();
    let x0 = sat_round((m[1] * y as f64 + m[2]) * AB_SCALE + 16.0);
    let y0 = sat_round((m[4] * y as f64 + m[5]) * AB_SCALE + 16.0);
    for (a, b) in adelta.iter().zip(bdelta) {
        let xx = x0.wrapping_add(*a) >> 5;
        let yy = y0.wrapping_add(*b) >> 5;
        out.push((xx >> 5, yy >> 5, ((yy & 31) * 32 + (xx & 31)) as usize));
    }
}

fn deltas(m: &[f64; 6], ow: usize) -> (Vec<i32>, Vec<i32>) {
    let a = (0..ow).map(|x| sat_round(m[0] * x as f64 * 1024.0)).collect();
    let b = (0..ow).map(|x| sat_round(m[3] * x as f64 * 1024.0)).collect();
    (a, b)
}

/// 8-bit, BORDER_REPLICATE (opaque images)
fn warp_u8(src: View, m: &[f64; 6], ow: usize, oh: usize) -> Px {
    match src.c {
        3 => warp_u8_c::<3>(src, m, ow, oh),
        _ => warp_u8_c::<4>(src, m, ow, oh),
    }
}

fn warp_u8_c<const C: usize>(src: View, m: &[f64; 6], ow: usize, oh: usize) -> Px {
    let t = tables();
    let (sw, sh) = (src.w as i32, src.h as i32);
    let (w1, h1) = ((sw - 7).max(0) as u32, (sh - 7).max(0) as u32);
    let stride = src.w * C;
    let (adelta, bdelta) = deltas(m, ow);
    let mut out = Px::new(ow, oh, C);
    out.data.par_chunks_mut(ow * C).enumerate().for_each_init(Vec::new, |xy, (y, line)| {
        row_coords(m, y, &adelta, &bdelta, xy);
        for (&(ix, iy, a), d) in xy.iter().zip(line.chunks_exact_mut(C)) {
            let (sx, sy) = (ix - 3, iy - 3);
            let w = &t.i[a * 64..a * 64 + 64];
            let mut sum = [0i32; C];
            if (sx as u32) < w1 && (sy as u32) < h1 {
                for r in 0..8 {
                    let s = &src.data[(sy as usize + r) * stride + sx as usize * C..][..8 * C];
                    for (p, &wj) in s.chunks_exact(C).zip(&w[r * 8..r * 8 + 8]) {
                        for k in 0..C {
                            sum[k] += p[k] as i32 * wj;
                        }
                    }
                }
            } else {
                // replicated edges
                let xs: [usize; 8] = std::array::from_fn(|i| (sx + i as i32).clamp(0, sw - 1) as usize * C);
                for r in 0..8 {
                    let row = &src.data[(sy + r as i32).clamp(0, sh - 1) as usize * stride..];
                    for j in 0..8 {
                        for k in 0..C {
                            sum[k] += row[xs[j] + k] as i32 * w[r * 8 + j];
                        }
                    }
                }
            }
            for k in 0..C {
                d[k] = ((sum[k] + (1 << 14)) >> 15).clamp(0, 255) as u8;
            }
        }
    });
    out
}

/// RGBA: premultiplied float Lanczos-4 with a transparent (0) border, then straight alpha again, as
/// geometry.py did with NumPy (float32 maths; the sums in OpenCV's order, so the same results).
fn warp_rgba(src: View, m: &[f64; 6], ow: usize, oh: usize) -> Px {
    let t = tables();
    let (sw, sh) = (src.w as i32, src.h as i32);
    let (w1, h1) = ((sw - 7).max(0) as u32, (sh - 7).max(0) as u32);
    // premultiplied: rgb * (a / 255), alpha as it is (0..255)
    let mut pm = vec![0f32; src.data.len()];
    pm.par_chunks_mut(src.w * 4).zip(src.data.par_chunks(src.w * 4)).for_each(|(d, s)| {
        for (q, p) in d.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
            let a = p[3] as f32 / 255.0f32;
            q[0] = p[0] as f32 * a;
            q[1] = p[1] as f32 * a;
            q[2] = p[2] as f32 * a;
            q[3] = p[3] as f32;
        }
    });
    let stride = src.w * 4;
    let (adelta, bdelta) = deltas(m, ow);
    let mut out = Px::new(ow, oh, 4);
    out.data.par_chunks_mut(ow * 4).enumerate().for_each_init(Vec::new, |xy, (y, line)| {
        row_coords(m, y, &adelta, &bdelta, xy);
        for (&(ix, iy, a), d) in xy.iter().zip(line.chunks_exact_mut(4)) {
            let (sx, sy) = (ix - 3, iy - 3);
            let w = &t.f[a * 64..a * 64 + 64];
            let mut v = [0f32; 4];
            if (sx as u32) < w1 && (sy as u32) < h1 {
                for r in 0..8 {
                    let s = &pm[(sy as usize + r) * stride + sx as usize * 4..][..32];
                    let wr = &w[r * 8..r * 8 + 8];
                    for k in 0..4 {
                        v[k] += s[k] * wr[0] + s[4 + k] * wr[1] + s[8 + k] * wr[2] + s[12 + k] * wr[3]
                            + s[16 + k] * wr[4] + s[20 + k] * wr[5] + s[24 + k] * wr[6] + s[28 + k] * wr[7];
                    }
                }
            } else if !(sx >= sw || sx + 8 <= 0 || sy >= sh || sy + 8 <= 0) {
                let inside = |v: i32, n: i32| if (0..n).contains(&v) { v } else { -1 };
                let xs: [i32; 8] = std::array::from_fn(|i| inside(sx + i as i32, sw));
                for (k, vk) in v.iter_mut().enumerate() {
                    for r in 0..8 {
                        let yr = inside(sy + r as i32, sh);
                        if yr < 0 {
                            continue;
                        }
                        let row = &pm[yr as usize * stride..];
                        for j in 0..8 {
                            if xs[j] >= 0 {
                                *vk += row[xs[j] as usize * 4 + k] * w[r * 8 + j];
                            }
                        }
                    }
                }
            }
            // back to straight alpha (np.clip / np.where / np.round as in geometry.py)
            let oa = v[3].clamp(0.0, 255.0);
            for k in 0..3 {
                let rgb = if oa > 0.0 { v[k] / (oa / 255.0f32).max(1e-6) } else { 0.0 };
                d[k] = rgb.clamp(0.0, 255.0).round_ties_even() as u8;
            }
            d[3] = oa.round_ties_even() as u8;
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_sum() {
        let t = tables();
        for a in 0..TAB * TAB {
            assert_eq!(t.i[a * 64..a * 64 + 64].iter().sum::<i32>(), COEF_SCALE);
        }
        // fraction 0: the source pixel itself, 32767 (short) + 1 put on the (4, 4) tap by the sum fix-up, as OpenCV
        assert_eq!((t.i[3 * 8 + 3], t.i[4 * 8 + 4]), (32767, 1));
    }

    #[test]
    fn quarters() {
        let p = Px::from_vec(2, 1, 3, vec![1, 1, 1, 2, 2, 2]);
        let p = p.view();
        assert_eq!(quarter(p, 1).data, vec![1, 1, 1, 2, 2, 2]);
        assert_eq!((quarter(p, 1).w, quarter(p, 1).h), (1, 2));
        assert_eq!(quarter(p, 3).data, vec![2, 2, 2, 1, 1, 1]);
        assert_eq!(quarter(p, 2).data, vec![2, 2, 2, 1, 1, 1]);
    }
}
