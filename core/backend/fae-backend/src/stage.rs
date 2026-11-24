//! The stage (wave B3): what the editor works on, as engine.py `Engine` / `Entry` and api.py
//! `load_stage` / `load_source` / `sample` did it. An opened file is kept decoded (the last 4), with its
//! geometry result for the last geometry settings and the resized results for the last few resize
//! settings; colour balance runs on top. The preview pixels (RGBA) are stored under a token the shell
//! serves at `/preview/<token>` ([`Stage::preview`]); the last stage image (full resolution) stays for
//! `sample`.

use crate::fit::{self, StageParams};
use crate::px::{Px, View};
use crate::resample::{self, Filter};
use crate::{colour, geometry, Error, ErrorKind};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// config.py PREVIEW_MAX: the long side of the live preview on the CPU core.
pub const PREVIEW_MAX: usize = 2560;
const MAX_ENTRIES: usize = 4;
const MAX_FITS: usize = 4;
/// image + Mid / Detail base per load (engine.py)
const MAX_PREVIEWS: usize = 8;

/// A decoded image as the stage starts from it: upright RGB / RGBA + its ICC profile.
#[derive(Clone)]
pub struct Source {
    pub img: Arc<Px>,
    pub icc: Option<Arc<Vec<u8>>>,
}

struct Entry {
    key: String,
    mtime: Option<SystemTime>,
    src: Source,
    geo: Option<(String, Arc<Px>)>,
    fits: Vec<(String, Arc<Px>)>,
}

impl Entry {
    fn geometried(&mut self, p: &StageParams) -> Arc<Px> {
        let key = geom_key(p);
        if let Some((k, g)) = &self.geo {
            if *k == key {
                return g.clone();
            }
        }
        let g = if p.has_geom() { Arc::new(geometry::apply(self.src.img.view(), p)) } else { self.src.img.clone() };
        self.geo = Some((key, g.clone()));
        g
    }

    fn fitted(&mut self, p: &StageParams) -> Arc<Px> {
        let key = fit_key(p);
        if let Some((_, f)) = self.fits.iter().find(|(k, _)| *k == key) {
            return f.clone();
        }
        if self.fits.len() >= MAX_FITS {
            self.fits.clear();
        }
        let g = self.geometried(p);
        let f = if p.resize_on { Arc::new(fit::fit_image(g.view(), p)) } else { g };
        self.fits.push((key, f.clone()));
        f
    }
}

/// config.py StageParams.geom_key (as a string: only compared).
fn geom_key(p: &StageParams) -> String {
    format!(
        "{}|{:.4}|{}|{:.3}|{:.3}|{:.3}|{:.3}|{}",
        p.rot_q.rem_euclid(4),
        p.angle,
        p.flip,
        p.crop_x,
        p.crop_y,
        p.crop_w,
        p.crop_h,
        p.crop_free
    )
}

/// config.py StageParams.fit_key
fn fit_key(p: &StageParams) -> String {
    let f = if p.resize_on {
        format!("{}|{}|{}|{}|{}|{}|{}|{}", p.mode, p.mpix, p.prio, p.preset_ar, p.preset_res, p.cw, p.ch, p.cmode)
    } else {
        "none".into()
    };
    format!("{}#{f}", geom_key(p))
}

/// os.path.normcase(os.path.abspath(path))
fn path_key(path: &Path) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let s = abs.to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('/', "\\").to_lowercase()
    } else {
        s
    }
}

fn mtime(path: &Path) -> Result<Option<SystemTime>, Error> {
    let m = std::fs::metadata(path).map_err(|e| Error::new(ErrorKind::Io, format!("{}: {e}", path.display())))?;
    Ok(m.modified().ok())
}

struct Current {
    rgb: Arc<Px>,
}

#[derive(Default)]
struct State {
    entries: VecDeque<Entry>,
    current: Option<Current>,
    latest_req: i64,
}

/// The stage of one back end: entries, previews, the current stage image.
pub struct Stage {
    state: Mutex<State>,
    previews: Mutex<VecDeque<(String, Arc<Vec<u8>>)>>,
    /// sources the shell decoded itself (formats not in Rust yet): path key -> (mtime, source)
    given: Mutex<Vec<(String, Option<SystemTime>, Source)>>,
}

impl Default for Stage {
    fn default() -> Self {
        Stage {
            state: Mutex::new(State { latest_req: -1, ..Default::default() }),
            previews: Mutex::new(VecDeque::new()),
            given: Mutex::new(Vec::new()),
        }
    }
}

/// api.py `_pv_size`: the preview's size. `full` (> 0: the GPU draws the preview) is the most pixels it
/// takes; else PREVIEW_MAX on the long side.
pub fn pv_size(w: usize, h: usize, full: f64) -> (usize, usize) {
    let s = if full > 0.0 {
        (full / (w * h) as f64).powf(0.5).min(1.0)
    } else {
        (PREVIEW_MAX as f64 / w.max(h) as f64).min(1.0)
    };
    if s < 1.0 {
        ((fit::pyround(w as f64 * s).max(1)) as usize, (fit::pyround(h as f64 * s).max(1)) as usize)
    } else {
        (w, h)
    }
}

/// api.py `_preview_rgba`: RGB resized at `size` (alpha dropped first, as convert("RGB")), the alpha
/// resized on its own (255 without one).
pub fn preview_rgba(img: View, size: (usize, usize), f: Filter) -> Vec<u8> {
    let (pw, ph) = size;
    let rgb = if img.c == 3 { img.to_px() } else { img.to_px().to_rgb() };
    let rgb = if (rgb.w, rgb.h) != size { resample::resize(rgb.view(), size, f, None, None) } else { rgb };
    let alpha: Option<Vec<u8>> = if img.alpha() {
        let a: Vec<u8> = img.data.chunks_exact(4).map(|p| p[3]).collect();
        if (img.w, img.h) == size {
            Some(a)
        } else {
            // Pillow resizes an "L" image with the same coefficients as each channel of an RGB one
            let mut a3 = Vec::with_capacity(a.len() * 3);
            for v in a {
                a3.extend_from_slice(&[v, v, v]);
            }
            let r = resample::resize(View::new(img.w, img.h, 3, &a3), size, f, None, None);
            Some(r.data.chunks_exact(3).map(|p| p[0]).collect())
        }
    } else {
        None
    };
    let mut out = Vec::with_capacity(pw * ph * 4);
    for (i, p) in rgb.data.chunks_exact(3).enumerate() {
        out.extend_from_slice(p);
        out.push(alpha.as_ref().map_or(255, |a| a[i]));
    }
    out
}

fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut out = String::with_capacity(32);
    for salt in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(n);
        h.write_u128(t);
        h.write_u64(salt);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

impl Stage {
    /// Store preview pixels (RGBA); the token the shell serves them under.
    pub fn put_preview(&self, rgba: Vec<u8>) -> String {
        let token = new_token();
        let mut p = self.previews.lock().unwrap();
        p.push_back((token.clone(), Arc::new(rgba)));
        while p.len() > MAX_PREVIEWS {
            p.pop_front();
        }
        token
    }

    /// The pixels stored under `token` (None: gone).
    pub fn preview(&self, token: &str) -> Option<Arc<Vec<u8>>> {
        self.previews.lock().unwrap().iter().find(|(t, _)| t == token).map(|(_, d)| d.clone())
    }

    /// The shell decoded `path` itself (a format not in Rust yet): use these pixels for it.
    pub fn give_source(&self, path: &Path, img: Px, icc: Option<Vec<u8>>) -> Result<(), Error> {
        let key = path_key(path);
        let m = mtime(path)?;
        let mut g = self.given.lock().unwrap();
        g.retain(|(k, _, _)| *k != key);
        g.push((key, m, Source { img: Arc::new(img), icc: icc.map(Arc::new) }));
        while g.len() > MAX_ENTRIES {
            g.remove(0);
        }
        Ok(())
    }

    fn with_entry<R>(
        &self,
        st: &mut State,
        path: &Path,
        decode: &dyn Fn(&Path) -> Result<Source, Error>,
        f: impl FnOnce(&mut Entry) -> R,
    ) -> Result<R, Error> {
        let key = path_key(path);
        let m = mtime(path)?;
        let pos = st.entries.iter().position(|e| e.key == key);
        let mut e = match pos {
            Some(i) => {
                let e = st.entries.remove(i).unwrap();
                if e.mtime == m {
                    Some(e)
                } else {
                    None
                }
            }
            None => None,
        };
        if e.is_none() {
            let given = {
                let g = self.given.lock().unwrap();
                g.iter().find(|(k, gm, _)| *k == key && *gm == m).map(|(_, _, s)| s.clone())
            };
            let src = match given {
                Some(s) => s,
                None => decode(path)?,
            };
            e = Some(Entry { key, mtime: m, src, geo: None, fits: Vec::new() });
        }
        let mut e = e.unwrap();
        let r = f(&mut e);
        st.entries.push_back(e);
        while st.entries.len() > MAX_ENTRIES {
            st.entries.pop_front();
        }
        Ok(r)
    }

    /// engine.py `Engine.stage`: the image the edit starts from (decode → geometry → resize → colour
    /// balance), and the entry's source.
    pub fn stage(&self, path: &Path, p: &StageParams, decode: &dyn Fn(&Path) -> Result<Source, Error>) -> Result<(Px, Source, (usize, usize)), Error> {
        let mut st = self.state.lock().unwrap();
        let (fitted, src, geo) = self.with_entry(&mut st, path, decode, |e| {
            let f = e.fitted(p);
            let g = e.geometried(p);
            (f, e.src.clone(), (g.w, g.h))
        })?;
        let out = if p.wp_on || p.ge_on { colour::apply((*fitted).clone(), p) } else { (*fitted).clone() };
        Ok((out, src, geo))
    }

    /// api.py `load_stage(path, params, req_id, full)`.
    pub fn load_stage(&self, args: &[Value], decode: &dyn Fn(&Path) -> Result<Source, Error>) -> Result<Value, Error> {
        let path = PathBuf::from(args.first().and_then(Value::as_str).unwrap_or_default());
        let p = StageParams::from_json(args.get(1).unwrap_or(&Value::Null));
        let req_id = args.get(2).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).unwrap_or(0);
        let full = args.get(3).and_then(Value::as_f64).unwrap_or(0.0);
        let (img, src, (gw, gh)) = match self.stage(&path, &p, decode) {
            Ok(r) => r,
            Err(e) if e.kind == ErrorKind::Unknown => return Err(e),
            Err(e) => return Ok(json!({"ok": false, "req_id": req_id, "error": e.message})),
        };
        let (w, h) = (img.w, img.h);
        let (pw, ph) = pv_size(w, h, full);
        let token = self.put_preview(preview_rgba(img.view(), (pw, ph), Filter::Lanczos));
        let alpha = img.alpha();
        {
            let mut st = self.state.lock().unwrap();
            if req_id >= st.latest_req {
                st.latest_req = req_id;
                st.current = Some(Current { rgb: Arc::new(img.to_rgb()) });
            }
        }
        let scale: f64 = format!("{:.4}", fit::fit_scale(gw, gh, &p)).parse().unwrap_or(1.0);
        Ok(json!({
            "ok": true, "req_id": req_id, "token": token,
            "w": w, "h": h, "pw": pw, "ph": ph, "full": full,
            "src_w": src.img.w, "src_h": src.img.h,
            "alpha": alpha,
            "scale": scale,
            "geo_w": gw, "geo_h": gh,
        }))
    }

    /// api.py `load_source(path)`: the upright, uncropped source for crop mode (downscaled for display).
    pub fn load_source(&self, args: &[Value], decode: &dyn Fn(&Path) -> Result<Source, Error>) -> Result<Value, Error> {
        let path = PathBuf::from(args.first().and_then(Value::as_str).unwrap_or_default());
        let src = {
            let mut st = self.state.lock().unwrap();
            match self.with_entry(&mut st, &path, decode, |e| e.src.clone()) {
                Ok(s) => s,
                Err(e) if e.kind == ErrorKind::Unknown => return Err(e),
                Err(e) => return Ok(json!({"ok": false, "error": e.message})),
            }
        };
        let (ww, hh) = (src.img.w, src.img.h);
        let (mut pw, mut ph) = (ww, hh);
        if ww.max(hh) > PREVIEW_MAX {
            let s = PREVIEW_MAX as f64 / ww.max(hh) as f64;
            pw = fit::pyround(ww as f64 * s).max(1) as usize;
            ph = fit::pyround(hh as f64 * s).max(1) as usize;
        }
        let token = self.put_preview(preview_rgba(src.img.view(), (pw, ph), Filter::Bilinear));
        Ok(json!({"ok": true, "token": token, "W": ww, "H": hh, "pw": pw, "ph": ph, "alpha": src.img.alpha()}))
    }

    /// api.py `sample(nx, ny)`: the 3 × 3 median of the full-resolution stage image (for point
    /// correction); null before a stage was loaded.
    pub fn sample(&self, args: &[Value]) -> Value {
        let nx = args.first().and_then(Value::as_f64).unwrap_or(0.0);
        let ny = args.get(1).and_then(Value::as_f64).unwrap_or(0.0);
        let rgb = match &self.state.lock().unwrap().current {
            Some(c) => c.rgb.clone(),
            None => return Value::Null,
        };
        let (w, h) = (rgb.w, rgb.h);
        let x = (nx * w as f64).max(0.0).min((w - 1) as f64) as usize;
        let y = (ny * h as f64).max(0.0).min((h - 1) as f64) as usize;
        let mut out = vec![];
        for ch in 0..3 {
            let mut v: Vec<u8> = vec![];
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    v.push(rgb.data[(yy * w + xx) * 3 + ch]);
                }
            }
            v.sort_unstable();
            let n = v.len();
            // np.median: the mean of the two middle values for an even count; int() truncates
            let m = if n % 2 == 1 { v[n / 2] as f64 } else { (v[n / 2 - 1] as f64 + v[n / 2] as f64) / 2.0 };
            out.push(json!(m as i64));
        }
        Value::Array(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(pv_size(4000, 3000, 0.0), (2560, 1920));
        assert_eq!(pv_size(800, 600, 0.0), (800, 600));
        assert_eq!(pv_size(4000, 3000, 3e6), (2000, 1500));
        assert_eq!(pv_size(4000, 3000, 2e7), (4000, 3000));
    }

    #[test]
    fn preview_pixels() {
        let mut p = Px::new(4, 2, 4);
        for (i, v) in p.data.iter_mut().enumerate() {
            *v = (i * 9) as u8;
        }
        let r = preview_rgba(p.view(), (4, 2), Filter::Lanczos);
        assert_eq!(r, p.data);
        let q = Px::from_vec(2, 1, 3, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(preview_rgba(q.view(), (2, 1), Filter::Lanczos), vec![1, 2, 3, 255, 4, 5, 6, 255]);
        let s = Stage::default();
        let t = s.put_preview(vec![1, 2, 3]);
        assert_eq!(t.len(), 32);
        assert_eq!(s.preview(&t).unwrap().as_slice(), &[1, 2, 3]);
        for _ in 0..MAX_PREVIEWS {
            s.put_preview(vec![]);
        }
        assert!(s.preview(&t).is_none());
    }
}
