//! WebAssembly build of the GPU core for the live preview: wgpu on the browser's WebGPU (WebView2
//! is Chromium). Single-threaded (no atomics build): the GPU does the work, the page drives it.
//! Used by ui/js/gpu-view.js (the preview) and ui/js/gpu-spike.js (the G0 benchmark page):
//!   const g = await create(canvas);  g.info() -> JSON
//!   g.upload(px, w, h, bpp)                         the stage, RGBA (bpp 4) or RGB (3)
//!   g.steps_begin(); g.step_curves(luts768); g.step_point(...); ...; g.steps_end()   the chain
//!   g.draw(cw, ch, ox, oy, s, split, nearestFrom)   runs the chain if it changed, draws the canvas
//!   await g.stats()           the UI's histograms (Uint32Array, 2048: hin, hout, hmx, hmn)
//!   await g.readback_rgba()   the result as RGBA (S.outData)
//!   await g.done(); await g.readback(); g.errors()
use fae_gpu::{view::ViewParams, wgpu, Gpu, Step};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct GpuPreview {
    g: Gpu,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    building: Vec<Step>,
}

fn js(e: String) -> JsValue {
    JsValue::from_str(&e)
}

fn three(v: Option<Vec<f64>>) -> Result<Option<[f64; 3]>, JsValue> {
    match v {
        None => Ok(None),
        Some(v) if v.len() == 3 => Ok(Some([v[0], v[1], v[2]])),
        Some(v) => Err(js(format!("need 3 values, got {}", v.len()))),
    }
}

/// WebGPU adapter + device (the adapter's full limits) and a surface on `canvas`.
#[wasm_bindgen]
pub async fn create(canvas: web_sys::HtmlCanvasElement) -> Result<GpuPreview, JsValue> {
    let (cw, ch) = (canvas.width().max(1), canvas.height().max(1));
    let instance = Gpu::instance(&Default::default());
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|e| js(format!("canvas surface: {e}")))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        })
        .await
        .map_err(|e| js(format!("no WebGPU adapter: {e}")))?;
    let caps = surface.get_capabilities(&adapter);
    // the image is already sRGB-encoded: draw into a plain (non-sRGB) format, no conversion
    let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).ok_or_else(|| js("canvas: no non-sRGB format".into()))?;
    // transparent outside the image (the page's background and the image's shadow show through)
    let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
        wgpu::CompositeAlphaMode::PreMultiplied
    } else {
        caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto)
    };
    let mut g = Gpu::with_adapter(adapter).await.map_err(js)?;
    g.enable_view(format);
    g.set_stats(true);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: cw,
        height: ch,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode,
        view_formats: vec![],
        color_space: wgpu::SurfaceColorSpace::Srgb,
    };
    surface.configure(&g.device, &config);
    Ok(GpuPreview { g, surface, config, building: vec![] })
}

#[wasm_bindgen]
impl GpuPreview {
    /// Adapter name, backend, limits, canvas format (JSON).
    pub fn info(&self) -> String {
        let mut s = self.g.info.to_json();
        s.pop();
        s.push_str(&format!(",\"format\":\"{:?}\",\"alpha\":\"{:?}\"}}", self.config.format, self.config.alpha_mode));
        s
    }

    /// The stage: w*h*bpp bytes, bpp 4 (RGBA, alpha ignored) or 3.
    pub fn upload(&mut self, px: &[u8], w: u32, h: u32, bpp: u32) -> Result<(), JsValue> {
        self.g.upload(px, w, h, bpp).map_err(js)
    }

    pub fn steps_begin(&mut self) {
        self.building.clear();
    }

    /// Curves: 768 levels (r, g, b tables).
    pub fn step_curves(&mut self, luts: &[u8]) -> Result<(), JsValue> {
        self.building.push(Step::curves(luts).map_err(js)?);
        Ok(())
    }

    /// Point correction: luma levels k, o; gains (3) or undefined.
    pub fn step_point(&mut self, k: f64, o: f64, gains: Option<Vec<f64>>) -> Result<(), JsValue> {
        self.building.push(Step::point(k, o, three(gains)?));
        Ok(())
    }

    /// Adjustments: md = Mid/Detail amount (-1..1, 0 = off); wb (9, row major) / con (4096) / boost
    /// (256) / sat or undefined; tones = the tone tables one after the other (4096 each), ratios = 1 / 0
    /// per table.
    #[allow(clippy::too_many_arguments)]
    pub fn step_adjust(&mut self, md: f64, wb: Option<Vec<f64>>, con: Option<Vec<f64>>, tones: Vec<f64>, ratios: Vec<u8>,
                       boost: Option<Vec<f64>>, sat: Option<f64>) -> Result<(), JsValue> {
        let wb = match wb {
            None => None,
            Some(w) if w.len() == 9 => Some([[w[0], w[1], w[2]], [w[3], w[4], w[5]], [w[6], w[7], w[8]]]),
            Some(w) => return Err(js(format!("adjust: wb has {} values, need 9", w.len()))),
        };
        if tones.len() != ratios.len() * 4096 {
            return Err(js(format!("adjust: {} tone values for {} tables", tones.len(), ratios.len())));
        }
        let t: Vec<(&[f64], bool)> = tones.chunks(4096).zip(&ratios).map(|(c, &r)| (c, r != 0)).collect();
        self.building.push(Step::adjust(md, wb, con.as_deref(), &t, boost.as_deref(), sat).map_err(js)?);
        Ok(())
    }

    /// The Perceptual Saturation filter: s, opacity, warmth (3) or undefined; ps = the perceptual
    /// amount (0 = off) with psk = its grid kernel.
    pub fn step_bw(&mut self, s: f64, op: f64, w: Option<Vec<f64>>, ps: f64, psk: Option<Vec<f64>>) -> Result<(), JsValue> {
        let st = if ps != 0.0 {
            Step::bw_ps(s, op, three(w)?, ps, psk.as_deref().unwrap_or(&[])).map_err(js)?
        } else {
            Step::bw(s, op, three(w)?)
        };
        self.building.push(st);
        Ok(())
    }

    /// Film Effect: n, the n^3 x 3 table or undefined, the 256 tone table or undefined, opacity.
    pub fn step_film(&mut self, n: usize, d: Option<Vec<f64>>, t1: Option<Vec<f64>>, op: f64) -> Result<(), JsValue> {
        self.building.push(Step::film(n, d.as_deref(), t1.as_deref(), op).map_err(js)?);
        Ok(())
    }

    /// Super Contrast: Color Cast, Correct Contrast, Dynamic Contrast + its cell kernel (or
    /// undefined), opacity, the Shadows / Highlights table (256) or undefined.
    pub fn step_sc(&mut self, cast: f64, cc: f64, dyn_: f64, op: f64, dk: Option<Vec<f64>>, shl: Option<Vec<f64>>) -> Result<(), JsValue> {
        self.building.push(Step::sc(cast, cc, dyn_, op, dk.as_deref(), shl.as_deref()).map_err(js)?);
        Ok(())
    }

    pub fn steps_end(&mut self) -> Result<(), JsValue> {
        let st = std::mem::take(&mut self.building);
        self.g.set_steps(&st).map_err(js)
    }

    /// Run the chain `times` times now (benchmarks; draw() runs it when needed).
    pub fn run(&mut self, times: u32) -> Result<(), JsValue> {
        self.g.run_times(times).map_err(js)
    }

    /// Draw into the canvas (cw x ch device pixels): image corner at (ox, oy), s canvas px per image
    /// px, unedited left of image x `split` (0 = none, >= width = all), nearest from scale `nearest_from`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, cw: u32, ch: u32, ox: f32, oy: f32, s: f32, split: f32, nearest_from: f32) -> Result<(), JsValue> {
        let (cw, ch) = (cw.max(1), ch.max(1));
        if cw != self.config.width || ch != self.config.height {
            self.config.width = cw;
            self.config.height = ch;
            self.surface.configure(&self.g.device, &self.config);
        }
        let st = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            other => return Err(js(format!("canvas texture: {other:?}"))),
        };
        let view = st.texture.create_view(&Default::default());
        self.g.draw_view(&view, &ViewParams { cw, ch, ox, oy, s, split, nearest_from }).map_err(js)?;
        self.g.queue.present(st);
        Ok(())
    }

    /// Resolves when the GPU has finished everything submitted so far.
    pub fn done(&self) -> js_sys::Promise {
        let f = self.g.done();
        wasm_bindgen_futures::future_to_promise(async move {
            f.await;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// The chain's result as packed RGB (Uint8Array, w*h*3).
    pub fn readback(&mut self) -> Result<js_sys::Promise, JsValue> {
        let rb = self.g.readback_start().map_err(js)?;
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let v = rb.finish().await.map_err(js)?;
            Ok(js_sys::Uint8Array::from(&v[..]).into())
        }))
    }

    /// The UI's histograms of the current result (runs the chain first if needed): Uint32Array of
    /// 2048 bins = hin (768, before the first Curves step), hout (768), hmx (256), hmn (256).
    pub fn stats(&mut self) -> Result<js_sys::Promise, JsValue> {
        let rb = self.g.stats_start().map_err(js)?;
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let v = rb.finish().await.map_err(js)?;
            let u: Vec<u32> = v.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            Ok(js_sys::Uint32Array::from(&u[..]).into())
        }))
    }

    /// The current result as RGBA (Uint8Array, w*h*4, alpha 255).
    pub fn readback_rgba(&mut self) -> Result<js_sys::Promise, JsValue> {
        let rb = self.g.readback_rgba_start().map_err(js)?;
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let v = rb.finish().await.map_err(js)?;
            Ok(js_sys::Uint8Array::from(&v[..]).into())
        }))
    }

    /// WebGPU errors reported since the last call ("" = none). They arrive asynchronously: ask
    /// after `done()`.
    pub fn errors(&self) -> String {
        self.g.take_errors().join("; ")
    }
}

/// The GPU core's version (shown by the preview's engine badge).
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
