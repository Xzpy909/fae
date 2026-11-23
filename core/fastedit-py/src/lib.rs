//! Python extension `fastedit_core`: thin wrapper around fastedit-core (imported as `fe`,
//! because this crate's own library is named fastedit_core).
//! Works on HxWx3 uint8 NumPy arrays; the heavy work runs without the GIL.
use numpy::{PyArray1, PyArray2, PyArray3, PyReadonlyArray1, PyReadonlyArray2, PyReadonlyArray3, PyUntypedArrayMethods};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyTuple;

#[pyfunction]
fn version() -> &'static str {
    fe::VERSION
}

#[pyfunction]
fn threads() -> usize {
    fe::threads()
}

/// apply_luts(rgb: HxWx3 uint8, luts: 3 x 256 ints) -> new HxWx3 uint8 array
#[pyfunction]
fn apply_luts<'py>(py: Python<'py>, rgb: PyReadonlyArray3<'py, u8>, luts: Vec<Vec<u8>>) -> PyResult<Bound<'py, PyArray3<u8>>> {
    let shape = rgb.shape();
    let (h, w, c) = (shape[0], shape[1], shape[2]);
    if c != 3 {
        return Err(PyValueError::new_err("rgb must be HxWx3"));
    }
    if luts.len() != 3 || luts.iter().any(|l| l.len() != 256) {
        return Err(PyValueError::new_err("luts must be 3 lists of 256 values"));
    }
    let flat: Vec<u8> = luts.concat();
    let tables = fe::luts_from_flat(&flat).unwrap();
    let mut buf: Vec<u8> = match rgb.as_slice() {
        Ok(s) => s.to_vec(),
        Err(_) => rgb.as_array().iter().copied().collect(), // non-contiguous input
    };
    py.detach(|| fe::apply_luts(&mut buf, w, 3, &tables));
    let out = numpy::ndarray::Array3::from_shape_vec((h, w, 3), buf).unwrap();
    Ok(PyArray3::from_owned_array(py, out))
}

/// luma01(rgb: HxWx3 uint8) -> HxW float32: edit.rgb_luma01 (the input of the Mid/Detail guided filter)
#[pyfunction]
fn luma01<'py>(py: Python<'py>, rgb: PyReadonlyArray3<'py, u8>) -> PyResult<Bound<'py, PyArray2<f32>>> {
    let shape = rgb.shape();
    let (h, w) = (shape[0], shape[1]);
    if shape[2] != 3 {
        return Err(PyValueError::new_err("rgb must be HxWx3"));
    }
    let src = rgb.as_slice().map_err(|_| PyValueError::new_err("rgb must be C-contiguous"))?;
    let y = py.detach(|| fe::edit::luma01(src));
    Ok(PyArray2::from_owned_array(py, numpy::ndarray::Array2::from_shape_vec((h, w), y).unwrap()))
}

/// md_base(rgb: HxWx3 uint8) -> HxW float32: the Mid/Detail smoothed luma (guided filter at md_radius(H))
#[pyfunction]
fn md_base<'py>(py: Python<'py>, rgb: PyReadonlyArray3<'py, u8>) -> PyResult<Bound<'py, PyArray2<f32>>> {
    let shape = rgb.shape();
    let (h, w) = (shape[0], shape[1]);
    if shape[2] != 3 {
        return Err(PyValueError::new_err("rgb must be HxWx3"));
    }
    let src = rgb.as_slice().map_err(|_| PyValueError::new_err("rgb must be C-contiguous"))?;
    let b = py.detach(|| fe::md::base(src, w, h));
    Ok(PyArray2::from_owned_array(py, numpy::ndarray::Array2::from_shape_vec((h, w), b).unwrap()))
}

fn f64v(o: &Bound<'_, PyAny>, name: &str) -> PyResult<Vec<f64>> {
    let a: PyReadonlyArray1<'_, f64> = o.extract().map_err(|_| PyValueError::new_err(format!("{name} must be a float64 array")))?;
    Ok(a.as_array().iter().copied().collect())
}

fn opt_f64v(o: &Bound<'_, PyAny>, name: &str) -> PyResult<Option<Vec<f64>>> {
    if o.is_none() { Ok(None) } else { f64v(o, name).map(Some) }
}

fn opt3(o: &Bound<'_, PyAny>, name: &str) -> PyResult<Option<[f64; 3]>> {
    if o.is_none() {
        return Ok(None);
    }
    o.extract::<[f64; 3]>().map(Some).map_err(|_| PyValueError::new_err(format!("{name}: need 3 values")))
}

/// One step from its tuple (see apply_chain).
fn step_from(t: &Bound<'_, PyTuple>) -> PyResult<fe::chain::StepBuf> {
    use fe::chain::StepBuf;
    let kind: String = t.get_item(0)?.extract()?;
    let g = |i: usize| t.get_item(i);
    let need = |n: usize| -> PyResult<()> {
        if t.len() != n {
            return Err(PyValueError::new_err(format!("step {kind}: {} values, need {n}", t.len())));
        }
        Ok(())
    };
    Ok(match kind.as_str() {
        "point" => {
            need(4)?;
            StepBuf::Point { k: g(1)?.extract()?, o: g(2)?.extract()?, gains: opt3(&g(3)?, "gains")? }
        }
        "adjust" => {
            need(7)?;
            let wbo = g(2)?;
            let wb = if wbo.is_none() {
                None
            } else {
                let m: PyReadonlyArray2<'_, f64> = wbo.extract()?;
                if m.shape() != [3, 3] {
                    return Err(PyValueError::new_err("wb must be 3x3"));
                }
                let v: Vec<f64> = m.as_array().iter().copied().collect();
                Some([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
            };
            let mut tones = Vec::new();
            for it in g(4)?.try_iter()? {
                let tt = it?;
                let tt = tt.cast::<PyTuple>().map_err(|_| PyValueError::new_err("tones: (table, ratio) tuples"))?;
                tones.push((f64v(&tt.get_item(0)?, "tones")?, tt.get_item(1)?.extract::<bool>()?));
            }
            let sat = g(6)?;
            StepBuf::Adjust {
                md: g(1)?.extract()?,
                wb,
                con: opt_f64v(&g(3)?, "con")?,
                tones,
                boost: opt_f64v(&g(5)?, "boost")?,
                sat: if sat.is_none() { None } else { Some(sat.extract()?) },
            }
        }
        "curves" => {
            need(2)?;
            let l: PyReadonlyArray2<'_, u8> = g(1)?.extract()?;
            if l.shape() != [3, 256] {
                return Err(PyValueError::new_err("luts must be 3x256"));
            }
            let v: Vec<u8> = l.as_array().iter().copied().collect();
            StepBuf::Curves(fe::luts_from_flat(&v).unwrap())
        }
        "bw" => {
            need(6)?;
            let ps: f64 = g(4)?.extract()?;
            let psk = opt_f64v(&g(5)?, "psk")?;
            if ps != 0.0 && psk.as_ref().is_none_or(|k| k.is_empty()) {
                return Err(PyValueError::new_err("bw: ps without psk"));
            }
            StepBuf::Bw { s: g(1)?.extract()?, op: g(2)?.extract()?, w: opt3(&g(3)?, "bw w")?, ps, psk: psk.unwrap_or_default() }
        }
        "film" => {
            need(5)?;
            StepBuf::Film { n: g(1)?.extract()?, d: opt_f64v(&g(2)?, "film d")?, t1: opt_f64v(&g(3)?, "film t1")?, op: g(4)?.extract()? }
        }
        "sc" => {
            need(7)?;
            StepBuf::Sc {
                cast: g(1)?.extract()?, cc: g(2)?.extract()?, dyn_: g(3)?.extract()?, op: g(4)?.extract()?,
                dk: opt_f64v(&g(5)?, "dk")?.unwrap_or_default(), shl: opt_f64v(&g(6)?, "shl")?,
            }
        }
        _ => return Err(PyValueError::new_err(format!("unknown step {kind}"))),
    })
}

/// apply_chain(rgb, steps, hist=False, adaptive=0.0, balance=0.0, structure=0.0, local_contrast=0.0, focus=0.0, sharp_scale=1.0)
///   -> HxWx3 uint8, or (image, hin, hout, hmx, hmn) with hist=True
///
/// The edit after the stage as an ordered list of steps (the panel's tool order; a tool may appear
/// more than once), then Sharpen. Steps (tuples, built by fastedit/core.py):
///   ("point", k, o, gains [3] | None)
///   ("adjust", md, wb 3x3 float64 | None, con | None, tones [(float64 array, ratio)], boost | None, sat | None)
///   ("curves", luts 3x256 uint8)
///   ("bw", s, op, w [3] | None, ps, psk | None)          Perceptual Saturation filter; ps = 0: without PS
///   ("film", n, d | None, t1 | None, op)
///   ("sc", cast, cc, dyn, op, dk | None, shl | None)
/// Histograms as the UI draws them (uint32: 768, 768, 256, 256); hin = input of the first Curves step.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (rgb, steps, hist=false, adaptive=0.0, balance=0.0, structure=0.0, local_contrast=0.0, focus=0.0, sharp_scale=1.0))]
fn apply_chain<'py>(
    py: Python<'py>,
    rgb: PyReadonlyArray3<'py, u8>,
    steps: Vec<Bound<'py, PyTuple>>,
    hist: bool,
    adaptive: f64,
    balance: f64,
    structure: f64,
    local_contrast: f64,
    focus: f64,
    sharp_scale: f64,
) -> PyResult<Bound<'py, PyAny>> {
    let shape = rgb.shape();
    let (h, w, c) = (shape[0], shape[1], shape[2]);
    if c != 3 {
        return Err(PyValueError::new_err("rgb must be HxWx3"));
    }
    let src = rgb.as_slice().map_err(|_| PyValueError::new_err("rgb must be C-contiguous"))?;
    let bufs = steps.iter().map(step_from).collect::<PyResult<Vec<_>>>()?;
    let st: Vec<fe::chain::Step<'_>> = bufs.iter().map(|b| b.step()).collect();
    for s in &st {
        s.validate().map_err(PyValueError::new_err)?;
    }
    let sharp = fe::sharp::Sharp { adaptive, balance, structure, local_contrast, focus, scale: sharp_scale };
    let mut buf = vec![0u8; src.len()];
    let hs = py.detach(|| fe::chain::run(src, &mut buf, w, 3, &st, &sharp, hist));
    let img = PyArray3::from_owned_array(py, numpy::ndarray::Array3::from_shape_vec((h, w, 3), buf).unwrap());
    if !hist {
        return Ok(img.into_any());
    }
    let hs = hs.unwrap_or_default();
    let v = |x: Vec<u32>| PyArray1::from_vec(py, x);
    Ok((img, v(hs.hin), v(hs.hout), v(hs.hmx), v(hs.hmn)).into_pyobject(py)?.into_any())
}

#[pymodule]
fn fastedit_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(threads, m)?)?;
    m.add_function(wrap_pyfunction!(apply_luts, m)?)?;
    m.add_function(wrap_pyfunction!(apply_chain, m)?)?;
    m.add_function(wrap_pyfunction!(luma01, m)?)?;
    m.add_function(wrap_pyfunction!(md_base, m)?)?;
    Ok(())
}
