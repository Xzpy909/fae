//! Encoders and saving (B4; output.py, jpeg.py, avif.py): PNG and WebP as Pillow writes them, JPEG through
//! cjpegli (or libjpeg-turbo as Pillow when it's missing), AVIF through SvtAv1EncApp + the muxer
//! (avif.rs). The UI's encoder options are normalised exactly as the Python did, and come back as the
//! "settings used" of the encoded compare.

use crate::avif;
use crate::px::View;
use crate::run::{Runner, TempDir};
use crate::{Error, ErrorKind};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const FORMATS: [&str; 4] = ["WEBP", "PNG", "JPEG", "AVIF"];
const JPEG_Q_DEFAULT: f64 = 90.0;
const JPEG_D_DEFAULT: f64 = 1.0;
const AVIF_CRF_DEFAULT: f64 = 25.0;
const AVIF_PRESET_DEFAULT: i64 = 4;
const AOM_Q_DEFAULT: i64 = 80;
const AOM_SPEED_DEFAULT: i64 = 6;
const WEBP_Q_DEFAULT: i64 = 90;

fn failed(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::Failed, msg)
}

fn io_err(p: &Path, e: std::io::Error) -> Error {
    Error::new(ErrorKind::Io, format!("{}: {e}", p.display()))
}

/// The external encoders: `encoders/cjpegli(.exe)`, `encoders/SvtAv1EncApp(.exe)` next to the app.
#[derive(Clone, Debug, Default)]
pub struct Encoders {
    pub cjpegli: Option<PathBuf>,
    pub svt: Option<PathBuf>,
}

impl Encoders {
    pub fn in_dir(dir: Option<&Path>) -> Encoders {
        let exe = if cfg!(windows) { ".exe" } else { "" };
        Encoders {
            cjpegli: dir.map(|d| d.join(format!("cjpegli{exe}"))),
            svt: dir.map(|d| d.join(format!("SvtAv1EncApp{exe}"))),
        }
    }

    fn usable(p: &Option<PathBuf>) -> Option<&Path> {
        p.as_deref().filter(|p| p.is_file())
    }

    pub fn jpegli(&self) -> Option<&Path> {
        Self::usable(&self.cjpegli)
    }

    pub fn svt(&self) -> Option<&Path> {
        Self::usable(&self.svt)
    }
}

// ---------------------------------------------------------------------------------------------------
// The UI's options, normalised as the Python did (float(), round() half to even, min / max with NaN)
// ---------------------------------------------------------------------------------------------------

/// Python's float(v): numbers, booleans, numeric strings; None for anything else (TypeError / ValueError).
fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(*b as u8 as f64),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Python's max(lo, v) / min(hi, v): the first argument wins unless the second compares greater / less.
fn py_clamp(v: f64, lo: f64, hi: f64) -> f64 {
    let v = if v > lo { v } else { lo };
    if v < hi {
        v
    } else {
        hi
    }
}

/// avif.py `_clampi`: int(min(hi, max(lo, round(float(v))))), the default when v isn't a number.
fn clampi(v: Option<&Value>, lo: i64, hi: i64, default: i64) -> Result<i64, Error> {
    let Some(f) = v.and_then(py_float) else { return Ok(default) };
    if f.is_nan() {
        return Ok(default);
    }
    if f.is_infinite() {
        return Err(Error::new(ErrorKind::BadArgs, "cannot convert float infinity to integer"));
    }
    Ok(py_clamp(f.round_ties_even(), lo as f64, hi as f64) as i64)
}

/// Python's round(x, 3) (correctly rounded, ties to even on the exact value).
fn round3(x: f64) -> f64 {
    format!("{x:.3}").parse().unwrap_or(x)
}

fn obj(o: Option<&Value>) -> Map<String, Value> {
    match o {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    }
}

fn need_float(o: &Map<String, Value>, key: &str, default: f64) -> Result<f64, Error> {
    match o.get(key) {
        None => Ok(default),
        Some(v) => py_float(v).ok_or_else(|| Error::new(ErrorKind::BadArgs, format!("jpeg {key}: not a number: {v}"))),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JpegOpts {
    pub distance: bool,
    pub q: i64,
    pub d: f64,
}

impl JpegOpts {
    /// jpeg.py `jpeg_opts`: {"mode": "q" | "d", "q": 1..100, "d": 0..25}; a bare value is the quality.
    pub fn from(o: Option<&Value>) -> Result<JpegOpts, Error> {
        let o = match o {
            Some(Value::Object(m)) => m.clone(),
            None | Some(Value::Null) => Map::new(),
            Some(v) => Map::from_iter([("q".to_string(), v.clone())]),
        };
        let distance = match o.get("mode") {
            None => false,
            Some(Value::String(s)) => s.to_lowercase().starts_with('d'),
            Some(_) => false,
        };
        let q = need_float(&o, "q", JPEG_Q_DEFAULT)?;
        let d = need_float(&o, "d", JPEG_D_DEFAULT)?;
        let q = py_clamp(q, 1.0, 100.0);
        if q.is_infinite() {
            return Err(Error::new(ErrorKind::BadArgs, "jpeg q: infinity"));
        }
        Ok(JpegOpts { distance, q: q.round_ties_even() as i64, d: round3(py_clamp(d, 0.0, 25.0)) })
    }

    pub fn to_json(&self) -> Value {
        json!({"mode": if self.distance { "d" } else { "q" }, "q": self.q, "d": self.d})
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WebpOpts {
    pub lossless: bool,
    pub q: i64,
}

impl WebpOpts {
    /// output.py `webp_opts`: lossless unless "lossless" is false; quality 0..100.
    pub fn from(o: Option<&Value>) -> Result<WebpOpts, Error> {
        let o = obj(o);
        Ok(WebpOpts { lossless: o.get("lossless") != Some(&Value::Bool(false)), q: clampi(o.get("q"), 0, 100, WEBP_Q_DEFAULT)? })
    }

    pub fn to_json(&self) -> Value {
        json!({"lossless": self.lossless, "q": self.q})
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AvifOpts {
    /// the UI's choice: SVT-AV1 (falls back when it can't be used) or the other encoder
    pub svt: bool,
    /// 1..70 in 0.25 steps
    pub crf: f64,
    pub preset: i64,
    pub aom_q: i64,
    pub aom_speed: i64,
}

impl AvifOpts {
    /// avif.py `avif_opts`.
    pub fn from(o: Option<&Value>) -> Result<AvifOpts, Error> {
        let o = obj(o);
        let crf = match o.get("crf") {
            None => AVIF_CRF_DEFAULT,
            Some(v) => py_float(v).unwrap_or(AVIF_CRF_DEFAULT),
        };
        let crf = (py_clamp(crf, 1.0, 70.0) * 4.0).round_ties_even() / 4.0;
        Ok(AvifOpts {
            svt: o.get("enc") != Some(&Value::String("aom".into())),
            crf,
            preset: clampi(o.get("preset"), 0, 8, AVIF_PRESET_DEFAULT)?,
            aom_q: clampi(o.get("aom_q"), 0, 100, AOM_Q_DEFAULT)?,
            aom_speed: clampi(o.get("aom_speed"), 0, 10, AOM_SPEED_DEFAULT)?,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({"enc": if self.svt { "svt" } else { "aom" }, "crf": self.crf, "preset": self.preset,
               "aom_q": self.aom_q, "aom_speed": self.aom_speed})
    }
}

/// output.py `norm_fmt`: one of FORMATS, WEBP for anything else.
pub fn norm_fmt(fmt: &str) -> &'static str {
    let f = fmt.to_uppercase();
    FORMATS.iter().find(|x| **x == f).copied().unwrap_or("WEBP")
}

pub fn ext(fmt: &str) -> &'static str {
    match norm_fmt(fmt) {
        "PNG" => ".png",
        "JPEG" => ".jpg",
        "AVIF" => ".avif",
        _ => ".webp",
    }
}

fn sub<'a>(opts: &'a Value, key: &str) -> Option<&'a Value> {
    opts.as_object().and_then(|m| m.get(key))
}

/// output.py `is_lossy`: JPEG, AVIF, and WebP when not lossless.
pub fn is_lossy(fmt: &str, opts: &Value) -> Result<bool, Error> {
    Ok(match norm_fmt(fmt) {
        "JPEG" | "AVIF" => true,
        "WEBP" => !WebpOpts::from(sub(opts, "webp"))?.lossless,
        _ => false,
    })
}

/// The settings an encode of `fmt` would use (the encoded compare's "opts"): JPEG, AVIF, WebP.
pub fn normalised(fmt: &str, opts: &Value) -> Result<Value, Error> {
    Ok(match norm_fmt(fmt) {
        "JPEG" => JpegOpts::from(sub(opts, "jpeg"))?.to_json(),
        "AVIF" => AvifOpts::from(sub(opts, "avif"))?.to_json(),
        _ => WebpOpts::from(sub(opts, "webp"))?.to_json(),
    })
}

/// %g of a value with at most a few decimals (cjpegli's distance, SVT's crf): "1", "0.5", "25.25".
fn fmt_g(x: f64) -> String {
    format!("{x}")
}

// ---------------------------------------------------------------------------------------------------
// PNG: Pillow's writer (its filter choice, zlib level 9, Z_FILTERED, IDAT chunks of its buffer size)
// ---------------------------------------------------------------------------------------------------

fn chunk(out: &mut Vec<u8>, typ: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(typ);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(d: &[u8]) -> u32 {
    static T: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = T.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, e) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    !d.iter().fold(!0u32, |c, &b| t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

/// zlib (zlib-rs, the zlib-ng algorithms Pillow's wheels use): `data` in pieces, as Pillow feeds rows.
fn deflate<'a>(pieces: impl Iterator<Item = &'a [u8]>, level: i32, strategy: i32, mem_level: i32) -> Result<Vec<u8>, Error> {
    use libz_rs_sys as z;
    let mut s: z::z_stream = unsafe { std::mem::zeroed() };
    let ver = z::zlibVersion();
    let r = unsafe { z::deflateInit2_(&mut s, level, z::Z_DEFLATED, 15, mem_level, strategy, ver, std::mem::size_of::<z::z_stream>() as i32) };
    if r != z::Z_OK {
        return Err(failed(format!("zlib: init {r}")));
    }
    let mut out: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut run = |s: &mut z::z_stream, input: &[u8], flush: i32| -> Result<(), Error> {
        s.next_in = input.as_ptr();
        s.avail_in = input.len() as _;
        loop {
            s.next_out = buf.as_mut_ptr();
            s.avail_out = buf.len() as _;
            let r = unsafe { z::deflate(s, flush) };
            if r < 0 && r != z::Z_BUF_ERROR {
                return Err(failed(format!("zlib: deflate {r}")));
            }
            let n = buf.len() - s.avail_out as usize;
            out.extend_from_slice(&buf[..n]);
            if flush == z::Z_FINISH {
                if r == z::Z_STREAM_END {
                    return Ok(());
                }
            } else if s.avail_in == 0 && s.avail_out != 0 {
                return Ok(());
            }
        }
    };
    let mut res = Ok(());
    for p in pieces {
        res = run(&mut s, p, z::Z_NO_FLUSH);
        if res.is_err() {
            break;
        }
    }
    if res.is_ok() {
        res = run(&mut s, &[], z::Z_FINISH);
    }
    unsafe { z::deflateEnd(&mut s) };
    res.map(|_| out)
}

/// One row filtered as Pillow's ZipEncode picks it (least sum of |signed bytes|, libpng's heuristic):
/// none, up, sub ("prior"), average (with `optimize`), paeth, in that order, ties keep the earlier.
fn png_filter_row(row: &[u8], prev: &[u8], bpp: usize, optimize: bool, out: &mut Vec<u8>, tmp: &mut [Vec<u8>; 4]) {
    let cost = |v: u8| -> u32 { if v < 128 { v as u32 } else { 256 - v as u32 } };
    let mut best: Option<usize> = None; // index into tmp; None = no filter
    let mut sum: u32 = row.iter().map(|&v| cost(v)).sum();
    let n = row.len();
    // up
    if sum > 0 {
        let t = &mut tmp[0];
        let mut s = 0;
        for i in 0..n {
            t[i] = row[i].wrapping_sub(prev[i]);
            s += cost(t[i]);
        }
        if s < sum {
            best = Some(0);
            sum = s;
        }
    }
    // sub
    if sum > 0 {
        let t = &mut tmp[1];
        let mut s = 0;
        for i in 0..n {
            t[i] = if i < bpp { row[i] } else { row[i].wrapping_sub(row[i - bpp]) };
            s += cost(t[i]);
        }
        if s < sum {
            best = Some(1);
            sum = s;
        }
    }
    // average
    if optimize && sum > 0 {
        let t = &mut tmp[2];
        let mut s = 0;
        for i in 0..n {
            let a = if i < bpp { 0 } else { row[i - bpp] as u32 };
            t[i] = row[i].wrapping_sub(((a + prev[i] as u32) / 2) as u8);
            s += cost(t[i]);
        }
        if s < sum {
            best = Some(2);
            sum = s;
        }
    }
    // paeth
    if sum > 0 {
        let t = &mut tmp[3];
        let mut s = 0;
        for i in 0..n {
            let (a, b, c) = if i < bpp { (0, prev[i] as i32, 0) } else { (row[i - bpp] as i32, prev[i] as i32, prev[i - bpp] as i32) };
            let (pa, pb, pc) = ((b - c).abs(), (a - c).abs(), (a + b - 2 * c).abs());
            let p = if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c };
            t[i] = row[i].wrapping_sub(p as u8);
            s += cost(t[i]);
        }
        if s < sum {
            best = Some(3);
        }
    }
    let (ftype, data): (u8, &[u8]) = match best {
        None => (0, row),
        Some(0) => (2, &tmp[0]),
        Some(1) => (1, &tmp[1]),
        Some(2) => (3, &tmp[2]),
        _ => (4, &tmp[3]),
    };
    out.push(ftype);
    out.extend_from_slice(data);
}

/// PNG as Pillow's save(..., "PNG", compress_level=9, optimize=True[, icc_profile]): RGB / RGBA 8-bit.
pub fn encode_png(img: View, icc: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    let (w, h, c) = (img.w, img.h, img.c);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = (w as u32).to_be_bytes().to_vec();
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if c == 4 { 6 } else { 2 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    if let Some(icc) = icc.filter(|i| !i.is_empty()) {
        // zlib.compress(icc): default level and strategy
        let mut d = b"ICC Profile\0\0".to_vec();
        d.extend(deflate(std::iter::once(icc), 6, 0, 8)?);
        chunk(&mut out, b"iCCP", &d);
    }
    // the filtered rows (built in parallel; Pillow filters each row against the unfiltered one above)
    use rayon::prelude::*;
    let n = w * c;
    let zero = vec![0u8; n];
    let rows: Vec<Vec<u8>> = (0..h)
        .into_par_iter()
        .chunks(64)
        .map(|ys| {
            let mut tmp = [vec![0u8; n], vec![0u8; n], vec![0u8; n], vec![0u8; n]];
            let mut o = Vec::with_capacity(ys.len() * (n + 1));
            for y in ys {
                let prev = if y == 0 { &zero[..] } else { img.row(y - 1) };
                png_filter_row(img.row(y), prev, c, true, &mut o, &mut tmp);
            }
            o
        })
        .collect();
    // Z_FILTERED, memLevel 9, one row at a time (as ZipEncode.c)
    let z = deflate(rows.iter().flat_map(|b| b.chunks(n + 1)), 9, 1, 9)?;
    let bufsize = (w * 4).max(65536); // ImageFile._save's buffer: one IDAT per full buffer
    for part in z.chunks(bufsize) {
        chunk(&mut out, b"IDAT", part);
    }
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------
// WebP: libwebp as Pillow's _webp.WebPEncode calls it (ARGB picture, then always through WebPMux)
// ---------------------------------------------------------------------------------------------------

/// WebP as Pillow writes it: `quality`, `method`, `exact`; lossless or lossy (alpha_quality 100).
pub fn encode_webp(img: View, lossless: bool, quality: f32, method: i32, exact: bool, icc: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    use libwebp_sys as wp;
    unsafe {
        let mut config: wp::WebPConfig = std::mem::zeroed();
        if wp::WebPConfigInitInternal(&mut config, wp::WebPPreset::WEBP_PRESET_DEFAULT, 75.0, wp::WEBP_ENCODER_ABI_VERSION as i32) == 0 {
            return Err(failed("webp: config"));
        }
        config.lossless = lossless as i32;
        config.quality = quality;
        config.alpha_quality = 100;
        config.method = method;
        config.exact = exact as i32;
        if wp::WebPValidateConfig(&config) == 0 {
            return Err(failed("webp: invalid configuration"));
        }
        let mut pic: wp::WebPPicture = std::mem::zeroed();
        if wp::WebPPictureInitInternal(&mut pic, wp::WEBP_ENCODER_ABI_VERSION as i32) == 0 {
            return Err(failed("webp: picture"));
        }
        pic.width = img.w as i32;
        pic.height = img.h as i32;
        pic.use_argb = 1;
        if wp::WebPPictureAlloc(&mut pic) == 0 {
            return Err(failed(format!("webp: can't allocate a {} x {} picture", img.w, img.h)));
        }
        let c = img.c;
        for y in 0..img.h {
            let dst = std::slice::from_raw_parts_mut(pic.argb.add(pic.argb_stride as usize * y), img.w);
            for (d, p) in dst.iter_mut().zip(img.row(y).chunks_exact(c)) {
                let a = if c == 4 { p[3] as u32 } else { 0xFF };
                *d = (p[2] as u32) | ((p[1] as u32) << 8) | ((p[0] as u32) << 16) | (a << 24);
            }
        }
        let mut writer: wp::WebPMemoryWriter = std::mem::zeroed();
        wp::WebPMemoryWriterInit(&mut writer);
        pic.writer = Some(wp::WebPMemoryWrite);
        pic.custom_ptr = &mut writer as *mut _ as *mut std::ffi::c_void;
        let ok = wp::WebPEncode(&config, &mut pic);
        let code = pic.error_code;
        wp::WebPPictureFree(&mut pic);
        if ok == 0 {
            wp::WebPMemoryWriterClear(&mut writer);
            return Err(failed(format!("webp: encoding error {}", code as i32)));
        }
        let image = wp::WebPData { bytes: writer.mem, size: writer.size };
        let mux = wp::WebPMuxNew();
        if mux.is_null() {
            wp::WebPMemoryWriterClear(&mut writer);
            return Err(failed("webp: could not create mux object"));
        }
        wp::WebPMuxSetImage(mux, &image, 0);
        let mut err = None;
        if let Some(icc) = icc.filter(|i| !i.is_empty()) {
            let d = wp::WebPData { bytes: icc.as_ptr(), size: icc.len() };
            let r = wp::WebPMuxSetChunk(mux, c"ICCP".as_ptr(), &d, 0);
            if r != wp::WebPMuxError::WEBP_MUX_OK {
                err = Some(format!("webp: ICCP chunk: {}", r as i32));
            }
        }
        let mut out = wp::WebPData { bytes: std::ptr::null(), size: 0 };
        if err.is_none() {
            wp::WebPMuxAssemble(mux, &mut out);
        }
        wp::WebPMuxDelete(mux);
        wp::WebPMemoryWriterClear(&mut writer);
        if let Some(e) = err {
            return Err(failed(e));
        }
        if out.size == 0 || out.bytes.is_null() {
            return Err(failed("cannot write file as WebP (encoder returned None)"));
        }
        let v = std::slice::from_raw_parts(out.bytes, out.size).to_vec();
        wp::WebPFree(out.bytes as *mut std::ffi::c_void);
        Ok(v)
    }
}

// ---------------------------------------------------------------------------------------------------
// JPEG: cjpegli, or libjpeg-turbo as Pillow's save(quality=q, optimize=True, progressive=True)
// ---------------------------------------------------------------------------------------------------

/// jpeg.py `flatten_rgb`: transparent images onto white (Pillow's paste with the alpha as mask).
pub fn flatten_rgb(img: View) -> Vec<u8> {
    if img.c == 3 {
        return img.data.to_vec();
    }
    let mut v = Vec::with_capacity(img.w * img.h * 3);
    for p in img.data.chunks_exact(4) {
        let a = p[3] as u32;
        for &ch in &p[..3] {
            let t = 255 * (255 - a) + ch as u32 * a + 128; // Pillow's BLEND + DIV255
            v.push((((t >> 8) + t) >> 8) as u8);
        }
    }
    v
}

/// libjpeg-turbo's encoder (mozjpeg built with its libjpeg-turbo defaults), set up as Pillow does.
pub fn encode_jpeg_turbo(rgb: &[u8], w: usize, h: usize, quality: i32, icc: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    use mozjpeg_sys as j;
    unsafe extern "C-unwind" fn error_exit(c: &mut j::jpeg_common_struct) {
        let buf = [0u8; 80];
        if let Some(f) = (*c.err).format_message {
            f(c, &buf);
        }
        let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        std::panic::panic_any(String::from_utf8_lossy(&buf[..n]).into_owned());
    }
    unsafe extern "C-unwind" fn emit_message(_c: &mut j::jpeg_common_struct, _lvl: std::os::raw::c_int) {}
    unsafe extern "C-unwind" fn output_message(_c: &mut j::jpeg_common_struct) {}

    let mut err: Box<j::jpeg_error_mgr> = Box::new(unsafe { std::mem::zeroed() });
    let mut ci: Box<j::jpeg_compress_struct> = Box::new(unsafe { std::mem::zeroed() });
    let mut outbuf: *mut u8 = std::ptr::null_mut();
    let mut outsize: std::os::raw::c_ulong = 0;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        j::jpeg_std_error(&mut err);
        err.error_exit = Some(error_exit);
        err.emit_message = Some(emit_message);
        err.output_message = Some(output_message);
        ci.common.err = &mut *err;
        j::jpeg_create_compress(&mut *ci);
        j::jpeg_mem_dest(&mut ci, &mut outbuf, &mut outsize);
        ci.image_width = w as u32;
        ci.image_height = h as u32;
        ci.input_components = 3;
        ci.in_color_space = j::J_COLOR_SPACE::JCS_RGB;
        // Pillow's wheels use libjpeg-turbo: its defaults, not mozjpeg's
        j::jpeg_c_set_int_param(&mut ci, j::J_INT_PARAM::JINT_COMPRESS_PROFILE, j::JINT_COMPRESS_PROFILE_VALUE::JCP_FASTEST as i32);
        j::jpeg_set_defaults(&mut ci);
        j::jpeg_set_quality(&mut ci, quality, 1);
        j::jpeg_simple_progression(&mut ci);
        ci.smoothing_factor = 0;
        ci.optimize_coding = 1;
        j::jpeg_start_compress(&mut ci, 1);
        if let Some(icc) = icc.filter(|i| !i.is_empty()) {
            // Pillow's "extra": APP2 ICC_PROFILE markers, numbered from 1
            let parts: Vec<&[u8]> = icc.chunks(65533 - 14).collect();
            for (i, p) in parts.iter().enumerate() {
                let mut m = b"ICC_PROFILE\0".to_vec();
                m.extend_from_slice(&[(i + 1) as u8, parts.len() as u8]);
                m.extend_from_slice(p);
                j::jpeg_write_marker(&mut ci, 0xE2, m.as_ptr(), m.len() as u32);
            }
        }
        let stride = w * 3;
        for y in 0..h {
            let mut row = rgb[y * stride..].as_ptr() as *mut u8;
            j::jpeg_write_scanlines(&mut ci, &mut row as *mut *mut u8 as _, 1);
        }
        j::jpeg_finish_compress(&mut ci);
    }));
    let data = if !outbuf.is_null() && r.is_ok() {
        Some(unsafe { std::slice::from_raw_parts(outbuf, outsize as usize) }.to_vec())
    } else {
        None
    };
    unsafe {
        j::jpeg_destroy_compress(&mut ci);
        if !outbuf.is_null() {
            libc_free(outbuf as *mut std::ffi::c_void);
        }
    }
    match r {
        Ok(()) => data.ok_or_else(|| failed("jpeg: no output")),
        Err(e) => Err(failed(format!("jpeg: {}", e.downcast_ref::<String>().cloned().unwrap_or_default()))),
    }
}

extern "C" {
    #[link_name = "free"]
    fn libc_free(p: *mut std::ffi::c_void);
}

/// jpeg.py `encode_jpeg`: (bytes, encoder) — "jpegli" through cjpegli, else "pillow" (libjpeg-turbo, which
/// has no distance: distance mode uses the quality).
pub fn encode_jpeg(img: View, o: &JpegOpts, icc: Option<&[u8]>, enc: &Encoders, run: &Runner, req: Option<i64>) -> Result<(Vec<u8>, &'static str), Error> {
    let rgb = flatten_rgb(img);
    let Some(exe) = enc.jpegli() else {
        return Ok((encode_jpeg_turbo(&rgb, img.w, img.h, o.q as i32, icc)?, "pillow"));
    };
    let td = TempDir::new("fie_jpeg_")?;
    let (src, dst) = (td.0.join("in.png"), td.0.join("out.jpg"));
    // what cjpegli reads: the pixels and the ICC profile (a fast PNG, as jpeg.py's compress_level 1)
    let png = encode_png_fast(View::new(img.w, img.h, 3, &rgb), icc)?;
    std::fs::write(&src, png).map_err(|e| io_err(&src, e))?;
    let arg = if o.distance { format!("--distance={}", fmt_g(o.d)) } else { format!("--quality={}", o.q) };
    let cmd: Vec<OsString> = vec![exe.into(), src.into(), dst.clone().into(), arg.into()];
    let r = run.run(&cmd, req)?;
    if !r.ok() || !dst.is_file() {
        return Err(failed(format!("cjpegli failed ({}): {}", r.code_str(), r.message(300))));
    }
    Ok((std::fs::read(&dst).map_err(|e| io_err(&dst, e))?, "jpegli"))
}

/// A quickly written PNG (the encoders' input file): same pixels and ICC as `encode_png`.
fn encode_png_fast(img: View, icc: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = (img.w as u32).to_be_bytes().to_vec();
    ihdr.extend_from_slice(&(img.h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if img.c == 4 { 6 } else { 2 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    if let Some(icc) = icc.filter(|i| !i.is_empty()) {
        let mut d = b"ICC Profile\0\0".to_vec();
        d.extend(deflate(std::iter::once(icc), 6, 0, 8)?);
        chunk(&mut out, b"iCCP", &d);
    }
    let n = img.w * img.c;
    let mut raw = Vec::with_capacity(img.h * (n + 1));
    for y in 0..img.h {
        raw.push(0);
        raw.extend_from_slice(img.row(y));
    }
    let z = deflate(std::iter::once(&raw[..]), 1, 0, 8)?;
    for part in z.chunks(1 << 20) {
        chunk(&mut out, b"IDAT", part);
    }
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------
// AVIF
// ---------------------------------------------------------------------------------------------------

/// avif.py `encode_avif_svt`: 10-bit 4:4:4 YUV through SvtAv1EncApp, muxed (alpha ignored: RGB only).
pub fn encode_avif_svt(img: View, o: &AvifOpts, icc: Option<&[u8]>, exe: &Path, run: &Runner, req: Option<i64>) -> Result<Vec<u8>, Error> {
    let td = TempDir::new("fie_avif_")?;
    let (src, dst) = (td.0.join("in.yuv"), td.0.join("out.ivf"));
    std::fs::write(&src, avif::rgb_to_yuv444p10(img.data, img.c)).map_err(|e| io_err(&src, e))?;
    run.check(req)?;
    let s = |v: &str| OsString::from(v);
    let cmd: Vec<OsString> = vec![
        exe.into(), s("-i"), src.into(), s("-w"), s(&img.w.to_string()), s("-h"), s(&img.h.to_string()),
        s("--input-depth"), s("10"), s("--color-format"), s("444"), s("-n"), s("1"), s("--avif"), s("1"),
        s("--tune"), s("3"), s("--crf"), s(&fmt_g(o.crf)), s("--preset"), s(&o.preset.to_string()),
        s("--color-primaries"), s("1"), s("--transfer-characteristics"), s("13"),
        s("--matrix-coefficients"), s("1"), s("--color-range"), s("1"),
        s("--progress"), s("0"), s("-b"), dst.clone().into(),
    ];
    let r = run.run(&cmd, req)?;
    let size = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
    if !r.ok() || size == 0 {
        return Err(failed(format!("SvtAv1EncApp failed ({}): {}", r.code_str(), r.message(400))));
    }
    let data = std::fs::read(&dst).map_err(|e| io_err(&dst, e))?;
    avif::mux(avif::ivf_payload(&data)?, img.w as u32, img.h as u32, icc, (1, 13, 1, 1))
}

/// What an encode gave: the file's bytes, the encoder's name (the UI shows it), the settings used.
pub struct Encoded {
    pub data: Vec<u8>,
    pub encoder: &'static str,
    pub used: Value,
}

/// output.py `encode_lossy`: JPEG / AVIF / lossy WebP. AVIF: SvtAv1EncApp when chosen and usable (exe
/// there, no alpha), else rav1e with the "aom" settings (it replaces Pillow's aom, B4).
pub fn encode_lossy(img: View, fmt: &str, opts: &Value, icc: Option<&[u8]>, enc: &Encoders, run: &Runner, req: Option<i64>) -> Result<Encoded, Error> {
    match norm_fmt(fmt) {
        "JPEG" => {
            let o = JpegOpts::from(sub(opts, "jpeg"))?;
            let (data, encoder) = encode_jpeg(img, &o, icc, enc, run, req)?;
            Ok(Encoded { data, encoder, used: o.to_json() })
        }
        "AVIF" => {
            let o = AvifOpts::from(sub(opts, "avif"))?;
            match enc.svt() {
                Some(exe) if o.svt && img.c == 3 => {
                    Ok(Encoded { data: encode_avif_svt(img, &o, icc, exe, run, req)?, encoder: "svt", used: o.to_json() })
                }
                _ => {
                    run.check(req)?;
                    let data = avif::encode_rav1e(img.data, img.w, img.h, img.c, o.aom_q, o.aom_speed, icc)?;
                    run.check(req)?;
                    Ok(Encoded { data, encoder: "rav1e", used: o.to_json() })
                }
            }
        }
        _ => {
            let o = WebpOpts::from(sub(opts, "webp"))?;
            Ok(Encoded { data: encode_webp(img, false, o.q as f32, 6, false, icc)?, encoder: "libwebp", used: o.to_json() })
        }
    }
}

/// output.py `_out_path`: `<source without extension>_<w>x<h><ext>`, `_1`, `_2`… when taken.
pub fn out_path(src: &str, w: usize, h: usize, ext: &str) -> PathBuf {
    let base = splitext(src).0;
    let mut p = PathBuf::from(format!("{base}_{w}x{h}{ext}"));
    let mut n = 1;
    while p.exists() {
        p = PathBuf::from(format!("{base}_{w}x{h}_{n}{ext}"));
        n += 1;
    }
    p
}

/// Python's os.path.splitext: the extension is from the last dot of the file name (not a leading one).
fn splitext(p: &str) -> (&str, &str) {
    let seps: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    let name_start = p.rfind(seps).map_or(0, |i| i + 1);
    let name = &p[name_start..];
    let lead = name.len() - name.trim_start_matches('.').len();
    match name[lead..].rfind('.') {
        Some(i) => p.split_at(name_start + lead + i),
        None => (p, ""),
    }
}

/// output.py `save_image`: encode and write next to the source; the path written.
pub fn save(img: View, src: &str, fmt: &str, icc: Option<&[u8]>, opts: &Value, enc: &Encoders, run: &Runner) -> Result<PathBuf, Error> {
    let fmt = norm_fmt(fmt);
    let data = if is_lossy(fmt, opts)? {
        encode_lossy(img, fmt, opts, icc, enc, run, None)?.data
    } else if fmt == "WEBP" {
        encode_webp(img, true, 100.0, 6, true, icc)?
    } else {
        encode_png(img, icc)?
    };
    let p = out_path(src, img.w, img.h, ext(fmt));
    std::fs::write(&p, data).map_err(|e| io_err(&p, e))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        let j = |v: Value| JpegOpts::from(Some(&v)).unwrap();
        assert_eq!(j(json!({})).to_json(), json!({"mode": "q", "q": 90, "d": 1.0}));
        assert_eq!(j(json!(72.5)).q, 72);
        assert_eq!(j(json!({"mode": "Dist", "q": 0, "d": 0.1875})).to_json(), json!({"mode": "d", "q": 1, "d": 0.188}));
        assert_eq!(j(json!({"q": "300", "d": 99})).to_json(), json!({"mode": "q", "q": 100, "d": 25.0}));
        assert!(JpegOpts::from(Some(&json!({"q": null}))).is_err());
        let a = AvifOpts::from(Some(&json!({"crf": 25.13, "preset": "x", "enc": "aom", "aom_q": 2.5, "aom_speed": 99}))).unwrap();
        assert_eq!(a.to_json(), json!({"enc": "aom", "crf": 25.25, "preset": 4, "aom_q": 2, "aom_speed": 10}));
        assert_eq!(AvifOpts::from(Some(&json!({"crf": 0.1}))).unwrap().crf, 1.0);
        assert_eq!(WebpOpts::from(None).unwrap().to_json(), json!({"lossless": true, "q": 90}));
        assert_eq!(WebpOpts::from(Some(&json!({"lossless": 0, "q": 50.5}))).unwrap().to_json(), json!({"lossless": true, "q": 50}));
        assert!(is_lossy("webp", &json!({"webp": {"lossless": false}})).unwrap());
        assert!(!is_lossy("PNG", &json!(null)).unwrap());
        assert_eq!(norm_fmt("tiff"), "WEBP");
        assert_eq!((fmt_g(1.0), fmt_g(0.5), fmt_g(25.25), fmt_g(12.345)), ("1".into(), "0.5".into(), "25.25".into(), "12.345".into()));
    }

    #[test]
    fn paths() {
        assert_eq!(splitext("/a/b.c/photo.jpg"), ("/a/b.c/photo", ".jpg"));
        assert_eq!(splitext("/a/b.c/photo"), ("/a/b.c/photo", ""));
        assert_eq!(splitext("/a/.hidden"), ("/a/.hidden", ""));
        assert_eq!(splitext("/a/..x.png"), ("/a/..x", ".png"));
    }

    #[test]
    fn flatten() {
        let px = [10, 200, 255, 0, 10, 200, 255, 255, 10, 200, 255, 128];
        assert_eq!(flatten_rgb(View::new(3, 1, 4, &px)), vec![255, 255, 255, 10, 200, 255, 132, 227, 255]);
    }
}
