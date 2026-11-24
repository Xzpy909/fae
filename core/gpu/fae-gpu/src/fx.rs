//! The filter kit (G7, docs/filter-kit.md): filters described by `filters/<id>.json` and written as
//! `filters/<id>.wgsl`, run by generic host code. build.rs turns the folder into [`FILTERS`]; a step is
//! [`Step::fx`] (id, the parameters in field order, opacity); the passes come from the filter's kind:
//!
//! - **pixel**: one pass, `fx(c, at)` (template shaders/kit/pixel.wgsl).
//! - **neighbourhood**: the step's input blurred first (Sharpen's block means + separable Gaussian
//!   passes of sharp_blur.wgsl, on a shrunk copy when sigma is large), then `fx(c, at)` with `blur(at)`.
//! - **statistics**: R, G, B and luma histograms of the input (kit/hist.wgsl, atomics), the filter's
//!   `stats()` once to make up to 64 numbers, then `fx(c, at)` with `stat(i)`.
//!
//! Every kind ends in kit/common.wgsl `finish`: round, clamp, opacity from a host f64 table. Kit
//! filters exist only on the GPU: without one they are skipped (fastedit/core.py, the CPU preview).

use crate::sharp::{blur_k, gauss_kernel, small_sigma};
use crate::{bind_group_at, Aux, Gpu, Kind, Pass, Pipe, Step, COMMON_WGSL};

pub const KIT_COMMON_WGSL: &str = include_str!("../shaders/kit/common.wgsl");
pub const KIT_PIXEL_WGSL: &str = include_str!("../shaders/kit/pixel.wgsl");
pub const KIT_NEIGHBOURHOOD_WGSL: &str = include_str!("../shaders/kit/neighbourhood.wgsl");
pub const KIT_STATISTICS_WGSL: &str = include_str!("../shaders/kit/statistics.wgsl");
pub const KIT_HIST_WGSL: &str = include_str!("../shaders/kit/hist.wgsl");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxKind {
    Pixel,
    Neighbourhood,
    Statistics,
}

/// One filter of the kit, as build.rs reads it from filters/<id>.json.
#[derive(Debug)]
pub struct FxDef {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: FxKind,
    /// filters/<id>.wgsl
    pub src: &'static str,
    /// `const P_<FIELD>: u32 = i;` for every parameter
    pub consts: &'static str,
    /// parameters (the fields without Opacity, in order; a select gives its option's index)
    pub nparams: usize,
    /// neighbourhood: the parameter that sets the blur (sigma = value x blur_scale px at 1440 px height)
    pub blur: Option<usize>,
    pub blur_scale: f64,
    /// a test filter: not in the app's Filters list (fae.py --test-filters), not compiled at start-up
    pub hidden: bool,
}

include!(concat!(env!("OUT_DIR"), "/fx_registry.rs"));

/// The filter's index in [`FILTERS`].
pub fn find(id: &str) -> Option<usize> {
    FILTERS.iter().position(|f| f.id == id)
}

/// Results a statistics filter's stats() can store (kit/statistics.wgsl put / stat).
pub const STATS_N: usize = 64;
/// histogram bins of a statistics filter (R, G, B, luma)
const HIST_N: usize = 1024;
/// P layout (kit/common.wgsl): the header and the blur's level map come first
const P_MAP: usize = 16;
const P_KER: usize = 272;

/// Entry points of a filter's module: apply always, reduce for statistics.
pub(crate) const ENTRIES: [&str; 2] = ["apply", "reduce"];

impl FxDef {
    /// The filter's WGSL module: the shared parts, its kind's template, its constants, its shader.
    pub fn module_source(&self) -> String {
        let tpl = match self.kind {
            FxKind::Pixel => KIT_PIXEL_WGSL,
            FxKind::Neighbourhood => KIT_NEIGHBOURHOOD_WGSL,
            FxKind::Statistics => KIT_STATISTICS_WGSL,
        };
        format!("{COMMON_WGSL}\n{KIT_COMMON_WGSL}\n{tpl}\n// ---- {} ----\n{}\n{}", self.id, self.consts, self.src)
    }

    /// Its entry points (indices into ENTRIES).
    pub(crate) fn entries(&self) -> &'static [usize] {
        if self.kind == FxKind::Statistics { &[0, 1] } else { &[0] }
    }
}

impl Step {
    /// A kit filter: `id` as its file name, `params` = the descriptor's fields in order without the
    /// Opacity field (slider values as shown; a select: its option's index), `opacity` 0..1.
    pub fn fx(id: &str, params: &[f64], opacity: f64) -> Result<Step, String> {
        let f = find(id).ok_or_else(|| format!("fx: no filter {id:?}"))?;
        let def = &FILTERS[f];
        if params.len() != def.nparams {
            return Err(format!("fx {id}: {} parameters, need {}", params.len(), def.nparams));
        }
        if params.iter().any(|v| !v.is_finite()) || !opacity.is_finite() {
            return Err(format!("fx {id}: parameters must be finite"));
        }
        // [opacity, params...] in f64; the GPU's layout is made with the stage (Gpu::fx_params)
        let v: Vec<u8> = std::iter::once(opacity.clamp(0.0, 1.0)).chain(params.iter().copied()).flat_map(|x| x.to_le_bytes()).collect();
        Ok(Step { kind: Kind::Fx, data: v, aux: Aux::Fx { f: f as u16 } })
    }

    fn fx_values(&self) -> (f64, Vec<f64>) {
        let v: Vec<f64> = self.data.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect();
        (v[0], v[1..].to_vec())
    }
}

/// The blur of a neighbourhood step on an image `h` px high: shrink factor and kernel.
fn blur_shape(def: &FxDef, p: &[f64], h: u32) -> (usize, Vec<f32>) {
    let Some(b) = def.blur else { return (1, vec![]) };
    let sigma = (p[b] * def.blur_scale * h as f64 / 1440.0).max(0.3);
    let k = blur_k(sigma);
    (k, if k == 1 { gauss_kernel(sigma) } else { gauss_kernel(small_sigma(sigma, k)) })
}

impl Gpu {
    /// What decides a kit step's passes (besides its filter): shrink factor, kernel length, opacity
    /// table on. Same shape -> a slider move only rewrites the parameters.
    pub(crate) fn fx_shape(&self, st: &Step) -> u64 {
        let Aux::Fx { f } = st.aux else { return 0 };
        let def = &FILTERS[f as usize];
        let (op, p) = st.fx_values();
        let h = self.stage.as_ref().map_or(1, |s| s.h);
        let (k, ker) = blur_shape(def, &p, h);
        ((k as u64) << 20) | ((ker.len() as u64) << 1) | (op != 1.0) as u64
    }

    /// The kit step's parameter buffer (layout: kit/common.wgsl).
    pub(crate) fn fx_params(&self, st: &Step) -> Vec<f32> {
        let Aux::Fx { f } = st.aux else { unreachable!() };
        let def = &FILTERS[f as usize];
        let (op, p) = st.fx_values();
        let (w, h) = self.stage.as_ref().map_or((1, 1), |s| (s.w, s.h));
        let (k, ker) = blur_shape(def, &p, h);
        let fxo = P_KER + ker.len();
        let mix = if op != 1.0 { fxo + p.len() } else { 0 };
        let mut v = vec![0f32; fxo + p.len() + if mix > 0 { crate::MIX_N } else { 0 }];
        v[2] = k as f32;
        v[3] = ker.len() as f32;
        v[4] = (ker.len() / 2) as f32;
        v[6] = w.div_ceil(k as u32) as f32;
        v[7] = h.div_ceil(k as u32) as f32;
        v[8] = w as f32;
        v[9] = h as f32;
        v[12] = fxo as f32;
        v[13] = p.len() as f32;
        v[14] = mix as f32;
        for i in 0..256 {
            v[P_MAP + i] = i as f32;
        }
        v[P_KER..fxo].copy_from_slice(&ker);
        for (i, x) in p.iter().enumerate() {
            v[fxo + i] = *x as f32;
        }
        if mix > 0 {
            v[mix..].copy_from_slice(&crate::mix_table(op, |j| j as f64));
        }
        v
    }

    /// The pipeline index of filter f's entry e (after the built-in ones).
    pub(crate) fn fx_pipe(f: usize, e: usize) -> usize {
        Pipe::COUNT + f * ENTRIES.len() + e
    }

    /// The whole kit step (`inp` -> `out`): passes, buffers to keep, buffers to zero before each run.
    pub(crate) fn fx_passes(&mut self, st: &Step, inp: usize, out: usize, params: &wgpu::Buffer)
                            -> (Vec<Pass>, Vec<wgpu::Buffer>, Vec<wgpu::Buffer>) {
        let Aux::Fx { f } = st.aux else { unreachable!() };
        let f = f as usize;
        let def = &FILTERS[f];
        let apply = Self::fx_pipe(f, 0);
        let mut passes = vec![];
        let mut keep = vec![];
        let mut clears = vec![];
        let pr = || params.as_entire_binding();
        match def.kind {
            FxKind::Pixel => {
                let s = self.stage.as_ref().unwrap();
                let tv = |i: usize| wgpu::BindingResource::TextureView(&s.views[i]);
                passes.push(Pass { pipe: apply, groups: (s.w.div_ceil(16), s.h.div_ceil(16), 1),
                    bg: bind_group_at(&self.device, self.pipe_at(apply), &[(0, tv(inp)), (1, tv(out)), (2, pr())]) });
            }
            FxKind::Neighbourhood => {
                let [pa, pb] = self.sharp_planes();
                let s = self.stage.as_ref().unwrap();
                let k = self.fx_shape(st) >> 20;
                let (w2, h2) = (s.w.div_ceil(k as u32), s.h.div_ceil(k as u32));
                let d = &self.device;
                let tv = |i: usize| wgpu::BindingResource::TextureView(&s.views[i]);
                let gg = (w2.div_ceil(16), h2.div_ceil(16), 1);
                let (a, b) = (pa.as_entire_binding(), pb.as_entire_binding());
                let sb = |p: Pipe| p as usize;
                passes.push(Pass { pipe: sb(Pipe::SbMeans), groups: gg,
                    bg: bind_group_at(d, self.pipe(Pipe::SbMeans), &[(0, tv(inp)), (2, pr()), (3, a.clone())]) });
                passes.push(Pass { pipe: sb(Pipe::SbAcross), groups: gg,
                    bg: bind_group_at(d, self.pipe(Pipe::SbAcross), &[(2, pr()), (3, a.clone()), (4, b.clone())]) });
                passes.push(Pass { pipe: sb(Pipe::SbDown), groups: gg,
                    bg: bind_group_at(d, self.pipe(Pipe::SbDown), &[(2, pr()), (3, a.clone()), (4, b)]) });
                passes.push(Pass { pipe: apply, groups: (s.w.div_ceil(16), s.h.div_ceil(16), 1),
                    bg: bind_group_at(d, self.pipe_at(apply), &[(0, tv(inp)), (1, tv(out)), (2, pr()), (3, a)]) });
            }
            FxKind::Statistics => {
                let hist = self.storage("fx hist", (HIST_N * 4) as u64);
                let res = self.storage("fx stats", (STATS_N * 4) as u64);
                let s = self.stage.as_ref().unwrap();
                let d = &self.device;
                let tv = |i: usize| wgpu::BindingResource::TextureView(&s.views[i]);
                let groups = ((s.w as u64 * s.h as u64).div_ceil(16384) as u32).clamp(1, 1024);
                passes.push(Pass { pipe: Pipe::KitHist as usize, groups: (groups, 1, 1),
                    bg: bind_group_at(d, self.pipe(Pipe::KitHist), &[(0, tv(inp)), (5, hist.as_entire_binding())]) });
                let reduce = Self::fx_pipe(f, 1);
                passes.push(Pass { pipe: reduce, groups: (1, 1, 1),
                    bg: bind_group_at(d, self.pipe_at(reduce), &[(2, pr()), (5, hist.as_entire_binding()), (6, res.as_entire_binding())]) });
                passes.push(Pass { pipe: apply, groups: (s.w.div_ceil(16), s.h.div_ceil(16), 1),
                    bg: bind_group_at(d, self.pipe_at(apply), &[(0, tv(inp)), (1, tv(out)), (2, pr()), (6, res.as_entire_binding())]) });
                clears.push(hist.clone());
                keep.extend([hist, res]);
            }
        }
        (passes, keep, clears)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry() {
        for (i, f) in FILTERS.iter().enumerate() {
            assert_eq!(find(f.id), Some(i));
            assert_eq!(f.blur.is_some(), f.kind == FxKind::Neighbourhood, "{}", f.id);
            assert!(f.consts.matches("const P_").count() == f.nparams, "{}", f.id);
        }
        let t = find("test_pixel").expect("test filter");
        assert!(Step::fx("test_pixel", &[0.0; 2], 1.0).is_err());
        assert!(Step::fx("nope", &[], 1.0).is_err());
        let st = Step::fx("test_pixel", &[10.0, 0.0, 5.0], 0.5).unwrap();
        assert_eq!(st.aux, Aux::Fx { f: t as u16 });
        assert_eq!(st.fx_values(), (0.5, vec![10.0, 0.0, 5.0]));
    }
}
