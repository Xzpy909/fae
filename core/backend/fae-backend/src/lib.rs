//! fae-backend: the app's back end in Rust (claude/rust-backend-plan.md). Wave by wave it takes over the
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

pub mod decode;
pub mod heif;
pub mod store;

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

/// The waves done in this build (B1 has no command of its own: decoding is used by load_stage, B3).
const DONE: &[Wave] = &[Wave::B0, Wave::B1];

pub struct Backend {
    cfg: Config,
    /// libheif, loaded on the first HEIC file (Err: why it can't be)
    heif: OnceLock<Result<heif::Lib, String>>,
}

fn args_n<'a>(name: &str, args: &'a [Value], n: usize) -> Result<&'a [Value], Error> {
    if args.len() < n {
        return Err(Error::new(ErrorKind::BadArgs, format!("{name}: {} argument(s), need {n}", args.len())));
    }
    Ok(args)
}

impl Backend {
    pub fn new(cfg: Config) -> Backend {
        Backend { cfg, heif: OnceLock::new() }
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
        let b = Backend::new(Config { settings: d.join("s.json"), presets: d.join("p.json"), heif_dir: None });
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
        assert_eq!(b.call("load_stage", &[]).unwrap_err().kind, ErrorKind::Unknown);
        assert_eq!(b.call("nope", &[]).unwrap_err().kind, ErrorKind::Unknown);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn table() {
        assert_eq!(Backend::commands(), vec!["get_settings", "save_settings", "get_presets", "save_presets"]);
        let mut names: Vec<_> = COMMANDS.iter().map(|c| c.0).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), COMMANDS.len());
    }
}
