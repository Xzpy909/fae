//! The edit as an ordered list of steps (the right panel's tool order). Each step is one tool:
//! Point correction, Adjustments (with Mid/Detail), Curves, the Perceptual Saturation filter, Film
//! Effect, Super Contrast; the same tool may appear more than once. Sharpen is not a step: it always
//! runs last, on the finished pixels.
//!
//! The maths of every step is the one in edit.rs / ps.rs / sc.rs, unchanged; only the order is free.
//! Runs of per-pixel steps are fused into one pass over the image. Steps that need the whole image
//! (Mid/Detail's smoothed luma, Perceptual Saturation's grid, Super Contrast's statistics) start a
//! new pass on the image as it arrives at them, so a step always sees the result of the steps before.
//!
//! Histograms (the UI's): hin = the input of the first Curves step (what the curve editor shows
//! under the curve; the output when there is no Curves step), hout / hmx / hmn = the final pixels.
use crate::edit::{adjust_px, film_px, filter_px, md_px, point_px, Adjust, Bw, Film, Sc};
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

/// One pixel through the per-pixel steps `steps` (Mid/Detail: `base` is that pixel's smoothed luma).
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

/// Mid/Detail's smoothed luma of the image arriving at an Adjustments step, kept between renders
/// (the live preview): reused while that image is the same (checked by a fingerprint).
#[derive(Default)]
pub struct MdCache {
    key: Option<(u64, usize)>,
    base: Vec<f32>,
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
    let mut buf = src.to_vec();
    let ident = crate::edit::identity_luts();

    let mut i = 0;
    while i < steps.len() {
        match &steps[i] {
            Step::Bw(bw) if bw.ps.is_some() => {
                crate::ps::step(&mut buf, width, height, bw);
                i += 1;
                continue;
            }
            Step::Sc(sc) => {
                crate::sc::finish(&mut buf, width, height, 3, sc, &ident, false);
                i += 1;
                continue;
            }
            _ => {}
        }
        // a run of per-pixel steps; Mid/Detail may only start a run (its smoothing needs the image
        // as it arrives), so an Adjustments step with Mid/Detail ends the run before it
        let mut j = i + 1;
        while j < steps.len() && steps[j].per_pixel() {
            j += 1;
        }
        let md_here = matches!(&steps[i], Step::Adjust(a) if a.md != 0.0);
        let run = &steps[i..j];
        let fresh: Vec<f32>;
        let base: &[f32] = if !md_here {
            &[]
        } else if let Some(c) = md_cache.take() {
            let key = (fingerprint(&mut buf, row), buf.len());
            if c.key != Some(key) {
                c.base = crate::md::base(&buf, width, height);
                c.key = Some(key);
            }
            &c.base
        } else {
            fresh = crate::md::base(&buf, width, height);
            &fresh
        };
        let hin_at = match first_curves {
            Some(c) if want_hist && c >= i && c < j => Some(c - i),
            _ => None,
        };
        let parts = crate::map_bands(&mut buf, row, |band, row0| {
            let mut h = hin_at.map(|_| vec![0u32; 768]);
            let p0 = row0 * width;
            for (k, p) in band.chunks_exact_mut(3).enumerate() {
                let b = if md_here { base[p0 + k] } else { 0.0 };
                let q = run_px([p[0], p[1], p[2]], run, b, hin_at, &mut h);
                p.copy_from_slice(&q);
            }
            h
        });
        if hin_at.is_some() {
            let mut h = vec![0u32; 768];
            for part in parts.into_iter().flatten() {
                h.iter_mut().zip(part).for_each(|(a, b)| *a += b);
            }
            hin = Some(h);
        }
        i = j;
    }

    if sharp.active() {
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
    }

    // write out (RGB or RGBA), counting the output histograms
    let orow = width * ob;
    let buf = &buf;
    let h = Hist::merge(crate::map_bands(dst, orow, |band, row0| {
        let mut hist = want_hist.then(Hist::default);
        let s = &buf[row0 * row..row0 * row + band.len() / ob * 3];
        for (o, p) in band.chunks_exact_mut(ob).zip(s.chunks_exact(3)) {
            let px = [p[0], p[1], p[2]];
            crate::write_px(o, px, &ident, &mut hist);
        }
        hist
    }));
    h.map(|mut h| {
        if let Some(hi) = hin {
            h.hin = hi;          // else hin = the output (write_px counted the same pixels)
        }
        h
    })
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

    /// The old fixed pipeline (md -> point -> adjust -> filter -> film -> sc -> curves) against the
    /// chain in that order: pixels and histograms must be identical.
    #[test]
    fn chain_in_old_order_matches_the_fixed_pipeline() {
        let mut r = Rng(0x9e3779b97f4a7c15);
        let t = tables(&mut r);
        let (w, h) = (97usize, 71usize);
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
            let film = Film { n: 5, d: if film_d { Some(t.film_d.as_slice()) } else { None }, t1: Some(&t.t1), op: 0.8 };
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
                assert!(a == b, "case {case} ob {ob}: pixels differ");
                assert_eq!(ha.hout, hb.hout, "case {case}: hout");
                assert_eq!(ha.hmx, hb.hmx, "case {case}: hmx");
                assert_eq!(ha.hmn, hb.hmn, "case {case}: hmn");
                if !sc_on {     // the old pipeline counts hin before the curves (with SC: after it, same place)
                    assert_eq!(ha.hin, hb.hin, "case {case}: hin");
                } else {
                    assert_eq!(ha.hin, hb.hin, "case {case}: hin (sc)");
                }
            }
        }
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
