//! The edit after the stage (wave B5), as fastedit/core.py did it: the GPU core (fae-gpu, linked as a
//! crate) when it has a device and the image fits, else the CPU core (fastedit-core); the same result
//! within the GPU core's tolerance rule (tests/gpu/tolerance.py).
//!
//! - The edit is the UI's JSON (`jobFor` in ui/js/process.js): `steps` = the chain in panel order (each a
//!   dict with `t` = the tool and that tool's entries), or without it the flat edit in [`DEFAULT_ORDER`];
//!   `sharp` = Sharpen, run last. [`parse`] turns it into the cores' steps (core.py `_step` / `_steps`).
//! - One GPU device for the back end, made on first use and shared by Process and the encoded compare
//!   (a lock); after a device error it is not used again in this session (a lost device or a driver
//!   problem would fail every image); a step it can't build (a filter this build lacks) only sends that
//!   image to the CPU core. Off from the start with `--no-gpu` / FAE_NO_GPU (the shell says so).
//! - The CPU core has none of the filter kit's filters (`fx`): it leaves them out and reports their ids.

use crate::fit::{as_float, as_int, truthy};
use crate::{Error, ErrorKind};
use fe::chain::StepBuf;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// The tool pill order: the chain's order when the edit has no explicit `steps` list. Crop, Resize and
/// Color balance run before (the stage), Sharpen after.
pub const DEFAULT_ORDER: [&str; 6] = ["point", "adjust", "curves", "bw", "film", "sc"];

fn bad(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::BadArgs, msg)
}

/// One step of the chain: a step both cores run, or a filter of the filter kit (GPU core only).
pub enum Step {
    Core(StepBuf),
    Fx { id: String, p: Vec<f64>, op: f64 },
}

/// Sharpen's sliders (`sharp`: as, bs, st, lc, fo).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sharpen {
    pub adaptive: f64,
    pub balance: f64,
    pub structure: f64,
    pub local_contrast: f64,
    pub focus: f64,
}

impl Sharpen {
    /// core.py `_gpu_steps`: Sharpen runs when Adaptive > 0 or Structure / Local Contrast / Focus ≠ 0.
    pub fn active(&self) -> bool {
        self.adaptive > 0.0 || self.structure != 0.0 || self.local_contrast != 0.0 || self.focus != 0.0
    }
}

/// The edit, parsed: the chain (no-op steps left out) and Sharpen.
pub struct Edit {
    pub steps: Vec<Step>,
    pub sharp: Sharpen,
}

impl Edit {
    /// core.py `fx_used`: the ids of the kit filters the edit uses, each once, in order.
    pub fn fx_used(&self) -> Vec<String> {
        let mut ids: Vec<String> = vec![];
        for s in &self.steps {
            if let Step::Fx { id, .. } = s {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
        ids
    }
}

// ---- the edit's JSON → steps (core.py `_step`) ----------------------------------------------------

fn get<'a>(o: &'a Value, k: &str) -> Option<&'a Value> {
    o.get(k).filter(|v| !v.is_null())
}

/// float(x): numbers, bools, numeric strings.
fn num(v: &Value, what: &str) -> Result<f64, Error> {
    as_float(v).ok_or_else(|| bad(format!("{what}: not a number: {v}")))
}

/// float(o.get(k, default)) (null counts as missing).
fn num_or(o: &Value, k: &str, d: f64) -> Result<f64, Error> {
    get(o, k).map_or(Ok(d), |v| num(v, k))
}

/// float(o.get(k) or 0)
fn num_or0(o: &Value, k: &str) -> Result<f64, Error> {
    match get(o, k) {
        Some(v) if truthy(v) => num(v, k),
        _ => Ok(0.0),
    }
}

/// A float64 array (nested lists flattened, as NumPy's reshape takes them).
fn floats(v: &Value, what: &str) -> Result<Vec<f64>, Error> {
    fn go(v: &Value, out: &mut Vec<f64>, what: &str) -> Result<(), Error> {
        match v {
            Value::Array(a) => a.iter().try_for_each(|x| go(x, out, what)),
            x => {
                out.push(num(x, what)?);
                Ok(())
            }
        }
    }
    let mut out = vec![];
    go(v, &mut out, what)?;
    Ok(out)
}

fn opt_floats(o: &Value, k: &str) -> Result<Option<Vec<f64>>, Error> {
    get(o, k).map(|v| floats(v, k)).transpose()
}

fn three(v: Vec<f64>, what: &str) -> Result<[f64; 3], Error> {
    <[f64; 3]>::try_from(v.as_slice()).map_err(|_| bad(format!("{what}: {} values, need 3", v.len())))
}

/// One step from a dict with the flat edit's entries and `t` = the kind (`kind` overrides `t`: the
/// flat edit). None when the step does nothing.
fn step(e: &Value, kind: &str) -> Result<Option<Step>, Error> {
    let core = |s: StepBuf| Ok(Some(Step::Core(s)));
    match kind {
        "point" => {
            if !get(e, "point").is_some_and(truthy) {
                return Ok(None);
            }
            let gains = opt_floats(e, "gains")?.map(|g| three(g, "gains")).transpose()?;
            core(StepBuf::Point { k: num_or(e, "k", 1.0)?, o: num_or(e, "o", 0.0)?, gains })
        }
        "adjust" => {
            let md = num_or0(e, "md")?;
            let wb = match opt_floats(e, "wb")? {
                None => None,
                Some(v) if v.len() == 9 => Some([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]]),
                Some(v) => return Err(bad(format!("wb: {} values, need 3 x 3", v.len()))),
            };
            let con = opt_floats(e, "con")?;
            let boost = opt_floats(e, "boost")?;
            let sat = get(e, "sat").map(|v| num(v, "sat")).transpose()?;
            let mut tones = vec![];
            if let Some(Value::Array(ts)) = get(e, "tones") {
                for t in ts {
                    let table = floats(get(t, "T").ok_or_else(|| bad("tones: no T"))?, "tones")?;
                    tones.push((table, get(t, "ratio").is_some_and(truthy)));
                }
            }
            if md == 0.0 && wb.is_none() && con.is_none() && tones.is_empty() && boost.is_none() && sat.is_none() {
                return Ok(None);
            }
            core(StepBuf::Adjust { md, wb, con, tones, boost, sat })
        }
        "curves" => {
            let Some(Value::Array(ls)) = get(e, "luts") else { return Ok(None) };
            if ls.len() != 3 {
                return Ok(None);
            }
            let mut luts = [[0u8; 256]; 3];
            for (c, l) in ls.iter().enumerate() {
                let v = floats(l, "luts")?;
                if v.len() != 256 {
                    return Err(bad(format!("luts: {} values, need 256", v.len())));
                }
                for (i, x) in v.iter().enumerate() {
                    luts[c][i] = x.clamp(0.0, 255.0) as u8;
                }
            }
            if luts.iter().all(|l| l.iter().enumerate().all(|(i, &x)| x as usize == i)) {
                return Ok(None); // identity: nothing to do
            }
            core(StepBuf::Curves(luts))
        }
        "bw" => {
            let Some(bw) = get(e, "bw").filter(|v| truthy(v)) else { return Ok(None) };
            let w = match get(bw, "w").filter(|v| truthy(v)) {
                Some(v) => Some(three(floats(v, "bw w")?, "bw w")?),
                None => None,
            };
            let ps = num_or0(bw, "ps")?;
            let psk = if ps != 0.0 { floats(get(bw, "psk").ok_or_else(|| bad("bw: ps without psk"))?, "psk")? } else { vec![] };
            if ps != 0.0 && psk.is_empty() {
                return Err(bad("bw: ps without psk"));
            }
            let s = num(get(bw, "s").ok_or_else(|| bad("bw: no s"))?, "bw s")?;
            let op = num(get(bw, "op").ok_or_else(|| bad("bw: no op"))?, "bw op")?;
            core(StepBuf::Bw { s, op, w, ps, psk })
        }
        "film" => {
            let Some(f) = get(e, "film").filter(|v| truthy(v)) else { return Ok(None) };
            let n = get(f, "n").and_then(as_int).ok_or_else(|| bad("film: no n"))?;
            let tab = |k: &str| -> Result<Option<Vec<f64>>, Error> {
                get(f, k).filter(|v| truthy(v)).map(|v| floats(v, k)).transpose()
            };
            core(StepBuf::Film { n: n.max(0) as usize, d: tab("d")?, t1: tab("t1")?, op: num_or(f, "op", 1.0)? })
        }
        "fx" => {
            let Some(f) = get(e, "fx").filter(|v| truthy(v)) else { return Ok(None) };
            let id = match get(f, "id") {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => return Err(bad("fx: no id")),
            };
            let p = opt_floats(f, "p")?.unwrap_or_default();
            Ok(Some(Step::Fx { id, p, op: num_or(f, "op", 1.0)? }))
        }
        "sc" => {
            let Some(sc) = get(e, "sc").filter(|v| truthy(v)) else { return Ok(None) };
            let dyn_ = num_or0(sc, "dyn")?;
            let dk = if dyn_ > 0.0 { floats(get(sc, "dk").ok_or_else(|| bad("sc: no dk"))?, "dk")? } else { vec![] };
            let shl = get(sc, "shl").filter(|v| truthy(v)).map(|v| floats(v, "shl")).transpose()?;
            core(StepBuf::Sc { cast: num_or(sc, "cast", 0.0)?, cc: num_or(sc, "cc", 0.0)?, dyn_, op: num_or(sc, "op", 1.0)?, dk, shl })
        }
        other => Err(bad(format!("unknown step {other:?}"))),
    }
}

/// core.py `_steps` + the Sharpen sliders: the edit (null = no edit) as the chain the cores run.
pub fn parse(edit: &Value) -> Result<Edit, Error> {
    let mut steps = vec![];
    match get(edit, "steps") {
        Some(Value::Array(specs)) => {
            for sp in specs {
                let t = match get(sp, "t") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => "null".into(),
                };
                steps.extend(step(sp, &t)?);
            }
        }
        Some(v) => return Err(bad(format!("steps: not a list: {v}"))),
        None => {
            for k in DEFAULT_ORDER {
                steps.extend(step(edit, k)?);
            }
        }
    }
    let sp = get(edit, "sharp").cloned().unwrap_or(Value::Null);
    let sharp = Sharpen {
        adaptive: num_or0(&sp, "as")?,
        balance: num_or0(&sp, "bs")?,
        structure: num_or0(&sp, "st")?,
        local_contrast: num_or0(&sp, "lc")?,
        focus: num_or0(&sp, "fo")?,
    };
    Ok(Edit { steps, sharp })
}

// ---- the CPU core -------------------------------------------------------------------------------

/// The edit on the CPU core (packed RGB in and out); the kit filters are left out.
pub fn render_cpu(rgb: &[u8], w: usize, edit: &Edit) -> Result<Vec<u8>, Error> {
    let st: Vec<fe::chain::Step<'_>> = edit.steps.iter().filter_map(|s| if let Step::Core(b) = s { Some(b.step()) } else { None }).collect();
    for s in &st {
        s.validate().map_err(bad)?;
    }
    let s = &edit.sharp;
    let sharp = fe::sharp::Sharp {
        adaptive: s.adaptive,
        balance: s.balance,
        structure: s.structure,
        local_contrast: s.local_contrast,
        focus: s.focus,
        scale: 1.0,
        href: 0,
    };
    let mut out = vec![0u8; rgb.len()];
    fe::chain::run(rgb, &mut out, w, 3, &st, &sharp, false);
    Ok(out)
}

// ---- the GPU core -------------------------------------------------------------------------------

/// The edit as the GPU core's steps (core.py `_gpu_steps`): the chain with the kit filters, then
/// Sharpen at scale 1 (Process). Err: a step this build can't make (not a device problem).
pub fn gpu_steps(edit: &Edit) -> Result<Vec<gpu::Step>, String> {
    let mut out = vec![];
    for s in &edit.steps {
        out.push(match s {
            Step::Fx { id, p, op } => gpu::Step::fx(id, p, *op)?,
            Step::Core(b) => match b {
                StepBuf::Point { k, o, gains } => gpu::Step::point(*k, *o, *gains),
                StepBuf::Adjust { md, wb, con, tones, boost, sat } => {
                    let tr: Vec<(&[f64], bool)> = tones.iter().map(|(t, r)| (t.as_slice(), *r)).collect();
                    gpu::Step::adjust(*md, *wb, con.as_deref(), &tr, boost.as_deref(), *sat)?
                }
                StepBuf::Curves(l) => gpu::Step::curves(&l.concat())?,
                StepBuf::Bw { s, op, w, ps, psk } => {
                    if *ps != 0.0 {
                        gpu::Step::bw_ps(*s, *op, *w, *ps, psk)?
                    } else {
                        gpu::Step::bw(*s, *op, *w)
                    }
                }
                StepBuf::Film { n, d, t1, op } => gpu::Step::film(*n, d.as_deref(), t1.as_deref(), *op)?,
                StepBuf::Sc { cast, cc, dyn_, op, dk, shl } => {
                    let dk = if dk.is_empty() { None } else { Some(dk.as_slice()) };
                    gpu::Step::sc(*cast, *cc, *dyn_, *op, dk, shl.as_deref())?
                }
            },
        });
    }
    let s = &edit.sharp;
    if s.active() {
        out.extend(gpu::Step::sharp(s.adaptive, s.balance, s.structure, s.local_contrast, s.focus, 1.0));
    }
    Ok(out)
}

struct GpuState {
    dev: Option<gpu::Gpu>,
    info: Option<gpu::Info>,
    /// why the GPU core isn't used (turned off, no device, failed earlier); None: usable
    why: Option<String>,
}

impl GpuState {
    /// The device, made on first use; None when it can't be (`why` says why).
    fn device(&mut self) -> Option<&mut gpu::Gpu> {
        if self.why.is_some() {
            return None;
        }
        if self.dev.is_none() {
            match pollster::block_on(gpu::Gpu::new(&gpu::Options::default())) {
                Ok(g) => {
                    self.info = Some(g.info.clone());
                    self.dev = Some(g);
                }
                Err(e) => self.why = Some(format!("no GPU device: {e}")),
            }
        }
        self.dev.as_mut()
    }

    fn fail(&mut self, e: String) {
        self.dev = None;
        self.why = Some(format!("failed: {e}"));
    }
}

/// The GPU state's lock; a panic while it was held (see render_gpu) doesn't leave it unusable.
fn lock(m: &Mutex<GpuState>) -> std::sync::MutexGuard<'_, GpuState> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which core renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// the GPU core when it can, else the CPU core
    Auto,
    Cpu,
    /// only the GPU core (an error when it can't)
    Gpu,
}

/// Both cores of one back end.
pub struct Cores {
    gpu: Arc<Mutex<GpuState>>,
}

impl Cores {
    /// `no_gpu`: the GPU core is off for the session (--no-gpu / FAE_NO_GPU).
    pub fn new(no_gpu: bool) -> Cores {
        let why = no_gpu.then(|| "turned off (--no-gpu)".to_string());
        Cores { gpu: Arc::new(Mutex::new(GpuState { dev: None, info: None, why })) }
    }

    /// core.py `gpu_info`: {"ok": true, "version", "name", "backend", "max_pixels", …} or {"ok": false,
    /// "error"} (turned off, no device, failed earlier). Makes the device when there is none yet.
    pub fn gpu_info(&self) -> Value {
        let mut g = lock(&self.gpu);
        if g.device().is_none() {
            return json!({"ok": false, "error": g.why.clone().unwrap_or_default()});
        }
        let i = g.info.as_ref().unwrap();
        json!({"ok": true, "version": env!("FAE_GPU_VERSION"), "name": i.name, "backend": i.backend, "device_type": i.device_type,
               "driver": i.driver, "driver_info": i.driver_info, "max_texture_2d": i.max_texture_2d,
               "max_buffer": i.max_buffer, "max_storage_binding": i.max_storage_binding, "max_pixels": i.max_pixels()})
    }

    /// core.py `warm_gpu`: compile the GPU core's shaders in the background (app start), one pipeline at
    /// a time with the lock taken only for that one, so a render meanwhile waits for one compile at most.
    pub fn warm(&self) {
        let st = self.gpu.clone();
        if lock(&st).why.is_some() {
            return;
        }
        let _ = std::thread::Builder::new().name("gpu-warm".into()).spawn(move || {
            let n = if lock(&st).device().is_some() { gpu::Gpu::pipeline_count() } else { 0 };
            for i in 0..n {
                let g = lock(&st);
                match &g.dev {
                    Some(d) => d.warm(i),
                    None => return, // turned off or failed meanwhile
                }
            }
        });
    }

    /// core.py `gpu_release`: free the GPU memory of the last image (after a Process batch / a compare).
    pub fn release(&self) {
        let mut g = lock(&self.gpu);
        if let Some(d) = g.dev.as_mut() {
            d.release();
            let mut e = d.wait().err().into_iter().collect::<Vec<_>>();
            e.extend(d.take_errors());
            if !e.is_empty() {
                g.fail(e.join("; "));
            }
        }
    }

    /// core.py `_gpu_render`: the edit on the GPU core, or None when it can't (then the CPU core
    /// renders): turned off, no device, the image too big for this GPU, a step it can't make, an error.
    fn render_gpu(&self, rgb: &[u8], w: usize, h: usize, edit: &Edit) -> Option<Vec<u8>> {
        let mut g = lock(&self.gpu);
        g.device()?;
        let i = g.info.as_ref().unwrap();
        if (w * h) as u64 > i.max_pixels() || w.max(h) as u64 > i.max_texture_2d as u64 {
            return None; // too big for this GPU: the CPU core (no tiling yet)
        }
        let steps = match gpu_steps(edit) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("GPU core can't run this edit, Process uses the CPU core: {e}");
                return None; // not a device problem, the GPU stays on
            }
        };
        let d = g.dev.as_mut().unwrap();
        // a panic in the GPU code counts as a device error (the CPU core renders, the GPU stays off)
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Vec<u8>, String> {
            d.set_steps(&steps)?;
            d.upload(rgb, w as u32, h as u32, 3)?;
            let out = d.readback_start()?.finish_blocking(d)?;
            let e = d.take_errors();
            if e.is_empty() { Ok(out) } else { Err(e.join("; ")) }
        }))
        .unwrap_or_else(|_| Err("panic in the GPU core".into()));
        match r {
            Ok(out) => Some(out),
            Err(e) => {
                g.fail(e);
                eprintln!("GPU core off for this session, Process uses the CPU core: {}", g.why.as_deref().unwrap_or(""));
                None
            }
        }
    }

    /// core.py `render`: the full edit as Process writes it (packed RGB in and out). `skipped` gets the
    /// ids of the kit filters the CPU core left out.
    pub fn render(&self, rgb: &[u8], w: usize, h: usize, edit: &Edit, engine: Engine, skipped: &mut Vec<String>) -> Result<Vec<u8>, Error> {
        if rgb.len() != w * h * 3 {
            return Err(bad(format!("render: {} bytes for {w} x {h} RGB", rgb.len())));
        }
        if engine != Engine::Cpu {
            if let Some(out) = self.render_gpu(rgb, w, h, edit) {
                return Ok(out);
            }
            if engine == Engine::Gpu {
                let why = lock(&self.gpu).why.clone().unwrap_or_else(|| "image too big for this GPU, or a step it can't make".into());
                return Err(Error::new(ErrorKind::Failed, format!("GPU core can't render this: {why}")));
            }
        }
        skipped.extend(edit.fx_used());
        render_cpu(rgb, w, edit)
    }

    /// core.py `selftest` (the self-test page's "Python core" / "Process engine" lines): the CPU core runs
    /// a test edit; the GPU core runs the same, compared by the chain part of the tolerance rule.
    pub fn selftest(&self) -> Value {
        let (w, h) = (131usize, 257usize);
        let mut seed = 7u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let a: Vec<u8> = (0..w * h * 3).map(|_| (rnd() >> 24) as u8).collect();
        let luts: Vec<Vec<u8>> = (0..3).map(|_| (0..256).map(|_| (rnd() >> 24) as u8).collect()).collect();
        let edit = json!({"luts": luts, "point": true, "k": 1.1, "o": -3.0, "gains": [1.02, 0.97, 1.0], "sat": 1.3, "md": 0.4,
            "bw": {"s": 1.2, "op": 0.7, "w": [1.05, 1.0, 0.95], "ps": 0.5, "psk": [0.25, 0.5, 0.25]},
            "sc": {"cast": 0.4, "cc": 35, "dyn": 0.5, "op": 0.8, "dk": [0.2, 0.6, 0.2], "shl": null},
            "sharp": {"as": 0.5, "bs": 0.3, "st": 0.4, "lc": 0.2, "fo": -0.3}});
        let base = json!({"ok": true, "version": fe::VERSION, "threads": fe::threads()});
        let mut r = base.clone();
        let ed = match parse(&edit) {
            Ok(e) => e,
            Err(e) => return json!({"ok": false, "error": e.message}),
        };
        let out = match self.render(&a, w, h, &ed, Engine::Cpu, &mut vec![]) {
            Ok(o) => o,
            Err(e) => return json!({"ok": false, "error": e.message}),
        };
        let ok = out.len() == a.len() && out != a;
        let mut g = self.gpu_info();
        if g["ok"] == json!(true) {
            match self.render(&a, w, h, &ed, Engine::Gpu, &mut vec![]) {
                Ok(o) => {
                    let (mut max, mut sum, mut far) = (0i32, 0f64, 0usize);
                    for (p, q) in o.chunks_exact(3).zip(out.chunks_exact(3)) {
                        let mut m = 0;
                        for c in 0..3 {
                            let d = (p[c] as i32 - q[c] as i32).abs();
                            sum += d as f64;
                            m = m.max(d);
                        }
                        max = max.max(m);
                        far += (m > 2) as usize;
                    }
                    let mean = sum / out.len() as f64;
                    g["max_diff"] = json!(max);
                    g["mean_diff"] = json!((mean * 1e4).round() / 1e4);
                    g["same"] = json!(mean < 0.05 && far as f64 / (w * h) as f64 <= 1e-3);
                }
                Err(e) => g = json!({"ok": false, "error": e.message}),
            }
        }
        r["ok"] = json!(ok);
        r["gpu"] = g;
        if !ok {
            r["error"] = json!("unexpected result");
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_flat_and_steps() {
        // nothing set: no steps
        let id: Vec<u32> = (0..256).collect();
        let e = parse(&json!({"luts": [id, id, id], "point": false})).unwrap();
        assert!(e.steps.is_empty() && !e.sharp.active());
        assert!(parse(&Value::Null).unwrap().steps.is_empty());
        // the flat edit in DEFAULT_ORDER
        let e = parse(&json!({"sat": 1.2, "point": true, "k": 1.1, "sharp": {"as": 0.3}})).unwrap();
        assert!(matches!(e.steps[0], Step::Core(StepBuf::Point { .. })));
        assert!(matches!(e.steps[1], Step::Core(StepBuf::Adjust { sat: Some(_), .. })));
        assert!(e.sharp.active());
        // an explicit list; fx kept with its id; an unknown kind fails
        let e = parse(&json!({"steps": [{"t": "fx", "fx": {"id": "glow", "p": [1, 2]}}, {"t": "sc", "sc": {"cast": 0.2}},
                                         {"t": "fx", "fx": {"id": "glow"}}]})).unwrap();
        assert_eq!(e.steps.len(), 3);
        assert_eq!(e.fx_used(), vec!["glow".to_string()]);
        assert!(parse(&json!({"steps": [{"t": "nope"}]})).is_err());
        assert!(parse(&json!({"wb": [1, 2]})).is_err());
    }

    #[test]
    fn cpu_render_and_skipped() {
        let c = Cores::new(true);
        let rgb: Vec<u8> = (0..4 * 3 * 3).map(|i| (i * 7) as u8).collect();
        let e = parse(&json!({"steps": [{"t": "point", "point": true, "k": 1.2, "o": 3}, {"t": "fx", "fx": {"id": "x"}}]})).unwrap();
        let mut sk = vec![];
        let out = c.render(&rgb, 4, 3, &e, Engine::Auto, &mut sk).unwrap();
        assert_eq!(sk, vec!["x".to_string()]);
        assert_eq!(out.len(), rgb.len());
        assert_ne!(out, rgb);
        assert!(c.render(&rgb, 4, 3, &e, Engine::Gpu, &mut vec![]).is_err());
        assert_eq!(c.gpu_info()["ok"], json!(false));
        assert_eq!(c.selftest()["ok"], json!(true));
    }
}
