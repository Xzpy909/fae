//! The stage parameters and the resize step (wave B2), as fastedit/config.py `StageParams` and
//! fastedit/geometry.py (`mpix_size`, `preset_size`, `fit_target`, `fit_scale`, `fit_image`,
//! `resize_cover_crop`): Pillow's Lanczos (resample.rs), Python's float and rounding rules, so sizes and
//! pixels are identical.

use crate::px::{Px, View};
use crate::resample::{self, Filter};
use serde_json::Value;

pub const TARGET_W: usize = 1280;
pub const TARGET_H: usize = 720;

/// What changes the image handed to the live preview (config.py StageParams; the same names and
/// defaults). Colour balance fields are read here too; their step arrives in B3.
#[derive(Clone, Debug, PartialEq)]
pub struct StageParams {
    pub resize_on: bool,
    pub mode: String,
    pub mpix: f64,
    pub prio: String,
    pub preset_ar: String,
    pub preset_res: i64,
    pub cw: i64,
    pub ch: i64,
    pub cmode: String,
    pub wp_on: bool,
    pub wp_pct: f64,
    pub wp_strength: f64,
    pub ge_on: bool,
    pub ge_p: i64,
    pub ge_sigma: f64,
    pub ge_strength: f64,
    pub rot_q: i64,
    pub angle: f64,
    pub flip: bool,
    pub crop_x: f64,
    pub crop_y: f64,
    pub crop_w: f64,
    pub crop_h: f64,
    pub crop_free: bool,
}

impl Default for StageParams {
    fn default() -> Self {
        StageParams {
            resize_on: false,
            mode: "preset".into(),
            mpix: 2.0,
            prio: "Balanced".into(),
            preset_ar: "16:9".into(),
            preset_res: 720,
            cw: 1920,
            ch: 1080,
            cmode: "crop".into(),
            wp_on: false,
            wp_pct: 99.0,
            wp_strength: 1.0,
            ge_on: false,
            ge_p: 1,
            ge_sigma: 2.0,
            ge_strength: 1.0,
            rot_q: 0,
            angle: 0.0,
            flip: false,
            crop_x: 0.0,
            crop_y: 0.0,
            crop_w: 0.0,
            crop_h: 0.0,
            crop_free: false,
        }
    }
}

pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Null => false,
    }
}

pub(crate) fn as_float(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(*b as i64 as f64),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn as_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl StageParams {
    /// StageParams.from_dict: missing or null fields keep their defaults, values converted to the field's
    /// type (int() truncates); a value that can't be converted keeps the default.
    pub fn from_json(v: &Value) -> StageParams {
        let mut p = StageParams::default();
        let Some(o) = v.as_object() else { return p };
        let get = |k: &str| o.get(k).filter(|v| !v.is_null());
        macro_rules! set {
            (bool $($f:ident)*) => { $( if let Some(v) = get(stringify!($f)) { p.$f = truthy(v); } )* };
            (f64 $($f:ident)*) => { $( if let Some(x) = get(stringify!($f)).and_then(as_float) { p.$f = x; } )* };
            (i64 $($f:ident)*) => { $( if let Some(x) = get(stringify!($f)).and_then(as_int) { p.$f = x; } )* };
            (str $($f:ident)*) => { $( if let Some(v) = get(stringify!($f)) { p.$f = as_str(v); } )* };
        }
        set!(bool resize_on wp_on ge_on flip crop_free);
        set!(f64 mpix wp_pct wp_strength ge_sigma ge_strength angle crop_x crop_y crop_w crop_h);
        set!(i64 preset_res cw ch ge_p rot_q);
        set!(str mode prio preset_ar cmode);
        p
    }

    pub fn has_geom(&self) -> bool {
        self.rot_q.rem_euclid(4) != 0 || self.angle != 0.0 || self.flip || self.crop_w > 0.0
    }
}

/// Python's round() to an integer (half to even).
pub fn pyround(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// Python's round(x, 6): the nearest double to the correctly rounded 6-decimal value.
fn round6(x: f64) -> f64 {
    format!("{x:.6}").parse().unwrap_or(x)
}

/// geometry.py mpix_size: the size closest to `mpix` megapixels, by the priority's error order.
pub fn mpix_size(w: usize, h: usize, mpix: f64, priority: &str) -> (usize, usize) {
    let target = mpix * 1_000_000.0;
    let scale = (target / (w * h) as f64).sqrt();
    let ideal_w = w as f64 * scale;
    let ratio = w as f64 / h as f64;
    let (lo, hi) = if priority == "Ratio error priority" {
        (((ideal_w * 0.99) as i64 - 3).max(1), (ideal_w * 1.01) as i64 + 5)
    } else {
        ((ideal_w as i64 - 3).max(1), ideal_w as i64 + 5)
    };
    let mut best: Option<((f64, f64), i64, i64)> = None;
    for cw in lo..hi {
        let ch = pyround(cw as f64 / ratio).max(1);
        let ratio_err = round6((cw as f64 / ch as f64 - ratio).abs() / ratio);
        let px_err = ((cw * ch) as f64 - target).abs() / target;
        let key = if priority == "Mpix priority" { (px_err, ratio_err) } else { (ratio_err, px_err) };
        if best.as_ref().is_none_or(|b| key < b.0) {
            best = Some((key, cw, ch));
        }
    }
    match best {
        Some((_, cw, ch)) => (cw as usize, ch as usize),
        None => (w, h),
    }
}

/// geometry.py preset_size: "16:9", 1080 → (1920, 1080); the p-number is the short side, the long side
/// rounded to even.
pub fn preset_size(ar: &str, res: i64) -> (usize, usize) {
    let parts: Vec<&str> = ar.split(':').collect();
    let mut r = 16.0 / 9.0;
    if parts.len() == 2 {
        if let (Ok(a), Ok(b)) = (parts[0].trim().parse::<f64>(), parts[1].trim().parse::<f64>()) {
            if a > 0.0 && b > 0.0 && (a / b).is_finite() {
                r = a / b;
            }
        }
    }
    let res = res.max(16);
    let even = |x: f64| (pyround(x / 2.0) * 2).max(2) as usize;
    if r >= 1.0 {
        (even(res as f64 * r), res as usize)
    } else {
        (res as usize, even(res as f64 / r))
    }
}

/// geometry.py fit_target: (w, h, stretch?) for preset / custom modes; None for Mpix or resize off.
pub fn fit_target(p: &StageParams) -> Option<(usize, usize, bool)> {
    if !p.resize_on || p.mode == "Mpix" {
        return None;
    }
    if p.mode == "custom" {
        let w = p.cw.clamp(1, 32768) as usize;
        let h = p.ch.clamp(1, 32768) as usize;
        return Some((w, h, p.cmode == "stretch"));
    }
    if p.mode == "720p" {
        return Some((TARGET_W, TARGET_H, false));
    }
    let (w, h) = preset_size(&p.preset_ar, p.preset_res);
    Some((w, h, false))
}

/// geometry.py fit_scale: the largest per-axis factor the resize applies (> 1: upscaling).
pub fn fit_scale(w: usize, h: usize, p: &StageParams) -> f64 {
    if !p.resize_on {
        return 1.0;
    }
    let (tw, th) = if p.mode == "Mpix" {
        mpix_size(w, h, p.mpix, &p.prio)
    } else {
        let t = fit_target(p).unwrap();
        (t.0, t.1)
    };
    (tw as f64 / w as f64).max(th as f64 / h as f64)
}

/// geometry.py resize_cover_crop: scale to cover tw × th (Lanczos), then crop the middle.
pub fn resize_cover_crop(img: View, tw: usize, th: usize) -> Px {
    let (w, h) = (img.w as f64, img.h as f64);
    let scale = (tw as f64 / w).max(th as f64 / h);
    let nw = (pyround(w * scale) as usize).max(tw);
    let nh = (pyround(h * scale) as usize).max(th);
    let r;
    let src = if (nw, nh) != (img.w, img.h) {
        r = resample::resize(img, (nw, nh), Filter::Lanczos, None, None);
        r.view()
    } else {
        img
    };
    let left = ((nw - tw) / 2) as i64;
    let top = ((nh - th) / 2) as i64;
    resample::crop(src, left, top, left + tw as i64, top + th as i64)
}

/// geometry.py fit_image: the resize step (after the geometry).
pub fn fit_image(img: View, p: &StageParams) -> Px {
    if !p.resize_on {
        return img.to_px();
    }
    if p.mode == "Mpix" {
        let (nw, nh) = mpix_size(img.w, img.h, p.mpix, &p.prio);
        if (nw, nh) == (img.w, img.h) {
            return img.to_px();
        }
        return resample::resize(img, (nw, nh), Filter::Lanczos, None, None);
    }
    let (tw, th, stretch) = fit_target(p).unwrap();
    if stretch {
        if (img.w, img.h) == (tw, th) {
            return img.to_px();
        }
        return resample::resize(img, (tw, th), Filter::Lanczos, None, None);
    }
    resize_cover_crop(img, tw, th)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sizes() {
        assert_eq!(preset_size("16:9", 1080), (1920, 1080));
        assert_eq!(preset_size("9:16", 300), (300, 534));
        assert_eq!(preset_size("bad", 720), (1280, 720));
        assert_eq!(preset_size("0:5", 720), (1280, 720));
        assert_eq!(round6(0.12345650000000001), 0.123457);
    }

    #[test]
    fn params() {
        let p = StageParams::from_json(&json!({"rot_q": 1.9, "flip": 1, "mode": "Mpix", "angle": null, "cw": "300"}));
        assert_eq!((p.rot_q, p.flip, p.mode.as_str(), p.angle, p.cw), (1, true, "Mpix", 0.0, 300));
    }
}
