//! Python extension `fae_backend`: the Rust back end for the pywebview shell (fastedit/backend.py) and
//! the tests. API:
//!   b = Backend(settings="…/fae_settings.json", presets="…/fae_presets.json")
//!   b.call(name, args_json) -> result_json   one command of the table (JSON text both ways)
//!   b.decode(path) -> (pixels, w, h, channels, icc, format)   open an image (B1); b.heif_info()
//!   b.thumb(path, max) -> JPEG bytes   a file-list thumbnail (B2)
//!   geometry(px, w, h, ch, params_json) / fit(...) -> (px, w, h, ch)   the stage's geometry and resize
//!     steps on raw pixels (B2; params = StageParams as JSON); fit_scale(w, h, params_json);
//!     mpix_size(w, h, mpix, prio), preset_size(ar, res)   the size maths
//!   b.stage(path, params_json) -> (px, w, h, ch, icc)   the stage image at full resolution (B3; for
//!     Process / the encoded compare until B5); b.give_source(path, px, w, h, ch, icc)   pixels the shell
//!     decoded itself (AVIF until B4); b.preview(token) -> RGBA bytes | None; b.put_preview(rgba) -> token
//!   white_patch(px, w, h, ch, pct, strength) / gray_edge(px, w, h, ch, p, sigma, strength) -> (px, w, h, ch)
//!     the colour balance (B3)
//!   b.encode(px, w, h, ch, fmt, opts_json, icc, req) -> (bytes, encoder, used_json)   JPEG / AVIF / lossy
//!     WebP (B4); b.save(px, w, h, ch, src, fmt, opts_json, icc) -> path written; b.preview_begin(req)
//!   encode_png / encode_webp / encode_jpeg_pillow / yuv444p10 / avif_mux / out_path /
//!     encoder_opts   the encoders' parts (tests)
//!   commands() -> [names this build runs]; table() -> [(name, wave, what)]; features(); version()
//! Errors: RuntimeError("<kind>: <message>"); kind "unknown" = not in this build (the shell runs its own).
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

fn perr(e: backend::Error) -> PyErr {
    PyRuntimeError::new_err(format!("{}: {}", format!("{:?}", e.kind).to_lowercase(), e.message))
}

#[pyclass(name = "Backend")]
struct PyBackend {
    b: backend::Backend,
}

#[pymethods]
impl PyBackend {
    #[new]
    #[pyo3(signature = (settings, presets, heif_dir=None, encoders_dir=None))]
    fn new(settings: String, presets: String, heif_dir: Option<String>, encoders_dir: Option<String>) -> Self {
        PyBackend {
            b: backend::Backend::new(backend::Config {
                settings: settings.into(),
                presets: presets.into(),
                heif_dir: heif_dir.map(Into::into),
                encoders_dir: encoders_dir.map(Into::into),
            }),
        }
    }

    /// Open an image (B1): (pixels: bytes, w, h, channels 3 | 4, icc: bytes | None, format). Upright,
    /// as Pillow + exif_transpose gave it. RuntimeError "unknown: ..." for a format not in Rust yet.
    fn decode<'py>(&self, py: Python<'py>, path: &str) -> PyResult<(Bound<'py, PyBytes>, u32, u32, u8, Option<Bound<'py, PyBytes>>, &'static str)> {
        let r = py.detach(|| self.b.decode_file(std::path::Path::new(path)));
        let im = r.map_err(perr)?;
        Ok((PyBytes::new(py, &im.px), im.w, im.h, im.channels, im.icc.as_deref().map(|b| PyBytes::new(py, b)), im.format))
    }

    /// A file-list thumbnail (B2): JPEG bytes (quality 82), at most max × max. RuntimeError
    /// "unknown: ..." for a format not in Rust yet.
    fn thumb<'py>(&self, py: Python<'py>, path: &str, max: u32) -> PyResult<Bound<'py, PyBytes>> {
        let r = py.detach(|| self.b.thumb_file(std::path::Path::new(path), max));
        Ok(PyBytes::new(py, &r.map_err(perr)?))
    }

    /// The libheif in use: (path, version), or the reason it can't be loaded.
    fn heif_info(&self) -> PyResult<(String, String)> {
        let l = self.b.heif().map_err(perr)?;
        Ok((l.path.display().to_string(), l.version()))
    }

    /// The stage image of `path` at full resolution (B3): (pixels, w, h, channels, icc | None).
    /// RuntimeError "unknown: ..." for a format not in Rust yet (hand it over with give_source).
    fn stage<'py>(&self, py: Python<'py>, path: &str, params_json: &str) -> PyResult<(Bound<'py, PyBytes>, usize, usize, usize, Option<Bound<'py, PyBytes>>)> {
        let p = params(params_json)?;
        let r = py.detach(|| self.b.stage_image(std::path::Path::new(path), &p));
        let (img, icc) = r.map_err(perr)?;
        Ok((PyBytes::new(py, &img.data), img.w, img.h, img.c, icc.as_deref().map(|b| PyBytes::new(py, b))))
    }

    /// Pixels the shell decoded itself for `path` (a format not in Rust yet): upright RGB / RGBA.
    #[pyo3(signature = (path, px, w, h, ch, icc=None))]
    fn give_source(&self, path: &str, px: &[u8], w: usize, h: usize, ch: usize, icc: Option<&[u8]>) -> PyResult<()> {
        let img = px_in(px, w, h, ch)?.to_px();
        self.b.give_source(std::path::Path::new(path), img, icc.map(<[u8]>::to_vec)).map_err(perr)
    }

    /// The preview pixels (RGBA) under `token`, None when gone (the shell serves /preview/<token>).
    fn preview<'py>(&self, py: Python<'py>, token: &str) -> Option<Bound<'py, PyBytes>> {
        self.b.preview(token).map(|d| PyBytes::new(py, &d))
    }

    /// Store preview pixels (RGBA) the shell made itself; their token.
    fn put_preview(&self, rgba: &[u8]) -> String {
        self.b.put_preview(rgba.to_vec())
    }

    /// JPEG / AVIF / lossy WebP (B4): (bytes, encoder name, settings used as JSON). `req`: the encoded
    /// compare's request id (cancellable: RuntimeError "cancelled: ..."), None for Process. RuntimeError
    /// "unknown: ..." for an AVIF encoder not in Rust yet.
    #[pyo3(signature = (px, w, h, ch, fmt, opts_json, icc=None, req=None))]
    #[allow(clippy::too_many_arguments)]
    fn encode<'py>(&self, py: Python<'py>, px: &[u8], w: usize, h: usize, ch: usize, fmt: &str, opts_json: &str, icc: Option<&[u8]>, req: Option<i64>) -> PyResult<(Bound<'py, PyBytes>, &'static str, String)> {
        let (img, opts) = (px_in(px, w, h, ch)?, json_in(opts_json)?);
        let r = py.detach(|| self.b.encode_lossy(img, fmt, &opts, icc, req)).map_err(perr)?;
        Ok((PyBytes::new(py, &r.data), r.encoder, r.used.to_string()))
    }

    /// Encode and write next to `src` (B4; output.py save_image): the path written.
    #[pyo3(signature = (px, w, h, ch, src, fmt, opts_json, icc=None))]
    #[allow(clippy::too_many_arguments)]
    fn save(&self, py: Python<'_>, px: &[u8], w: usize, h: usize, ch: usize, src: &str, fmt: &str, opts_json: &str, icc: Option<&[u8]>) -> PyResult<String> {
        let (img, opts) = (px_in(px, w, h, ch)?, json_in(opts_json)?);
        let r = py.detach(|| self.b.save(img, src, fmt, icc, &opts)).map_err(perr)?;
        Ok(r.display().to_string())
    }

    /// A new encoded-compare request: encoder runs of older ones are killed (proc.py preview_begin).
    fn preview_begin(&self, req: i64) {
        self.b.preview_begin(req)
    }

    /// Run one command: `args_json` = the UI's positional arguments as a JSON array. Without the GIL.
    fn call(&self, py: Python<'_>, name: &str, args_json: &str) -> PyResult<String> {
        let args: Vec<serde_json::Value> =
            serde_json::from_str(args_json).map_err(|e| PyValueError::new_err(format!("{name}: arguments: {e}")))?;
        let r = py.detach(|| self.b.call(name, &args));
        r.map(|v| v.to_string()).map_err(perr)
    }
}

type Raw<'py> = (Bound<'py, PyBytes>, usize, usize, usize);

fn params(json: &str) -> PyResult<backend::fit::StageParams> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| PyValueError::new_err(format!("params: {e}")))?;
    Ok(backend::fit::StageParams::from_json(&v))
}

fn json_in(s: &str) -> PyResult<serde_json::Value> {
    serde_json::from_str(s).map_err(|e| PyValueError::new_err(format!("options: {e}")))
}

fn px_in(px: &[u8], w: usize, h: usize, ch: usize) -> PyResult<backend::px::View<'_>> {
    if !(ch == 3 || ch == 4) || px.len() != w * h * ch || w == 0 || h == 0 {
        return Err(PyValueError::new_err(format!("pixels: {} bytes for {w} x {h} x {ch}", px.len())));
    }
    Ok(backend::px::View::new(w, h, ch, px))
}

fn px_out(py: Python<'_>, p: backend::px::Px) -> Raw<'_> {
    (PyBytes::new(py, &p.data), p.w, p.h, p.c)
}

/// The geometry step (B2): flip, quarter turns, fine angle, crop, crop_free (geometry.py apply_geometry).
#[pyfunction]
fn geometry<'py>(py: Python<'py>, px: &[u8], w: usize, h: usize, ch: usize, params_json: &str) -> PyResult<Raw<'py>> {
    let (img, p) = (px_in(px, w, h, ch)?, params(params_json)?);
    let out = py.detach(|| backend::geometry::apply(img, &p));
    Ok(px_out(py, out))
}

/// The resize step (B2): preset / custom / Mpix with Pillow's Lanczos (geometry.py fit_image).
#[pyfunction]
fn fit<'py>(py: Python<'py>, px: &[u8], w: usize, h: usize, ch: usize, params_json: &str) -> PyResult<Raw<'py>> {
    let (img, p) = (px_in(px, w, h, ch)?, params(params_json)?);
    let out = py.detach(|| backend::fit::fit_image(img, &p));
    Ok(px_out(py, out))
}

/// geometry.py fit_scale: the largest per-axis factor the resize applies to a w × h image.
/// colour.py white_patch_enhance (B3).
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn white_patch<'py>(py: Python<'py>, px: &[u8], w: usize, h: usize, ch: usize, pct: f64, strength: f64) -> PyResult<Raw<'py>> {
    let img = px_in(px, w, h, ch)?;
    let out = py.detach(|| backend::colour::white_patch(img, pct, strength));
    Ok(px_out(py, out))
}

/// colour.py gray_edge_enhance (B3).
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn gray_edge<'py>(py: Python<'py>, px: &[u8], w: usize, h: usize, ch: usize, p: i64, sigma: f64, strength: f64) -> PyResult<Raw<'py>> {
    let img = px_in(px, w, h, ch)?;
    let out = py.detach(|| backend::colour::gray_edge(img, p, sigma, strength));
    Ok(px_out(py, out))
}

#[pyfunction]
fn fit_scale(w: usize, h: usize, params_json: &str) -> PyResult<f64> {
    Ok(backend::fit::fit_scale(w.max(1), h.max(1), &params(params_json)?))
}

#[pyfunction]
fn mpix_size(w: usize, h: usize, mpix: f64, prio: &str) -> (usize, usize) {
    backend::fit::mpix_size(w.max(1), h.max(1), mpix, prio)
}

#[pyfunction]
fn preset_size(ar: &str, res: i64) -> (usize, usize) {
    backend::fit::preset_size(ar, res)
}

/// PNG as Pillow's save(compress_level=9, optimize=True) (B4).
#[pyfunction]
#[pyo3(signature = (px, w, h, ch, icc=None))]
fn encode_png<'py>(py: Python<'py>, px: &[u8], w: usize, h: usize, ch: usize, icc: Option<&[u8]>) -> PyResult<Bound<'py, PyBytes>> {
    let img = px_in(px, w, h, ch)?;
    let r = py.detach(|| backend::encode::encode_png(img, icc)).map_err(perr)?;
    Ok(PyBytes::new(py, &r))
}

/// WebP as Pillow's save(lossless=, quality=, method=, exact=) (B4).
#[pyfunction]
#[pyo3(signature = (px, w, h, ch, lossless, quality, method, exact, icc=None))]
#[allow(clippy::too_many_arguments)]
fn encode_webp<'py>(py: Python<'py>, px: &[u8], w: usize, h: usize, ch: usize, lossless: bool, quality: f32, method: i32, exact: bool, icc: Option<&[u8]>) -> PyResult<Bound<'py, PyBytes>> {
    let img = px_in(px, w, h, ch)?;
    let r = py.detach(|| backend::encode::encode_webp(img, lossless, quality, method, exact, icc)).map_err(perr)?;
    Ok(PyBytes::new(py, &r))
}

/// JPEG as Pillow's save(quality=, optimize=True, progressive=True) (B4; jpeg.py's fallback); RGBA is
/// flattened onto white first, as jpeg.py does.
#[pyfunction]
#[pyo3(signature = (px, w, h, ch, quality, icc=None))]
fn encode_jpeg_pillow<'py>(py: Python<'py>, px: &[u8], w: usize, h: usize, ch: usize, quality: i32, icc: Option<&[u8]>) -> PyResult<Bound<'py, PyBytes>> {
    let img = px_in(px, w, h, ch)?;
    let r = py.detach(|| backend::encode::encode_jpeg_turbo(&backend::encode::flatten_rgb(img), w, h, quality, icc)).map_err(perr)?;
    Ok(PyBytes::new(py, &r))
}

/// avif.py rgb_to_yuv444p10 (B4): planar 10-bit Y, Cb, Cr.
#[pyfunction]
fn yuv444p10<'py>(py: Python<'py>, px: &[u8], w: usize, h: usize, ch: usize) -> PyResult<Bound<'py, PyBytes>> {
    let img = px_in(px, w, h, ch)?;
    let r = py.detach(|| backend::avif::rgb_to_yuv444p10(img.data, ch));
    Ok(PyBytes::new(py, &r))
}

/// avif.py mux_avif (B4): an AV1 OBU stream (or IVF) -> AVIF file.
#[pyfunction]
#[pyo3(signature = (stream, w, h, icc=None, nclx=(1, 13, 1, 1)))]
fn avif_mux<'py>(py: Python<'py>, stream: &[u8], w: u32, h: u32, icc: Option<&[u8]>, nclx: (u16, u16, u16, u8)) -> PyResult<Bound<'py, PyBytes>> {
    let s = backend::avif::ivf_payload(stream).map_err(perr)?;
    Ok(PyBytes::new(py, &backend::avif::mux(s, w, h, icc, nclx).map_err(perr)?))
}

/// The settings used for `fmt` (B4): jpeg_opts / avif_opts / webp_opts of the Python, as JSON.
#[pyfunction]
fn encoder_opts(fmt: &str, opts_json: &str) -> PyResult<String> {
    Ok(backend::encode::normalised(fmt, &json_in(opts_json)?).map_err(perr)?.to_string())
}

/// output.py _out_path (B4).
#[pyfunction]
fn out_path(src: &str, w: usize, h: usize, ext: &str) -> String {
    backend::encode::out_path(src, w, h, ext).display().to_string()
}

#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Optional parts this build has ("jpeg-simd": libjpeg-turbo's SIMD code, "rav1e-asm": rav1e's assembly).
#[pyfunction]
fn features() -> Vec<&'static str> {
    let mut v = vec![];
    if cfg!(feature = "jpeg-simd") {
        v.push("jpeg-simd");
    }
    if cfg!(feature = "simd") {
        v.push("rav1e-asm");
    }
    v
}

/// The commands this build runs.
#[pyfunction]
fn commands() -> Vec<&'static str> {
    backend::Backend::commands()
}

/// The whole command table: (name, wave, what it does).
#[pyfunction]
fn table() -> Vec<(&'static str, String, &'static str)> {
    backend::COMMANDS.iter().map(|(n, w, d)| (*n, format!("{w:?}"), *d)).collect()
}

#[pymodule]
fn fae_backend(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBackend>()?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(commands, m)?)?;
    m.add_function(wrap_pyfunction!(table, m)?)?;
    m.add_function(wrap_pyfunction!(features, m)?)?;
    m.add_function(wrap_pyfunction!(geometry, m)?)?;
    m.add_function(wrap_pyfunction!(fit, m)?)?;
    m.add_function(wrap_pyfunction!(fit_scale, m)?)?;
    m.add_function(wrap_pyfunction!(white_patch, m)?)?;
    m.add_function(wrap_pyfunction!(gray_edge, m)?)?;
    m.add_function(wrap_pyfunction!(mpix_size, m)?)?;
    m.add_function(wrap_pyfunction!(preset_size, m)?)?;
    m.add_function(wrap_pyfunction!(encode_png, m)?)?;
    m.add_function(wrap_pyfunction!(encode_webp, m)?)?;
    m.add_function(wrap_pyfunction!(encode_jpeg_pillow, m)?)?;
    m.add_function(wrap_pyfunction!(yuv444p10, m)?)?;
    m.add_function(wrap_pyfunction!(avif_mux, m)?)?;
    m.add_function(wrap_pyfunction!(out_path, m)?)?;
    m.add_function(wrap_pyfunction!(encoder_opts, m)?)?;
    Ok(())
}
