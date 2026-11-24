//! Python extension `fae_backend`: the Rust back end for the pywebview shell (fastedit/backend.py) and
//! the tests. API:
//!   b = Backend(settings="…/fae_settings.json", presets="…/fae_presets.json")
//!   b.call(name, args_json) -> result_json   one command of the table (JSON text both ways)
//!   b.decode(path) -> (pixels, w, h, channels, icc, format)   open an image (B1); b.heif_info()
//!   b.thumb(path, max) -> JPEG bytes   a file-list thumbnail (B2)
//!   geometry(px, w, h, ch, params_json) / fit(...) -> (px, w, h, ch)   the stage's geometry and resize
//!     steps on raw pixels (B2; params = StageParams as JSON); fit_scale(w, h, params_json);
//!     mpix_size(w, h, mpix, prio), preset_size(ar, res)   the size maths
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
    #[pyo3(signature = (settings, presets, heif_dir=None))]
    fn new(settings: String, presets: String, heif_dir: Option<String>) -> Self {
        PyBackend {
            b: backend::Backend::new(backend::Config {
                settings: settings.into(),
                presets: presets.into(),
                heif_dir: heif_dir.map(Into::into),
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

#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Optional parts this build has ("jpeg-simd": libjpeg-turbo's SIMD code).
#[pyfunction]
fn features() -> Vec<&'static str> {
    let mut v = vec![];
    if cfg!(feature = "jpeg-simd") {
        v.push("jpeg-simd");
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
    m.add_function(wrap_pyfunction!(mpix_size, m)?)?;
    m.add_function(wrap_pyfunction!(preset_size, m)?)?;
    Ok(())
}
