//! fae-backend: the app's back end in Rust (docs/rust-backend-plan.md). Wave by wave it takes over the
//! work of `fastedit/` (Python); the shell around it (pywebview now, maybe Tauri later) only forwards.
//!
//! ## The command table
//! Every method the UI calls on the back end (`api().name(...)` in ui/js, through ui/js/bridge.js) is a
//! **command**: a name, positional JSON arguments (as the UI passes them), a JSON result.
//! [`Backend::call`] runs one; [`COMMANDS`] lists them all with the wave that moves each one here.
//! Under pywebview, fastedit/api.py forwards the moved ones (`fae_backend.Backend.call`); under Tauri the
//! same functions become `#[tauri::command]`s. Conventions, so the table maps 1:1 onto either shell:
//! - arguments positional, exactly as the UI sends them; results plain JSON (no Python objects);
//! - pixels never travel as JSON: a command stores them and returns a token, the shell serves
//!   `/preview/<token>` (later waves: [`Backend::take_preview`]-style accessors next to `call`);
//! - errors: [`Error`] (the shell turns it into a rejected promise). Commands that never fail in the
//!   UI's eyes (reading settings) keep that behaviour and return a neutral value instead;
//! - long work (Process, encoded compare, B4 / B5): cancellation by a request id the UI already sends,
//!   progress through a callback the shell supplies (pywebview: evaluate_js, Tauri: an event).

pub mod avif;
pub mod colour;
pub mod decode;
pub mod encode;
pub mod fit;
pub mod geometry;
pub mod heif;
pub mod px;
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
/// window controls, dialogs, revealing a file (pywebview / Tauri do those).
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
    ("encoder_info", Wave::B4, "which encoders are available"),
    ("lossy_preview", Wave::B5, "render, encode, decode back, size -> preview token"),
    ("lossy_cancel", Wave::B5, "cancel an encoded compare request"),
    ("process", Wave::B5, "render + encode + save a batch of images, with progress"),
    ("core_info", Wave::B5, "the cores' self-test (self-test page)"),
];

/// The waves done in this build. B1 (decoding) and B2 (geometry, resize, thumbnails) have no command of
/// their own: load_stage (B3) runs them. The shell still asks for the stage image itself for Process and
/// the encoded compare until B5 ([`Backend::stage_image`]), and for thumbnails until B6.
const DONE: &[Wave] = &[Wave::B0, Wave::B1, Wave::B2, Wave::B3];

pub struct Backend {
    cfg: Config,
    /// libheif, loaded on the first HEIC file (Err: why it can't be)
    heif: OnceLock<Result<heif::Lib, String>>,
    /// the stage: opened images, previews, the current stage image (B3)
    stage: stage::Stage,
    /// the external encoders' runs: encoded-compare requests and their cancellation (B4)
    runner: run::Runner,
}

fn args_n<'a>(name: &str, args: &'a [Value], n: usize) -> Result<&'a [Value], Error> {
    if args.len() < n {
        return Err(Error::new(ErrorKind::BadArgs, format!("{name}: {} argument(s), need {n}", args.len())));
    }
    Ok(args)
}

impl Backend {
    pub fn new(cfg: Config) -> Backend {
        Backend { cfg, heif: OnceLock::new(), stage: stage::Stage::default(), runner: run::Runner::new() }
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
        let im = self.decode_file(path)?;
        let icc = im.icc.clone().map(std::sync::Arc::new);
        Ok(stage::Source { img: std::sync::Arc::new(im.into()), icc })
    }

    /// The stage image of `path` at full resolution (decode, geometry, resize, colour balance; cached as
    /// for `load_stage`) + the source's ICC profile: what Process and the encoded compare edit (the shell
    /// does those until B5). Kind Unknown for a format the back end doesn't open (see [`Backend::give_source`]).
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
            "load_stage" => self.stage.load_stage(args, &|p| self.source(p)),
            "load_source" => self.stage.load_source(args, &|p| self.source(p)),
            "sample" => Ok(self.stage.sample(args)),
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
        let b = Backend::new(Config { settings: d.join("s.json"), presets: d.join("p.json"), heif_dir: None, encoders_dir: None });
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
        assert_eq!(b.call("encoder_info", &[]).unwrap_err().kind, ErrorKind::Unknown);
        assert_eq!(b.call("sample", &[json!(0.5), json!(0.5)]).unwrap(), Value::Null);
        assert_eq!(b.call("load_stage", &[json!("no/such/file.png"), json!({}), json!(1)]).unwrap()["ok"], json!(false));
        assert_eq!(b.call("nope", &[]).unwrap_err().kind, ErrorKind::Unknown);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn table() {
        assert_eq!(
            Backend::commands(),
            vec!["get_settings", "save_settings", "get_presets", "save_presets", "load_stage", "load_source", "sample"]
        );
        let mut names: Vec<_> = COMMANDS.iter().map(|c| c.0).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), COMMANDS.len());
    }
}
