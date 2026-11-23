//! The edit as an ordered list of steps (the right panel's tool order). Each step is one tool:
//! Point correction, Adjustments (with Mid/Detail), Curves, the Perceptual Saturation filter, Film
//! Effect, Super Contrast; the same tool may appear more than once. Sharpen is not a step: it always
//! runs last, on the finished pixels.
//!
//! The maths of every step is the one in edit.rs / ps.rs / sc.rs, unchanged; only the order is free.
//! Runs of per-pixel steps are fused into one pass over the image (px32.rs, chunk by chunk). Steps that need the whole image
//! (Mid/Detail's smoothed luma, Perceptual Saturation's grid, Super Contrast's statistics) start a
//! new pass on the image as it arrives at them, so a step always sees the result of the steps before.
//!
//! Histograms (the UI's): hin = the input of the first Curves step (what the curve editor shows
//! under the curve; the output when there is no Curves step), hout / hmx / hmn = the final pixels.
use crate::edit::{Adjust, Bw, Film, Sc};
#[cfg(test)]
use crate::edit::{adjust_px, film_px, filter_px, md_px, point_px};
use crate::Hist;

pub enum Step<'a> {
    Point { k: f64, o: f64, gains: Option<[f64; 3]> },
    Adjust(Adjust<'a>),
    /// R, G, B tables (the master curve folded in)
    Curves([[u8; 256]; 3]),
    /// The Perceptual Saturation filter (Saturation / Warmth, optionally Perceptual Saturation)
    Bw(Bw<'a>),
    Film(Film<'a>),
    Sc(Sc<'a>),
}

impl Step<'_> {
    /// One pixel at a time, no whole-image pass needed (Mid/Detail and Perceptual Saturation do).
    fn per_pixel(&self) -> bool {
        match self {
            Step::Point { .. } | Step::Curves(_) | Step::Film(_) => true,
            Step::Adjust(a) => a.md == 0.0,
            Step::Bw(bw) => bw.ps.is_none(),
            Step::Sc(_) => false,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Step::Adjust(a) => a.validate(),
            Step::Film(f) => {
                if f.n < 2 {
                    return Err("film: n must be at least 2".into());
                }
                if let Some(d) = f.d {
                    let want = f.n * f.n * f.n * 3;
                    if d.len() != want {
                        return Err(format!("film: d has {} values, need {want}", d.len()));
                    }
                }
                match f.t1 {
                    Some(t) if t.len() < 256 => Err(format!("film t1: {} values, need 256", t.len())),
                    _ => Ok(()),
                }
            }
            Step::Bw(Bw { ps: Some((_, k)), .. }) if k.is_empty() => Err("bw psk: empty kernel".into()),
            Step::Sc(sc) => {
                if sc.dyn_ > 0.0 && sc.dk.is_empty() {
                    return Err("sc dk: empty kernel".into());
                }
                match sc.shl {
                    Some(t) if t.len() < 256 => Err(format!("sc shl: {} values, need 256", t.len())),
                    _ => Ok(()),
                }
            }
            _ => Ok(()),
        }
    }
}

/// A step that owns its tables (what the Python binding builds from its arguments); `step()` lends it
/// out as a `Step`.
pub enum StepBuf {
    Point { k: f64, o: f64, gains: Option<[f64; 3]> },
    Adjust { md: f64, wb: Option<[[f64; 3]; 3]>, con: Option<Vec<f64>>, tones: Vec<(Vec<f64>, bool)>,
             boost: Option<Vec<f64>>, sat: Option<f64> },
    Curves([[u8; 256]; 3]),
    Bw { s: f64, op: f64, w: Option<[f64; 3]>, ps: f64, psk: Vec<f64> },
    Film { n: usize, d: Option<Vec<f64>>, t1: Option<Vec<f64>>, op: f64 },
    Sc { cast: f64, cc: f64, dyn_: f64, op: f64, dk: Vec<f64>, shl: Option<Vec<f64>> },
}

impl StepBuf {
    pub fn step(&self) -> Step<'_> {
        use crate::edit::Tone;
        match self {
            StepBuf::Point { k, o, gains } => Step::Point { k: *k, o: *o, gains: *gains },
            StepBuf::Adjust { md, wb, con, tones, boost, sat } => Step::Adjust(Adjust {
                md: *md,
                wb: *wb,
                con: con.as_deref(),
                tones: tones.iter().map(|(t, r)| Tone { table: t, ratio: *r }).collect(),
                boost: boost.as_deref(),
                sat: *sat,
            }),
            StepBuf::Curves(l) => Step::Curves(*l),
            StepBuf::Bw { s, op, w, ps, psk } => Step::Bw(Bw {
                s: *s,
                op: *op,
                w: *w,
                ps: if *ps != 0.0 { Some((*ps, psk.as_slice())) } else { None },
            }),
            StepBuf::Film { n, d, t1, op } => Step::Film(Film { n: *n, d: d.as_deref(), t1: t1.as_deref(), op: *op }),
            StepBuf::Sc { cast, cc, dyn_, op, dk, shl } => Step::Sc(Sc {
                cast: *cast, cc: *cc, dyn_: *dyn_, op: *op, dk: dk.as_slice(), shl: shl.as_deref(),
            }),
        }
    }
}

/// One pixel through the per-pixel steps in f64 (the reference the f32 engine is measured against).
#[cfg(test)]
#[inline(always)]
fn run_px(mut p: [u8; 3], steps: &[Step], base: f32, hin_at: Option<usize>, hin: &mut Option<Vec<u32>>) -> [u8; 3] {
    for (i, s) in steps.iter().enumerate() {
        if hin_at == Some(i) {
            if let Some(h) = hin {
                h[p[0] as usize] += 1;
                h[256 + p[1] as usize] += 1;
                h[512 + p[2] as usize] += 1;
            }
        }
        p = match s {
            Step::Point { k, o, gains } => point_px(p, *k, *o, *gains),
            Step::Adjust(a) => {
                let q = if a.md != 0.0 { md_px(p, a.md, base) } else { p };
                adjust_px(q, a)
            }
            Step::Curves(l) => [l[0][p[0] as usize], l[1][p[1] as usize], l[2][p[2] as usize]],
            Step::Bw(bw) => filter_px(p, bw),
            Step::Film(fm) => film_px(p, fm),
            Step::Sc(_) => p, // never in a per-pixel run
        };
    }
    p
}

/// What the live preview keeps between renders (one per image):
/// - Mid/Detail's smoothed luma of the image arriving at an Adjustments step, reused while that
///   image is the same (checked by a fingerprint);
/// - a prefix: the image after the first `pre_k` steps, when one of them is a whole-image step
///   (Perceptual Saturation, Super Contrast). While a slider later in the chain moves, the
///   render starts from it instead of redoing those passes. Exact: it is reused only while those
///   steps' parameters are unchanged (compared by hash).
#[derive(Default)]
pub struct MdCache {
    key: Option<(u64, usize)>,
    base: Vec<f32>,
    last: Vec<u64>,
    pre_hashes: Vec<u64>,
    pre_k: usize,
    pre_buf: Vec<u8>,
    pre_hin: Option<Vec<u32>>,
    /// renders that started from the prefix (tests)
    hits: usize,
    /// the working image, kept so a frame doesn't allocate (and touch fresh memory) again
    scratch: Vec<u8>,
    /// the last render's phases, ms (with a clock, see set_clock)
    pub timing: Vec<(&'static str, f64)>,
    /// the whole-image measurements of the last full render, by step (run_region reuses them)
    stats: Vec<Option<Stat>>,
}

/// a whole-image step's measurements
enum Stat {
    Ps(crate::ps::PsGrid),
    Sc(crate::sc::ScStats),
}

impl MdCache {
    /// Forget the cached results (keeps the memory): the next render is a full one.
    pub fn forget(&mut self) {
        self.key = None;
        self.last.clear();
        self.pre_k = 0;
        self.pre_hashes.clear();
    }
}

/// `dst` = `src`, copied band by band on the thread pool (a frame's whole-image copies are
/// otherwise single-threaded memcpys of up to ~100 MB)
fn par_copy(dst: &mut Vec<u8>, src: &[u8], row: usize) {
    dst.resize(src.len(), 0);
    crate::for_each_band(dst, row, |band, row0| band.copy_from_slice(&src[row0 * row..row0 * row + band.len()]));
}

/// FNV-1a, for step parameters
struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf29ce484222325)
    }
    fn u(&mut self, v: u64) {
        self.0 = (self.0 ^ v).wrapping_mul(0x100000001b3);
    }
    fn f(&mut self, v: f64) {
        self.u(v.to_bits());
    }
    fn fs(&mut self, t: &[f64]) {
        self.u(t.len() as u64);
        t.iter().for_each(|v| self.f(*v));
    }
    fn of(&mut self, t: Option<&[f64]>) {
        match t {
            Some(t) => self.fs(t),
            None => self.u(u64::MAX),
        }
    }
}

/// a hash of everything a step's result depends on
fn step_hash(s: &Step) -> u64 {
    let mut h = Fnv::new();
    match s {
        Step::Point { k, o, gains } => {
            h.u(1);
            h.f(*k);
            h.f(*o);
            h.of(gains.as_ref().map(|g| &g[..]));
        }
        Step::Adjust(a) => {
            h.u(2);
            h.f(a.md);
            h.of(a.wb.as_ref().map(|w| w.as_flattened()));
            h.of(a.con);
            h.u(a.tones.len() as u64);
            for t in &a.tones {
                h.fs(t.table);
                h.u(t.ratio as u64);
            }
            h.of(a.boost);
            h.of(a.sat.as_ref().map(core::slice::from_ref));
        }
        Step::Curves(l) => {
            h.u(3);
            l.iter().flatten().for_each(|v| h.u(*v as u64));
        }
        Step::Bw(bw) => {
            h.u(4);
            h.f(bw.s);
            h.f(bw.op);
            h.of(bw.w.as_ref().map(|w| &w[..]));
            match bw.ps {
                Some((a, k)) => {
                    h.f(a);
                    h.fs(k);
                }
                None => h.u(u64::MAX),
            }
        }
        Step::Film(fm) => {
            h.u(5);
            h.u(fm.n as u64);
            h.of(fm.d);
            h.of(fm.t1);
            h.f(fm.op);
        }
        Step::Sc(sc) => {
            h.u(6);
            for v in [sc.cast, sc.cc, sc.dyn_, sc.op] {
                h.f(v);
            }
            h.fs(sc.dk);
            h.of(sc.shl);
        }
    }
    h.0
}

/// whole-image steps worth keeping a prefix for
fn heavy(s: &Step) -> bool {
    matches!(s, Step::Sc(_) | Step::Bw(Bw { ps: Some(_), .. }))
}

/// A fast fingerprint of an image (not cryptographic): FNV-1a per row band, in parallel, then the
/// band hashes in order. It runs on every preview frame with Mid/Detail, so it must not be one long
/// single-threaded scan (36 MP at the zoom cap).
fn fingerprint(buf: &mut [u8], row: usize) -> u64 {
    crate::map_bands(buf, row, |band, _| fnv(band)).into_iter()
        .fold(0xcbf29ce484222325u64, |h, b| (h ^ b).wrapping_mul(0x100000001b3))
}

/// FNV-1a over 8-byte words
fn fnv(buf: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut it = buf.chunks_exact(8);
    for c in &mut it {
        h = (h ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(0x100000001b3);
    }
    for b in it.remainder() {
        h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// The whole edit: `src` is RGB, `dst` has `ob` bytes per pixel (3 = RGB, 4 = RGBA with alpha 255).
pub fn run(src: &[u8], dst: &mut [u8], width: usize, ob: usize, steps: &[Step], sharp: &crate::sharp::Sharp,
           want_hist: bool) -> Option<Hist> {
    run_cached(src, dst, width, ob, steps, sharp, want_hist, None)
}

/// `run` with a Mid/Detail cache (the first Adjustments step with Mid/Detail uses it).
#[allow(clippy::too_many_arguments)]
pub fn run_cached(src: &[u8], dst: &mut [u8], width: usize, ob: usize, steps: &[Step], sharp: &crate::sharp::Sharp,
                  want_hist: bool, mut md_cache: Option<&mut MdCache>) -> Option<Hist> {
    assert!(ob == 3 || ob == 4, "ob must be 3 or 4");
    let row = width * 3;
    if row == 0 {
        return want_hist.then(Hist::default);
    }
    assert!(src.len().is_multiple_of(row), "src is not whole rows");
    let height = src.len() / row;
    assert_eq!(dst.len(), height * width * ob, "dst size");
    let first_curves = steps.iter().position(|s| matches!(s, Step::Curves(_)));
    let mut hin: Option<Vec<u32>> = None;
    let ident = crate::edit::identity_luts();

    // prefix (preview only): start after the steps that are unchanged and already done; plan to keep
    // the image after the last whole-image step before the first step that changed since last time
    let mut i = 0;
    let mut capture = 0;
    let mut buf = Vec::new();
    let mut tm: Vec<(&'static str, f64)> = Vec::new();
    let mut t0 = crate::now();
    let mut mark = |name: &'static str| {
        let t = crate::now();
        tm.push((name, t - t0));
        t0 = t;
    };
    if let Some(c) = md_cache.as_deref_mut() {
        let hashes: Vec<u64> = steps.iter().map(step_hash).collect();
        let changed = (0..hashes.len()).find(|&j| c.last.get(j) != Some(&hashes[j])).unwrap_or(hashes.len());
        capture = steps[..changed].iter().rposition(heavy).map_or(0, |j| j + 1);
        let ok = c.pre_k > 0 && c.pre_buf.len() == src.len() && hashes.len() >= c.pre_k && c.pre_hashes[..] == hashes[..c.pre_k];
        buf = core::mem::take(&mut c.scratch);
        if ok {
            par_copy(&mut buf, &c.pre_buf, row);
            i = c.pre_k;
            c.hits += 1;
            if want_hist {
                hin = c.pre_hin.clone();
            }
        }
        if ok && capture == c.pre_k {
            capture = 0;                         // already kept
        }
        c.last = hashes;
        c.stats.resize_with(steps.len(), || None);
    }
    // `buf` holds the current image once `have` is set; until then the current image is still `src`
    // (a first per-pixel run reads it straight from there, so most frames never copy it)
    let mut have = i > 0;
    if have {
        mark("copy in (kept)");
    }
    let ensure = |buf: &mut Vec<u8>, have: &mut bool| {
        if !*have {
            par_copy(buf, src, row);
            *have = true;
        }
    };

    let keep = |c: Option<&mut MdCache>, buf: &[u8], hin: &Option<Vec<u32>>| {
        if let Some(c) = c {
            c.pre_k = capture;
            c.pre_hashes = c.last[..capture].to_vec();
            par_copy(&mut c.pre_buf, buf, row);
            c.pre_hin = hin.clone();
        }
    };
    let orow = width * ob;
    let mut out_hist: Option<Option<Hist>> = None;     // set when a run wrote the final pixels itself
    while i < steps.len() {
        if capture > 0 && i == capture {
            ensure(&mut buf, &mut have);
            keep(md_cache.as_deref_mut(), &buf, &hin);
        }
        match &steps[i] {
            Step::Bw(bw) if bw.ps.is_some() => {
                ensure(&mut buf, &mut have);
                let g = crate::ps::step(&mut buf, width, height, bw);
                if let Some(c) = md_cache.as_deref_mut() {
                    c.stats[i] = Some(Stat::Ps(g));
                }
                mark("perceptual sat");
                i += 1;
                continue;
            }
            Step::Sc(sc) => {
                ensure(&mut buf, &mut have);
                let (st, _) = crate::sc::finish(&mut buf, width, height, 3, sc, &ident, false);
                if let Some(c) = md_cache.as_deref_mut() {
                    c.stats[i] = Some(Stat::Sc(st));
                }
                mark("super contrast");
                i += 1;
                continue;
            }
            _ => {}
        }
        // a run of per-pixel steps; Mid/Detail may only start a run (its smoothing needs the image
        // as it arrives), so an Adjustments step with Mid/Detail ends the run before it
        let mut j = i + 1;
        while j < steps.len() && steps[j].per_pixel() && j != capture {
            j += 1;
        }
        let md_here = matches!(&steps[i], Step::Adjust(a) if a.md != 0.0);
        let run = &steps[i..j];
        let fresh: Vec<f32>;
        let base: &[f32] = if !md_here {
            &[]
        } else if let Some(c) = md_cache.as_deref_mut() {
            ensure(&mut buf, &mut have);
            let key = (fingerprint(&mut buf, row), buf.len());
            if c.key != Some(key) {
                c.base = crate::md::base(&buf, width, height);
                c.key = Some(key);
            }
            &c.base
        } else {
            fresh = crate::md::base(if have { &buf } else { src }, width, height);
            &fresh
        };
        if md_here {
            mark("mid/detail base");
        }
        let hin_at = match first_curves {
            Some(c) if want_hist && c >= i && c < j => Some(c - i),
            _ => None,
        };
        let ops: Vec<crate::px32::Op> = run.iter().map(|s| crate::px32::Op::from_step(s).expect("per-pixel step")).collect();
        // one chunk through the run (hin counted before the first Curves step)
        let go = |c: &mut crate::px32::Chunk, n: usize, at: usize, h: &mut Option<Vec<u32>>| {
            let b: &[f32] = if md_here { &base[at..at + n] } else { &[] };
            for (k, op) in ops.iter().enumerate() {
                if hin_at == Some(k) {
                    if let Some(h) = h.as_mut() {
                        for i in 0..n {
                            h[c.r[i] as usize] += 1;
                            h[256 + c.g[i] as usize] += 1;
                            h[512 + c.b[i] as usize] += 1;
                        }
                    }
                }
                op.run(c, b);
            }
        };
        const CH: usize = crate::px32::N;
        let last = j == steps.len() && !sharp.active() && !(capture > 0 && capture == steps.len());
        let parts: Vec<Option<Vec<u32>>>;
        if last {
            // the last thing before the output: write the final pixels (+ histograms) directly
            let cur: &[u8] = if have { &buf } else { src };
            let res = crate::map_bands(dst, orow, |band, row0| {
                let mut h = hin_at.map(|_| vec![0u32; 768]);
                let mut hist = want_hist.then(Hist::default);
                let p0 = row0 * width;
                let input = &cur[p0 * 3..p0 * 3 + band.len() / ob * 3];
                let mut c = crate::px32::Chunk::new();
                for (ci, (o, px)) in band.chunks_mut(CH * ob).zip(input.chunks(CH * 3)).enumerate() {
                    let n = px.len() / 3;
                    c.load(px);
                    go(&mut c, n, p0 + ci * CH, &mut h);
                    c.store_out(o, ob, &mut hist);
                }
                (h, hist)
            });
            let (hs, hists): (Vec<_>, Vec<_>) = res.into_iter().unzip();
            parts = hs;
            out_hist = Some(Hist::merge(hists));
        } else if have {
            parts = crate::map_bands(&mut buf, row, |band, row0| {
                let mut h = hin_at.map(|_| vec![0u32; 768]);
                let p0 = row0 * width;
                let mut c = crate::px32::Chunk::new();
                for (ci, px) in band.chunks_mut(CH * 3).enumerate() {
                    let n = px.len() / 3;
                    c.load(px);
                    go(&mut c, n, p0 + ci * CH, &mut h);
                    c.store(px);
                }
                h
            });
        } else {
            // first run: from `src` into `buf` (no separate copy)
            buf.resize(src.len(), 0);
            parts = crate::map_bands(&mut buf, row, |band, row0| {
                let mut h = hin_at.map(|_| vec![0u32; 768]);
                let p0 = row0 * width;
                let input = &src[p0 * 3..p0 * 3 + band.len()];
                let mut c = crate::px32::Chunk::new();
                for (ci, (px, inp)) in band.chunks_mut(CH * 3).zip(input.chunks(CH * 3)).enumerate() {
                    let n = px.len() / 3;
                    c.load(inp);
                    go(&mut c, n, p0 + ci * CH, &mut h);
                    c.store(px);
                }
                h
            });
            have = true;
        }
        if hin_at.is_some() {
            let mut h = vec![0u32; 768];
            for part in parts.into_iter().flatten() {
                h.iter_mut().zip(part).for_each(|(a, b)| *a += b);
            }
            hin = Some(h);
        }
        mark(if last { "per-pixel steps + write out" } else { "per-pixel steps" });
        i = j;
    }

    if capture > 0 && capture == steps.len() {
        ensure(&mut buf, &mut have);
        keep(md_cache.as_deref_mut(), &buf, &hin);
    }

    if sharp.active() {
        ensure(&mut buf, &mut have);
        if want_hist && hin.is_none() {        // no Curves step: hin = the image before Sharpen
            let mut h = vec![0u32; 768];
            for p in buf.chunks_exact(3) {
                h[p[0] as usize] += 1;
                h[256 + p[1] as usize] += 1;
                h[512 + p[2] as usize] += 1;
            }
            hin = Some(h);
        }
        crate::sharp::apply(&mut buf, width, height, 3, sharp);
        mark("sharpen");
    }

    // write out (RGB or RGBA), counting the output histograms, unless the last run did
    let h = match out_hist {
        Some(h) => h,
        None => {
            let cur: &[u8] = if have { &buf } else { src };
            let h = Hist::merge(crate::map_bands(dst, orow, |band, row0| {
                let mut hist = want_hist.then(Hist::default);
                let s = &cur[row0 * row..row0 * row + band.len() / ob * 3];
                for (o, p) in band.chunks_exact_mut(ob).zip(s.chunks_exact(3)) {
                    let px = [p[0], p[1], p[2]];
                    crate::write_px(o, px, &ident, &mut hist);
                }
                hist
            }));
            mark("write out + histograms");
            h
        }
    };
    if let Some(c) = md_cache {
        c.scratch = buf;                         // reused by the next frame
        c.timing = tm;
    }
    h.map(|mut h| {
        if let Some(hi) = hin {
            h.hin = hi;          // else hin = the output (the histograms counted the same pixels)
        }
        h
    })
}

/// Pixels around a draft's rectangle that are rendered too, for Sharpen's neighbourhood.
const MARGIN: usize = 128;

/// A draft of part of the image for the live preview while a control is being dragged (zoomed in,
/// so the rectangle is much smaller than the image): the rectangle (`rx`, `ry`, `rw`, `rh`) of
/// what run_cached would give for the whole `fw` x `fh` image `src`, written to `dst` (RGBA, `rw` x
/// `rh`). Whole-image steps use the measurements of the last full render (`c`), so the part matches
/// the full image exactly while the steps before them are unchanged, and is close while they are
/// being dragged (the full render after the drag settles it). Sharpen sees a margin around the
/// rectangle. No histograms. Returns false (nothing written) when the measurements are missing.
#[allow(clippy::too_many_arguments)]
pub fn run_region(src: &[u8], fw: usize, fh: usize, (rx, ry, rw, rh): (usize, usize, usize, usize), dst: &mut [u8],
                  steps: &[Step], sharp: &crate::sharp::Sharp, c: &mut MdCache) -> bool {
    if rw == 0 || rh == 0 || rx + rw > fw || ry + rh > fh || src.len() != fw * fh * 3 || dst.len() != rw * rh * 4
        || c.stats.len() != steps.len() {
        return false;
    }
    for (k, s) in steps.iter().enumerate() {
        let ok = match s {
            Step::Bw(Bw { ps: Some(_), .. }) => matches!(c.stats[k], Some(Stat::Ps(_))),
            Step::Sc(_) => matches!(c.stats[k], Some(Stat::Sc(_))),
            Step::Adjust(a) if a.md != 0.0 => c.base.len() == fw * fh,
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    let mut tm: Vec<(&'static str, f64)> = Vec::new();
    let mut t0 = crate::now();
    let mut mark = |name: &'static str| {
        let t = crate::now();
        tm.push((name, t - t0));
        t0 = t;
    };
    let m = if sharp.active() { MARGIN } else { 0 };
    let (x0, y0) = (rx.saturating_sub(m), ry.saturating_sub(m));
    let (x1, y1) = ((rx + rw + m).min(fw), (ry + rh + m).min(fh));
    let (w, h) = (x1 - x0, y1 - y0);
    let row = w * 3;
    let mut buf = core::mem::take(&mut c.scratch);
    buf.resize(w * h * 3, 0);
    crate::for_each_band(&mut buf, row, |band, r0| {
        for (k, out) in band.chunks_exact_mut(row).enumerate() {
            let y = y0 + r0 + k;
            out.copy_from_slice(&src[(y * fw + x0) * 3..(y * fw + x1) * 3]);
        }
    });
    let ident = crate::edit::identity_luts();
    let mut i = 0;
    while i < steps.len() {
        match (&steps[i], &c.stats[i]) {
            (Step::Bw(bw), Some(Stat::Ps(g))) if bw.ps.is_some() => {
                crate::ps::apply_grid(&mut buf, w, h, bw, g, None, (x0, y0), (fw, fh));
                mark("perceptual sat");
                i += 1;
                continue;
            }
            (Step::Sc(sc), Some(Stat::Sc(st))) => {
                crate::sc::apply(&mut buf, w, h, 3, sc, st, &ident, false, (x0, y0), (fw, fh));
                mark("super contrast");
                i += 1;
                continue;
            }
            _ => {}
        }
        let mut j = i + 1;
        while j < steps.len() && steps[j].per_pixel() {
            j += 1;
        }
        let md_here = matches!(&steps[i], Step::Adjust(a) if a.md != 0.0);
        let base: Vec<f32> = if md_here {
            (0..h).flat_map(|y| c.base[(y0 + y) * fw + x0..(y0 + y) * fw + x1].iter().copied()).collect()
        } else {
            Vec::new()
        };
        let ops: Vec<crate::px32::Op> = steps[i..j].iter().map(|s| crate::px32::Op::from_step(s).expect("per-pixel step")).collect();
        const CH: usize = crate::px32::N;
        crate::for_each_band(&mut buf, row, |band, r0| {
            let p0 = r0 * w;
            let mut ch = crate::px32::Chunk::new();
            for (ci, px) in band.chunks_mut(CH * 3).enumerate() {
                let n = px.len() / 3;
                let at = p0 + ci * CH;
                let b: &[f32] = if md_here { &base[at..at + n] } else { &[] };
                ch.load(px);
                for op in &ops {
                    op.run(&mut ch, b);
                }
                ch.store(px);
            }
        });
        mark("per-pixel steps");
        i = j;
    }
    if sharp.active() {
        let sp = crate::sharp::Sharp { href: fh, ..sharp.clone() };
        crate::sharp::apply(&mut buf, w, h, 3, &sp);
        mark("sharpen");
    }
    // the rectangle out of the rendered part, RGBA
    let (ox, oy) = (rx - x0, ry - y0);
    let bufr = &buf;
    crate::for_each_band(dst, rw * 4, |band, r0| {
        for (k, out) in band.chunks_exact_mut(rw * 4).enumerate() {
            let s = &bufr[((oy + r0 + k) * w + ox) * 3..][..rw * 3];
            for (o, p) in out.chunks_exact_mut(4).zip(s.chunks_exact(3)) {
                o[0] = p[0];
                o[1] = p[1];
                o[2] = p[2];
                o[3] = 255;
            }
        }
    });
    mark("write out");
    c.scratch = buf;
    c.timing = tm;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{apply_ex, Edit, Tone};

    /// xorshift: deterministic test data
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn f(&mut self) -> f64 {
            (self.next() % 1_000_000) as f64 / 1_000_000.0
        }
    }

    fn gauss(n: usize) -> Vec<f64> {
        let r = n as isize / 2;
        let k: Vec<f64> = (-r..=r).map(|t| (-0.5 * (t as f64 / 1.5).powi(2)).exp()).collect();
        let s: f64 = k.iter().sum();
        k.iter().map(|v| v / s).collect()
    }

    struct Tables { con: Vec<f64>, tone: Vec<f64>, boost: Vec<f64>, film_d: Vec<f64>, t1: Vec<f64>, k: Vec<f64> }

    fn tables(r: &mut Rng) -> Tables {
        Tables {
            con: (0..4096).map(|i| (i as f64 / 16.0 * 1.1 - 10.0).clamp(0.0, 255.0)).collect(),
            tone: (0..4096).map(|i| (i as f64 / 16.0 + 12.0 * (i as f64 / 4096.0 * 3.14).sin()).clamp(0.0, 255.0)).collect(),
            boost: (0..256).map(|i| 1.0 + 0.4 * (1.0 - i as f64 / 255.0)).collect(),
            film_d: (0..5 * 5 * 5 * 3).map(|_| r.f() * 30.0 - 15.0).collect(),
            t1: (0..256).map(|i| ((i as f64).powf(0.9) * 1.1).clamp(0.0, 255.0)).collect(),
            k: gauss(7),
        }
    }

    /// The old fixed pipeline (md -> point -> adjust -> filter -> film -> sc -> curves, f64) against the
    /// chain in that order (the fast engine): identical, Mid/Detail within 1 level.
    #[test]
    fn chain_in_old_order_matches_the_fixed_pipeline() {
        let mut r = Rng(0x9e3779b97f4a7c15);
        let t = tables(&mut r);
        let (w, h) = (97usize, 71usize);
        let (mut worst, mut diff, mut total) = (0, 0usize, 0usize);
        for case in 0..48 {
            let src: Vec<u8> = (0..w * h * 3).map(|i| ((i * 7 + (r.next() % 40) as usize) % 256) as u8).collect();
            let md = if case % 4 == 1 { 0.6 } else { 0.0 };
            let point = md == 0.0 && case % 3 != 0;        // the old order runs Mid/Detail before Point
            let ps = case % 5 == 2;
            let sc_on = case % 6 >= 3;
            let film_on = case % 2 == 0;
            let film_d = case % 4 == 0;
            let sharp = crate::sharp::Sharp { focus: if case % 7 == 3 { 0.5 } else { 0.0 }, ..Default::default() };
            let mut luts = crate::edit::identity_luts();
            if case % 3 == 1 {
                for c in 0..3 {
                    for v in 0..256 {
                        luts[c][v] = ((v as f64).powf(0.85 + 0.1 * c as f64) * 1.3).clamp(0.0, 255.0) as u8;
                    }
                }
            }
            let gains = Some([1.0 + r.f() * 0.2, 1.0, 0.9 + r.f() * 0.1]);
            let wb = Some([[1.02, 0.01, 0.0], [0.0, 0.98, 0.02], [0.01, 0.0, 1.03]]);
            let bw = Bw { s: 0.5 + r.f(), op: 0.4 + r.f() * 0.6, w: Some([1.05, 1.0, 0.95]),
                          ps: if ps { Some((0.4, t.k.as_slice())) } else { None } };
            let film = Film { n: 5, d: if film_d { Some(t.film_d.as_slice()) } else { None }, t1: Some(&t.t1),
                              op: if case % 3 == 0 { 1.0 } else { 0.8 } };    // t1 has fractions: op 1 must truncate
            let sc = Sc { cast: 0.5, cc: 40.0, dyn_: if case % 2 == 0 { 0.6 } else { 0.0 }, op: 0.9, dk: &t.k, shl: None };
            let tones = || vec![Tone { table: &t.tone, ratio: case % 2 == 1 }];
            let base = if md != 0.0 { crate::md::base(&src, w, h) } else { Vec::new() };
            let old = Edit {
                md: if md != 0.0 { Some((md, base.as_slice())) } else { None },
                point, k: 1.1, o: -6.0, gains, wb, con: Some(&t.con), tones: tones(), boost: Some(&t.boost),
                sat: Some(1.2), bw: Some(Bw { s: bw.s, op: bw.op, w: bw.w, ps: bw.ps }),
                film: if film_on { Some(Film { n: film.n, d: film.d, t1: film.t1, op: film.op }) } else { None },
                sc: if sc_on { Some(Sc { cast: sc.cast, cc: sc.cc, dyn_: sc.dyn_, op: sc.op, dk: sc.dk, shl: None }) } else { None },
                luts, sharp: sharp.clone(),
            };
            let mut steps = Vec::new();
            if point {
                steps.push(Step::Point { k: 1.1, o: -6.0, gains });
            }
            steps.push(Step::Adjust(Adjust { md, wb, con: Some(&t.con), tones: tones(), boost: Some(&t.boost), sat: Some(1.2) }));
            steps.push(Step::Bw(bw));
            if film_on {
                steps.push(Step::Film(film));
            }
            if sc_on {
                steps.push(Step::Sc(sc));
            }
            if case % 8 != 5 {                 // no Curves step at all now and then (hin then = before Sharpen)
                steps.push(Step::Curves(luts));
            } else {
                luts = crate::edit::identity_luts();
            }
            let old = Edit { luts, ..old };
            for ob in [3usize, 4] {
                let mut a = vec![0u8; w * h * ob];
                let mut b = vec![0u8; w * h * ob];
                let ha = apply_ex(&src, &mut a, w, ob, &old, true).unwrap();
                let hb = run(&src, &mut b, w, ob, &steps, &sharp, true).unwrap();
                // the fast engine (px32.rs) is bit-identical except Mid/Detail's weight table (±1)
                let tol = if md != 0.0 { 1 } else { 0 };
                let mx = a.iter().zip(&b).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap();
                let nd = a.iter().zip(&b).filter(|(x, y)| x != y).count();
                worst = worst.max(mx);
                diff += nd;
                total += a.len();
                assert!(mx <= tol, "case {case} ob {ob}: pixels differ by {mx} (> {tol})");
                for (x, y) in [(&ha.hin, &hb.hin), (&ha.hout, &hb.hout)] {
                    assert_eq!(x.iter().sum::<u32>(), y.iter().sum::<u32>(), "case {case}: histogram counts");
                }
            }
        }
        println!("fast engine vs the old pipeline: max diff {worst}, {:.4} % of values differ", diff as f64 * 100.0 / total as f64);
    }

    /// The preview's prefix cache: frame after frame, changing different steps, reordering, the
    /// cached render must equal a fresh one (pixels and histograms).
    #[test]
    fn prefix_cache_is_exact() {
        let mut r = Rng(42);
        let t = tables(&mut r);
        let (w, h) = (83usize, 61usize);
        let src: Vec<u8> = (0..w * h * 3).map(|i| ((i * 11 + (r.next() % 50) as usize) % 256) as u8).collect();
        let mut cache = MdCache::default();
        let mut luts = crate::edit::identity_luts();
        for v in 0..256 {
            luts[1][v] = (v as f64 * 0.9 + 10.0) as u8;
        }
        for frame in 0..40 {
            let f = frame as f64;
            let bw = Bw { s: 1.2, op: 0.9, w: None, ps: Some((0.3 + 0.01 * (frame / 10) as f64, t.k.as_slice())) };
            let sc = Sc { cast: 0.3, cc: 20.0, dyn_: if frame % 7 < 3 { 0.4 } else { 0.0 }, op: 0.9, dk: &t.k, shl: None };
            let film = Film { n: 5, d: Some(t.film_d.as_slice()), t1: Some(&t.t1), op: 0.5 + 0.01 * f };
            let adj = Adjust { md: if frame % 13 == 5 { 0.4 } else { 0.0 }, wb: None, con: Some(&t.con), tones: vec![],
                               boost: None, sat: Some(if (15..18).contains(&frame) { 1.1 + 0.02 * f } else { 1.0 }) };
            let mut steps = vec![Step::Adjust(adj), Step::Bw(bw), Step::Curves(luts), Step::Film(film), Step::Sc(sc)];
            if frame >= 25 {
                steps.swap(1, 3);           // reorder mid-way
            }
            let sharp = crate::sharp::Sharp { focus: if frame % 9 == 4 { 0.4 } else { 0.0 }, ..Default::default() };
            let mut a = vec![0u8; w * h * 4];
            let mut b = vec![0u8; w * h * 4];
            let ha = run(&src, &mut a, w, 4, &steps, &sharp, true).unwrap();
            let hb = run_cached(&src, &mut b, w, 4, &steps, &sharp, true, Some(&mut cache)).unwrap();
            assert!(a == b, "frame {frame}: pixels differ");
            assert!(ha.hin == hb.hin && ha.hout == hb.hout && ha.hmx == hb.hmx, "frame {frame}: histograms differ");
        }
        assert!(cache.hits >= 12, "the cache was used: {} of 40 frames", cache.hits);
    }

    /// The fast engine against the f64 reference (run_px), per-pixel steps in many orders and twice
    /// over: bit-identical (Mid/Detail aside, it has its own test above).
    #[test]
    fn fast_engine_matches_reference_in_any_order() {
        let mut r = Rng(7);
        let t = tables(&mut r);
        let (w, h) = (61usize, 37usize);
        let src: Vec<u8> = (0..w * h * 3).map(|i| ((i * 29 + (r.next() % 90) as usize) % 256) as u8).collect();
        let mut luts = crate::edit::identity_luts();
        for v in 0..256 {
            luts[0][v] = (255.0 * (v as f64 / 255.0).powf(0.8)) as u8;
        }
        let tone = vec![Tone { table: &t.tone, ratio: true }];
        let mk = |k: usize| -> Step {
            match k % 7 {
                0 => Step::Point { k: 1.1, o: -6.0, gains: Some([1.05, 1.0, 0.97]) },
                1 => Step::Adjust(Adjust { md: 0.0, wb: Some([[1.02, 0.01, 0.0], [0.0, 0.98, 0.02], [0.01, 0.0, 1.03]]),
                    con: Some(&t.con), tones: vec![Tone { table: &t.tone, ratio: false }], boost: Some(&t.boost), sat: Some(1.3) }),
                2 => Step::Curves(luts),
                3 => Step::Bw(Bw { s: 1.4, op: 0.7, w: Some([1.05, 1.0, 0.95]), ps: None }),
                4 => Step::Film(Film { n: 5, d: Some(t.film_d.as_slice()), t1: Some(&t.t1), op: 1.0 }),   // t1 has fractions
                5 => Step::Film(Film { n: 5, d: None, t1: Some(&t.t1), op: 0.6 }),
                _ => Step::Adjust(Adjust { md: 0.0, wb: None, con: None, tones: vec![Tone { table: tone[0].table, ratio: true }],
                    boost: None, sat: Some(0.7) }),
            }
        };
        for case in 0..30 {
            let mut order: Vec<usize> = (0..7).collect();
            for k in (1..order.len()).rev() {
                order.swap(k, (r.next() % (k as u64 + 1)) as usize);
            }
            order.truncate(3 + case % 5);
            if case % 4 == 0 {
                order.extend(order.clone());     // every step twice
            }
            let steps: Vec<Step> = order.iter().map(|&k| mk(k)).collect();
            let mut out = vec![0u8; src.len()];
            run(&src, &mut out, w, 3, &steps, &crate::sharp::Sharp::default(), false);
            let mut none = None;
            let want: Vec<u8> = src.chunks_exact(3).flat_map(|p| run_px([p[0], p[1], p[2]], &steps, 0.0, None, &mut none)).collect();
            assert!(out == want, "case {case} order {order:?}: fast engine differs from the reference");
        }
    }

    /// A draft of a rectangle equals that rectangle of the full render it measured from: exact
    /// without Sharpen; with Sharpen (it sees a margin, and its shrunk blurs align differently) close.
    #[test]
    fn region_matches_the_full_render() {
        let mut r = Rng(99);
        let t = tables(&mut r);
        let (w, h) = (300usize, 220usize);
        let src: Vec<u8> = (0..w * h * 3).map(|i| {
            let (x, y) = ((i / 3) % w, (i / 3) / w);
            ((x * 7 / 5 + y * 3 / 2 + (i % 3) * 40 + (r.next() % 25) as usize) % 256) as u8
        }).collect();
        let mut luts = crate::edit::identity_luts();
        for v in 0..256 {
            luts[2][v] = (255.0 * (v as f64 / 255.0).powf(1.2)) as u8;
        }
        let steps = || vec![
            Step::Point { k: 1.05, o: -3.0, gains: None },
            Step::Adjust(Adjust { md: 0.5, wb: None, con: Some(&t.con), tones: vec![], boost: None, sat: Some(1.2) }),
            Step::Bw(Bw { s: 1.2, op: 0.9, w: Some([1.02, 1.0, 0.98]), ps: Some((0.4, t.k.as_slice())) }),
            Step::Curves(luts),
            Step::Film(Film { n: 5, d: Some(t.film_d.as_slice()), t1: Some(&t.t1), op: 0.8 }),
            Step::Sc(Sc { cast: 0.4, cc: 30.0, dyn_: 0.5, op: 0.9, dk: &t.k, shl: None }),
        ];
        for (sh, tol) in [(0.0, 0), (0.5, 3)] {
            let sharp = crate::sharp::Sharp { focus: sh, structure: sh, ..Default::default() };
            let mut cache = MdCache::default();
            let mut full = vec![0u8; w * h * 4];
            run_cached(&src, &mut full, w, 4, &steps(), &sharp, true, Some(&mut cache)).unwrap();
            for (rx, ry, rw, rh) in [(0, 0, w, h), (37, 21, 120, 90), (250, 170, 50, 50), (0, 100, 300, 1), (5, 0, 1, 220)] {
                let mut part = vec![0u8; rw * rh * 4];
                assert!(run_region(&src, w, h, (rx, ry, rw, rh), &mut part, &steps(), &sharp, &mut cache), "measurements kept");
                let mut mx = 0;
                for y in 0..rh {
                    for x in 0..rw * 4 {
                        let a = full[((ry + y) * w + rx) * 4 + x] as i32;
                        mx = mx.max((a - part[y * rw * 4 + x] as i32).abs());
                    }
                }
                assert!(mx <= tol, "rect {rx},{ry} {rw}x{rh} sharpen {sh}: differs by {mx} (> {tol})");
            }
        }
        // without the measurements (no full render yet) there is no draft
        let mut fresh = MdCache::default();
        let mut part = vec![0u8; 10 * 10 * 4];
        assert!(!run_region(&src, w, h, (0, 0, 10, 10), &mut part, &steps(), &crate::sharp::Sharp::default(), &mut fresh));
    }

    #[test]
    fn order_matters_and_duplicates_run_twice() {
        let src: Vec<u8> = (0..30 * 20 * 3).map(|i| (i * 13 % 256) as u8).collect();
        let mut l = crate::edit::identity_luts();
        for c in 0..3 {
            for v in 0..256 {
                l[c][v] = (v / 2) as u8;
            }
        }
        let sat = || Step::Adjust(Adjust { md: 0.0, wb: None, con: None, tones: vec![], boost: None, sat: Some(1.8) });
        let sh = crate::sharp::Sharp::default();
        let mut a = vec![0u8; src.len()];
        let mut b = vec![0u8; src.len()];
        let mut c = vec![0u8; src.len()];
        run(&src, &mut a, 30, 3, &[sat(), Step::Curves(l)], &sh, false);
        run(&src, &mut b, 30, 3, &[Step::Curves(l), sat()], &sh, false);
        run(&src, &mut c, 30, 3, &[Step::Curves(l), Step::Curves(l)], &sh, false);
        assert_ne!(a, b, "order must change the result");
        assert!(c.iter().zip(&src).all(|(o, i)| *o == i / 4), "two Curves steps run twice");
    }

    /// cargo test --release -p fastedit-core bench_chain -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_chain() {
        use std::time::Instant;
        let mut r = Rng(7);
        let t = tables(&mut r);
        let (w, h) = (2560usize, 1440usize);
        let src: Vec<u8> = (0..w * h * 3).map(|i| ((i * 7 + (r.next() % 40) as usize) % 256) as u8).collect();
        let base = crate::md::base(&src, w, h);
        let sharp = crate::sharp::Sharp::default();
        let luts = crate::edit::identity_luts();
        let film = || Film { n: 5, d: Some(t.film_d.as_slice()), t1: Some(&t.t1), op: 0.8 };
        let adj = |md: f64| Adjust { md, wb: Some([[1.02, 0.01, 0.0], [0.0, 0.98, 0.02], [0.01, 0.0, 1.03]]), con: Some(&t.con),
                                     tones: vec![Tone { table: &t.tone, ratio: false }], boost: Some(&t.boost), sat: Some(1.2) };
        {
            let mut out = vec![0u8; w * h * 4];
            let t0 = Instant::now();
            for _ in 0..10 { run(&src, &mut out, w, 4, &[], &sharp, true); }
            println!("{:24} chain {:7.1} ms", "no steps (copy+hist)", t0.elapsed().as_secs_f64() * 100.0);
            let t0 = Instant::now();
            for _ in 0..10 { run(&src, &mut out, w, 4, &[Step::Adjust(Adjust { md: 0.0, wb: None, con: None, tones: vec![], boost: None, sat: Some(1.2) })], &sharp, true); }
            println!("{:24} chain {:7.1} ms", "sat only", t0.elapsed().as_secs_f64() * 100.0);
            let t0 = Instant::now();
            for _ in 0..10 { run(&src, &mut out, w, 4, &[Step::Adjust(Adjust { md: 0.0, wb: None, con: Some(&t.con), tones: vec![], boost: None, sat: None })], &sharp, true); }
            println!("{:24} chain {:7.1} ms", "con only", t0.elapsed().as_secs_f64() * 100.0);
            let wbm = Some([[1.02, 0.01, 0.0], [0.0, 0.98, 0.02], [0.01, 0.0, 1.03]]);
            for (nm, st) in [("wb only", Adjust { md: 0.0, wb: wbm, con: None, tones: vec![], boost: None, sat: None }),
                             ("tone only", Adjust { md: 0.0, wb: None, con: None, tones: vec![Tone { table: &t.tone, ratio: false }], boost: None, sat: None }),
                             ("boost only", Adjust { md: 0.0, wb: None, con: None, tones: vec![], boost: Some(&t.boost), sat: None })] {
                let st = [Step::Adjust(st)];
                let t0 = Instant::now();
                for _ in 0..10 { run(&src, &mut out, w, 4, &st, &sharp, true); }
                println!("{:24} chain {:7.1} ms", nm, t0.elapsed().as_secs_f64() * 100.0);
            }
        }
        for (name, md, point, film_on) in [("adjust", 0.0, false, false), ("adjust+md", 0.6, false, false),
                                            ("point+adjust+film", 0.0, true, true), ("point+adjust(md)+film", 0.6, true, true)] {
            let old = Edit { md: if md != 0.0 { Some((md, base.as_slice())) } else { None }, point, k: 1.1, o: -6.0,
                gains: None, wb: adj(0.0).wb, con: Some(&t.con), tones: adj(0.0).tones, boost: Some(&t.boost), sat: Some(1.2),
                bw: None, film: if film_on { Some(film()) } else { None }, sc: None, luts, sharp: sharp.clone() };
            let mut steps = Vec::new();
            if point { steps.push(Step::Point { k: 1.1, o: -6.0, gains: None }); }
            steps.push(Step::Adjust(adj(md)));
            if film_on { steps.push(Step::Film(film())); }
            let mut out = vec![0u8; w * h * 4];
            let n = 10;
            let t0 = Instant::now();
            for _ in 0..n { apply_ex(&src, &mut out, w, 4, &old, true); }
            let a = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
            let mut cache = MdCache::default();
            let t0 = Instant::now();
            for _ in 0..n { run_cached(&src, &mut out, w, 4, &steps, &sharp, true, Some(&mut cache)); }
            let b = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
            println!("{name:24} old {a:7.1} ms   chain {b:7.1} ms");
        }
    }
}

#[cfg(test)]
mod per_op {
    use super::*;
    use crate::edit::Tone;
    #[test]
    #[ignore]
    fn per_op_diff() {
        let con: Vec<f64> = (0..4096).map(|i| (i as f64 / 16.0 * 1.1 - 10.0).clamp(0.0, 255.0)).collect();
        let tone: Vec<f64> = (0..4096).map(|i| (i as f64 / 16.0 + 12.0 * (i as f64 / 4096.0 * 3.14).sin()).clamp(0.0, 255.0)).collect();
        let boost: Vec<f64> = (0..256).map(|i| 1.0 + 0.4 * (1.0 - i as f64 / 255.0)).collect();
        let wb = Some([[1.02, 0.01, 0.0], [0.0, 0.98, 0.02], [0.01, 0.0, 1.03]]);
        let mk = |k: usize| -> Step {
            match k {
                0 => Step::Point { k: 1.1, o: -6.0, gains: Some([1.1, 1.0, 0.95]) },
                1 => Step::Adjust(Adjust { md: 0.0, wb, con: None, tones: vec![], boost: None, sat: None }),
                2 => Step::Adjust(Adjust { md: 0.0, wb: None, con: Some(&con), tones: vec![], boost: None, sat: None }),
                3 => Step::Adjust(Adjust { md: 0.0, wb: None, con: None, tones: vec![Tone { table: &tone, ratio: false }], boost: None, sat: None }),
                4 => Step::Adjust(Adjust { md: 0.0, wb: None, con: None, tones: vec![Tone { table: &tone, ratio: true }], boost: None, sat: None }),
                5 => Step::Adjust(Adjust { md: 0.0, wb: None, con: None, tones: vec![], boost: Some(&boost), sat: None }),
                6 => Step::Adjust(Adjust { md: 0.0, wb: None, con: None, tones: vec![], boost: None, sat: Some(1.2) }),
                _ => Step::Bw(Bw { s: 1.3, op: 0.8, w: Some([1.05, 1.0, 0.95]), ps: None }),
            }
        };
        let names = ["point", "wb", "con", "tone", "tone ratio", "boost", "sat", "filter"];
        for k in 0..8 {
            let st = mk(k);
            let op = crate::px32::Op::from_step(&st).unwrap();
            let (mut mx, mut nd, mut worst) = (0, 0, [0u8; 3]);
            let mut c = crate::px32::Chunk::new();
            let mut px = vec![0u8; crate::px32::N * 3];
            let mut idx = 0u32;
            while idx < (1 << 24) {
                for i in 0..crate::px32::N { let v = idx + i as u32 * 263; px[i * 3] = (v >> 16) as u8; px[i * 3 + 1] = (v >> 8) as u8; px[i * 3 + 2] = v as u8; }
                c.load(&px);
                op.run(&mut c, &[]);
                for i in 0..crate::px32::N {
                    let p = [px[i * 3], px[i * 3 + 1], px[i * 3 + 2]];
                    let mut h = None;
                    let q = run_px(p, std::slice::from_ref(&st), 0.0, None, &mut h);
                    let f = [c.r[i] as u8, c.g[i] as u8, c.b[i] as u8];
                    for ch in 0..3 { let d = (q[ch] as i32 - f[ch] as i32).abs(); if d > 0 { nd += 1; } if d > mx { mx = d; worst = p; } }
                }
                idx += 4099;
            }
            println!("{:10} max {mx} differing {nd} worst {:?}", names[k], worst);
        }
    }
}
