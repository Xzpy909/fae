//! WebAssembly build of fastedit-core for the live preview (ui/wasm/, used by ui/js/preview-worker.js).
//!
//! Threads: `initThreadPool(n)` (wasm-bindgen-rayon) starts a pool of Web Workers on shared memory,
//! then `enable_threads(true)` lets the core use it. Without that (e.g. in node for the parity tests)
//! the same module runs single-threaded. Must be built with atomics + bulk-memory (build_core.bat).
use wasm_bindgen::prelude::*;

pub use wasm_bindgen_rayon::init_thread_pool;

#[wasm_bindgen]
extern "C" {
    /// the worker's clock, for the core's timing breakdown
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn perf_now() -> f64;
}

#[wasm_bindgen]
pub fn version() -> String {
    fastedit_core::VERSION.to_string()
}

/// Let the core use the thread pool (call after initThreadPool has resolved).
#[wasm_bindgen]
pub fn enable_threads(on: bool) {
    fastedit_core::set_parallel(on);
}

#[wasm_bindgen]
pub fn threads() -> usize {
    fastedit_core::threads()
}

/// In place on an RGBA buffer (ImageData layout); `luts` = 768 values (R, G, B tables).
#[wasm_bindgen]
pub fn apply_luts_rgba(buf: &mut [u8], width: usize, luts: &[u8]) -> Result<(), JsError> {
    let t = fastedit_core::luts_from_flat(luts).ok_or_else(|| JsError::new("luts must have 768 values"))?;
    if width == 0 || !buf.len().is_multiple_of(width * 4) {
        return Err(JsError::new("buffer is not whole RGBA rows"));
    }
    fastedit_core::apply_luts(buf, width, 4, &t);
    Ok(())
}

fn arr3(v: &[f64], name: &str) -> Result<[f64; 3], JsError> {
    v.try_into().map_err(|_| JsError::new(&format!("{name}: need 3 values")))
}

/// (s, op, w, ps, psk): the Filter; ps = 0 means no Perceptual Saturation
type BwP = (f64, f64, Option<[f64; 3]>, f64, Vec<f64>);
/// (n, d, t1, op): Film Effect
type FilmP = (usize, Option<Vec<f64>>, Option<Vec<f64>>, f64);
/// (cast, cc, dyn, op, dk, shl): Super Contrast
type ScP = (f64, f64, f64, f64, Vec<f64>, Option<Vec<f64>>);

/// Adjustments, as the setters fill it before push_adjust()
#[derive(Default)]
struct AdjP {
    md: f64,
    wb: Option<[[f64; 3]; 3]>,
    con: Option<Vec<f64>>,
    tones: Vec<(Vec<f64>, bool)>,
    boost: Option<Vec<f64>>,
    sat: Option<f64>,
}

/// One step of the chain, owning its tables
enum StepP {
    Point(f64, f64, Option<[f64; 3]>),
    Adjust(AdjP),
    Curves([[u8; 256]; 3]),
    Bw(BwP),
    Film(FilmP),
    Sc(ScP),
}

/// The edit as an ordered list of steps (filled by ui/js/wasm-params.js): push_* adds a step at the
/// end; Adjustments are first filled with set_md / set_wb / set_con / add_tone / set_boost / set_sat,
/// then push_adjust() adds them. A tool may be pushed more than once. Sharpen always runs last.
#[wasm_bindgen]
#[derive(Default)]
pub struct Params {
    steps: Vec<StepP>,
    adj: AdjP,
    sharp: (f64, f64, f64, f64, f64, f64),
}

#[wasm_bindgen]
impl Params {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Params {
        Params::default()
    }

    pub fn push_point(&mut self, k: f64, o: f64, gains: Option<Vec<f64>>) -> Result<(), JsError> {
        let gains = gains.map(|g| arr3(&g, "gains")).transpose()?;
        self.steps.push(StepP::Point(k, o, gains));
        Ok(())
    }

    /// Mid/Detail amount of the Adjustments being filled; the Preview computes the smoothing it needs
    pub fn set_md(&mut self, amount: f64) {
        self.adj.md = amount;
    }

    /// 9 values, row-major: out_c = wb[c][0] r + wb[c][1] g + wb[c][2] b
    pub fn set_wb(&mut self, m: &[f64]) -> Result<(), JsError> {
        if m.len() != 9 {
            return Err(JsError::new("wb: need 9 values"));
        }
        self.adj.wb = Some([[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]]);
        Ok(())
    }

    pub fn set_con(&mut self, t: Vec<f64>) {
        self.adj.con = Some(t);
    }

    pub fn add_tone(&mut self, t: Vec<f64>, ratio: bool) {
        self.adj.tones.push((t, ratio));
    }

    pub fn set_boost(&mut self, t: Vec<f64>) {
        self.adj.boost = Some(t);
    }

    pub fn set_sat(&mut self, s: f64) {
        self.adj.sat = Some(s);
    }

    /// Adds the Adjustments filled so far as a step (and starts a fresh one)
    pub fn push_adjust(&mut self) {
        let a = core::mem::take(&mut self.adj);
        self.steps.push(StepP::Adjust(a));
    }

    /// 768 values: R, G, B curves (master folded in)
    pub fn push_curves(&mut self, flat: &[u8]) -> Result<(), JsError> {
        let l = fastedit_core::luts_from_flat(flat).ok_or_else(|| JsError::new("luts: need 768 values"))?;
        self.steps.push(StepP::Curves(l));
        Ok(())
    }

    /// ps = 0: no Perceptual Saturation (psk ignored)
    pub fn push_bw(&mut self, s: f64, op: f64, w: Option<Vec<f64>>, ps: f64, psk: Option<Vec<f64>>) -> Result<(), JsError> {
        let w = w.map(|w| arr3(&w, "bw w")).transpose()?;
        if ps != 0.0 && psk.as_ref().is_none_or(|k| k.is_empty()) {
            return Err(JsError::new("bw: ps without psk"));
        }
        self.steps.push(StepP::Bw((s, op, w, ps, psk.unwrap_or_default())));
        Ok(())
    }

    pub fn push_film(&mut self, n: usize, d: Option<Vec<f64>>, t1: Option<Vec<f64>>, op: f64) {
        self.steps.push(StepP::Film((n, d, t1, op)));
    }

    pub fn push_sc(&mut self, cast: f64, cc: f64, dyn_: f64, op: f64, dk: Vec<f64>, shl: Option<Vec<f64>>) {
        self.steps.push(StepP::Sc((cast, cc, dyn_, op, dk, shl)));
    }

    /// Sharpen: Adaptive Sharpening (0..1), Balance (-1 areas .. 1 edges), Structure, Local Contrast, Focus (-1..1, 0 = off) and the
    /// preview's scale (preview pixels per output pixel, so fixed-size effects cover the same part of
    /// the picture as in Process)
    pub fn set_sharp(&mut self, adaptive: f64, balance: f64, structure: f64, local_contrast: f64, focus: f64, scale: f64) {
        self.sharp = (structure, local_contrast, focus, scale, adaptive, balance);
    }
}

impl Params {
    fn steps(&self) -> Result<Vec<fastedit_core::chain::Step<'_>>, JsError> {
        use fastedit_core::chain::Step;
        use fastedit_core::edit::{Adjust, Bw, Film, Sc, Tone};
        let mut out = Vec::with_capacity(self.steps.len());
        for s in &self.steps {
            let st = match s {
                StepP::Point(k, o, g) => Step::Point { k: *k, o: *o, gains: *g },
                StepP::Adjust(a) => Step::Adjust(Adjust {
                    md: a.md,
                    wb: a.wb,
                    con: a.con.as_deref(),
                    tones: a.tones.iter().map(|(t, r)| Tone { table: t, ratio: *r }).collect(),
                    boost: a.boost.as_deref(),
                    sat: a.sat,
                }),
                StepP::Curves(l) => Step::Curves(*l),
                StepP::Bw((s, op, w, ps, psk)) => Step::Bw(Bw {
                    s: *s,
                    op: *op,
                    w: *w,
                    ps: if *ps != 0.0 { Some((*ps, psk.as_slice())) } else { None },
                }),
                StepP::Film((n, d, t1, op)) => Step::Film(Film { n: *n, d: d.as_deref(), t1: t1.as_deref(), op: *op }),
                StepP::Sc((cast, cc, dyn_, op, dk, shl)) => Step::Sc(Sc {
                    cast: *cast,
                    cc: *cc,
                    dyn_: *dyn_,
                    op: *op,
                    dk: dk.as_slice(),
                    shl: shl.as_deref(),
                }),
            };
            st.validate().map_err(|m| JsError::new(&m))?;
            out.push(st);
        }
        Ok(out)
    }

    fn sharp(&self) -> fastedit_core::sharp::Sharp {
        fastedit_core::sharp::Sharp { adaptive: self.sharp.4, balance: self.sharp.5, structure: self.sharp.0,
            local_contrast: self.sharp.1, focus: self.sharp.2, scale: if self.sharp.3 > 0.0 { self.sharp.3 } else { 1.0 }, href: 0 }
    }
}

/// One preview image (RGB kept inside), rendered to RGBA with the UI's histograms.
#[wasm_bindgen]
pub struct Preview {
    w: usize,
    h: usize,
    src: Vec<u8>,
    out: Vec<u8>,
    /// the last draft's rectangle (render_region), RGBA
    part: Vec<u8>,
    md: fastedit_core::chain::MdCache,
    hist: fastedit_core::Hist,
}

#[wasm_bindgen]
impl Preview {
    /// `rgba`: the preview pixels as the UI has them (ImageData layout, alpha ignored)
    #[wasm_bindgen(constructor)]
    pub fn new(w: usize, h: usize, rgba: &[u8]) -> Result<Preview, JsError> {
        if rgba.len() != w * h * 4 {
            return Err(JsError::new("rgba: size does not match w x h"));
        }
        fastedit_core::set_clock(perf_now);
        let mut src = Vec::with_capacity(w * h * 3);
        for p in rgba.chunks_exact(4) {
            src.extend_from_slice(&p[..3]);
        }
        Ok(Preview { w, h, src, out: vec![0; w * h * 4], part: Vec::new(), md: Default::default(), hist: fastedit_core::Hist::default() })
    }

    pub fn render(&mut self, p: &Params) -> Result<(), JsError> {
        let steps = p.steps()?;
        // Mid/Detail's smoothing is kept between renders while the image arriving at it is the same
        self.hist = fastedit_core::chain::run_cached(&self.src, &mut self.out, self.w, 4, &steps, &p.sharp(), true,
                                                     Some(&mut self.md)).unwrap_or_default();
        Ok(())
    }

    /// A draft of the rectangle (x, y, w, h) only, while a control is being dragged (zoomed in): the
    /// whole-image steps reuse the last full render's measurements. Its RGBA pixels: part_ptr /
    /// part_len. False when there is no full render to take them from yet (render() instead).
    pub fn render_region(&mut self, p: &Params, x: usize, y: usize, w: usize, h: usize) -> Result<bool, JsError> {
        let steps = p.steps()?;
        self.part.resize(w * h * 4, 0);
        Ok(fastedit_core::chain::run_region(&self.src, self.w, self.h, (x, y, w, h), &mut self.part, &steps, &p.sharp(),
                                            &mut self.md))
    }
    /// the draft's pixels as a copy (the self-test; the worker reads them in place)
    pub fn part(&self) -> Vec<u8> {
        self.part.clone()
    }
    pub fn part_ptr(&self) -> *const u8 {
        self.part.as_ptr()
    }
    pub fn part_len(&self) -> usize {
        self.part.len()
    }

    /// The rendered RGBA pixels (a copy, safe to transfer to another thread). The worker reads them
    /// straight from WebAssembly memory instead (out_ptr / out_len: one copy, not two).
    pub fn output(&self) -> Vec<u8> {
        self.out.clone()
    }
    pub fn out_ptr(&self) -> *const u8 {
        self.out.as_ptr()
    }
    pub fn out_len(&self) -> usize {
        self.out.len()
    }

    /// The last render's phases: "name ms | name ms …" (the self-test shows it)
    pub fn timing(&self) -> String {
        self.md.timing.iter().map(|(n, t)| format!("{n} {t:.1}")).collect::<Vec<_>>().join(" | ")
    }

    /// Drop what is kept between renders (Mid/Detail smoothing, the prefix): the next render is a
    /// full one (the self-test's timings).
    pub fn forget(&mut self) {
        self.md.forget();
    }

    pub fn hin(&self) -> Vec<u32> {
        self.hist.hin.clone()
    }
    pub fn hout(&self) -> Vec<u32> {
        self.hist.hout.clone()
    }
    pub fn hmx(&self) -> Vec<u32> {
        self.hist.hmx.clone()
    }
    pub fn hmn(&self) -> Vec<u32> {
        self.hist.hmn.clone()
    }

    pub fn width(&self) -> usize {
        self.w
    }
    pub fn height(&self) -> usize {
        self.h
    }
}
