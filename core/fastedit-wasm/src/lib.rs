//! WebAssembly build of fastedit-core for the live preview (ui/wasm/, used by ui/js/preview-worker.js).
//!
//! Threads: `initThreadPool(n)` (wasm-bindgen-rayon) starts a pool of Web Workers on shared memory,
//! then `enable_threads(true)` lets the core use it. Without that (e.g. in node for the parity tests)
//! the same module runs single-threaded. Must be built with atomics + bulk-memory (build_core.bat).
use wasm_bindgen::prelude::*;

pub use wasm_bindgen_rayon::init_thread_pool;

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

/// The edit, as processPixels gets it from computeEdit (filled by ui/js/wasm-params.js).
#[wasm_bindgen]
#[derive(Default)]
pub struct Params {
    md: f64,
    luts: Option<[[u8; 256]; 3]>,
    point: bool,
    k: f64,
    o: f64,
    gains: Option<[f64; 3]>,
    wb: Option<[[f64; 3]; 3]>,
    con: Option<Vec<f64>>,
    tones: Vec<(Vec<f64>, bool)>,
    boost: Option<Vec<f64>>,
    sat: Option<f64>,
    bw: Option<BwP>,
    film: Option<FilmP>,
    sc: Option<ScP>,
    structure: f64,
}

#[wasm_bindgen]
impl Params {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Params {
        Params { k: 1.0, ..Default::default() }
    }

    /// Mid/Detail amount (E.md); the Preview computes the smoothing it needs
    pub fn set_md(&mut self, amount: f64) {
        self.md = amount;
    }

    /// 768 values: R, G, B curves (master folded in)
    pub fn set_luts(&mut self, flat: &[u8]) -> Result<(), JsError> {
        self.luts = Some(fastedit_core::luts_from_flat(flat).ok_or_else(|| JsError::new("luts: need 768 values"))?);
        Ok(())
    }

    pub fn set_point(&mut self, point: bool, k: f64, o: f64, gains: Option<Vec<f64>>) -> Result<(), JsError> {
        self.point = point;
        self.k = k;
        self.o = o;
        self.gains = gains.map(|g| arr3(&g, "gains")).transpose()?;
        Ok(())
    }

    /// 9 values, row-major: out_c = wb[c][0] r + wb[c][1] g + wb[c][2] b
    pub fn set_wb(&mut self, m: &[f64]) -> Result<(), JsError> {
        if m.len() != 9 {
            return Err(JsError::new("wb: need 9 values"));
        }
        self.wb = Some([[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]]);
        Ok(())
    }

    pub fn set_con(&mut self, t: Vec<f64>) {
        self.con = Some(t);
    }

    pub fn add_tone(&mut self, t: Vec<f64>, ratio: bool) {
        self.tones.push((t, ratio));
    }

    pub fn set_boost(&mut self, t: Vec<f64>) {
        self.boost = Some(t);
    }

    pub fn set_sat(&mut self, s: f64) {
        self.sat = Some(s);
    }

    /// ps = 0: no Perceptual Saturation (psk ignored)
    pub fn set_bw(&mut self, s: f64, op: f64, w: Option<Vec<f64>>, ps: f64, psk: Option<Vec<f64>>) -> Result<(), JsError> {
        let w = w.map(|w| arr3(&w, "bw w")).transpose()?;
        if ps != 0.0 && psk.as_ref().is_none_or(|k| k.is_empty()) {
            return Err(JsError::new("bw: ps without psk"));
        }
        self.bw = Some((s, op, w, ps, psk.unwrap_or_default()));
        Ok(())
    }

    pub fn set_film(&mut self, n: usize, d: Option<Vec<f64>>, t1: Option<Vec<f64>>, op: f64) {
        self.film = Some((n, d, t1, op));
    }

    /// Sharpen: Structure -1..1 (0 = off)
    pub fn set_sharp(&mut self, structure: f64) {
        self.structure = structure;
    }

    pub fn set_sc(&mut self, cast: f64, cc: f64, dyn_: f64, op: f64, dk: Vec<f64>, shl: Option<Vec<f64>>) {
        self.sc = Some((cast, cc, dyn_, op, dk, shl));
    }
}

impl Params {
    /// `md_base`: the Preview's smoothed luma (one value per pixel), present when md != 0
    fn edit<'a>(&'a self, md_base: Option<&'a [f32]>) -> Result<fastedit_core::edit::Edit<'a>, JsError> {
        use fastedit_core::edit::{Bw, Edit, Film, Sc, Tone};
        let e = Edit {
            md: match md_base {
                Some(b) if self.md != 0.0 => Some((self.md, b)),
                _ => None,
            },
            point: self.point,
            k: self.k,
            o: self.o,
            gains: self.gains,
            wb: self.wb,
            con: self.con.as_deref(),
            tones: self.tones.iter().map(|(t, r)| Tone { table: t, ratio: *r }).collect(),
            boost: self.boost.as_deref(),
            sat: self.sat,
            bw: self.bw.as_ref().map(|(s, op, w, ps, psk)| Bw {
                s: *s,
                op: *op,
                w: *w,
                ps: if *ps != 0.0 { Some((*ps, psk.as_slice())) } else { None },
            }),
            film: self.film.as_ref().map(|(n, d, t1, op)| Film { n: *n, d: d.as_deref(), t1: t1.as_deref(), op: *op }),
            sc: self.sc.as_ref().map(|(cast, cc, dyn_, op, dk, shl)| Sc {
                cast: *cast,
                cc: *cc,
                dyn_: *dyn_,
                op: *op,
                dk: dk.as_slice(),
                shl: shl.as_deref(),
            }),
            luts: self.luts.unwrap_or_else(|| {
                let id: [u8; 256] = core::array::from_fn(|i| i as u8);
                [id, id, id]
            }),
            sharp: fastedit_core::sharp::Sharp { structure: self.structure },
        };
        e.validate().map_err(|m| JsError::new(&m))?;
        Ok(e)
    }
}

/// One preview image (RGB kept inside), rendered to RGBA with the UI's histograms.
#[wasm_bindgen]
pub struct Preview {
    w: usize,
    h: usize,
    src: Vec<u8>,
    out: Vec<u8>,
    md_base: Option<Vec<f32>>,
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
        let mut src = Vec::with_capacity(w * h * 3);
        for p in rgba.chunks_exact(4) {
            src.extend_from_slice(&p[..3]);
        }
        Ok(Preview { w, h, src, out: vec![0; w * h * 4], md_base: None, hist: fastedit_core::Hist::default() })
    }

    pub fn render(&mut self, p: &Params) -> Result<(), JsError> {
        // Mid/Detail smoothing of this image: computed the first time it's needed, then kept
        if p.md != 0.0 && self.md_base.is_none() {
            self.md_base = Some(fastedit_core::md::base(&self.src, self.w, self.h));
        }
        let e = p.edit(if p.md != 0.0 { self.md_base.as_deref() } else { None })?;
        self.hist = fastedit_core::edit::apply_ex(&self.src, &mut self.out, self.w, 4, &e, true).unwrap_or_default();
        Ok(())
    }

    /// The rendered RGBA pixels (a copy, safe to transfer to another thread)
    pub fn output(&self) -> Vec<u8> {
        self.out.clone()
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
