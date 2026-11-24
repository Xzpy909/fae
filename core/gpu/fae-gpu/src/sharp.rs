//! Sharpen on the GPU (G4): the same parts, constants and order as the CPU core's sharp.rs, run after
//! every other step on the finished 8-bit pixels: Adaptive Sharpening (with Balance), Structure,
//! Local Contrast, Focus. Each active part is a step of its own (`Step::sharp` returns them in that
//! order), so moving a slider only rewrites its parameters; a new shape (kernel length, shrink
//! factor) sets the passes up again.
//!
//! - Adaptive Sharpening, Local Contrast, Focus (sharp_blur.wgsl): per channel, out = f(c, blur_c);
//!   the blur is a Gaussian of a 256-entry map of the level (identity, or v^3.8 for Focus < 0), on a
//!   copy shrunk by k x k block means when sigma is large (k = 1: full size), read back bilinearly.
//!   Passes: block means -> Gaussian across -> Gaussian down -> apply (Balance's Sobel weights are
//!   computed there from the input).
//! - Structure (sharp_st.wgsl): the 3x3 self-guided filters (fine), and the mid and coarse fast guided
//!   filters (box filters of box_h / box_v on a copy shrunk by k, as Mid/Detail), then the tanh table.

use crate::{bind_group, bind_group_at, Aux, Gpu, Kind, Pass, Pipe, Step};

pub const BLUR_WGSL: &str = include_str!("../shaders/sharp_blur.wgsl");
pub const ST_WGSL: &str = include_str!("../shaders/sharp_st.wgsl");

// constants: the CPU core's sharp.rs
pub const ST_EPS: f32 = 3e-3;
pub const ST_R_1440: f64 = 29.0;
pub const ST_FAST_R: u32 = 12;
pub const ST_FAST_RM: u32 = 8;
pub const ST_RM_1440: f64 = 10.0;
pub const ST_POS_GAIN: f64 = 2.0;
pub const ST_LIM_POS: f64 = 38.0;
pub const ST_LIM_NEG: f64 = 20.0;
const TQ: f64 = 4.0;
const TN: usize = 4096;
pub const LC_SIGMA: f64 = 20.0;
pub const LC_POS: f64 = 0.51;
pub const LC_NEG: f64 = 0.75;
const LC_SMALL_SIGMA: f64 = 4.0;
pub const FO_POS: f32 = 1.0;
pub const FO_SIGMA_POS: f64 = 1.0;
pub const FO_GAMMA: f64 = 3.8;
pub const FO_SIGMA_NEG: f64 = 5.44;
pub const FO_SIGMA_EXP: f64 = 1.86;
pub const FO_MIX: f32 = 0.96;
pub const AS_SIGMA: f64 = 1.93;
pub const AS_GAIN: f64 = 1.78;
pub const AS_RAMP: f64 = 0.85;
/// entries of Focus's back table (4th root grid)
const NB: usize = 4096;

/// parameter layout of a blur step (sharp_blur.wgsl)
pub(crate) const BL_TAB: usize = 16;
pub(crate) const BL_KER: usize = 272;
/// parameter layout of the Structure step (sharp_st.wgsl)
pub(crate) const ST_TABLE: usize = 16;

/// The radius for an image `height` pixels high (`at_1440` px at 1440 px), half to even.
fn rad(at_1440: f64, height: u32) -> u32 {
    ((at_1440 * height as f64 / 1440.0).round_ties_even() as u32).max(1)
}

/// Structure's coarse filter: radius and shrink factor for an image `h` pixels high.
pub fn st_coarse(h: u32) -> (u32, u32) {
    let r = rad(ST_R_1440, h);
    (r, (r / ST_FAST_R).max(1))
}

/// Structure's mid filter: radius and shrink factor.
pub fn st_mid(h: u32) -> (u32, u32) {
    let r = rad(ST_RM_1440, h);
    (r, (r / ST_FAST_RM).max(1))
}

/// The box radius on the shrunk copy (md::guided_fast).
pub fn small_radius(r: u32, k: u32) -> u32 {
    if k <= 1 { r } else { ((r as f64 / k as f64).round() as u32).max(1) }
}

/// Normalised Gaussian taps (as sharp.rs gauss_kernel: f64 exp to f32, f32 sum, f32 division).
fn gauss_kernel(sigma: f64) -> Vec<f32> {
    let rad = ((3.0 * sigma).ceil() as usize).max(1);
    let mut ker: Vec<f32> = (0..=2 * rad)
        .map(|i| {
            let d = i as f64 - rad as f64;
            (-0.5 * d * d / (sigma * sigma)).exp() as f32
        })
        .collect();
    let ks: f32 = ker.iter().sum();
    ker.iter_mut().for_each(|v| *v /= ks);
    ker
}

/// sigma on the copy shrunk by k (the block mean already blurred by sqrt((k^2-1)/12))
fn small_sigma(sigma: f64, k: usize) -> f64 {
    ((sigma * sigma - ((k * k - 1) as f64) / 12.0).max(1e-6)).sqrt() / k as f64
}

/// Which formula a blur step applies (P[0]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BlurMode {
    /// out = c + g (c - blur): Adaptive Sharpening (with Balance), Local Contrast, Focus > 0
    Unsharp = 0,
    /// Focus < 0: blur of c^3.8, back through the 4th-root table, mixed in at FO_MIX
    Gamma = 1,
}

/// One blur step: the map of the level, sigma (pixels of this image), shrink factor, formula.
fn blur_step(mode: BlurMode, g: f32, sigma: f64, k: usize, balance: f32, tab: &[f32; 256], back: Option<&[f32]>) -> Step {
    let ker = if k == 1 { gauss_kernel(sigma) } else { gauss_kernel(small_sigma(sigma, k)) };
    let taps = ker.len();
    let mut p = vec![0f32; BL_KER + taps + back.map_or(0, |b| b.len())];
    p[0] = mode as u32 as f32;
    p[1] = g;
    p[2] = k as f32;
    p[3] = taps as f32;
    p[4] = (taps / 2) as f32;
    p[5] = balance;
    p[11] = FO_MIX;
    p[BL_TAB..BL_TAB + 256].copy_from_slice(tab);
    p[BL_KER..BL_KER + taps].copy_from_slice(&ker);
    if let Some(b) = back {
        p[BL_KER + taps..].copy_from_slice(b);
    }
    Step::raw(Kind::Sharp, p, Aux::SharpBlur { k: k as u32, taps: taps as u32, mode: mode as u32 })
}

impl Step {
    /// Sharpen (sliders -1..1 as the edit's `sharp`: Adaptive 0..1, Balance, Structure, Local
    /// Contrast, Focus; `scale` = pixels of this image per output pixel, 1 for Process): 0 to 4 steps,
    /// to go at the end of the chain. The same parts and order as the CPU core's sharp::apply.
    pub fn sharp(adaptive: f64, balance: f64, structure: f64, local_contrast: f64, focus: f64, scale: f64) -> Vec<Step> {
        let mut out = vec![];
        let scale = scale.clamp(1e-3, 1.0e3);
        let id: [f32; 256] = core::array::from_fn(|i| i as f32);
        if adaptive > 0.0 {
            let a = adaptive.min(1.0);
            let g = (AS_GAIN * (a / 0.5).min(1.0).powf(AS_RAMP)) as f32;
            let sigma = AS_SIGMA * a * scale;
            out.push(blur_step(BlurMode::Unsharp, g, sigma, blur_k(sigma), balance.clamp(-1.0, 1.0) as f32, &id, None));
        }
        if structure != 0.0 {
            let (g, lim) = if structure > 0.0 { (structure * ST_POS_GAIN, ST_LIM_POS) } else { (structure, ST_LIM_NEG) };
            let k = g / lim;
            let mut p = vec![0f32; ST_TABLE + 2 * TN + 1];
            for i in 0..=2 * TN {
                p[ST_TABLE + i] = (lim * (k * ((i as f64 - TN as f64) / TQ)).tanh()) as f32;
            }
            out.push(Step::raw(Kind::Sharp, p, Aux::SharpSt));
        }
        if local_contrast != 0.0 {
            let s = local_contrast;
            let g = if s > 0.0 { LC_POS * s } else { LC_NEG * s };
            let sigma = LC_SIGMA * scale;
            let k = ((sigma / LC_SMALL_SIGMA).round() as usize).max(1);
            out.push(blur_step(BlurMode::Unsharp, g as f32, sigma, k, 0.0, &id, None));
        }
        if focus > 0.0 {
            let sigma = FO_SIGMA_POS * scale;
            out.push(blur_step(BlurMode::Unsharp, FO_POS * focus as f32, sigma, blur_k(sigma), 0.0, &id, None));
        } else if focus < 0.0 {
            let fwd: [f32; 256] = core::array::from_fn(|v| (v as f64 / 255.0).powf(FO_GAMMA) as f32);
            let back: Vec<f32> = (0..=NB).map(|i| (255.0 * (i as f64 / NB as f64).powf(4.0 / FO_GAMMA)) as f32).collect();
            let sigma = FO_SIGMA_NEG * (-focus).powf(FO_SIGMA_EXP) * scale;
            out.push(blur_step(BlurMode::Gamma, 0.0, sigma, blur_k(sigma), 0.0, &fwd, Some(&back)));
        }
        out
    }
}

/// sharp.rs blur_map's shrink factor
fn blur_k(sigma: f64) -> usize {
    if sigma >= 3.0 { ((sigma / 1.5).round() as usize).max(1) } else { 1 }
}

impl Gpu {
    /// The stage's two RGB float planes for the blur steps (3 f32 per pixel), made on first use.
    fn sharp_planes(&mut self) -> [wgpu::Buffer; 2] {
        let d = self.device.clone();
        let s = self.stage.as_mut().unwrap();
        if s.splanes.is_none() {
            let size = (s.w as u64 * s.h as u64 * 12).max(16);
            let mk = || d.create_buffer(&wgpu::BufferDescriptor { label: Some("sharp plane"), size, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
            s.splanes = Some([mk(), mk()]);
        }
        s.splanes.clone().unwrap()
    }

    /// A blur step (`inp` -> `out`): block means, Gaussian across, Gaussian down, apply.
    pub(crate) fn sharp_blur_passes(&mut self, inp: usize, out: usize, params: &wgpu::Buffer, k: u32) -> Vec<Pass> {
        let [pa, pb] = self.sharp_planes();
        let s = self.stage.as_ref().unwrap();
        let (w2, h2) = (s.w.div_ceil(k), s.h.div_ceil(k));
        let d = &self.device;
        let px = |p: Pipe| self.pipe(p);
        let tv = |i: usize| wgpu::BindingResource::TextureView(&s.views[i]);
        let pr = || params.as_entire_binding();
        let gg = (w2.div_ceil(16), h2.div_ceil(16), 1);
        vec![
            Pass { pipe: Pipe::SbMeans, groups: gg,
                   bg: bind_group_at(d, px(Pipe::SbMeans), &[(0, tv(inp)), (2, pr()), (3, pa.as_entire_binding())]) },
            Pass { pipe: Pipe::SbAcross, groups: gg,
                   bg: bind_group_at(d, px(Pipe::SbAcross), &[(2, pr()), (3, pa.as_entire_binding()), (4, pb.as_entire_binding())]) },
            Pass { pipe: Pipe::SbDown, groups: gg,
                   bg: bind_group_at(d, px(Pipe::SbDown), &[(2, pr()), (3, pa.as_entire_binding()), (4, pb.as_entire_binding())]) },
            Pass { pipe: Pipe::SbApply, groups: (s.w.div_ceil(16), s.h.div_ceil(16), 1),
                   bg: bind_group_at(d, px(Pipe::SbApply), &[(0, tv(inp)), (1, tv(out)), (2, pr()), (3, pa.as_entire_binding())]) },
        ]
    }

    /// Structure (`inp` -> `out`): the coarse and mid fast guided filters (prep on the shrunk copy,
    /// box -> (a, b) -> box -> means, as Mid/Detail), the 3x3 coefficients of the fine filters into
    /// the stage's scratch planes, then apply. Returns the passes and the buffers to keep.
    pub(crate) fn sharp_st_passes(&mut self, inp: usize, out: usize, params: &wgpu::Buffer) -> (Vec<Pass>, Vec<wgpu::Buffer>) {
        let [f0, f1] = self.scratch();
        let s = self.stage.as_ref().unwrap();
        let (w, h) = (s.w, s.h);
        let mut passes = vec![];
        let mut keep = vec![];
        let mut results = vec![];
        let gfull = (w.div_ceil(16), h.div_ceil(16), 1);
        for (r, k) in [st_coarse(h), st_mid(h)] {
            let (w2, h2) = (w.div_ceil(k), h.div_ceil(k));
            let r2 = small_radius(r, k);
            let plane = |label| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: (w2 as u64 * h2 as u64 * 8).max(16),
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                })
            };
            let (b0, b1) = (plane("st grid"), plane("st grid 2"));
            let pp = self.small_storage(&[k as f32, w2 as f32, h2 as f32]);
            let ph = self.small_storage(&[w2 as f32, h2 as f32, r2 as f32]);
            let pv0 = self.small_storage(&[w2 as f32, h2 as f32, r2 as f32, 0.0, ST_EPS]);
            let pv1 = self.small_storage(&[w2 as f32, h2 as f32, r2 as f32, 1.0, ST_EPS]);
            let d = &self.device;
            let px = |p: Pipe| self.pipe(p);
            let grow = (w2.div_ceil(64).div_ceil(64), h2, 1);
            let gcol = (w2.div_ceil(64), h2.div_ceil(64), 1);
            let bb = |a: &wgpu::Buffer, b2: &wgpu::Buffer, p: &wgpu::Buffer, pipe: Pipe| Pass {
                pipe,
                bg: bind_group(d, px(pipe), &[a.as_entire_binding(), b2.as_entire_binding(), p.as_entire_binding()]),
                groups: if pipe == Pipe::BoxH { grow } else { gcol },
            };
            passes.push(Pass { pipe: Pipe::StPrep, groups: (w2.div_ceil(16), h2.div_ceil(16), 1),
                bg: bind_group_at(d, px(Pipe::StPrep), &[(0, wgpu::BindingResource::TextureView(&s.views[inp])), (3, b0.as_entire_binding()), (4, pp.as_entire_binding())]) });
            passes.push(bb(&b0, &b1, &ph, Pipe::BoxH));
            passes.push(bb(&b1, &b0, &pv0, Pipe::BoxV));
            passes.push(bb(&b0, &b1, &ph, Pipe::BoxH));
            passes.push(bb(&b1, &b0, &pv1, Pipe::BoxV));
            results.push(b0);
            keep.extend([b1, pp, ph, pv0, pv1]);
        }
        let d = &self.device;
        let px = |p: Pipe| self.pipe(p);
        let tv = |i: usize| wgpu::BindingResource::TextureView(&s.views[i]);
        passes.push(Pass { pipe: Pipe::StFine, groups: gfull,
            bg: bind_group_at(d, px(Pipe::StFine), &[(0, tv(inp)), (5, f0.as_entire_binding()), (6, f1.as_entire_binding())]) });
        passes.push(Pass { pipe: Pipe::StApply, groups: gfull,
            bg: bind_group_at(d, px(Pipe::StApply), &[(0, tv(inp)), (1, tv(out)), (2, params.as_entire_binding()),
                (5, f0.as_entire_binding()), (6, f1.as_entire_binding()), (7, results[0].as_entire_binding()), (8, results[1].as_entire_binding())]) });
        keep.extend(results);
        (passes, keep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radii_match_the_cpu_core() {
        // sharp.rs: st_radius(1440) = 29, k = 29 / 12 = 2; mid 10, k 1
        assert_eq!(st_coarse(1440), (29, 2));
        assert_eq!(st_mid(1440), (10, 1));
        assert_eq!(st_coarse(4000), (81, 6));
        assert_eq!(small_radius(81, 6), 14);
        assert_eq!(st_coarse(3), (1, 1));
    }

    #[test]
    fn steps_per_part() {
        assert!(Step::sharp(0.0, 0.5, 0.0, 0.0, 0.0, 1.0).is_empty()); // Balance alone: off
        assert_eq!(Step::sharp(0.5, 0.0, 0.3, -0.2, -0.4, 1.0).len(), 4);
        let k = gauss_kernel(1.0);
        assert_eq!(k.len(), 7);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }
}
