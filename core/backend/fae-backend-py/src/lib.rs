//! Python extension `fae_backend`: the Rust back end for the pywebview shell (fastedit/backend.py) and
//! the tests. API:
//!   b = Backend(settings="…/fae_settings.json", presets="…/fae_presets.json")
//!   b.call(name, args_json) -> result_json   one command of the table (JSON text both ways)
//!   commands() -> [names this build runs]; table() -> [(name, wave, what)]; version()
//! Errors: RuntimeError("<kind>: <message>"); kind "unknown" = not in this build (the shell runs its own).
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

#[pyclass(name = "Backend")]
struct PyBackend {
    b: backend::Backend,
}

#[pymethods]
impl PyBackend {
    #[new]
    #[pyo3(signature = (settings, presets))]
    fn new(settings: String, presets: String) -> Self {
        PyBackend { b: backend::Backend::new(backend::Config { settings: settings.into(), presets: presets.into() }) }
    }

    /// Run one command: `args_json` = the UI's positional arguments as a JSON array. Without the GIL.
    fn call(&self, py: Python<'_>, name: &str, args_json: &str) -> PyResult<String> {
        let args: Vec<serde_json::Value> =
            serde_json::from_str(args_json).map_err(|e| PyValueError::new_err(format!("{name}: arguments: {e}")))?;
        let r = py.detach(|| self.b.call(name, &args));
        match r {
            Ok(v) => Ok(v.to_string()),
            Err(e) => Err(PyRuntimeError::new_err(format!("{}: {}", format!("{:?}", e.kind).to_lowercase(), e.message))),
        }
    }
}

#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
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
    Ok(())
}
