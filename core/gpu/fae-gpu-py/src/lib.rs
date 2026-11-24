//! Python extension `fae_gpu`: the GPU core natively (DirectX 12 on Windows, FAE_GPU_BACKEND=vulkan
//! to try Vulkan; Vulkan elsewhere). API:
//!   g = Gpu(fallback=False, backends=None); g.info()
//!   g.render(rgb, steps) -> HxWx3 uint8     steps as fastedit/core.py's _steps builds them, then
//!                                           optionally ("sharp", as, bs, st, lc, fo, scale) last;
//!                                           ValueError for a step the GPU can't run yet
//!   g.upload(rgb); g.set_steps(steps); g.run(times=1); g.readback()   (benchmarks)
//!   g.release()                             frees the last image's GPU memory (Process, after a batch)
//! Every call waits for the GPU (so Python can time it); the waits run without the GIL.
use numpy::{PyArray3, PyReadonlyArray1, PyReadonlyArray2, PyReadonlyArray3, PyUntypedArrayMethods};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList, PyTuple};

fn err(e: String) -> PyErr {
    PyRuntimeError::new_err(e)
}

fn arr3(v: Option<Vec<f64>>) -> PyResult<Option<[f64; 3]>> {
    match v {
        None => Ok(None),
        Some(v) => <[f64; 3]>::try_from(v.as_slice()).map(Some).map_err(|_| PyValueError::new_err("need 3 values")),
    }
}

fn table<'py>(v: Bound<'py, PyAny>) -> PyResult<Option<Vec<f64>>> {
    if v.is_none() {
        return Ok(None);
    }
    let a: PyReadonlyArray1<'py, f64> = v.extract()?;
    Ok(Some(a.as_array().iter().copied().collect()))
}

/// core._steps entries -> GPU steps; ValueError for what the GPU doesn't run yet.
fn to_steps(steps: &Bound<'_, PyList>) -> PyResult<Vec<gpu::Step>> {
    let ve = |e: String| PyValueError::new_err(e);
    let mut out = vec![];
    for item in steps.iter() {
        let t = item.cast::<PyTuple>().map_err(|_| PyValueError::new_err("a step must be a tuple"))?;
        let kind: String = t.get_item(0)?.extract()?;
        out.push(match kind.as_str() {
            "curves" => {
                let l: PyReadonlyArray2<'_, u8> = t.get_item(1)?.extract()?;
                if l.shape() != [3, 256] {
                    return Err(PyValueError::new_err("curves: luts must be 3 x 256 uint8"));
                }
                let flat: Vec<u8> = l.as_array().iter().copied().collect();
                gpu::Step::curves(&flat).map_err(ve)?
            }
            // ("point", k, o, gains | None)
            "point" => gpu::Step::point(t.get_item(1)?.extract()?, t.get_item(2)?.extract()?, arr3(t.get_item(3)?.extract::<Option<Vec<f64>>>()?)?),
            // ("adjust", md, wb 3x3 | None, con | None, [(T, ratio)], boost | None, sat | None)
            "adjust" => {
                let md: f64 = t.get_item(1)?.extract()?;
                let wb = match t.get_item(2)?.extract::<Option<PyReadonlyArray2<'_, f64>>>()? {
                    None => None,
                    Some(a) => {
                        let a = a.as_array();
                        if a.shape() != [3, 3] {
                            return Err(PyValueError::new_err("wb must be 3 x 3"));
                        }
                        Some([[a[[0, 0]], a[[0, 1]], a[[0, 2]]], [a[[1, 0]], a[[1, 1]], a[[1, 2]]], [a[[2, 0]], a[[2, 1]], a[[2, 2]]]])
                    }
                };
                let con = table(t.get_item(3)?)?;
                let mut tones: Vec<(Vec<f64>, bool)> = vec![];
                for x in t.get_item(4)?.cast::<PyList>()?.iter() {
                    let x = x.cast::<PyTuple>()?;
                    tones.push((table(x.get_item(0)?)?.unwrap_or_default(), x.get_item(1)?.extract()?));
                }
                let boost = table(t.get_item(5)?)?;
                let sat: Option<f64> = t.get_item(6)?.extract()?;
                let tr: Vec<(&[f64], bool)> = tones.iter().map(|(v, r)| (v.as_slice(), *r)).collect();
                gpu::Step::adjust(md, wb, con.as_deref(), &tr, boost.as_deref(), sat).map_err(ve)?
            }
            // ("bw", s, op, w | None, ps, psk | None)
            "bw" => {
                let ps: f64 = t.get_item(4)?.extract()?;
                let (s, op) = (t.get_item(1)?.extract()?, t.get_item(2)?.extract()?);
                let w = arr3(t.get_item(3)?.extract::<Option<Vec<f64>>>()?)?;
                if ps != 0.0 {
                    let k = table(t.get_item(5)?)?.unwrap_or_default();
                    gpu::Step::bw_ps(s, op, w, ps, &k).map_err(ve)?
                } else {
                    gpu::Step::bw(s, op, w)
                }
            }
            // ("film", n, d | None, t1 | None, op)
            "film" => {
                let d = table(t.get_item(2)?)?;
                let t1 = table(t.get_item(3)?)?;
                gpu::Step::film(t.get_item(1)?.extract()?, d.as_deref(), t1.as_deref(), t.get_item(4)?.extract()?).map_err(ve)?
            }
            // ("sc", cast, cc, dyn, op, dk | None, shl | None)
            "sc" => {
                let dk = table(t.get_item(5)?)?;
                let shl = table(t.get_item(6)?)?;
                gpu::Step::sc(t.get_item(1)?.extract()?, t.get_item(2)?.extract()?, t.get_item(3)?.extract()?,
                              t.get_item(4)?.extract()?, dk.as_deref(), shl.as_deref()).map_err(ve)?
            }
            // ("sharp", adaptive, balance, structure, local_contrast, focus, scale): Sharpen, last
            "sharp" => {
                let v: Vec<f64> = (1..7).map(|i| t.get_item(i)?.extract()).collect::<PyResult<_>>()?;
                out.extend(gpu::Step::sharp(v[0], v[1], v[2], v[3], v[4], v[5]));
                continue;
            }
            other => return Err(PyValueError::new_err(format!("not on the GPU yet: {other}"))),
        });
    }
    Ok(out)
}

fn rgb_of<'a>(rgb: &'a PyReadonlyArray3<'_, u8>) -> PyResult<(&'a [u8], u32, u32)> {
    let s = rgb.shape();
    if s[2] != 3 {
        return Err(PyValueError::new_err("rgb must be HxWx3"));
    }
    let src = rgb.as_slice().map_err(|_| PyValueError::new_err("rgb must be C-contiguous"))?;
    Ok((src, s[1] as u32, s[0] as u32))
}

#[pyclass(name = "Gpu")]
struct PyGpu {
    g: gpu::Gpu,
}

#[pymethods]
impl PyGpu {
    /// fallback=True asks for the software adapter (WARP on Windows). backends: "dx12", "vulkan"...
    #[new]
    #[pyo3(signature = (fallback=false, backends=None))]
    fn new(py: Python<'_>, fallback: bool, backends: Option<String>) -> PyResult<Self> {
        let backends = match backends {
            None => None,
            Some(s) => Some(gpu::parse_backends(&s).ok_or_else(|| PyValueError::new_err(format!("unknown backend in {s:?}")))?),
        };
        let opts = gpu::Options { fallback, backends };
        let g = py.detach(|| pollster::block_on(gpu::Gpu::new(&opts))).map_err(err)?;
        Ok(PyGpu { g })
    }

    fn info<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let i = &self.g.info;
        let d = PyDict::new(py);
        d.set_item("name", &i.name)?;
        d.set_item("backend", &i.backend)?;
        d.set_item("device_type", &i.device_type)?;
        d.set_item("driver", &i.driver)?;
        d.set_item("driver_info", &i.driver_info)?;
        d.set_item("max_texture_2d", i.max_texture_2d)?;
        d.set_item("max_buffer", i.max_buffer)?;
        d.set_item("max_storage_binding", i.max_storage_binding)?;
        d.set_item("max_pixels", i.max_pixels())?;
        Ok(d)
    }

    /// The whole chain on HxWx3 uint8: upload, run, read back.
    fn render<'py>(&mut self, py: Python<'py>, rgb: PyReadonlyArray3<'py, u8>, steps: &Bound<'py, PyList>) -> PyResult<Bound<'py, PyArray3<u8>>> {
        let st = to_steps(steps)?;
        let (src, w, h) = rgb_of(&rgb)?;
        let g = &mut self.g;
        let buf = py
            .detach(|| {
                g.set_steps(&st)?;
                g.upload(src, w, h, 3)?;
                g.readback_start()?.finish_blocking(g)
            })
            .map_err(err)?;
        self.check()?;
        let a = numpy::ndarray::Array3::from_shape_vec((h as usize, w as usize, 3), buf).unwrap();
        Ok(PyArray3::from_owned_array(py, a))
    }

    /// Upload HxWx3 uint8 (C-contiguous) as the stage. Waits.
    fn upload(&mut self, py: Python<'_>, rgb: PyReadonlyArray3<'_, u8>) -> PyResult<()> {
        let (src, w, h) = rgb_of(&rgb)?;
        let g = &mut self.g;
        py.detach(|| {
            g.upload(src, w, h, 3)?;
            g.wait()
        })
        .map_err(err)?;
        self.check()
    }

    fn set_steps(&mut self, steps: &Bound<'_, PyList>) -> PyResult<()> {
        let st = to_steps(steps)?;
        self.g.set_steps(&st).map_err(err)?;
        self.check()
    }

    /// Run the chain `times` times in one submit (timing). Waits.
    #[pyo3(signature = (times=1))]
    fn run(&mut self, py: Python<'_>, times: u32) -> PyResult<()> {
        let g = &mut self.g;
        py.detach(|| {
            g.run_times(times)?;
            g.wait()
        })
        .map_err(err)?;
        self.check()
    }

    /// Free the GPU memory of the last image (the device stays; the next render sets up again).
    fn release(&mut self) -> PyResult<()> {
        self.release_impl()
    }

    /// The chain's result as HxWx3 uint8. Waits.
    fn readback<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyArray3<u8>>> {
        let (w, h) = self.g.size().ok_or_else(|| err("no image".into()))?;
        let g = &mut self.g;
        let buf = py.detach(|| g.readback_start()?.finish_blocking(g)).map_err(err)?;
        self.check()?;
        let a = numpy::ndarray::Array3::from_shape_vec((h as usize, w as usize, 3), buf).unwrap();
        Ok(PyArray3::from_owned_array(py, a))
    }
}

impl PyGpu {
    /// Free the GPU memory of the last image (the device stays). Waits.
    fn release_impl(&mut self) -> PyResult<()> {
        self.g.release();
        self.g.wait().map_err(err)?;
        self.check()
    }

    fn check(&self) -> PyResult<()> {
        let e = self.g.take_errors();
        if e.is_empty() { Ok(()) } else { Err(err(e.join("; "))) }
    }
}

#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[pymodule]
fn fae_gpu(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyGpu>()?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add("PORTED", gpu::PORTED.to_vec())?;
    Ok(())
}
