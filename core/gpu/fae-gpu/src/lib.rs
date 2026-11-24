//! fae-gpu: the edit chain on the GPU (wgpu + WGSL). See claude/gpu-core-plan.md.
//!
//! One crate, two builds: natively (DirectX 12 / Vulkan; the Python extension fae-gpu-py, Process
//! and tests) and in the browser (WebGPU; fae-gpu-wasm, the live preview). The same WGSL files run
//! in both, so preview = export by construction.
//!
//! Pieces:
//! - [`Gpu`]: adapter + device (with the adapter's full limits), the pipelines, error collection.
//! - the stage ([`Gpu::upload`]): 8-bit RGB / RGBA -> `orig` (rgba16float), never written again.
//! - the pass graph ([`Gpu::set_steps`], [`Gpu::run`]): one pass per chain step, in chain order,
//!   ping-ponging between two images; parameters live in buffers that are only rewritten when the
//!   kinds of steps stay the same (a slider move), rebuilt when they change.
//! - screen output ([`view`]): the result (or `orig`, for the split / before views) resampled to
//!   exact screen pixels with Lanczos-3, drawn into a surface.
//! - read back ([`Gpu::readback_start`]): result -> packed 8-bit RGB (Process, tests) or RGBA
//!   ([`Gpu::readback_rgba_start`], the preview's pixels); the UI's histograms counted on the GPU
//!   ([`Gpu::set_stats`], [`Gpu::stats_start`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub use wgpu;

pub mod ops;
pub mod sharp;
pub mod view;
#[cfg(feature = "emulate")]
pub mod emulate;

pub const DECODE_WGSL: &str = include_str!("../shaders/decode.wgsl");
pub const ENCODE_WGSL: &str = include_str!("../shaders/encode.wgsl");
pub const ENCODE_RGBA_WGSL: &str = include_str!("../shaders/encode_rgba.wgsl");
pub const HIST_WGSL: &str = include_str!("../shaders/hist.wgsl");
pub const CURVES_WGSL: &str = include_str!("../shaders/curves.wgsl");
/// prepended to the per-pixel steps' shaders below
pub const COMMON_WGSL: &str = include_str!("../shaders/common.wgsl");
pub const POINT_WGSL: &str = include_str!("../shaders/point.wgsl");
pub const ADJUST_WGSL: &str = include_str!("../shaders/adjust.wgsl");
pub const BW_WGSL: &str = include_str!("../shaders/bw.wgsl");
pub const FILM_WGSL: &str = include_str!("../shaders/film.wgsl");

/// Histogram bins: hin 768, hout 768, hmx 256, hmn 256 (see hist.wgsl).
pub const HIST_BINS: usize = 2048;

/// Format of the images between passes.
pub const IMG_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Ask for the software adapter (WARP on Windows, lavapipe / llvmpipe on Linux).
    pub fallback: bool,
    /// Restrict the backends; None = the default (native: DX12 on Windows, the same API Chrome's
    /// WebGPU uses there; FAE_GPU_BACKEND=vulkan|dx12 overrides).
    pub backends: Option<wgpu::Backends>,
}

/// What the device is, for reports.
#[derive(Clone, Debug)]
pub struct Info {
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    pub driver_info: String,
    pub max_texture_2d: u32,
    pub max_buffer: u64,
    pub max_storage_binding: u64,
}

pub fn json_str(x: &str) -> String {
    let mut o = String::from("\"");
    for c in x.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

impl Info {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"name\":{},\"backend\":{},\"device_type\":{},\"driver\":{},\"driver_info\":{},\
             \"max_texture_2d\":{},\"max_buffer\":{},\"max_storage_binding\":{},\"max_pixels\":{}}}",
            json_str(&self.name),
            json_str(&self.backend),
            json_str(&self.device_type),
            json_str(&self.driver),
            json_str(&self.driver_info),
            self.max_texture_2d,
            self.max_buffer,
            self.max_storage_binding,
            self.max_pixels()
        )
    }

    /// Largest image (pixels) these limits allow: the upload buffer holds 4 bytes per pixel (RGBA
    /// from the preview) and must fit one storage binding.
    pub fn max_pixels(&self) -> u64 {
        let side = self.max_texture_2d as u64;
        (self.max_storage_binding.min(self.max_buffer) / 4).min(side * side)
    }
}

fn backend_name(b: wgpu::Backend) -> &'static str {
    match b {
        wgpu::Backend::Vulkan => "vulkan",
        wgpu::Backend::Metal => "metal",
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Gl => "gl",
        wgpu::Backend::BrowserWebGpu => "webgpu",
        _ => "other",
    }
}

pub fn parse_backends(s: &str) -> Option<wgpu::Backends> {
    let mut b = wgpu::Backends::empty();
    for p in s.split([',', ' ']).filter(|p| !p.is_empty()) {
        b |= match p.to_ascii_lowercase().as_str() {
            "dx12" | "d3d12" => wgpu::Backends::DX12,
            "vulkan" | "vk" => wgpu::Backends::VULKAN,
            "gl" | "gles" => wgpu::Backends::GL,
            "metal" => wgpu::Backends::METAL,
            "webgpu" => wgpu::Backends::BROWSER_WEBGPU,
            "all" => wgpu::Backends::all(),
            _ => return None,
        };
    }
    if b.is_empty() { None } else { Some(b) }
}

/// The kinds of steps the GPU runs: every tool (G2: the per-pixel ones; G3: Mid/Detail, Perceptual
/// Saturation's perceptual mode, Super Contrast; G4: Sharpen, as steps at the end of the chain).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Curves = 0,
    Point = 1,
    Adjust = 2,
    Bw = 3,
    Film = 4,
    Sc = 5,
    /// one part of Sharpen (sharp.rs)
    Sharp = 6,
}

/// One step of the chain, as the GPU runs it: its kind and its parameter buffer (the layouts are
/// described at the top of each shader).
#[derive(Clone, Debug)]
pub struct Step {
    kind: Kind,
    data: Vec<u8>,
    /// whole-image work the step needs before its per-pixel pass (ops.rs)
    pub(crate) aux: Aux,
}

/// The whole-image part of a step (passes before its per-pixel pass, see ops.rs).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Aux {
    None,
    /// Mid/Detail: the guided filter of the luma
    Md,
    /// Perceptual Saturation: the grid of cell means, blurred
    Ps,
    /// Super Contrast: global statistics, and with `dyn` the Dynamic Contrast cell curves
    Sc { dyn_on: bool },
    /// Sharpen's blur-based parts: shrink factor, kernel length, formula (sharp_blur.wgsl)
    SharpBlur { k: u32, taps: u32, mode: u32 },
    /// Sharpen's Structure (sharp_st.wgsl)
    SharpSt,
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn need(name: &str, t: &[f64], n: usize) -> Result<(), String> {
    if t.len() < n { Err(format!("{name}: {} values, need {n}", t.len())) } else { Ok(()) }
}

/// most tone tables one Adjustments step can carry (Shadows and Highlights use one each)
pub const MAX_TONES: usize = 16;

impl Step {
    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub(crate) fn raw(kind: Kind, p: Vec<f32>, aux: Aux) -> Step {
        Step { kind, data: f32_bytes(&p), aux }
    }

    /// Curves: 3 x 256 levels (r, g, b; the master curve folded in).
    pub fn curves(luts: &[u8]) -> Result<Step, String> {
        if luts.len() != 768 {
            return Err(format!("curves: {} table entries, need 768", luts.len()));
        }
        Ok(Step { kind: Kind::Curves, data: luts.iter().flat_map(|&v| (v as u32).to_le_bytes()).collect(), aux: Aux::None })
    }

    /// Point correction: luma levels (k, o; off when k = 1 and o = 0), then gains.
    pub fn point(k: f64, o: f64, gains: Option<[f64; 3]>) -> Step {
        let g = gains.unwrap_or([1.0; 3]);
        let lv = if k != 1.0 || o != 0.0 { 1.0 } else { 0.0 };
        // the anchor: the new luma at 255, whole when within 1e-9 of a whole level (as the CPU core's
        // edit::point_anchor); the shader evaluates anchor + (luma - 255) k, exact for white
        let a = 255.0 * k + o;
        let a = if (a - a.round()).abs() < 1e-9 { a.round() } else { a };
        let p = [lv, k as f32, a as f32, 0.0, g[0] as f32, g[1] as f32, g[2] as f32, 0.0];
        Step { kind: Kind::Point, data: f32_bytes(&p), aux: Aux::None }
    }

    /// Adjustments (tables as the UI's computeEdit builds them); `md` = Mid/Detail amount (-1..1, 0 = off).
    #[allow(clippy::too_many_arguments)]
    pub fn adjust(md: f64, wb: Option<[[f64; 3]; 3]>, con: Option<&[f64]>, tones: &[(&[f64], bool)],
                  boost: Option<&[f64]>, sat: Option<f64>) -> Result<Step, String> {
        if tones.len() > MAX_TONES {
            return Err(format!("adjust: {} tone tables, at most {MAX_TONES}", tones.len()));
        }
        const CON: usize = 48;
        const BOOST: usize = CON + 4096;
        const MDW: usize = BOOST + 256;
        const TONES: usize = MDW + 4097;
        let mut p = vec![0f32; TONES + 4096 * tones.len()];
        if let Some(w) = wb {
            p[0] = 1.0;
            for r in 0..3 {
                for c in 0..3 {
                    p[6 + r * 3 + c] = w[r][c] as f32;
                }
            }
        }
        if let Some(t) = con {
            need("con", t, 4096)?;
            p[1] = 1.0;
            for i in 0..4096 {
                p[CON + i] = t[i] as f32;
            }
        }
        p[2] = tones.len() as f32;
        for (k, (t, ratio)) in tones.iter().enumerate() {
            need("tones", t, 4096)?;
            p[16 + k] = if *ratio { 1.0 } else { 0.0 };
            for i in 0..4096 {
                p[TONES + k * 4096 + i] = t[i] as f32;
            }
        }
        if let Some(t) = boost {
            need("boost", t, 256)?;
            p[3] = 1.0;
            for i in 0..256 {
                p[BOOST + i] = t[i] as f32;
            }
        }
        if let Some(v) = sat {
            p[4] = 1.0;
            p[5] = v as f32;
        }
        let aux = if md != 0.0 {
            p[15] = 1.0;
            p[32] = (md * ops::MD_GAIN * 255.0) as f32;
            // the fade-in weight min(1, (y / K) ^ GAMMA) on a grid of 4097 (as the CPU core's px32.rs)
            for i in 0..=4096 {
                p[MDW + i] = ((i as f64 / 4096.0 / ops::MD_K).powf(ops::MD_GAMMA)).min(1.0) as f32;
            }
            Aux::Md
        } else {
            Aux::None
        };
        Ok(Step { kind: Kind::Adjust, data: f32_bytes(&p), aux })
    }

    /// The Perceptual Saturation filter without its perceptual mode: Saturation s, opacity, Warmth gains.
    pub fn bw(s: f64, op: f64, w: Option<[f64; 3]>) -> Step {
        let mut p = vec![0f32; 8];
        p[0] = s as f32;
        p[1] = op as f32;
        p[2] = if w.is_some() { 1.0 } else { 0.0 };
        let g = w.unwrap_or([1.0; 3]);
        p[4..7].copy_from_slice(&[g[0] as f32, g[1] as f32, g[2] as f32]);
        p.extend(warmth_table(w));
        p.extend(sat_table(s));
        if op != 1.0 {
            p[3] = 1.0;
            p.extend(mix_table(op, |l| l as f64));
        }
        Step { kind: Kind::Bw, data: f32_bytes(&p), aux: Aux::None }
    }

    /// The Perceptual Saturation filter with its perceptual mode: amount `ps` (> 0) and the Gaussian
    /// kernel over the grid (`psk`, odd length, at most 64). Layout: ps.wgsl.
    pub fn bw_ps(s: f64, op: f64, w: Option<[f64; 3]>, ps: f64, psk: &[f64]) -> Result<Step, String> {
        if psk.is_empty() || psk.len() > 64 || psk.len() % 2 == 0 {
            return Err(format!("bw psk: {} values, need an odd count up to 64", psk.len()));
        }
        let g = w.unwrap_or([1.0; 3]);
        let mut p = vec![0f32; 80];
        p[0] = s as f32;
        p[1] = op as f32;
        p[2] = if w.is_some() { 1.0 } else { 0.0 };
        p[4] = g[0] as f32;
        p[5] = g[1] as f32;
        p[6] = g[2] as f32;
        p[8] = ps as f32;
        p[13] = psk.len() as f32;
        for (i, k) in psk.iter().enumerate() {
            p[16 + i] = *k as f32;
        }
        p.extend(warmth_table(w));
        p.extend(sat_table(s));
        if op != 1.0 {
            p[3] = 1.0;
            p.extend(mix_table(op, |q| q as f64));
        }
        Ok(Step { kind: Kind::Bw, data: f32_bytes(&p), aux: Aux::Ps })
    }

    /// Film Effect: n^3 x 3 table of deltas (or none), Brightness / Contrast table (256), opacity.
    pub fn film(n: usize, d: Option<&[f64]>, t1: Option<&[f64]>, op: f64) -> Result<Step, String> {
        if n < 2 {
            return Err("film: n must be at least 2".into());
        }
        if let Some(t) = t1 {
            need("film t1", t, 256)?;
        }
        let dn = n * n * n * 3;
        let mix = op != 1.0;
        let d0 = 272 + if mix { MIX_N } else { 0 };
        let mut p = vec![0f32; d0 + if d.is_some() { dn } else { 0 }];
        p[0] = n as f32;
        p[3] = op as f32;
        p[5] = d0 as f32;
        if let Some(t) = t1 {
            p[2] = 1.0;
            for i in 0..256 {
                p[16 + i] = t[i] as f32;
            }
        }
        if mix {
            // opacity over the level before the tone table: rnd(f + (t1[j] - f) * op), in f64 as the
            // CPU core computes it
            p[4] = 1.0;
            let tab = mix_table(op, |j| match t1 {
                Some(t) => t[j],
                None => j as f64,
            });
            p[272..272 + MIX_N].copy_from_slice(&tab);
        }
        if let Some(t) = d {
            if t.len() != dn {
                return Err(format!("film: d has {} values, need {dn}", t.len()));
            }
            p[1] = 1.0;
            for (i, v) in t.iter().enumerate() {
                p[d0 + i] = *v as f32;
            }
        }
        Ok(Step { kind: Kind::Film, data: f32_bytes(&p), aux: Aux::None })
    }
}

impl Step {
    /// Super Contrast: Correct Color Cast (0..1), Correct Contrast (0..100), Dynamic Contrast (0..1)
    /// with its kernel over the cells (`dk`, odd length up to 64), opacity, the Shadows / Highlights
    /// table (256) or none. Layout: sc.wgsl.
    pub fn sc(cast: f64, cc: f64, dyn_: f64, op: f64, dk: Option<&[f64]>, shl: Option<&[f64]>) -> Result<Step, String> {
        let dk = dk.unwrap_or(&[]);
        if dyn_ > 0.0 && (dk.is_empty() || dk.len() > 64 || dk.len() % 2 == 0) {
            return Err(format!("sc dk: {} values, need an odd count up to 64", dk.len()));
        }
        let mut p = vec![0f32; 336];
        p[0] = cast as f32;
        p[1] = cc as f32;
        p[2] = dyn_ as f32;
        p[3] = op as f32;
        p[9] = dk.len() as f32;
        for (i, k) in dk.iter().enumerate() {
            p[16 + i] = *k as f32;
        }
        if let Some(t) = shl {
            need("sc shl", t, 256)?;
            p[4] = 1.0;
            for i in 0..256 {
                p[80 + i] = t[i] as f32;
            }
        }
        if op != 1.0 {
            p[5] = 1.0;
            p.extend(mix_table(op, |j| shl.map_or(j as f64, |t| t[j])));
        }
        Ok(Step { kind: Kind::Sc, data: f32_bytes(&p), aux: Aux::Sc { dyn_on: dyn_ > 0.0 } })
    }
}

/// entries of an opacity table: 256 levels before x 256 levels after the filter
const MIX_N: usize = 65536;

/// The filter's Warmth, per channel and level: min(floor(f * gain), 255) in f64 (768 entries; the
/// identity without Warmth). In f32 a gain like 1.05 is just below 1.05, and 20 * gain truncated
/// to 20 instead of 21.
fn warmth_table(w: Option<[f64; 3]>) -> Vec<f32> {
    let g = w.unwrap_or([1.0; 3]);
    (0..768).map(|i| ((i % 256) as f64 * g[i / 256]).floor().min(255.0) as f32).collect()
}

/// The filter's Saturation, rounded: rnd(y + (w - y) * s) with y = (r + 2g + b) / 4, for every
/// Y = r + 2g + b (0..=1020) and channel level w, in f64 (1021 x 256 entries). y and w - y are
/// quarters, so slider values put many results exactly on .5, where f32 (and fused multiply-adds
/// on GPUs) round the other way.
fn sat_table(s: f64) -> Vec<f32> {
    let mut t = Vec::with_capacity(SAT_N);
    for yy in 0..1021 {
        let y = yy as f64 / 4.0;
        for w in 0..256 {
            t.push(((y + (w as f64 - y) * s + 0.5).clamp(0.0, 255.0) as i32) as f32);
        }
    }
    t
}

/// entries of a Saturation table: Y = r + 2g + b (0..=1020) x level
const SAT_N: usize = 1021 * 256;

/// Opacity `rnd(f + (v(j) - f) * op)` for every level before (f) and after (j) a filter, in f64:
/// the CPU core's result exactly. On the GPU the same expression in f32 goes wrong at the structured
/// boundaries a slider value makes (op = 0.66, v - f = 25 -> 16.5: whether it rounds up depends
/// on how the product is rounded, and GPUs fuse it into one fma), so whole levels look it up.
fn mix_table(op: f64, v: impl Fn(usize) -> f64) -> Vec<f32> {
    let mut t = Vec::with_capacity(MIX_N);
    for f in 0..256 {
        let fv = f as f64;
        for j in 0..256 {
            t.push(((fv + (v(j) - fv) * op + 0.5).clamp(0.0, 255.0) as i32) as f32);
        }
    }
    t
}

/// The tool kinds the GPU can run (names as in fastedit/core.py's steps; "sharp" = the Sharpen tool).
pub const PORTED: &[&str] = &["point", "adjust", "curves", "bw", "film", "sc", "sharp"];

/// Uniforms shared by decode / encode (16 bytes).
fn dims(w: u32, h: u32, third: u32) -> [u8; 16] {
    let mut b = [0u8; 16];
    for (i, v) in [w, h, third, w * h].iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    b
}

/// The images of one stage: tex[0] = orig (the stage, never written after upload), tex[1] / tex[2]
/// = ping-pong for the chain.
struct Stage {
    w: u32,
    h: u32,
    bpp: u32,
    packed_in: wgpu::Buffer,
    #[allow(dead_code)] // keeps the textures alive (the views below refer to them)
    tex: [wgpu::Texture; 3],
    views: [wgpu::TextureView; 3],
    #[allow(dead_code)]
    dims_dec: wgpu::Buffer,
    dims_enc: wgpu::Buffer,
    bg_decode: wgpu::BindGroup,
    /// histogram passes on each of the three images
    hb_in: [wgpu::BindGroup; 3],
    hb_out: [wgpu::BindGroup; 3],
    /// workgroups of a histogram pass
    hist_groups: u32,
    /// RGBA read back (the preview's pixels), made on first use: out buffer, staging
    rgba: Option<(wgpu::Buffer, wgpu::Buffer)>,
    /// two vec2<f32> planes for the whole-image passes (Mid/Detail's guided filter), made on first use
    scratch: Option<[wgpu::Buffer; 2]>,
    /// two RGB f32 planes (12 bytes per pixel) for Sharpen's blurs, made on first use
    splanes: Option<[wgpu::Buffer; 2]>,
    /// read back: packed RGB words, len = ceil(w*h*3 / 4)
    words: u64,
    out: wgpu::Buffer,
    staging: wgpu::Buffer,
    enc_groups: (u32, u32),
}

/// One dispatch.
pub(crate) struct Pass {
    pub pipe: Pipe,
    pub bg: wgpu::BindGroup,
    pub groups: (u32, u32, u32),
}

/// One step of the graph: its passes (whole-image work first, the per-pixel pass last).
struct Op {
    kind: Kind,
    aux: Aux,
    len: usize,
    params: wgpu::Buffer,
    passes: Vec<Pass>,
    /// buffers zeroed before the run (atomic counters)
    clears: Vec<wgpu::Buffer>,
    /// small parameter buffers of the whole-image passes, rewritten with the step's values
    #[allow(dead_code)]
    keep: Vec<wgpu::Buffer>,
    inp: usize,
    out: usize,
}

/// Compute pipelines by name (index into Gpu::pipes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pipe {
    Curves = 0,
    Point,
    Adjust,
    Bw,
    Film,
    MdPrep,
    BoxH,
    BoxV,
    PsMeasure,
    PsMean,
    PsBlur,
    PsApply,
    ScHist,
    ScReduce,
    ScCellHist,
    ScNorm,
    ScBlur,
    ScClip,
    ScCurve,
    ScApply,
    SbMeans,
    SbAcross,
    SbDown,
    SbApply,
    StPrep,
    StFine,
    StApply,
    Decode,
    Encode,
    EncodeRgba,
    HistIn,
    HistOut,
}

/// Shader modules: one per WGSL file (with what it's built from), shared by its entry points.
#[derive(Clone, Copy, Debug)]
enum Module { Curves, Point, Adjust, Bw, Film, MdPrep, BoxH, BoxV, Ps, Sc, SharpBlur, SharpSt, Decode, Encode, EncodeRgba, Hist }

impl Module {
    fn source(self) -> String {
        let common = |src: &str| format!("{COMMON_WGSL}\n{src}");
        let fixed = |src: &str| format!("{}\n{}", ops::FIXED_WGSL, src);
        match self {
            Module::Curves => CURVES_WGSL.into(),
            Module::Point => common(POINT_WGSL),
            Module::Adjust => common(ADJUST_WGSL),
            Module::Bw => common(BW_WGSL),
            Module::Film => common(FILM_WGSL),
            Module::MdPrep => ops::MD_PREP_WGSL.into(),
            Module::BoxH => fixed(ops::BOX_H_WGSL),
            Module::BoxV => fixed(ops::BOX_V_WGSL),
            Module::Ps => ops::PS_WGSL.into(),
            Module::Sc => ops::SC_WGSL.into(),
            Module::SharpBlur => sharp::BLUR_WGSL.into(),
            Module::SharpSt => sharp::ST_WGSL.into(),
            Module::Decode => DECODE_WGSL.into(),
            Module::Encode => ENCODE_WGSL.into(),
            Module::EncodeRgba => ENCODE_RGBA_WGSL.into(),
            Module::Hist => HIST_WGSL.into(),
        }
    }
}

impl Pipe {
    const COUNT: usize = Pipe::HistOut as usize + 1;
    const ALL: [Pipe; Pipe::COUNT] = [
        Pipe::Curves, Pipe::Point, Pipe::Adjust, Pipe::Bw, Pipe::Film, Pipe::MdPrep, Pipe::BoxH, Pipe::BoxV,
        Pipe::PsMeasure, Pipe::PsMean, Pipe::PsBlur, Pipe::PsApply, Pipe::ScHist, Pipe::ScReduce, Pipe::ScCellHist,
        Pipe::ScNorm, Pipe::ScBlur, Pipe::ScClip, Pipe::ScCurve, Pipe::ScApply, Pipe::SbMeans, Pipe::SbAcross,
        Pipe::SbDown, Pipe::SbApply, Pipe::StPrep, Pipe::StFine, Pipe::StApply, Pipe::Decode, Pipe::Encode,
        Pipe::EncodeRgba, Pipe::HistIn, Pipe::HistOut,
    ];

    /// the module and entry point of each pipeline
    fn spec(self) -> (Module, &'static str) {
        use Module as M;
        match self {
            Pipe::Curves => (M::Curves, "main"),
            Pipe::Point => (M::Point, "main"),
            Pipe::Adjust => (M::Adjust, "main"),
            Pipe::Bw => (M::Bw, "main"),
            Pipe::Film => (M::Film, "main"),
            Pipe::MdPrep => (M::MdPrep, "main"),
            Pipe::BoxH => (M::BoxH, "main"),
            Pipe::BoxV => (M::BoxV, "main"),
            Pipe::PsMeasure => (M::Ps, "measure"),
            Pipe::PsMean => (M::Ps, "mean"),
            Pipe::PsBlur => (M::Ps, "blur"),
            Pipe::PsApply => (M::Ps, "apply"),
            Pipe::ScHist => (M::Sc, "hist_main"),
            Pipe::ScReduce => (M::Sc, "reduce"),
            Pipe::ScCellHist => (M::Sc, "cellhist"),
            Pipe::ScNorm => (M::Sc, "norm"),
            Pipe::ScBlur => (M::Sc, "blur"),
            Pipe::ScClip => (M::Sc, "clip"),
            Pipe::ScCurve => (M::Sc, "curve"),
            Pipe::ScApply => (M::Sc, "apply"),
            Pipe::SbMeans => (M::SharpBlur, "means"),
            Pipe::SbAcross => (M::SharpBlur, "across"),
            Pipe::SbDown => (M::SharpBlur, "down"),
            Pipe::SbApply => (M::SharpBlur, "apply"),
            Pipe::StPrep => (M::SharpSt, "prep"),
            Pipe::StFine => (M::SharpSt, "fine"),
            Pipe::StApply => (M::SharpSt, "apply"),
            Pipe::Decode => (M::Decode, "main"),
            Pipe::Encode => (M::Encode, "main"),
            Pipe::EncodeRgba => (M::EncodeRgba, "main"),
            Pipe::HistIn => (M::Hist, "count_in"),
            Pipe::HistOut => (M::Hist, "count_out"),
        }
    }
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,
    pub info: Info,
    /// by Pipe, each made the first time a pass needs it (pipe()): compiling every shader up front
    /// kept the browser's GPU process (and with it the whole window) busy for seconds at start-up
    pipes: Vec<OnceLock<wgpu::ComputePipeline>>,
    modules: Mutex<HashMap<u8, wgpu::ShaderModule>>,
    /// bound where a pass has nothing to bind (Adjustments without Mid/Detail)
    dummy: wgpu::Buffer,
    /// the histograms (HIST_BINS u32) and their read-back copy
    hist: wgpu::Buffer,
    hist_staging: wgpu::Buffer,
    /// count the histograms in every run (the preview); off for Process / tests
    stats_on: bool,
    stage: Option<Stage>,
    /// the chain as last set (kept so a new stage size can rebuild the passes)
    steps: Vec<Step>,
    ops: Vec<Op>,
    /// the steps are set but not run yet
    dirty: bool,
    /// which of the stage's images holds the chain's result (0 = no steps: the stage itself)
    result: usize,
    pub(crate) view: Option<view::View>,
    errors: Arc<Mutex<Vec<String>>>,
    /// native: a mapped upload buffer reused between uploads (faster than queue.write_buffer's
    /// fresh staging memory for 100+ MB)
    #[cfg(not(target_arch = "wasm32"))]
    upload_staging: Option<wgpu::Buffer>,
}

impl Gpu {
    /// The pipeline, compiled on first use (its module too, once for all its entry points).
    pub(crate) fn pipe(&self, p: Pipe) -> &wgpu::ComputePipeline {
        self.pipes[p as usize].get_or_init(|| {
            let (m, entry) = p.spec();
            let mut mods = self.modules.lock().unwrap();
            let module = mods.entry(m as u8).or_insert_with(|| shader_module(&self.device, &m.source(), entry));
            self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        })
    }

    /// How many pipelines warm() can compile.
    pub fn pipeline_count() -> usize {
        Pipe::COUNT
    }

    /// Compile pipeline `i` now (0 .. pipeline_count()) instead of on first use: the preview does it
    /// one at a time behind its loading screen, so nothing compiles while the app is in use.
    pub fn warm(&self, i: usize) {
        if let Some(&p) = Pipe::ALL.get(i) {
            self.pipe(p);
        }
    }

    /// Pipeline i's name for logs: "module:entry", e.g. "Sc:hist_main".
    pub fn pipeline_name(i: usize) -> String {
        Pipe::ALL.get(i).map_or(String::new(), |p| {
            let (m, e) = p.spec();
            format!("{m:?}:{e}")
        })
    }

    pub fn instance(opts: &Options) -> wgpu::Instance {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = opts.backends.unwrap_or_else(default_backends);
        wgpu::Instance::new(desc)
    }

    /// Instance + adapter (high performance, or the software one) + device.
    pub async fn new(opts: &Options) -> Result<Gpu, String> {
        let instance = Self::instance(opts);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: opts.fallback,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| format!("no GPU adapter: {e}"))?;
        Self::with_adapter(adapter).await
    }

    pub async fn with_adapter(adapter: wgpu::Adapter) -> Result<Gpu, String> {
        let ai = adapter.get_info();
        // Exactly the adapter's limits: WebGPU's defaults (8192 px textures, 128 MiB storage
        // bindings) are too small for big photos.
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fae-gpu"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| format!("GPU device: {e}"))?;
        let errors: Arc<Mutex<Vec<String>>> = Default::default();
        {
            let errors = errors.clone();
            device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
                if let Ok(mut v) = errors.lock() {
                    v.push(e.to_string());
                }
            }));
        }
        let dl = device.limits();
        let info = Info {
            name: ai.name.clone(),
            backend: backend_name(ai.backend).into(),
            device_type: format!("{:?}", ai.device_type),
            driver: ai.driver.clone(),
            driver_info: ai.driver_info.clone(),
            max_texture_2d: dl.max_texture_dimension_2d,
            max_buffer: dl.max_buffer_size,
            max_storage_binding: dl.max_storage_buffer_binding_size as u64,
        };
        let pipes = (0..Pipe::COUNT).map(|_| OnceLock::new()).collect();
        let dummy = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dummy"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let hist = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist"),
            size: (HIST_BINS * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let hist_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist_staging"),
            size: (HIST_BINS * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Gpu {
            device,
            queue,
            adapter,
            info,
            pipes,
            modules: Mutex::new(HashMap::new()),
            dummy,
            hist,
            hist_staging,
            stats_on: false,
            stage: None,
            steps: vec![],
            ops: vec![],
            dirty: false,
            result: 0,
            view: None,
            errors,
            #[cfg(not(target_arch = "wasm32"))]
            upload_staging: None,
        })
    }

    /// Drop the stage and the passes (their GPU memory: for a 50 MP image well over 1 GB); the
    /// steps are kept, the next upload sets everything up again. Process calls it after a batch.
    pub fn release(&mut self) {
        self.stage = None;
        self.ops.clear();
        self.result = 0;
        self.dirty = false;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.upload_staging = None;
        }
    }

    /// Validation / out-of-memory errors reported since the last call.
    pub fn take_errors(&self) -> Vec<String> {
        self.errors.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }

    pub fn size(&self) -> Option<(u32, u32)> {
        self.stage.as_ref().map(|s| (s.w, s.h))
    }

    pub(crate) fn stage_views(&self) -> Option<(&wgpu::TextureView, &wgpu::TextureView)> {
        self.stage.as_ref().map(|s| (&s.views[self.result], &s.views[0]))
    }

    // ---- the stage --------------------------------------------------------------------------

    fn alloc(&mut self, w: u32, h: u32, bpp: u32) -> Result<(), String> {
        if let Some(s) = &self.stage {
            if s.w == w && s.h == h && s.bpp == bpp {
                return Ok(());
            }
        }
        self.stage = None;
        self.ops.clear();
        let max = self.info.max_texture_2d;
        if w == 0 || h == 0 || w > max || h > max {
            return Err(format!("image {w}x{h}: the GPU allows up to {max} px per side"));
        }
        let n = w as u64 * h as u64;
        let in_bytes = (n * bpp as u64).div_ceil(4) * 4;
        let limit = self.info.max_storage_binding.min(self.info.max_buffer);
        if in_bytes > limit {
            return Err(format!("image {w}x{h}: {in_bytes} bytes exceeds the GPU's buffer limit {limit}"));
        }
        let words = (n * 3).div_ceil(4);
        let d = &self.device;
        use wgpu::BufferUsages as U;
        let buf = |label, size, usage| {
            d.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
        };
        let packed_in = buf("packed_in", in_bytes, U::STORAGE | U::COPY_DST);
        let out = buf("out", words * 4, U::STORAGE | U::COPY_SRC);
        let staging = buf("staging", words * 4, U::MAP_READ | U::COPY_DST);
        let tex = |label| {
            d.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: IMG_FORMAT,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let tex = [tex("orig"), tex("img_a"), tex("img_b")];
        let views = [0, 1, 2].map(|i| tex[i].create_view(&Default::default()));
        let quads = n.div_ceil(4) as u32;
        let groups = quads.div_ceil(256);
        let gx = groups.clamp(1, 32768);
        let gy = groups.div_ceil(gx).max(1);
        let dims_dec = buf("dims_dec", 16, U::UNIFORM | U::COPY_DST);
        let dims_enc = buf("dims_enc", 16, U::UNIFORM | U::COPY_DST);
        self.queue.write_buffer(&dims_dec, 0, &dims(w, h, bpp));
        self.queue.write_buffer(&dims_enc, 0, &dims(w, h, gx * 256));
        let bg_decode = bind_group(
            d,
            self.pipe(Pipe::Decode),
            &[packed_in.as_entire_binding(), wgpu::BindingResource::TextureView(&views[0]), dims_dec.as_entire_binding()],
        );
        let hb = |p: &wgpu::ComputePipeline, t: usize| {
            bind_group(d, p, &[wgpu::BindingResource::TextureView(&views[t]), self.hist.as_entire_binding()])
        };
        let hb_in = [0, 1, 2].map(|t| hb(self.pipe(Pipe::HistIn), t));
        let hb_out = [0, 1, 2].map(|t| hb(self.pipe(Pipe::HistOut), t));
        let hist_groups = (n.div_ceil(16384) as u32).clamp(1, 1024);
        self.stage = Some(Stage {
            w,
            h,
            bpp,
            packed_in,
            tex,
            views,
            dims_dec,
            dims_enc,
            bg_decode,
            hb_in,
            hb_out,
            hist_groups,
            rgba: None,
            scratch: None,
            splanes: None,
            words,
            out,
            staging,
            enc_groups: (gx, gy),
        });
        Ok(())
    }

    /// Upload the stage: 8-bit pixels, `bpp` = 3 (RGB) or 4 (RGBA, alpha ignored), w*h*bpp bytes.
    /// Becomes `orig`; the steps run again on the next `run()`. Submits; doesn't wait.
    pub fn upload(&mut self, px: &[u8], w: u32, h: u32, bpp: u32) -> Result<(), String> {
        if bpp != 3 && bpp != 4 {
            return Err(format!("upload: {bpp} bytes per pixel (3 or 4)"));
        }
        if px.len() as u64 != w as u64 * h as u64 * bpp as u64 {
            return Err(format!("upload: {} bytes for {w}x{h}x{bpp}", px.len()));
        }
        self.alloc(w, h, bpp)?;
        self.write_input(px)?;
        self.dirty = true;
        let s = self.stage.as_ref().unwrap();
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(self.pipe(Pipe::Decode));
            pass.set_bind_group(0, &s.bg_decode, &[]);
            pass.dispatch_workgroups(w.div_ceil(16), h.div_ceil(16), 1);
        }
        self.queue.submit([enc.finish()]);
        // a new size dropped the passes (their bind groups pointed at the old images): set them up
        // again from the last steps; either way the chain has to run again
        if self.ops.len() != self.steps.len() {
            let steps = std::mem::take(&mut self.steps);
            self.set_steps(&steps)?;
        }
        self.dirty = true;
        self.result = self.ops.last().map(|o| o.out).unwrap_or(0);
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    fn write_input(&mut self, px: &[u8]) -> Result<(), String> {
        let s = self.stage.as_ref().unwrap();
        let whole = px.len() / 4 * 4;
        self.queue.write_buffer(&s.packed_in, 0, &px[..whole]);
        if whole < px.len() {
            let mut tail = [0u8; 4];
            tail[..px.len() - whole].copy_from_slice(&px[whole..]);
            self.queue.write_buffer(&s.packed_in, whole as u64, &tail);
        }
        Ok(())
    }

    /// Native: copy into a persistent mapped buffer, then GPU-copy it into the input buffer.
    #[cfg(not(target_arch = "wasm32"))]
    fn write_input(&mut self, px: &[u8]) -> Result<(), String> {
        let size = (px.len() as u64).div_ceil(4) * 4;
        let reuse = matches!(&self.upload_staging, Some(b) if b.size() >= size);
        if !reuse {
            self.upload_staging = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("upload_staging"),
                size,
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            }));
        } else {
            // map it again (it was unmapped for the last copy) and wait
            let b = self.upload_staging.as_ref().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            b.slice(..size).map_async(wgpu::MapMode::Write, move |r| {
                let _ = tx.send(r);
            });
            self.wait()?;
            rx.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        }
        let b = self.upload_staging.as_ref().unwrap();
        {
            let mut m = b.slice(..size).get_mapped_range_mut().map_err(|e| e.to_string())?;
            m.slice(..px.len()).copy_from_slice(px);
            let tail = size as usize - px.len();
            m.slice(px.len()..).copy_from_slice(&[0u8; 4][..tail]);
        }
        b.unmap();
        let s = self.stage.as_ref().unwrap();
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(b, 0, &s.packed_in, 0, size);
        self.queue.submit([enc.finish()]);
        Ok(())
    }

    // ---- the pass graph ---------------------------------------------------------------------

    /// The chain to run on the stage, in order. Same kinds of steps as last time (a slider moved):
    /// only the parameters are rewritten; otherwise the passes are set up again.
    pub fn set_steps(&mut self, steps: &[Step]) -> Result<(), String> {
        self.steps = steps.to_vec();
        if self.stage.is_none() {
            return Ok(()); // applied at the upload
        }
        // same kinds and table sizes: only new values (a slider moved)
        let same = self.ops.len() == steps.len()
            && self.ops.iter().zip(steps).all(|(o, st)| o.kind == st.kind && o.aux == st.aux && o.len == st.data.len());
        if same {
            for (o, st) in self.ops.iter().zip(steps) {
                let data = self.params_of(st);
                self.queue.write_buffer(&o.params, 0, &data);
            }
        } else {
            let mut ops = Vec::with_capacity(steps.len());
            let mut cur = 0usize;
            for st in steps {
                let out = if cur == 1 { 2 } else { 1 };
                ops.push(self.build_op(st, cur, out));
                cur = out;
            }
            self.ops = ops;
        }
        self.result = self.ops.last().map(|o| o.out).unwrap_or(0);
        self.dirty = true;
        Ok(())
    }

    /// The step's parameter bytes with what depends on the stage filled in.
    fn params_of(&self, st: &Step) -> Vec<u8> {
        let mut d = st.data.clone();
        let (w, h) = self.stage.as_ref().map_or((1, 1), |s| (s.w, s.h));
        let mut put = |i: usize, v: f32| d[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        if st.kind == Kind::Adjust {
            put(33, w as f32);
        }
        if st.kind == Kind::Sc {
            put(6, w as f32);
            put(7, h as f32);
            put(8, ops::dc_rows(w, h) as f32);
        }
        if st.aux == Aux::Ps {
            let (gh, gw) = ops::ps_grid(w, h);
            put(9, gh as f32);
            put(10, gw as f32);
            put(11, w as f32);
            put(12, h as f32);
        }
        if let Aux::SharpBlur { k, .. } = st.aux {
            put(6, w.div_ceil(k) as f32);
            put(7, h.div_ceil(k) as f32);
            put(8, w as f32);
            put(9, h as f32);
        }
        if st.aux == Aux::SharpSt {
            put(0, sharp::st_coarse(h).1 as f32);
            put(1, sharp::st_mid(h).1 as f32);
        }
        d
    }

    fn build_op(&mut self, st: &Step, inp: usize, out: usize) -> Op {
        let data = self.params_of(st);
        let params = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("step params"),
            size: data.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&params, 0, &data);
        let mut passes = vec![];
        let mut keep = vec![];
        let mut clears = vec![];
        // whole-image passes first
        let extra: Option<wgpu::Buffer> = match st.aux {
            Aux::Md => {
                let (p, k, res) = self.md_passes(inp);
                passes.extend(p);
                keep.extend(k);
                Some(res)
            }
            Aux::Ps => {
                let (p, k, c) = self.ps_passes(inp, out, &params);
                passes.extend(p);
                keep.extend(k);
                clears.extend(c);
                None
            }
            Aux::Sc { dyn_on } => {
                let (p, k, c) = self.sc_passes(inp, out, &params, dyn_on);
                passes.extend(p);
                keep.extend(k);
                clears.extend(c);
                None
            }
            Aux::SharpBlur { k, .. } => {
                passes.extend(self.sharp_blur_passes(inp, out, &params, k));
                None
            }
            Aux::SharpSt => {
                let (p, k) = self.sharp_st_passes(inp, out, &params);
                passes.extend(p);
                keep.extend(k);
                None
            }
            Aux::None => None,
        };
        if matches!(st.aux, Aux::Ps | Aux::Sc { .. } | Aux::SharpBlur { .. } | Aux::SharpSt) {
            // ps_passes made the whole step, its apply pass included
            return Op { kind: st.kind, aux: st.aux.clone(), len: st.data.len(), params, passes, clears, keep, inp, out };
        }
        let s = self.stage.as_ref().unwrap();
        let pipe = match st.kind {
            Kind::Curves => Pipe::Curves,
            Kind::Point => Pipe::Point,
            Kind::Adjust => Pipe::Adjust,
            Kind::Bw => Pipe::Bw,
            Kind::Film => Pipe::Film,
            Kind::Sc | Kind::Sharp => unreachable!("their passes make the whole step"),
        };
        let mut res = vec![
            wgpu::BindingResource::TextureView(&s.views[inp]),
            wgpu::BindingResource::TextureView(&s.views[out]),
            params.as_entire_binding(),
        ];
        if st.kind == Kind::Adjust {
            res.push(extra.as_ref().unwrap_or(&self.dummy).as_entire_binding());
        }
        let bg = bind_group(&self.device, self.pipe(pipe), &res);
        passes.push(Pass { pipe, bg, groups: (s.w.div_ceil(16), s.h.div_ceil(16), 1) });
        Op { kind: st.kind, aux: st.aux.clone(), len: st.data.len(), params, passes, clears, keep, inp, out }
    }

    /// Count the UI's histograms in every run from now on (the preview).
    pub fn set_stats(&mut self, on: bool) {
        if on != self.stats_on {
            self.stats_on = on;
            self.dirty = true;
        }
    }

    /// Run the chain if its steps or the stage changed since the last run. Submits; doesn't wait.
    pub fn run(&mut self) -> Result<(), String> {
        self.run_times(1)
    }

    /// The chain `times` times in one submit (benchmarks).
    pub fn run_times(&mut self, times: u32) -> Result<(), String> {
        let s = self.stage.as_ref().ok_or("run: no image uploaded")?;
        if !self.dirty && times <= 1 {
            return Ok(());
        }
        let stats = self.stats_on;
        // hin = the input of the first Curves step; without one the image before Sharpen (the
        // output when there is no Sharpen either), as the CPU core counts it
        let first_curves = self.ops.iter().position(|o| o.kind == Kind::Curves)
            .or_else(|| self.ops.iter().position(|o| o.kind == Kind::Sharp));
        let mut enc = self.device.create_command_encoder(&Default::default());
        if stats {
            enc.clear_buffer(&self.hist, 0, None);
        }
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            for t in 0..times.max(1) {
                if self.ops.iter().any(|o| !o.clears.is_empty()) {
                    // atomic counters of the whole-image passes start from zero every run: a pass
                    // of its own can't clear buffers, so end this one and clear between passes
                    drop(pass);
                    for o in &self.ops {
                        for b in &o.clears {
                            enc.clear_buffer(b, 0, None);
                        }
                    }
                    pass = enc.begin_compute_pass(&Default::default());
                }
                for (i, o) in self.ops.iter().enumerate() {
                    if stats && t == 0 && first_curves == Some(i) {
                        pass.set_pipeline(self.pipe(Pipe::HistIn));
                        pass.set_bind_group(0, &s.hb_in[o.inp], &[]);
                        pass.dispatch_workgroups(s.hist_groups, 1, 1);
                    }
                    for p in &o.passes {
                        pass.set_pipeline(self.pipe(p.pipe));
                        pass.set_bind_group(0, &p.bg, &[]);
                        pass.dispatch_workgroups(p.groups.0, p.groups.1, p.groups.2);
                    }
                }
            }
            if stats {
                pass.set_pipeline(self.pipe(Pipe::HistOut));
                pass.set_bind_group(0, &s.hb_out[self.result], &[]);
                pass.dispatch_workgroups(s.hist_groups, 1, 1);
                if first_curves.is_none() {
                    pass.set_pipeline(self.pipe(Pipe::HistIn));
                    pass.set_bind_group(0, &s.hb_in[self.result], &[]);
                    pass.dispatch_workgroups(s.hist_groups, 1, 1);
                }
            }
        }
        self.queue.submit([enc.finish()]);
        self.dirty = false;
        Ok(())
    }

    // ---- read back --------------------------------------------------------------------------

    /// Encode the result to packed RGB, copy it to the staging buffer, map it. The returned
    /// receiver fires once the bytes can be read (native: after `wait()`; web: by itself).
    pub fn readback_start(&mut self) -> Result<Readback, String> {
        self.run()?;
        let s = self.stage.as_ref().ok_or("readback: no image")?;
        let bg = bind_group(
            &self.device,
            self.pipe(Pipe::Encode),
            &[
                wgpu::BindingResource::TextureView(&s.views[self.result]),
                s.out.as_entire_binding(),
                s.dims_enc.as_entire_binding(),
            ],
        );
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(self.pipe(Pipe::Encode));
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(s.enc_groups.0, s.enc_groups.1, 1);
        }
        enc.copy_buffer_to_buffer(&s.out, 0, &s.staging, 0, s.words * 4);
        self.queue.submit([enc.finish()]);
        Ok(Readback::start(&s.staging, s.w as usize * s.h as usize * 3))
    }

    /// The result as 8-bit RGBA (alpha 255): w*h*4 bytes. Like `readback_start`.
    pub fn readback_rgba_start(&mut self) -> Result<Readback, String> {
        self.run()?;
        let d = self.device.clone();
        let s = self.stage.as_mut().ok_or("readback: no image")?;
        let n = s.w as u64 * s.h as u64;
        if s.rgba.is_none() {
            let buf = |label, usage| d.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: n * 4, usage, mapped_at_creation: false });
            s.rgba = Some((
                buf("out_rgba", wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC),
                buf("staging_rgba", wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST),
            ));
        }
        let s = self.stage.as_ref().unwrap();
        let (out, staging) = s.rgba.as_ref().unwrap();
        let groups = (n as u32).div_ceil(256);
        let gx = groups.clamp(1, 32768);
        let gy = groups.div_ceil(gx).max(1);
        // dims_enc's stride is gx_enc * 256 for 4-pixel groups; this pass needs its own
        let uni = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dims_rgba"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&uni, 0, &dims(s.w, s.h, gx * 256));
        let bg = bind_group(
            &d,
            self.pipe(Pipe::EncodeRgba),
            &[wgpu::BindingResource::TextureView(&s.views[self.result]), out.as_entire_binding(), uni.as_entire_binding()],
        );
        let mut enc = d.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(self.pipe(Pipe::EncodeRgba));
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        enc.copy_buffer_to_buffer(out, 0, staging, 0, n * 4);
        self.queue.submit([enc.finish()]);
        Ok(Readback::start(staging, (n * 4) as usize))
    }

    /// The histograms of the last run (`set_stats(true)` first): HIST_BINS u32, little-endian bytes.
    pub fn stats_start(&mut self) -> Result<Readback, String> {
        if !self.stats_on {
            return Err("stats: set_stats(true) first".into());
        }
        self.run()?;
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&self.hist, 0, &self.hist_staging, 0, (HIST_BINS * 4) as u64);
        self.queue.submit([enc.finish()]);
        Ok(Readback::start(&self.hist_staging, HIST_BINS * 4))
    }

    /// A future that resolves when everything submitted so far has run.
    pub fn done(&self) -> impl std::future::Future<Output = ()> {
        let (tx, rx) = futures_channel::oneshot::channel::<()>();
        self.queue.on_submitted_work_done(move || {
            let _ = tx.send(());
        });
        async move {
            let _ = rx.await;
        }
    }

    /// Native: block until the GPU is idle (callbacks run). No-op on the web.
    pub fn wait(&self) -> Result<(), String> {
        self.device.poll(wgpu::PollType::wait_indefinitely()).map(|_| ()).map_err(|e| e.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
fn default_backends() -> wgpu::Backends {
    wgpu::Backends::BROWSER_WEBGPU
}

#[cfg(not(target_arch = "wasm32"))]
fn default_backends() -> wgpu::Backends {
    if let Some(b) = std::env::var("FAE_GPU_BACKEND").ok().and_then(|s| parse_backends(&s)) {
        return b;
    }
    if cfg!(windows) {
        wgpu::Backends::DX12
    } else {
        wgpu::Backends::all()
    }
}

pub(crate) fn compute_pipeline(d: &wgpu::Device, src: &str, label: &str) -> wgpu::ComputePipeline {
    compute_pipeline_at(d, src, label, "main")
}

fn shader_module(d: &wgpu::Device, src: &str, label: &str) -> wgpu::ShaderModule {
    #[cfg(feature = "emulate")]
    let src = &emulate::wgsl(src);
    d.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(src.into()) })
}

pub(crate) fn compute_pipeline_at(d: &wgpu::Device, src: &str, label: &str, entry: &str) -> wgpu::ComputePipeline {
    let m = shader_module(d, src, label);
    d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: None,
        module: &m,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

/// A bind group with explicit binding numbers (entry points of one module use different subsets).
pub(crate) fn bind_group_at(d: &wgpu::Device, p: &wgpu::ComputePipeline, res: &[(u32, wgpu::BindingResource)]) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> =
        res.iter().map(|(i, r)| wgpu::BindGroupEntry { binding: *i, resource: r.clone() }).collect();
    d.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &p.get_bind_group_layout(0), entries: &entries })
}

pub(crate) fn bind_group(d: &wgpu::Device, p: &wgpu::ComputePipeline, res: &[wgpu::BindingResource]) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> =
        res.iter().enumerate().map(|(i, r)| wgpu::BindGroupEntry { binding: i as u32, resource: r.clone() }).collect();
    d.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &p.get_bind_group_layout(0), entries: &entries })
}

pub struct Readback {
    buf: wgpu::Buffer,
    len: usize,
    rx: futures_channel::oneshot::Receiver<Result<(), String>>,
}

impl Readback {
    /// Map `buf` (submitted copies into it run first).
    fn start(buf: &wgpu::Buffer, len: usize) -> Readback {
        let (tx, rx) = futures_channel::oneshot::channel();
        buf.slice(..).map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res.map_err(|e| e.to_string()));
        });
        Readback { buf: buf.clone(), len, rx }
    }

    /// The packed RGB bytes (w*h*3). Native: call `Gpu::wait()` first (or `finish_blocking`).
    pub async fn finish(self) -> Result<Vec<u8>, String> {
        self.rx.await.map_err(|_| "readback cancelled".to_string())??;
        let out = {
            let view = self.buf.slice(..).get_mapped_range().map_err(|e| e.to_string())?;
            view[..self.len].to_vec()
        };
        self.buf.unmap();
        Ok(out)
    }

    /// Native helper: wait for the GPU and return the bytes.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn finish_blocking(self, gpu: &Gpu) -> Result<Vec<u8>, String> {
        gpu.wait()?;
        pollster::block_on(self.finish())
    }
}

#[cfg(test)]
mod pipe_tests {
    use super::Pipe;

    #[test]
    fn all_lists_every_pipe_in_order() {
        for (i, p) in Pipe::ALL.iter().enumerate() {
            assert_eq!(*p as usize, i, "{p:?}");
        }
    }
}
