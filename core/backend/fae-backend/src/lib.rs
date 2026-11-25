//! fae-backend: the app's back end in Rust (docs/rust-backend-plan.md). Wave by wave (B0–B6) it took over
//! the work of the Python back end; the shell around it (Tauri, fae-app: Fae.exe) only forwards.
//!
//! ## The command table
//! Every method the UI calls on the back end (`api().name(...)` in ui/js, through ui/js/bridge.js) is a
//! **command**: a name, positional JSON arguments (as the UI passes them), a JSON result.
//! [`Backend::call`] runs one; [`COMMANDS`] lists them all with the wave that moves each one here.
//! Fae.exe's one Tauri command runs them (`call(name, args)`, fae-app shell.rs); the tests reach the same
//! through the Python extension (`fae_backend.Backend.call`). Conventions:
//! - arguments positional, exactly as the UI sends them; results plain JSON (no Python objects);
//! - pixels never travel as JSON: a command stores them and returns a token, the shell serves
//!   `/preview/<token>` (later waves: [`Backend::take_preview`]-style accessors next to `call`);
//! - errors: [`Error`] (the shell turns it into a rejected promise). Commands that never fail in the
//!   UI's eyes (reading settings) keep that behaviour and return a neutral value instead;
//! - long work (Process, encoded compare, B4 / B5): cancellation by a request id the UI already sends,
//!   progress through a callback the shell supplies (Fae.exe: eval into the page).

pub mod app;
pub mod avif;
pub mod avifdec;
pub mod colour;
pub mod decode;
pub mod encode;
pub mod fit;
pub mod geometry;
pub mod heif;
pub mod px;
pub mod render;
pub mod resample;
pub mod run;
pub mod stage;
pub mod store;
pub mod thumb;

use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where the back end keeps its files (the shell decides: next to the app, or a test folder).
#[derive(Clone, Debug)]
pub struct Config {
    /// fae_settings.json
    pub settings: PathBuf,
    /// fae_presets.json (the tool presets and edit presets)
    pub presets: PathBuf,
    /// where libheif (+ libde265) are, for HEIC; None: HEIC can't be opened
    pub heif_dir: Option<PathBuf>,
    /// the external encoders' folder (cjpegli, SvtAv1EncApp; B4); None: none of them
    pub encoders_dir: Option<PathBuf>,
    /// the GPU core is off for the session (--no-gpu / FAE_NO_GPU; B5): Process and the encoded compare
    /// render on the CPU core
    pub no_gpu: bool,
    /// the UI's folder (index.html, js/, …) the HTTP handlers serve (B6); None: only /preview and /thumb
    pub ui_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// no such command, or not moved to Rust yet (the shell runs its own version)
    Unknown,
    /// wrong number / type of arguments
    BadArgs,
    /// reading / writing a file
    Io,
    /// the request was replaced or cancelled
    Cancelled,
    /// anything else
    Failed,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error { kind, message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Which wave moves a command to Rust (rust-backend-plan.md, section 4). `Shell` stays with the shell:
/// window controls, dialogs, revealing a file (fae-app does those).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    B0,
    B1,
    B2,
    B3,
    B4,
    B5,
    B6,
    Shell,
}

/// Every command of the UI: (name, the wave that moves it, what it does). `call` runs those whose wave is
/// done ([`Backend::commands`]).
pub const COMMANDS: &[(&str, Wave, &str)] = &[
    ("get_settings", Wave::B0, "the app's settings (fae_settings.json), {} when missing or unreadable"),
    ("save_settings", Wave::B0, "write the settings; always true"),
    ("get_presets", Wave::B0, "tool and edit presets (fae_presets.json), {} when missing / not an object"),
    ("save_presets", Wave::B0, "write the presets whole, then swap; true / false"),
    ("initial_files", Wave::B6, "the images given on the command line"),
    ("expand_paths", Wave::B6, "paths -> the images in them (folders expanded)"),
    ("pick_files", Wave::Shell, "file dialog -> images"),
    ("pick_folder", Wave::Shell, "folder dialog -> images"),
    ("paste_clipboard", Wave::Shell, "Ctrl+V: images from the clipboard (copied files, or a copied image saved as PNG)"),
    ("reveal", Wave::Shell, "show a file in Explorer"),
    ("win_info", Wave::Shell, "frameless window state"),
    ("win_begin", Wave::Shell, "start a window drag / resize"),
    ("win_toggle_max", Wave::Shell, "maximize / restore"),
    ("win_minimize", Wave::Shell, "minimize"),
    ("win_close", Wave::Shell, "close"),
    ("load_stage", Wave::B3, "decode, geometry, resize, colour balance -> preview token"),
    ("load_source", Wave::B3, "the source for crop mode -> preview token"),
    ("sample", Wave::B3, "median colour around a point of the stage"),
    ("encoder_info", Wave::B4, "which encoders are available (cjpegli, SvtAv1EncApp; AVIF always: rav1e)"),
    ("lossy_preview", Wave::B5, "render, encode, decode back, size -> preview token"),
    ("lossy_cancel", Wave::B5, "cancel an encoded compare request"),
    ("process", Wave::B5, "render + encode + save a batch of images, with progress"),
    ("process_cancel", Wave::B5, "cancel the running Process batch"),
    ("core_info", Wave::B5, "the cores' self-test (self-test page)"),
];

/// The waves done in this build. B1 (decoding) and B2 (geometry, resize, thumbnails) have no command of
/// their own: load_stage (B3) runs them; the thumbnails are served by the HTTP handlers (B6, app.rs).
const DONE: &[Wave] = &[Wave::B0, Wave::B1, Wave::B2, Wave::B3, Wave::B4, Wave::B5, Wave::B6];

pub struct Backend {
    cfg: Config,
    /// libheif, loaded on the first HEIC file (Err: why it can't be)
    heif: OnceLock<Result<heif::Lib, String>>,
    /// the stage: opened images, previews, the current stage image (B3)
    stage: stage::Stage,
    /// the external encoders' runs: encoded-compare requests and their cancellation (B4)
    runner: run::Runner,
    /// the CPU and GPU cores (B5)
    cores: render::Cores,
    /// the last encoded compare (key, result), so a preview-size change doesn't encode again (B5)
    lossy: std::sync::Mutex<Option<(String, Lossy)>>,
    /// the shell's decoder for what the back end can't open (AVIF grids, premultiplied alpha: Pillow)
    fallback: Option<Fallback>,
    /// the file list's thumbnails (B6)
    thumbs: app::Thumbs,
    /// the images given on the command line, collected (B6)
    initial: std::sync::Mutex<Vec<String>>,
}

/// The shell's own decoder: (path) → upright RGB / RGBA + ICC profile, for the files the back end
/// answers Unknown for.
pub type Fallback = Box<dyn Fn(&Path) -> Result<(px::Px, Option<Vec<u8>>), Error> + Send + Sync>;

/// An encoded compare's result, decoded back.
struct Lossy {
    dec: px::Px,
    bytes: usize,
    encoder: &'static str,
    used: Value,
    ms: f64,
}

/// What an error of a Process job / the encoded compare shows (api.py: "<Type>: <message>").
/// An error as the UI shows it (Python's exception names, as api.py reported them).
pub fn job_error(e: &Error) -> String {
    match e.kind {
        ErrorKind::Io => format!("OSError: {}", e.message),
        ErrorKind::BadArgs => format!("ValueError: {}", e.message),
        _ => format!("RuntimeError: {}", e.message),
    }
}

fn args_n<'a>(name: &str, args: &'a [Value], n: usize) -> Result<&'a [Value], Error> {
    if args.len() < n {
        return Err(Error::new(ErrorKind::BadArgs, format!("{name}: {} argument(s), need {n}", args.len())));
    }
    Ok(args)
}

impl Backend {
    pub fn new(cfg: Config) -> Backend {
        let no_gpu = cfg.no_gpu || std::env::var_os("FAE_NO_GPU").is_some_and(|v| !v.is_empty());
        Backend {
            cfg,
            heif: OnceLock::new(),
            stage: stage::Stage::default(),
            runner: run::Runner::new(),
            cores: render::Cores::new(no_gpu),
            lossy: std::sync::Mutex::new(None),
            fallback: None,
            thumbs: app::Thumbs::default(),
            initial: std::sync::Mutex::new(vec![]),
        }
    }

    /// The shell's decoder for the files the back end can't open (kind Unknown): used by the stage,
    /// Process and the encoded compare.
    pub fn with_fallback(mut self, f: Fallback) -> Backend {
        self.fallback = Some(f);
        self
    }

    /// libheif, loaded on first use.
    pub fn heif(&self) -> Result<&heif::Lib, Error> {
        let r = self.heif.get_or_init(|| match &self.cfg.heif_dir {
            Some(d) => heif::Lib::load(d).map_err(|e| e.message),
            None => Err("HEIC: no libheif folder given".into()),
        });
        r.as_ref().map_err(|m| Error::new(ErrorKind::Failed, m.clone()))
    }

    /// Open an image file: upright 8-bit RGB / RGBA + its ICC profile (decode.rs). Kind Unknown for a
    /// format the back end doesn't open yet (the shell opens it itself).
    pub fn decode_file(&self, path: &Path) -> Result<decode::Image, Error> {
        let data = std::fs::read(path).map_err(|e| Error::new(ErrorKind::Io, format!("{}: {e}", path.display())))?;
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let heif = if decode::sniff(&data, &ext) == Some("heif") { Some(self.heif()?) } else { None };
        decode::decode(&data, &ext, heif)
    }

    /// The file list's thumbnail of an image file (B2): JPEG bytes, at most `max` × `max` pixels (as
    /// engine.py's `_make_thumb`). Kind Unknown for a format the back end doesn't open yet.
    pub fn thumb_file(&self, path: &Path, max: u32) -> Result<Vec<u8>, Error> {
        let data = std::fs::read(path).map_err(|e| Error::new(ErrorKind::Io, format!("{}: {e}", path.display())))?;
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let heif = if decode::sniff(&data, &ext) == Some("heif") { Some(self.heif()?) } else { None };
        let img = decode::decode_draft(&data, &ext, heif, Some((max, max)))?;
        let t = thumb::thumbnail(img.into(), (max as usize, max as usize));
        thumb::encode_jpeg(&t, 82)
    }

    fn source(&self, path: &Path) -> Result<stage::Source, Error> {
        let (img, icc) = match self.decode_file(path) {
            Ok(im) => {
                let icc = im.icc.clone();
                (im.into(), icc)
            }
            Err(e) if e.kind == ErrorKind::Unknown => match &self.fallback {
                Some(f) => f(path)?,
                None => return Err(e),
            },
            Err(e) => return Err(e),
        };
        Ok(stage::Source { img: std::sync::Arc::new(img), icc: icc.map(std::sync::Arc::new) })
    }

    /// The stage image of `path` at full resolution (decode, geometry, resize, colour balance; cached as
    /// for `load_stage`) + the source's ICC profile: what Process and the encoded compare edit. Kind
    /// Unknown for a format the back end doesn't open and no fallback (see [`Backend::give_source`]).
    pub fn stage_image(&self, path: &Path, p: &fit::StageParams) -> Result<(px::Px, Option<Vec<u8>>), Error> {
        let (img, src, _) = self.stage.stage(path, p, &|p| self.source(p))?;
        Ok((img, src.icc.map(|i| (*i).clone())))
    }

    /// The shell opened `path` itself (a format the back end doesn't decode yet: AVIF until B4); the
    /// stage uses these pixels for it while the file is unchanged.
    pub fn give_source(&self, path: &Path, img: px::Px, icc: Option<Vec<u8>>) -> Result<(), Error> {
        self.stage.give_source(path, img, icc)
    }

    /// The preview pixels (RGBA) stored under `token` by load_stage / load_source / [`Backend::put_preview`]:
    /// what the shell serves at `/preview/<token>`. None when gone (only the last 8 are kept).
    pub fn preview(&self, token: &str) -> Option<std::sync::Arc<Vec<u8>>> {
        self.stage.preview(token)
    }

    /// Store preview pixels the shell made itself (the encoded compare until B5); their token.
    pub fn put_preview(&self, rgba: Vec<u8>) -> String {
        self.stage.put_preview(rgba)
    }

    /// The external encoders (B4).
    pub fn encoders(&self) -> encode::Encoders {
        encode::Encoders::in_dir(self.cfg.encoders_dir.as_deref())
    }

    /// A new encoded-compare request: encoder runs of older requests are killed (proc.py `preview_begin`).
    pub fn preview_begin(&self, req: i64) {
        self.runner.begin(req)
    }

    /// JPEG / AVIF / lossy WebP (B4; output.py `encode_lossy`). `req`: the encoded compare's request id
    /// (below-normal priority, cancelled by a newer one); None for Process.
    pub fn encode_lossy(&self, img: px::View, fmt: &str, opts: &Value, icc: Option<&[u8]>, req: Option<i64>) -> Result<encode::Encoded, Error> {
        self.runner.check(req)?;
        encode::encode_lossy(img, fmt, opts, icc, &self.encoders(), &self.runner, req)
    }

    /// Encode and write next to `src` as `<name>_<w>x<h>.<ext>` (B4; output.py `save_image`).
    pub fn save(&self, img: px::View, src: &str, fmt: &str, icc: Option<&[u8]>, opts: &Value) -> Result<PathBuf, Error> {
        encode::save(img, src, fmt, icc, opts, &self.encoders(), &self.runner)
    }

    /// The images given on the command line (files and folders): collected now, the UI asks for them
    /// with `initial_files` (B6).
    pub fn set_initial(&self, paths: &[String]) {
        *self.initial.lock().unwrap() = app::collect(paths);
    }

    /// The CPU and GPU cores (B5).
    pub fn cores(&self) -> &render::Cores {
        &self.cores
    }

    /// Compile the GPU core's shaders in the background (app start; core.py `warm_gpu`).
    pub fn warm_gpu(&self) {
        self.cores.warm()
    }

    /// The stage of a job + the whole edit at full resolution (what Process writes; api.py
    /// `_render_full`): the image (RGB, or RGBA with the stage's alpha untouched), the source's ICC
    /// profile, the kit filters the CPU core left out.
    pub fn render_job(&self, job: &Value) -> Result<(px::Px, Option<Vec<u8>>, Vec<String>), Error> {
        let path = job.get("path").and_then(Value::as_str).ok_or_else(|| Error::new(ErrorKind::BadArgs, "job: no path"))?;
        let p = fit::StageParams::from_json(job.get("stage").unwrap_or(&Value::Null));
        let edit = render::parse(job.get("edit").unwrap_or(&Value::Null))?;
        let (img, icc) = self.stage_image(Path::new(path), &p)?;
        let mut skipped = vec![];
        let out = if img.c == 4 {
            let rgb: Vec<u8> = img.data.chunks_exact(4).flat_map(|q| [q[0], q[1], q[2]]).collect();
            let o = self.cores.render(&rgb, img.w, img.h, &edit, render::Engine::Auto, &mut skipped)?;
            let mut d = img.data;
            for (q, s) in d.chunks_exact_mut(4).zip(o.chunks_exact(3)) {
                q[..3].copy_from_slice(s);
            }
            px::Px::from_vec(img.w, img.h, 4, d)
        } else {
            px::Px::from_vec(img.w, img.h, 3, self.cores.render(&img.data, img.w, img.h, &edit, render::Engine::Auto, &mut skipped)?)
        };
        Ok((out, icc, skipped))
    }

    /// api.py `process`: render, encode and save each job next to its source. `progress(i, n, result)`
    /// after each image of a batch of more than one (the shell: window.onProcessProgress). The results:
    /// {"path", "ok": true, "out", "skipped"?: [kit filter ids left out]} or {"path", "ok": false, "error"}.
    /// process_cancel stops it: the image being encoded with an external encoder at once, otherwise once
    /// that image is done; each image not written then answers {"path", "ok": false, "cancelled": true}.
    pub fn process(&self, jobs: &[Value], fmt: &str, opts: &Value, progress: &dyn Fn(usize, usize, &Value)) -> Vec<Value> {
        self.runner.begin_process();
        let fmt = encode::norm_fmt(fmt);
        let n = jobs.len();
        let mut results = vec![];
        for (i, job) in jobs.iter().enumerate() {
            let path = job.get("path").cloned().unwrap_or(Value::Null);
            if self.runner.check(None).is_err() {
                results.push(serde_json::json!({"path": path, "ok": false, "cancelled": true}));
                continue;
            }
            let r = self.render_job(job).and_then(|(img, icc, skipped)| {
                self.runner.check(None)?;
                let src = path.as_str().unwrap_or_default();
                let out = self.save(img.view(), src, fmt, icc.as_deref(), opts)?;
                let mut r = serde_json::json!({"path": path, "ok": true, "out": out.display().to_string()});
                if !skipped.is_empty() {
                    r["skipped"] = serde_json::json!(skipped);
                }
                Ok(r)
            });
            let r = r.unwrap_or_else(|e| match e.kind {
                ErrorKind::Cancelled => serde_json::json!({"path": path, "ok": false, "cancelled": true}),
                _ => serde_json::json!({"path": path, "ok": false, "error": job_error(&e)}),
            });
            if n > 1 {
                progress(i + 1, n, &r);
            }
            results.push(r);
        }
        self.cores.release(); // same-size images reused it; now give it back
        results
    }

    /// api.py `lossy_preview`: the fully edited image encoded with the lossy format, decoded again, as
    /// preview pixels (token) + its size in bytes, the encode's time, the encoder and the settings used.
    /// A newer request (or lossy_cancel) stops it: {"ok": false, "cancelled": true}.
    pub fn lossy_preview(&self, job: &Value, fmt: &str, opts: &Value, req: i64, full: f64) -> Value {
        self.runner.begin(req);
        let fmt = encode::norm_fmt(fmt);
        let r = (|| -> Result<Value, Error> {
            self.runner.check(Some(req))?;
            let key = serde_json::json!([job, fmt, opts]).to_string();
            let mut cache = self.lossy.lock().unwrap().take().filter(|(k, _)| *k == key);
            if cache.is_none() {
                let r = self.render_job(job);
                self.cores.release(); // the GPU memory of a full-size image isn't kept
                let (img, icc, _) = r?;
                self.runner.check(Some(req))?;
                let t0 = std::time::Instant::now();
                let e = self.encode_lossy(img.view(), fmt, opts, icc.as_deref(), Some(req))?;
                let ms = t0.elapsed().as_secs_f64() * 1000.0; // the encode alone, shown next to the size
                self.runner.check(Some(req))?;
                // the encoded file's own alpha (JPEG: none, the white it was written on)
                let ext = encode::ext(fmt).trim_start_matches('.');
                let d = decode::decode(&e.data, ext, None)?;
                let dec = px::Px::from_vec(d.w as usize, d.h as usize, d.channels as usize, d.px);
                cache = Some((key, Lossy { dec, bytes: e.data.len(), encoder: e.encoder, used: e.used, ms }));
            }
            let (key, l) = cache.unwrap();
            let (pw, ph) = stage::pv_size(l.dec.w, l.dec.h, full);
            let token = self.put_preview(stage::preview_rgba(l.dec.view(), (pw, ph), resample::Filter::Lanczos));
            let r = serde_json::json!({"ok": true, "req_id": req, "token": token, "pw": pw, "ph": ph, "bytes": l.bytes,
                "ms": (l.ms * 10.0).round() / 10.0, "encoder": l.encoder, "fmt": fmt, "opts": l.used});
            *self.lossy.lock().unwrap() = Some((key, l));
            Ok(r)
        })();
        r.unwrap_or_else(|e| {
            if e.kind == ErrorKind::Cancelled || req < self.runner.latest() {
                serde_json::json!({"ok": false, "req_id": req, "cancelled": true})
            } else {
                serde_json::json!({"ok": false, "req_id": req, "error": job_error(&e)})
            }
        })
    }

    /// The commands this build runs (the rest still run in the shell's Python).
    pub fn commands() -> Vec<&'static str> {
        COMMANDS.iter().filter(|(_, w, _)| DONE.contains(w)).map(|(n, _, _)| *n).collect()
    }

    /// Run command `name` with the UI's positional arguments.
    pub fn call(&self, name: &str, args: &[Value]) -> Result<Value, Error> {
        match name {
            "get_settings" => Ok(store::read_object(&self.cfg.settings, true)),
            "save_settings" => {
                let a = args_n(name, args, 1)?;
                // as api.py: failures are ignored, the UI is told it worked
                let _ = store::write_json(&self.cfg.settings, &a[0]);
                Ok(Value::Bool(true))
            }
            "get_presets" => Ok(store::read_object(&self.cfg.presets, false)),
            "save_presets" => {
                let a = args_n(name, args, 1)?;
                Ok(Value::Bool(store::write_json(&self.cfg.presets, &a[0]).is_ok()))
            }
            "initial_files" => Ok(serde_json::json!(*self.initial.lock().unwrap())),
            "expand_paths" => {
                let a = args_n(name, args, 1)?;
                Ok(serde_json::json!(app::collect(&app::paths_of(&a[0]))))
            }
            "load_stage" => self.stage.load_stage(args, &|p| self.source(p)),
            "load_source" => self.stage.load_source(args, &|p| self.source(p)),
            "sample" => Ok(self.stage.sample(args)),
            "lossy_preview" => {
                let a = args_n(name, args, 4)?;
                let full = a.get(4).and_then(fit::as_float).unwrap_or(0.0);
                let req = fit::as_int(&a[3]).ok_or_else(|| Error::new(ErrorKind::BadArgs, "lossy_preview: req_id"))?;
                Ok(self.lossy_preview(&a[0], a[1].as_str().unwrap_or_default(), &a[2], req, full))
            }
            "lossy_cancel" => {
                let a = args_n(name, args, 1)?;
                self.runner.begin(fit::as_int(&a[0]).ok_or_else(|| Error::new(ErrorKind::BadArgs, "lossy_cancel: req_id"))?);
                Ok(Value::Bool(true))
            }
            "process" => {
                let a = args_n(name, args, 1)?;
                let jobs = a[0].as_array().cloned().unwrap_or_default();
                let fmt = a.get(1).and_then(Value::as_str).unwrap_or("WEBP");
                let opts = a.get(2).cloned().unwrap_or(Value::Null);
                Ok(Value::Array(self.process(&jobs, fmt, &opts, &|_, _, _| {})))
            }
            "process_cancel" => {
                self.runner.stop_process();
                Ok(Value::Bool(true))
            }
            "core_info" => Ok(self.cores.selftest()),
            "encoder_info" => {
                let e = self.encoders();
                // "avif": AVIF can be written and read back (rav1e / rav1d are built in since B4)
                Ok(serde_json::json!({"jpegli": e.jpegli().is_some(), "avif": true, "svt": e.svt().is_some()}))
            }
            _ if COMMANDS.iter().any(|(n, _, _)| *n == name) => {
                Err(Error::new(ErrorKind::Unknown, format!("{name}: not in the Rust back end yet")))
            }
            _ => Err(Error::new(ErrorKind::Unknown, format!("no command {name:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fae-backend-test-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn settings_and_presets() {
        let d = tmp("sp");
        let b = Backend::new(Config { settings: d.join("s.json"), presets: d.join("p.json"), heif_dir: None, encoders_dir: None, no_gpu: true, ui_dir: None });
        assert_eq!(b.call("get_settings", &[]).unwrap(), json!({}));
        let s = json!({"fmt": "PNG", "secs": ["adjust", "f_sc"], "jpeg_d": 1.25, "name": "Mirelle ✓"});
        assert_eq!(b.call("save_settings", &[s.clone()]).unwrap(), json!(true));
        assert_eq!(b.call("get_settings", &[]).unwrap(), s);
        // key order kept
        let txt = std::fs::read_to_string(d.join("s.json")).unwrap();
        assert!(txt.find("\"fmt\"").unwrap() < txt.find("\"secs\"").unwrap());
        // presets: not an object -> {}
        assert_eq!(b.call("save_presets", &[json!([1, 2])]).unwrap(), json!(true));
        assert_eq!(b.call("get_presets", &[]).unwrap(), json!({}));
        let p = json!({"adjust": [{"name": "Starfall", "values": {"adj_sat": 60}}]});
        b.call("save_presets", &[p.clone()]).unwrap();
        assert_eq!(b.call("get_presets", &[]).unwrap(), p);
        assert!(!d.join("p.json.tmp").exists());
        // broken file -> {}
        std::fs::write(d.join("s.json"), "{broken").unwrap();
        assert_eq!(b.call("get_settings", &[]).unwrap(), json!({}));
        // errors
        assert_eq!(b.call("save_settings", &[]).unwrap_err().kind, ErrorKind::BadArgs);
        assert_eq!(b.call("encoder_info", &[]).unwrap(), json!({"jpegli": false, "avif": true, "svt": false}));
        assert_eq!(b.call("process", &[]).unwrap_err().kind, ErrorKind::BadArgs);
        assert_eq!(b.call("process", &[json!([])]).unwrap(), json!([]));
        let r = b.call("process", &[json!([{"path": "no/such/file.png"}])]).unwrap();
        assert_eq!(r[0]["ok"], json!(false));
        assert_eq!(b.call("lossy_preview", &[json!({"path": "no/such/file.png"}), json!("JPEG"), json!({}), json!(3)]).unwrap()["ok"], json!(false));
        // an older request than the latest: cancelled
        assert_eq!(b.call("lossy_cancel", &[json!(9)]).unwrap(), json!(true));
        assert_eq!(b.call("lossy_preview", &[json!({"path": "x.png"}), json!("JPEG"), json!({}), json!(4)]).unwrap()["cancelled"], json!(true));
        // a cancel between batches doesn't stop the next one
        assert_eq!(b.call("process_cancel", &[]).unwrap(), json!(true));
        assert_eq!(b.call("process", &[json!([{"path": "no/such/file.png"}])]).unwrap()[0].get("cancelled"), None);
        assert_eq!(b.call("core_info", &[]).unwrap()["ok"], json!(true));
        assert_eq!(b.call("sample", &[json!(0.5), json!(0.5)]).unwrap(), Value::Null);
        assert_eq!(b.call("load_stage", &[json!("no/such/file.png"), json!({}), json!(1)]).unwrap()["ok"], json!(false));
        assert_eq!(b.call("nope", &[]).unwrap_err().kind, ErrorKind::Unknown);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn process_cancel_mid_batch() {
        let d = tmp("pc");
        let b = Backend::new(Config { settings: d.join("s.json"), presets: d.join("p.json"), heif_dir: None, encoders_dir: None, no_gpu: true, ui_dir: None });
        let img = px::Px::from_vec(8, 8, 3, (0..192).map(|i| i as u8).collect());
        let jobs: Vec<Value> = (0..4)
            .map(|i| {
                let p = d.join(format!("starfall{i}.png"));
                std::fs::write(&p, encode::encode_png(img.view(), None).unwrap()).unwrap();
                json!({"path": p.display().to_string()})
            })
            .collect();
        // cancelled after the second image: the first two written, the other two left
        let r = b.process(&jobs, "PNG", &json!({}), &|i, _, _| {
            if i == 2 {
                b.call("process_cancel", &[]).unwrap();
            }
        });
        let ok: Vec<_> = r.iter().map(|r| r["ok"] == json!(true)).collect();
        assert_eq!(ok, [true, true, false, false]);
        assert!(r[2..].iter().all(|r| r["cancelled"] == json!(true) && r.get("error").is_none()));
        // the next batch runs whole
        assert!(b.process(&jobs, "PNG", &json!({}), &|_, _, _| {}).iter().all(|r| r["ok"] == json!(true)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn table() {
        assert_eq!(
            Backend::commands(),
            vec!["get_settings", "save_settings", "get_presets", "save_presets", "initial_files", "expand_paths", "load_stage",
                 "load_source", "sample", "encoder_info", "lossy_preview", "lossy_cancel", "process", "process_cancel", "core_info"]
        );
        let mut names: Vec<_> = COMMANDS.iter().map(|c| c.0).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), COMMANDS.len());
    }
}
