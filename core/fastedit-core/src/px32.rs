//! The fast per-pixel engine: the chain's per-pixel steps (Point, Adjustments incl. Mid/Detail,
//! Curves, the Filter without Perceptual Saturation, Film) on chunks of pixels stored as separate
//! R / G / B arrays, with every table lookup in a loop of its own, so the compiler can vectorize
//! the arithmetic (SSE on the PC, simd128 in WebAssembly). About 2x the old pixel-at-a-time loop.
//!
//! Same formulas, stage order and float type (f64) as the reference in edit.rs (kept as the test
//! oracle), so the result is bit-identical, with one exception: Mid/Detail's fade-in weight
//! min(1, (y / K) ^ GAMMA) comes from a table (4097 entries, linear in between) instead of a pow
//! per pixel, which can move a pixel by 1 level. (F = f32 was tried: ~10 % faster, but ±1 at many
//! stages, and later steps can amplify that.)

use crate::chain::Step;
use crate::edit::{MD_GAIN, MD_GAMMA, MD_K};

/// Pixels per chunk.
pub(crate) const N: usize = 64;

/// the engine's float type
pub(crate) type F = f64;

pub(crate) struct Chunk {
    pub r: [F; N],
    pub g: [F; N],
    pub b: [F; N],
}

impl Chunk {
    pub fn new() -> Self {
        Chunk { r: [0.0; N], g: [0.0; N], b: [0.0; N] }
    }
    /// interleaved RGB bytes (up to N pixels) in
    #[inline(always)]
    pub fn load(&mut self, px: &[u8]) {
        for (i, p) in px.chunks_exact(3).enumerate() {
            self.r[i] = p[0] as F;
            self.g[i] = p[1] as F;
            self.b[i] = p[2] as F;
        }
    }
    /// the final pixels: RGB or RGBA (alpha 255) bytes, counting the UI's histograms
    #[inline(always)]
    pub fn store_out(&self, px: &mut [u8], ob: usize, hist: &mut Option<crate::Hist>) {
        for (i, p) in px.chunks_exact_mut(ob).enumerate() {
            let o = [self.r[i] as u8, self.g[i] as u8, self.b[i] as u8];
            p[0] = o[0];
            p[1] = o[1];
            p[2] = o[2];
            if ob == 4 {
                p[3] = 255;
            }
            if let Some(h) = hist {
                h.add(o, o);
            }
        }
    }
    /// back to interleaved RGB bytes (values are whole numbers in 0..=255 here)
    #[inline(always)]
    pub fn store(&self, px: &mut [u8]) {
        for (i, p) in px.chunks_exact_mut(3).enumerate() {
            p[0] = self.r[i] as u8;
            p[1] = self.g[i] as u8;
            p[2] = self.b[i] as u8;
        }
    }
}

/// clip(x, 0, 255) then truncate, kept as a float
#[inline(always)]
fn u8c(x: F) -> F {
    (x.clamp(0.0, 255.0) as i32) as F
}

/// clip(floor(x + 0.5), 0, 255)
#[inline(always)]
fn rnd(x: F) -> F {
    ((x + 0.5).clamp(0.0, 255.0) as i32) as F
}

#[inline(always)]
fn luma(r: F, g: F, b: F) -> F {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// min(floor(y * 16 + 0.5), 4095) for y >= 0
#[inline(always)]
fn t16(y: F) -> usize {
    (y * 16.0 + 0.5).clamp(0.0, 4095.0) as usize
}

pub(crate) struct Tone {
    t: Vec<F>,
    ratio: bool,
}

/// one step, its tables as F
pub(crate) enum Op {
    Point { lv: bool, k: F, o: F, gains: Option<[F; 3]> },
    Adjust {
        /// Mid/Detail amount (0 = off) and its fade-in weight by luma (4097 entries over 0..=1)
        md: F,
        mdw: Vec<F>,
        wb: Option<[[F; 3]; 3]>,
        con: Option<Vec<F>>,
        tones: Vec<Tone>,
        boost: Option<Vec<F>>,
        sat: Option<F>,
    },
    Curves([[F; 256]; 3]),
    Filter { s: F, op: F, w: Option<[F; 3]> },
    Film { n: usize, d: Option<Vec<F>>, t1: Option<Vec<F>>, op: F },
}

fn v32(t: &[f64]) -> Vec<F> {
    t.iter().map(|&v| v as F).collect()
}

/// min(1, ((y / K) ^ GAMMA)) on a grid of y = i / 4096, i = 0..=4096 (linear in between)
fn md_weights() -> Vec<F> {
    (0..=4096).map(|i| libm::pow(i as f64 / 4096.0 / MD_K, MD_GAMMA).min(1.0) as F).collect()
}

impl Op {
    /// The engine's form of a per-pixel step (Step::per_pixel); None for the others.
    pub fn from_step(s: &Step) -> Option<Op> {
        Some(match s {
            Step::Point { k, o, gains } => Op::Point {
                lv: *k != 1.0 || *o != 0.0, k: *k as F, o: *o as F,
                gains: gains.map(|g| [g[0] as F, g[1] as F, g[2] as F]),
            },
            Step::Adjust(a) => Op::Adjust {
                md: a.md as F,
                mdw: if a.md != 0.0 { md_weights() } else { Vec::new() },
                wb: a.wb.map(|w| w.map(|row| row.map(|v| v as F))),
                con: a.con.map(v32),
                tones: a.tones.iter().map(|t| Tone { t: v32(t.table), ratio: t.ratio }).collect(),
                boost: a.boost.map(v32),
                sat: a.sat.map(|s| s as F),
            },
            Step::Curves(l) => Op::Curves(l.map(|c| c.map(|v| v as F))),
            Step::Bw(bw) if bw.ps.is_none() => Op::Filter {
                s: bw.s as F, op: bw.op as F, w: bw.w.map(|w| [w[0] as F, w[1] as F, w[2] as F]),
            },
            Step::Film(fm) => Op::Film { n: fm.n, d: fm.d.map(v32), t1: fm.t1.map(v32), op: fm.op as F },
            _ => return None,
        })
    }

    /// Runs on the chunk; `base` = Mid/Detail's smoothed luma of these pixels (only read when md != 0).
    #[inline(always)]
    pub fn run(&self, c: &mut Chunk, base: &[f32]) {
        match self {
            Op::Point { lv, k, o, gains } => point(c, *lv, *k, *o, *gains),
            Op::Adjust { md, mdw, wb, con, tones, boost, sat } => {
                if *md != 0.0 {
                    mid_detail(c, *md, mdw, base);
                }
                if let Some(w) = wb {
                    for i in 0..N {
                        let (r, g, b) = (c.r[i], c.g[i], c.b[i]);
                        c.r[i] = u8c(r * w[0][0] + g * w[0][1] + b * w[0][2]);
                        c.g[i] = u8c(r * w[1][0] + g * w[1][1] + b * w[1][2]);
                        c.b[i] = u8c(r * w[2][0] + g * w[2][1] + b * w[2][2]);
                    }
                }
                if let Some(t) = con {
                    contrast(c, t);
                }
                for t in tones {
                    tone(c, t);
                }
                if let Some(t) = boost {
                    color_boost(c, t);
                }
                if let Some(s) = sat {
                    for i in 0..N {
                        let (r, g, b) = (c.r[i], c.g[i], c.b[i]);
                        let y = luma(r, g, b);
                        c.r[i] = u8c(y + (r - y) * s);
                        c.g[i] = u8c(y + (g - y) * s);
                        c.b[i] = u8c(y + (b - y) * s);
                    }
                }
            }
            Op::Curves(l) => {
                for i in 0..N {
                    c.r[i] = at(&l[0], c.r[i] as usize);
                    c.g[i] = at(&l[1], c.g[i] as usize);
                    c.b[i] = at(&l[2], c.b[i] as usize);
                }
            }
            Op::Filter { s, op, w } => filter(c, *s, *op, *w),
            Op::Film { n, d, t1, op } => film(c, *n, d.as_deref(), t1.as_deref(), *op),
        }
    }
}

#[inline(always)]
fn point(c: &mut Chunk, lv: bool, k: F, o: F, gains: Option<[F; 3]>) {
    let gn = gains.unwrap_or([1.0; 3]);
    for i in 0..N {
        let (mut r, mut g, mut b) = (c.r[i], c.g[i], c.b[i]);
        if lv {
            let l = luma(r, g, b);
            let ra = (l * k + o).clamp(0.0, 255.0) / l.max(1.0);
            r *= ra;
            g *= ra;
            b *= ra;
        }
        c.r[i] = u8c(r * gn[0]);
        c.g[i] = u8c(g * gn[1]);
        c.b[i] = u8c(b * gn[2]);
    }
}

#[inline(always)]
fn mid_detail(c: &mut Chunk, amount: F, w: &[F], base: &[f32]) {
    let a = amount * MD_GAIN as F * 255.0;
    let n = N.min(base.len());
    let (mut j, mut fr) = ([0usize; N], [0 as F; N]);
    for i in 0..n {
        let p = (luma(c.r[i], c.g[i], c.b[i]) / 255.0).clamp(0.0, 1.0) * 4096.0;
        j[i] = (p as usize).min(4095);
        fr[i] = p - j[i] as F;
    }
    let (mut w0, mut w1) = ([0 as F; N], [0 as F; N]);
    for i in 0..n {
        w0[i] = at(w, j[i]);
        w1[i] = at(w, j[i] + 1);
    }
    for i in 0..n {
        let (r, g, b) = (c.r[i], c.g[i], c.b[i]);
        let y = luma(r, g, b) / 255.0;
        let d = a * (w0[i] + (w1[i] - w0[i]) * fr[i]) * (y - base[i] as F);
        c.r[i] = u8c(r + d);
        c.g[i] = u8c(g + d);
        c.b[i] = u8c(b + d);
    }
}

/// gathers t[ix[i]] (scalar), so the loops around it stay vectorizable
#[inline(always)]
fn gather(t: &[F], ix: &[usize; N]) -> [F; N] {
    let mut o = [0 as F; N];
    for i in 0..N {
        o[i] = at(t, ix[i]);
    }
    o
}

/// t[i] without the bounds check (WebAssembly pays for every one). Safe here: every step ends with
/// whole values in 0..=255 (u8c / rnd / u8 tables / Film's final cast), the indices built from them
/// are clamped, and the tables' lengths are checked by Step::validate (fixed arrays otherwise).
#[inline(always)]
fn at(t: &[F], i: usize) -> F {
    debug_assert!(i < t.len());
    unsafe { *t.get_unchecked(i) }
}

#[inline(always)]
fn contrast(c: &mut Chunk, t: &[F]) {
    let (mut ir, mut ig, mut ib, mut iy) = ([0usize; N], [0usize; N], [0usize; N], [0usize; N]);
    for i in 0..N {
        ir[i] = c.r[i] as usize * 16;
        ig[i] = c.g[i] as usize * 16;
        ib[i] = c.b[i] as usize * 16;
        iy[i] = t16(luma(c.r[i], c.g[i], c.b[i]));
    }
    let (c0, c1, c2, cy) = (gather(t, &ir), gather(t, &ig), gather(t, &ib), gather(t, &iy));
    for i in 0..N {
        let d = cy[i] - (0.2126 * c0[i] + 0.7152 * c1[i] + 0.0722 * c2[i]);
        c.r[i] = u8c(c0[i] + d);
        c.g[i] = u8c(c1[i] + d);
        c.b[i] = u8c(c2[i] + d);
    }
}

#[inline(always)]
fn tone(c: &mut Chunk, t: &Tone) {
    let mut iy = [0usize; N];
    for i in 0..N {
        iy[i] = t16(luma(c.r[i], c.g[i], c.b[i]));
    }
    let y2 = gather(&t.t, &iy);
    if t.ratio {
        for i in 0..N {
            let (r, g, b) = (c.r[i], c.g[i], c.b[i]);
            let q = y2[i] / luma(r, g, b).max(0.5);
            c.r[i] = u8c(r * q);
            c.g[i] = u8c(g * q);
            c.b[i] = u8c(b * q);
        }
    } else {
        for i in 0..N {
            let (r, g, b) = (c.r[i], c.g[i], c.b[i]);
            let d = y2[i] - luma(r, g, b);
            c.r[i] = u8c(r + d);
            c.g[i] = u8c(g + d);
            c.b[i] = u8c(b + d);
        }
    }
}

#[inline(always)]
fn color_boost(c: &mut Chunk, t: &[F]) {
    let mut ix = [0usize; N];
    for i in 0..N {
        let (r, g, b) = (c.r[i], c.g[i], c.b[i]);
        ix[i] = (r.max(g).max(b) - r.min(g).min(b)) as usize;
    }
    let gn = gather(t, &ix);
    for i in 0..N {
        let (r, g, b) = (c.r[i], c.g[i], c.b[i]);
        let y = luma(r, g, b);
        c.r[i] = u8c(y + (r - y) * gn[i]);
        c.g[i] = u8c(y + (g - y) * gn[i]);
        c.b[i] = u8c(y + (b - y) * gn[i]);
    }
}

#[inline(always)]
fn filter(c: &mut Chunk, s: F, op: F, w: Option<[F; 3]>) {
    let wg = w.unwrap_or([1.0; 3]);
    for i in 0..N {
        let f = [c.r[i], c.g[i], c.b[i]];
        let wv = [(f[0] * wg[0]).min(255.0) as i32 as F, (f[1] * wg[1]).min(255.0) as i32 as F,
                  (f[2] * wg[2]).min(255.0) as i32 as F];
        let y = (wv[0] + 2.0 * wv[1] + wv[2]) / 4.0;
        let l0 = (y + (wv[0] - y) * s).clamp(0.0, 255.0);
        let l1 = (y + (wv[1] - y) * s).clamp(0.0, 255.0);
        let l2 = (y + (wv[2] - y) * s).clamp(0.0, 255.0);
        c.r[i] = rnd(f[0] + (l0 - f[0]) * op);
        c.g[i] = rnd(f[1] + (l1 - f[1]) * op);
        c.b[i] = rnd(f[2] + (l2 - f[2]) * op);
    }
}

#[inline(always)]
fn film(c: &mut Chunk, n: usize, d: Option<&[F]>, t1: Option<&[F]>, op: F) {
    let (f0, f1, f2) = (c.r, c.g, c.b);
    if let Some(d) = d {
        let nm1 = (n - 1) as F;
        let top = (n - 2) as F;
        let (mut ix, mut fr) = ([[0usize; N]; 3], [[0 as F; N]; 3]);
        for i in 0..N {
            for (k, v) in [f0[i], f1[i], f2[i]].into_iter().enumerate() {
                let g = v * nm1 / 255.0;
                let ic = (g as i32 as F).min(top);
                ix[k][i] = ic as usize;
                fr[k][i] = g - ic;
            }
        }
        // the 8 corners, in the reference's order (r major, then g, then b)
        let mut acc = [[0 as F; N]; 3];
        for dr in 0..2 {
            for dg in 0..2 {
                for db in 0..2 {
                    let mut kx = [0usize; N];
                    let mut w = [0 as F; N];
                    for i in 0..N {
                        let wr = if dr == 1 { fr[0][i] } else { 1.0 - fr[0][i] };
                        let wg = if dg == 1 { fr[1][i] } else { 1.0 - fr[1][i] };
                        let wb = if db == 1 { fr[2][i] } else { 1.0 - fr[2][i] };
                        w[i] = wr * wg * wb;
                        kx[i] = (((ix[0][i] + dr) * n + (ix[1][i] + dg)) * n + (ix[2][i] + db)) * 3;
                    }
                    for i in 0..N {
                        let k = kx[i];
                        acc[0][i] += w[i] * at(d, k);
                        acc[1][i] += w[i] * at(d, k + 1);
                        acc[2][i] += w[i] * at(d, k + 2);
                    }
                }
            }
        }
        for i in 0..N {
            c.r[i] = rnd(f0[i] + acc[0][i]);
            c.g[i] = rnd(f1[i] + acc[1][i]);
            c.b[i] = rnd(f2[i] + acc[2][i]);
        }
    }
    if let Some(t) = t1 {
        for i in 0..N {
            c.r[i] = at(t, c.r[i] as usize);
            c.g[i] = at(t, c.g[i] as usize);
            c.b[i] = at(t, c.b[i] as usize);
        }
    }
    if op != 1.0 {
        for i in 0..N {
            c.r[i] = rnd(f0[i] + (c.r[i] - f0[i]) * op);
            c.g[i] = rnd(f1[i] + (c.g[i] - f1[i]) * op);
            c.b[i] = rnd(f2[i] + (c.b[i] - f2[i]) * op);
        }
    } else if t1.is_some() {
        // the reference ends with `as u8` (truncate, saturate): a table value need not be whole
        for i in 0..N {
            c.r[i] = c.r[i] as u8 as F;
            c.g[i] = c.g[i] as u8 as F;
            c.b[i] = c.b[i] as u8 as F;
        }
    }
}
