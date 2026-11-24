//! Pillow's resampling, ported (wave B2): `Image.resize` (Resample.c: separable filters with fixed-point
//! coefficients, horizontal pass then vertical), `Image.reduce` (Reduce.c: box averages) and the
//! premultiplied path Pillow takes for RGBA. Same integer maths in the same order, so the results are
//! identical to Pillow's (tests/stage: resize cases, the size maths, thumbnails). Rows run in parallel.

use crate::px::{Px, View};
use rayon::prelude::*;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Bilinear,
    Lanczos,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Bilinear => 1.0,
            Filter::Lanczos => 3.0,
        }
    }

    fn eval(self, x: f64) -> f64 {
        match self {
            Filter::Bilinear => {
                let x = x.abs();
                if x < 1.0 {
                    1.0 - x
                } else {
                    0.0
                }
            }
            Filter::Lanczos => {
                if (-3.0..3.0).contains(&x) {
                    sinc(x) * sinc(x / 3.0)
                } else {
                    0.0
                }
            }
        }
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * std::f64::consts::PI;
    x.sin() / x
}

const PRECISION_BITS: u32 = 32 - 8 - 2;

#[inline]
fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// One pass's filter: per output pixel the first input index, the count, and `ksize` fixed-point weights.
struct Coeffs {
    ksize: usize,
    bounds: Vec<(usize, usize)>,
    k: Vec<i32>,
}

/// precompute_coeffs + normalize_coeffs_8bpc. `in0`, `in1`: the box edges on this axis (floats, as Pillow).
fn coeffs(in_size: usize, in0: f32, in1: f32, out_size: usize, f: Filter) -> Coeffs {
    let scale = (in1 - in0) as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = f.support() * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let inv = 1.0 / filterscale;
    let mut bounds = Vec::with_capacity(out_size);
    let mut k = vec![0i32; out_size * ksize];
    let mut pre = vec![0f64; ksize];
    for xx in 0..out_size {
        let center = in0 as f64 + (xx as f64 + 0.5) * scale;
        let xmin = ((center - support + 0.5) as i32).max(0);
        let xmax = ((center + support + 0.5) as i32).min(in_size as i32) - xmin;
        let n = xmax.max(0) as usize;
        let mut ww = 0.0;
        for (x, p) in pre.iter_mut().enumerate().take(n) {
            let w = f.eval(((x as i32 + xmin) as f64 - center + 0.5) * inv);
            *p = w;
            ww += w;
        }
        let kk = &mut k[xx * ksize..(xx + 1) * ksize];
        for x in 0..ksize {
            let mut v = if x < n { pre[x] } else { 0.0 };
            if x < n && ww != 0.0 {
                v /= ww;
            }
            let s = v * (1u32 << PRECISION_BITS) as f64;
            kk[x] = if v < 0.0 { (-0.5 + s) as i32 } else { (0.5 + s) as i32 };
        }
        bounds.push((xmin as usize, n));
    }
    Coeffs { ksize, bounds, k }
}

// The vertical pass's inner loop is compiled twice: for the baseline CPU and with AVX2 (picked at run
// time); the horizontal pass has an SSE4.1 kernel (one tap = one 4-lane multiply-add). Same integer
// maths either way, so the same pixels.
#[cfg(target_arch = "x86_64")]
fn has(feature: &'static str) -> bool {
    static AVX2: OnceLock<bool> = OnceLock::new();
    static SSE41: OnceLock<bool> = OnceLock::new();
    match feature {
        "avx2" => *AVX2.get_or_init(|| is_x86_feature_detected!("avx2")),
        _ => *SSE41.get_or_init(|| is_x86_feature_detected!("sse4.1")),
    }
}

/// One output row of the horizontal pass (Pillow's loop: one sum per channel).
#[inline(always)]
fn h_row<const C: usize>(inp: &[u8], line: &mut [u8], cf: &Coeffs) {
    for (xx, (&(xmin, n), o)) in cf.bounds.iter().zip(line.chunks_exact_mut(C)).enumerate() {
        let k = &cf.k[xx * cf.ksize..xx * cf.ksize + n];
        let px = &inp[xmin * C..(xmin + n) * C];
        let mut ss = [1i32 << (PRECISION_BITS - 1); C];
        for (p, &w) in px.chunks_exact(C).zip(k) {
            for ch in 0..C {
                ss[ch] = ss[ch].wrapping_add((p[ch] as i32).wrapping_mul(w));
            }
        }
        for ch in 0..C {
            o[ch] = clip8(ss[ch]);
        }
    }
}

/// The same with SSE4.1, four taps at a time: 4 pixels loaded at once and split into channels
/// (pshufb), each channel's 4 bytes widened to i32 and multiplied by the 4 weights, one vector sum per
/// channel, added up at the end (integer sums: any order gives Pillow's result). The last taps of a
/// pixel (and where the 16-byte load would leave the row) as in the scalar loop.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn h_row_sse41<const C: usize>(inp: &[u8], line: &mut [u8], cf: &Coeffs) {
    use std::arch::x86_64::*;
    // bytes of 4 pixels → R0..R3 G0..G3 B0..B3 (A0..A3)
    let mask = if C == 3 {
        _mm_setr_epi8(0, 3, 6, 9, 1, 4, 7, 10, 2, 5, 8, 11, -1, -1, -1, -1)
    } else {
        _mm_setr_epi8(0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15)
    };
    for (xx, (&(xmin, n), o)) in cf.bounds.iter().zip(line.chunks_exact_mut(C)).enumerate() {
        let k = &cf.k[xx * cf.ksize..xx * cf.ksize + n];
        let start = xmin * C;
        let mut acc = [_mm_setzero_si128(); 4];
        let mut x = 0;
        while x + 4 <= n && start + x * C + 16 <= inp.len() {
            // SAFETY: 16 bytes from start + x*C are inside `inp` (checked above); k[x..x+4] exists
            let v = _mm_shuffle_epi8(_mm_loadu_si128(inp.as_ptr().add(start + x * C) as *const __m128i), mask);
            let w = _mm_loadu_si128(k.as_ptr().add(x) as *const __m128i);
            for (ch, a) in acc.iter_mut().enumerate().take(C) {
                let c = match ch {
                    0 => v,
                    1 => _mm_srli_si128(v, 4),
                    2 => _mm_srli_si128(v, 8),
                    _ => _mm_srli_si128(v, 12),
                };
                *a = _mm_add_epi32(*a, _mm_mullo_epi32(_mm_cvtepu8_epi32(c), w));
            }
            x += 4;
        }
        let mut ss = [1i32 << (PRECISION_BITS - 1); C];
        for ch in 0..C {
            let mut r = [0i32; 4];
            _mm_storeu_si128(r.as_mut_ptr() as *mut __m128i, acc[ch]);
            ss[ch] = ss[ch].wrapping_add(r[0]).wrapping_add(r[1]).wrapping_add(r[2]).wrapping_add(r[3]);
        }
        for (p, &w) in inp[start + x * C..start + n * C].chunks_exact(C).zip(&k[x..]) {
            for ch in 0..C {
                ss[ch] = ss[ch].wrapping_add((p[ch] as i32).wrapping_mul(w));
            }
        }
        for ch in 0..C {
            o[ch] = clip8(ss[ch]);
        }
    }
}

fn horizontal(src: View, row0: usize, rows: usize, cf: &Coeffs) -> Px {
    let (c, ow) = (src.c, cf.bounds.len());
    let mut out = Px::new(ow, rows, c);
    #[cfg(target_arch = "x86_64")]
    let fast = has("sse4.1");
    out.data.par_chunks_mut(ow * c).enumerate().for_each(|(y, line)| {
        let inp = src.row(y + row0);
        #[cfg(target_arch = "x86_64")]
        if fast {
            // SAFETY: the CPU has SSE4.1 (checked at run time)
            unsafe {
                if c == 3 { h_row_sse41::<3>(inp, line, cf) } else { h_row_sse41::<4>(inp, line, cf) }
            }
            return;
        }
        if c == 3 { h_row::<3>(inp, line, cf) } else { h_row::<4>(inp, line, cf) }
    });
    out
}

/// One output row of the vertical pass, in column blocks (the sums stay in the L1 cache).
#[inline(always)]
fn v_row(src: View, line: &mut [u8], ymin: usize, k: &[i32]) {
    const B: usize = 512;
    let mut acc = [0i32; B];
    let n = line.len();
    let mut x0 = 0;
    while x0 < n {
        let len = B.min(n - x0);
        let acc = &mut acc[..len];
        acc.fill(1i32 << (PRECISION_BITS - 1));
        for (y, &wt) in k.iter().enumerate() {
            let r = &src.row(y + ymin)[x0..x0 + len];
            for (a, &p) in acc.iter_mut().zip(r) {
                *a = a.wrapping_add((p as i32).wrapping_mul(wt));
            }
        }
        for (o, &a) in line[x0..x0 + len].iter_mut().zip(acc.iter()) {
            *o = clip8(a);
        }
        x0 += len;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn v_row_avx2(src: View, line: &mut [u8], ymin: usize, k: &[i32]) {
    v_row(src, line, ymin, k)
}

fn vertical(src: View, cf: &Coeffs) -> Px {
    let (c, w, oh) = (src.c, src.w, cf.bounds.len());
    let mut out = Px::new(w, oh, c);
    #[cfg(target_arch = "x86_64")]
    let fast = has("avx2");
    out.data.par_chunks_mut(w * c).enumerate().for_each(|(yy, line)| {
        let (ymin, n) = cf.bounds[yy];
        let k = &cf.k[yy * cf.ksize..yy * cf.ksize + n];
        #[cfg(target_arch = "x86_64")]
        if fast {
            // SAFETY: the CPU has AVX2 (checked at run time)
            unsafe { v_row_avx2(src, line, ymin, k) };
            return;
        }
        v_row(src, line, ymin, k)
    });
    out
}

/// ImagingResample (the C function): `src` → xsize × ysize from `bx` = (x0, y0, x1, y1), no premultiplying.
fn resample_inner(src: View, xsize: usize, ysize: usize, f: Filter, bx: [f32; 4]) -> Px {
    let need_h = xsize != src.w || bx[0] != 0.0 || bx[2] != xsize as f32;
    let need_v = ysize != src.h || bx[1] != 0.0 || bx[3] != ysize as f32;
    let mut cv = coeffs(src.h, bx[1], bx[3], ysize, f);
    let first = cv.bounds[0].0;
    let last = cv.bounds[ysize - 1].0 + cv.bounds[ysize - 1].1;
    let mut cur: Option<Px> = None;
    if need_h {
        let ch = coeffs(src.w, bx[0], bx[2], xsize, f);
        for b in cv.bounds.iter_mut() {
            b.0 -= first;
        }
        cur = Some(horizontal(src, first, last - first, &ch));
    }
    if need_v {
        return vertical(cur.as_ref().map(|p| p.view()).unwrap_or(src), &cv);
    }
    cur.unwrap_or_else(|| src.to_px())
}

/// Pillow's crop: the box may reach past the image (filled with 0).
pub fn crop(src: View, left: i64, top: i64, right: i64, bottom: i64) -> Px {
    let (w, h) = ((right - left).max(0) as usize, (bottom - top).max(0) as usize);
    let c = src.c;
    let mut out = Px::new(w, h, c);
    if w == 0 || h == 0 {
        return out;
    }
    let x0 = left.clamp(0, src.w as i64);
    let x1 = right.clamp(0, src.w as i64);
    if x1 <= x0 {
        return out;
    }
    let n = (x1 - x0) as usize * c;
    let dx = (x0 - left) as usize * c;
    out.data.par_chunks_mut(w * c).enumerate().for_each(|(y, line)| {
        let sy = top + y as i64;
        if sy >= 0 && sy < src.h as i64 {
            let r = src.row(sy as usize);
            line[dx..dx + n].copy_from_slice(&r[x0 as usize * c..x0 as usize * c + n]);
        }
    });
    out
}

/// _imaging.c `resize`: a whole-pixel box of the output's size is a plain crop.
fn core_resize(src: View, xsize: usize, ysize: usize, f: Filter, bx: [f32; 4]) -> Px {
    if bx[0] == bx[0].trunc() && bx[2] - bx[0] == xsize as f32 && bx[1] == bx[1].trunc() && bx[3] - bx[1] == ysize as f32 {
        return crop(src, bx[0] as i64, bx[1] as i64, bx[2] as i64, bx[3] as i64);
    }
    resample_inner(src, xsize, ysize, f, bx)
}

/// RGBA → RGBa (Pillow's rgbA2rgba: MULDIV255).
pub fn premultiply(p: View) -> Px {
    let mut o = p.to_px();
    if p.c == 4 {
        o.data.par_chunks_mut(4).for_each(|q| {
            let a = q[3] as u32;
            for v in &mut q[..3] {
                let t = *v as u32 * a + 128;
                *v = (((t >> 8) + t) >> 8) as u8;
            }
        });
    }
    o
}

/// RGBa → RGBA (Pillow's rgba2rgbA: integer division, alpha 0 / 255 unchanged).
pub fn unpremultiply(mut p: Px) -> Px {
    if p.c == 4 {
        p.data.par_chunks_mut(4).for_each(|q| {
            let a = q[3] as u32;
            if a != 0 && a != 255 {
                for v in &mut q[..3] {
                    *v = ((255 * *v as u32) / a).min(255) as u8;
                }
            }
        });
    }
    p
}

/// `Image.resize(size, filter, box, reducing_gap)` for RGB / RGBA (box: Python floats, None = whole image).
pub fn resize(src: View, size: (usize, usize), f: Filter, bx: Option<[f64; 4]>, reducing_gap: Option<f64>) -> Px {
    let full = [0.0, 0.0, src.w as f64, src.h as f64];
    let mut bx = bx.unwrap_or(full);
    if (src.w, src.h) == size && bx == full {
        return src.to_px();
    }
    if src.alpha() {
        // Pillow: RGBA goes through RGBa, without the reducing step
        let pm = premultiply(src);
        return unpremultiply(resize_rgb_path(pm.view(), size, f, bx));
    }
    let mut cur: Option<Px> = None;
    if let Some(gap) = reducing_gap {
        let fx = (((bx[2] - bx[0]) / size.0 as f64 / gap) as i64).max(1) as usize;
        let fy = (((bx[3] - bx[1]) / size.1 as f64 / gap) as i64).max(1) as usize;
        if fx > 1 || fy > 1 {
            let rb = safe_box(src, size, f, bx);
            cur = Some(reduce(src, fx, fy, rb));
            bx = [
                (bx[0] - rb[0] as f64) / fx as f64,
                (bx[1] - rb[1] as f64) / fy as f64,
                (bx[2] - rb[0] as f64) / fx as f64,
                (bx[3] - rb[1] as f64) / fy as f64,
            ];
        }
    }
    resize_rgb_path(cur.as_ref().map(|p| p.view()).unwrap_or(src), size, f, bx)
}

fn resize_rgb_path(src: View, size: (usize, usize), f: Filter, bx: [f64; 4]) -> Px {
    let b = bx.map(|v| v as f32);
    if src.h > src.w * 100 && size.1 < src.h {
        let t = core_resize(src, src.w, size.1, f, [0.0, b[1], src.w as f32, b[3]]);
        return core_resize(t.view(), size.0, size.1, f, [b[0], 0.0, b[2], size.1 as f32]);
    }
    core_resize(src, size.0, size.1, f, b)
}

/// Image._get_safe_box
fn safe_box(src: View, size: (usize, usize), f: Filter, bx: [f64; 4]) -> [usize; 4] {
    let fs = f.support() - 0.5;
    let sx = fs * ((bx[2] - bx[0]) / size.0 as f64);
    let sy = fs * ((bx[3] - bx[1]) / size.1 as f64);
    [
        ((bx[0] - sx) as i64).max(0) as usize,
        ((bx[1] - sy) as i64).max(0) as usize,
        ((bx[2] + sx).ceil() as i64).min(src.w as i64) as usize,
        ((bx[3] + sy).ceil() as i64).min(src.h as i64) as usize,
    ]
}

/// Reduce.c division_UINT32 (float maths, as there)
fn multiplier(n: usize) -> u32 {
    let max_dividend = ((1u32 << 8) * n as u32) as f32;
    (4294967296.0f32 / max_dividend) as u32
}

/// `Image.reduce((fx, fy), box)` for RGB (box = x0, y0, x1, y1 in whole pixels): box averages, the last
/// row / column averaging what is left.
pub fn reduce(src: View, fx: usize, fy: usize, rb: [usize; 4]) -> Px {
    if fx == 1 && fy == 1 {
        return crop(src, rb[0] as i64, rb[1] as i64, rb[2] as i64, rb[3] as i64);
    }
    if src.alpha() {
        return unpremultiply(reduce(premultiply(src).view(), fx, fy, rb));
    }
    let (bw, bh) = (rb[2] - rb[0], rb[3] - rb[1]);
    let (ow, oh) = (bw.div_ceil(fx), bh.div_ceil(fy));
    let c = src.c;
    let mut out = Px::new(ow, oh, c);
    out.data.par_chunks_mut(ow * c).enumerate().for_each(|(y, line)| {
        let y0 = rb[1] + y * fy;
        let ny = fy.min(rb[1] + bh - y0);
        for x in 0..ow {
            let x0 = rb[0] + x * fx;
            let nx = fx.min(rb[0] + bw - x0);
            let n = nx * ny;
            let (mul, amend) = (multiplier(n), (n / 2) as u32);
            let mut ss = [amend; 4];
            for yy in y0..y0 + ny {
                let r = src.row(yy);
                for xx in x0..x0 + nx {
                    for ch in 0..c {
                        ss[ch] += r[xx * c + ch] as u32;
                    }
                }
            }
            for ch in 0..c {
                line[x * c + ch] = (ss[ch].wrapping_mul(mul) >> 24) as u8;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipliers() {
        assert_eq!(multiplier(4), 1 << 22);
        assert_eq!(multiplier(1), 1 << 24);
        // (255 * n + n / 2) * mul must not wrap
        for n in 1..400usize {
            assert!((255 * n as u64 + n as u64 / 2) * (multiplier(n) as u64) < 1 << 32);
        }
    }

    #[test]
    fn identity_and_crop() {
        let p = Px::from_vec(3, 2, 3, (0..18).collect());
        assert_eq!(resize(p.view(), (3, 2), Filter::Lanczos, None, None), p);
        let c = crop(p.view(), -1, 1, 2, 3);
        assert_eq!(c.data, vec![0, 0, 0, 9, 10, 11, 12, 13, 14, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    #[test]
    #[ignore]
    fn bench_resize() {
        let (w, h) = (7952usize, 5304usize);
        let data: Vec<u8> = (0..w * h * 3).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let v = View::new(w, h, 3, &data);
        for (ow, oh) in [(1920, 5304), (7952, 1280)] {
            for _ in 0..2 {
                let t = std::time::Instant::now();
                let r = resize(v, (ow, oh), Filter::Lanczos, None, None);
                eprintln!("{ow}x{oh}: {:?} {}", t.elapsed(), r.data[12345]);
            }
        }
    }
}
