//! Python extension `fastedit_core`: thin wrapper around fastedit-core (imported as `fe`,
//! because this crate's own library is named fastedit_core).
//! Works on HxWx3 uint8 NumPy arrays; the heavy work runs without the GIL.
use numpy::{PyArray1, PyArray2, PyArray3, PyReadonlyArray1, PyReadonlyArray2, PyReadonlyArray3, PyUntypedArrayMethods};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

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

/// (n, d, t1, op) as core.py passes the Film Effect
type FilmArgs<'py> = (usize, Option<PyReadonlyArray1<'py, f64>>, Option<PyReadonlyArray1<'py, f64>>, f64);
/// (s, op, w, ps, psk): the Filter; ps = 0 means no Perceptual Saturation
type BwArgs<'py> = (f64, f64, Option<[f64; 3]>, f64, Option<PyReadonlyArray1<'py, f64>>);
/// (cast, cc, dyn, op, dk, shl): Super Contrast
type ScArgs<'py> = (f64, f64, f64, f64, Option<PyReadonlyArray1<'py, f64>>, Option<PyReadonlyArray1<'py, f64>>);

fn slice<'a>(a: &'a PyReadonlyArray1<'_, f64>, name: &str) -> PyResult<&'a [f64]> {
    a.as_slice().map_err(|_| PyValueError::new_err(format!("{name} must be a contiguous float64 array")))
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

/// apply_edit(rgb, luts, point, k, o, gains, wb, con, tones, boost, sat, bw, film, sc, md=None, hist=False)
///   -> HxWx3 uint8, or (image, hin, hout, hmx, hmn) with hist=True
///
/// The whole edit after the stage: Mid/Detail (md = amount; the smoothing is computed here),
/// then the chain. Histograms as the UI draws them (uint32: 768, 768, 256, 256).
/// Arguments are already normalized by fastedit/core.py:
///   rgb HxWx3 uint8 (C-contiguous), luts 3x256 uint8, gains [3] | None, wb 3x3 float64 | None,
///   con / boost float64 arrays | None, tones [(float64 array, ratio)], sat float | None,
///   bw (s, op, [3] | None, ps, psk | None) | None, film (n, d | None, t1 | None, op) | None,
///   sc (cast, cc, dyn, op, dk | None, shl | None) | None
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (rgb, luts, point, k, o, gains, wb, con, tones, boost, sat, bw, film, sc, md=None, hist=false))]
fn apply_edit<'py>(
    py: Python<'py>,
    rgb: PyReadonlyArray3<'py, u8>,
    luts: PyReadonlyArray2<'py, u8>,
    point: bool,
    k: f64,
    o: f64,
    gains: Option<[f64; 3]>,
    wb: Option<PyReadonlyArray2<'py, f64>>,
    con: Option<PyReadonlyArray1<'py, f64>>,
    tones: Vec<(PyReadonlyArray1<'py, f64>, bool)>,
    boost: Option<PyReadonlyArray1<'py, f64>>,
    sat: Option<f64>,
    bw: Option<BwArgs<'py>>,
    film: Option<FilmArgs<'py>>,
    sc: Option<ScArgs<'py>>,
    md: Option<f64>,
    hist: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let shape = rgb.shape();
    let (h, w, c) = (shape[0], shape[1], shape[2]);
    if c != 3 {
        return Err(PyValueError::new_err("rgb must be HxWx3"));
    }
    let src = rgb.as_slice().map_err(|_| PyValueError::new_err("rgb must be C-contiguous"))?;
    if luts.shape() != [3, 256] {
        return Err(PyValueError::new_err("luts must be 3x256"));
    }
    let lv = luts.as_slice().map_err(|_| PyValueError::new_err("luts must be C-contiguous"))?;
    let wb = match &wb {
        Some(m) => {
            if m.shape() != [3, 3] {
                return Err(PyValueError::new_err("wb must be 3x3"));
            }
            let v = m.as_slice().map_err(|_| PyValueError::new_err("wb must be C-contiguous"))?;
            Some([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
        }
        None => None,
    };
    let md = md.filter(|a| *a != 0.0);
    let base = match md {
        Some(_) => py.detach(|| fe::md::base(src, w, h)),
        None => Vec::new(),
    };
    let edit = fe::edit::Edit {
        md: md.map(|a| (a, base.as_slice())),
        point,
        k,
        o,
        gains,
        wb,
        con: con.as_ref().map(|a| slice(a, "con")).transpose()?,
        tones: tones
            .iter()
            .map(|(t, ratio)| Ok(fe::edit::Tone { table: slice(t, "tones")?, ratio: *ratio }))
            .collect::<PyResult<_>>()?,
        boost: boost.as_ref().map(|a| slice(a, "boost")).transpose()?,
        sat,
        bw: match &bw {
            Some((s, op, w, ps, psk)) => Some(fe::edit::Bw {
                s: *s,
                op: *op,
                w: *w,
                ps: if *ps != 0.0 {
                    let k = psk.as_ref().ok_or_else(|| PyValueError::new_err("bw: ps without psk"))?;
                    Some((*ps, slice(k, "psk")?))
                } else {
                    None
                },
            }),
            None => None,
        },
        sc: match &sc {
            Some((cast, cc, dyn_, op, dk, shl)) => Some(fe::edit::Sc {
                cast: *cast,
                cc: *cc,
                dyn_: *dyn_,
                op: *op,
                dk: match dk {
                    Some(a) => slice(a, "dk")?,
                    None => &[],
                },
                shl: shl.as_ref().map(|a| slice(a, "shl")).transpose()?,
            }),
            None => None,
        },
        film: match &film {
            Some((n, d, t1, op)) => Some(fe::edit::Film {
                n: *n,
                d: d.as_ref().map(|a| slice(a, "film d")).transpose()?,
                t1: t1.as_ref().map(|a| slice(a, "film t1")).transpose()?,
                op: *op,
            }),
            None => None,
        },
        luts: fe::luts_from_flat(lv).unwrap(),
    };
    edit.validate().map_err(PyValueError::new_err)?;
    let mut buf = vec![0u8; src.len()];
    let hs = py.detach(|| fe::edit::apply_ex(src, &mut buf, w, 3, &edit, hist));
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
    m.add_function(wrap_pyfunction!(apply_edit, m)?)?;
    m.add_function(wrap_pyfunction!(luma01, m)?)?;
    m.add_function(wrap_pyfunction!(md_base, m)?)?;
    Ok(())
}
