//! The command line and the app's folders (the pywebview shell's app.py main and config.py until B7).

use std::path::{Path, PathBuf};

/// What the command line asks for.
#[derive(Debug, Default, PartialEq)]
pub struct Options {
    /// images or folders to open
    pub files: Vec<String>,
    /// --debug: the web inspector
    pub debug: bool,
    /// --no-gpu / FAE_NO_GPU: the CPU core only (the preview and Process)
    pub no_gpu: bool,
    /// --selftest / --gpu-spike: the check pages, with the normal window frame
    pub selftest: bool,
    /// --native-frame: the normal Windows frame instead of the UI's own title bar
    pub native_frame: bool,
    /// the cross-origin isolation headers (SharedArrayBuffer for the threaded WebAssembly preview);
    /// FASTEDIT_COI=0 turns them off (the self-test page then says "not isolated")
    pub coi: bool,
    /// the page and its query: "", "?gpu=0&fxtest=1", "selftest.html", …
    pub page: String,
}

impl Options {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Options {
        Options::parse(args, |k| std::env::var(k).ok())
    }

    pub fn parse(args: impl IntoIterator<Item = String>, env: impl Fn(&str) -> Option<String>) -> Options {
        let args: Vec<String> = args.into_iter().collect();
        let has = |f: &str| args.iter().any(|a| a == f);
        let mut o = Options {
            files: args.iter().filter(|a| !a.starts_with("--")).cloned().collect(),
            debug: has("--debug"),
            no_gpu: has("--no-gpu") || env("FAE_NO_GPU").is_some_and(|v| !v.is_empty()),
            selftest: has("--selftest"),
            native_frame: has("--native-frame"),
            coi: env("FASTEDIT_COI").as_deref() != Some("0"),
            page: String::new(),
        };
        if has("--gpu-spike") {
            // the GPU core's feasibility page (gpu-core-plan, G0)
            o.selftest = true;
            o.page = "gpu-spike.html".into();
        } else if o.selftest {
            o.page = "selftest.html".into(); // the cores / WASM / isolation checks
        } else {
            let mut q = vec![];
            if has("--no-gpu") {
                q.push("gpu=0"); // the preview on the CPU core too (gpu-view.js)
            }
            if has("--test-filters") {
                q.push("fxtest=1"); // the filter kit's hidden test filters in the Filters list
            }
            if !q.is_empty() {
                o.page = format!("?{}", q.join("&"));
            }
        }
        o
    }
}

/// Where the app keeps things: next to Fae.exe (settings, presets, encoders\, heif\, ui\). A build run
/// from the repository (core\backend\target\release\Fae.exe) uses the repository's folder: its ui\,
/// its settings.
pub struct Dirs {
    pub app: PathBuf,
}

impl Dirs {
    pub fn find() -> Dirs {
        let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
        Dirs { app: Dirs::app_dir(&exe) }
    }

    /// The first of `exe_dir` and its parents with ui\index.html; `exe_dir` when none has.
    pub fn app_dir(exe_dir: &Path) -> PathBuf {
        exe_dir.ancestors().find(|d| d.join("ui").join("index.html").is_file()).unwrap_or(exe_dir).to_path_buf()
    }

    pub fn ui(&self) -> PathBuf {
        self.app.join("ui")
    }

    pub fn settings(&self) -> PathBuf {
        self.app.join("fae_settings.json")
    }

    /// the user's tool presets and edit presets
    pub fn presets(&self) -> PathBuf {
        self.app.join("fae_presets.json")
    }

    pub fn encoders(&self) -> PathBuf {
        self.app.join("encoders")
    }

    /// libheif (+ libde265) for HEIC: FAE_HEIF_DIR or heif\ next to the app (build.bat copies the
    /// libraries pillow-heif ships there).
    pub fn heif(&self) -> Option<PathBuf> {
        let has = |d: &Path| {
            std::fs::read_dir(d).is_ok_and(|r| {
                r.flatten().any(|e| e.file_name().to_string_lossy().to_lowercase().starts_with("libheif") || e.file_name().to_string_lossy().to_lowercase().starts_with("heif"))
            })
        };
        [std::env::var_os("FAE_HEIF_DIR").map(PathBuf::from), Some(self.app.join("heif"))].into_iter().flatten().find(|d| has(d))
    }

    /// Ctrl+V: an image copied to the clipboard (no file of its own) is saved here as PNG, then opened
    /// (Process writes next to it). Pictures\Fae\Pasted; FAE_PASTE_DIR overrides.
    pub fn paste(&self) -> PathBuf {
        if let Some(d) = std::env::var_os("FAE_PASTE_DIR").filter(|d| !d.is_empty()) {
            return PathBuf::from(d);
        }
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
        let pictures = home.map(|h| h.join("Pictures")).filter(|p| p.is_dir());
        pictures.unwrap_or_else(|| self.app.clone()).join("Fae").join("Pasted")
    }

    /// Settings from before the rename to Fae: adopted once.
    pub fn adopt_old_settings(&self) {
        let old = self.app.join("fast_image_editor_settings.json");
        if !self.settings().exists() && old.exists() {
            let _ = std::fs::rename(old, self.settings());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(a: &[&str], env: &[(&str, &str)]) -> Options {
        let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Options::parse(a.iter().map(|s| s.to_string()), |k| env.iter().find(|e| e.0 == k).map(|e| e.1.clone()))
    }

    #[test]
    fn command_line() {
        let o = parse(&["Starfall", "--debug", "a.png"], &[]);
        assert_eq!(o.files, ["Starfall", "a.png"]);
        assert!(o.debug && o.coi && !o.no_gpu && !o.selftest && o.page.is_empty());
        assert_eq!(parse(&["--no-gpu", "--test-filters"], &[]).page, "?gpu=0&fxtest=1");
        let o = parse(&[], &[("FAE_NO_GPU", "1"), ("FASTEDIT_COI", "0")]);
        assert!(o.no_gpu && !o.coi && o.page.is_empty());
        assert!(!parse(&[], &[("FAE_NO_GPU", "")]).no_gpu);
        let o = parse(&["--selftest", "--no-gpu"], &[]);
        assert!(o.selftest && o.page == "selftest.html");
        assert_eq!(parse(&["--gpu-spike"], &[]).page, "gpu-spike.html");
    }

    #[test]
    fn folders() {
        let d = std::env::temp_dir().join(format!("fae-app-dirs-{}", std::process::id()));
        let exe = d.join("core").join("backend").join("target").join("release");
        std::fs::create_dir_all(&exe).unwrap();
        assert_eq!(Dirs::app_dir(&exe), exe);
        std::fs::create_dir_all(d.join("ui")).unwrap();
        std::fs::write(d.join("ui").join("index.html"), "x").unwrap();
        assert_eq!(Dirs::app_dir(&exe), d);
        let dirs = Dirs { app: d.clone() };
        std::fs::write(d.join("fast_image_editor_settings.json"), "{}").unwrap();
        dirs.adopt_old_settings();
        assert!(dirs.settings().is_file() && !d.join("fast_image_editor_settings.json").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
