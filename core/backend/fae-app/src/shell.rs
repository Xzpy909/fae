//! The shell's side of the commands (the pywebview shell's api.py and winchrome.py until B7): the window buttons,
//! the dialogs, Ctrl+V, reveal in Explorer, Process with its progress; everything else goes to the back
//! end's command table. And the window's events the page hears about (drag-drop, maximized / restored).

use backend::{app, job_error, Backend};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tauri::{DragDropEvent, WebviewWindow, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_runtime::ResizeDirection;

pub struct Shell {
    b: Arc<Backend>,
    window: OnceLock<WebviewWindow>,
    frameless: bool,
    paste_dir: PathBuf,
    maximized: AtomicBool,
}

/// The page's one command: `name` with `args` as the UI calls api().<name>(...args) (bridge.js).
/// Runs off the UI thread, so a long job (Process) doesn't hold the window.
#[tauri::command]
pub async fn call(sh: tauri::State<'_, Arc<Shell>>, name: String, args: Vec<Value>) -> Result<Value, String> {
    let sh = sh.inner().clone();
    tauri::async_runtime::spawn_blocking(move || sh.call(&name, &args)).await.map_err(|e| e.to_string())?
}

impl Shell {
    pub fn new(b: Arc<Backend>, opts: &crate::paths::Options, paste_dir: PathBuf) -> Shell {
        Shell {
            b,
            window: OnceLock::new(),
            // the unified header: on Windows the UI's top bar is the title bar
            frameless: cfg!(windows) && !opts.native_frame && !opts.selftest,
            paste_dir,
            maximized: AtomicBool::new(false),
        }
    }

    pub fn frameless(&self) -> bool {
        self.frameless
    }

    pub fn set_window(&self, w: WebviewWindow) {
        self.maximized.store(w.is_maximized().unwrap_or(false), Ordering::Relaxed);
        let _ = self.window.set(w);
    }

    fn win(&self) -> Result<&WebviewWindow, String> {
        self.window.get().ok_or_else(|| "no window yet".to_string())
    }

    /// Run JS in the page (the globals of bridge.js); ignored before the window is there.
    fn js(&self, code: &str) {
        if let Some(w) = self.window.get() {
            let _ = w.eval(code);
        }
    }

    pub fn call(&self, name: &str, args: &[Value]) -> Result<Value, String> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Null);
        let text = |i: usize| args.get(i).and_then(Value::as_str).unwrap_or("").to_string();
        let w = || self.win();
        let err = |e: tauri::Error| e.to_string();
        match name {
            // ---- the unified header (winchrome.js) ----
            "win_info" => Ok(json!({"frameless": self.frameless, "maximized": w()?.is_maximized().unwrap_or(false)})),
            "win_begin" => {
                if self.frameless {
                    // Windows' own move / resize loop, started while the mouse button is still down
                    let w = w()?;
                    match resize_direction(&text(0)) {
                        Some(d) => w.as_ref().window().start_resize_dragging(d).map_err(err)?,
                        None => w.start_dragging().map_err(err)?,
                    }
                }
                Ok(Value::Null)
            }
            "win_toggle_max" => {
                let w = w()?;
                (if w.is_maximized().unwrap_or(false) { w.unmaximize() } else { w.maximize() }).map_err(err)?;
                Ok(Value::Null)
            }
            "win_minimize" => w()?.minimize().map(|_| Value::Null).map_err(err),
            "win_close" => w()?.close().map(|_| Value::Null).map_err(err),
            // ---- files ----
            "pick_files" => {
                let exts: Vec<&str> = app::IMAGE_EXTS.iter().map(|e| e.trim_start_matches('.')).collect();
                let picked = w()?.dialog().file().set_parent(w()?).add_filter("Images", &exts).add_filter("All files", &["*"]).blocking_pick_files();
                Ok(json!(self.collect(picked.unwrap_or_default().into_iter().filter_map(|p| p.into_path().ok()))))
            }
            "pick_folder" => {
                let picked = w()?.dialog().file().set_parent(w()?).blocking_pick_folder();
                Ok(json!(self.collect(picked.and_then(|p| p.into_path().ok()))))
            }
            "paste_clipboard" => Ok(self.paste_clipboard()),
            "reveal" => {
                reveal(&text(0));
                Ok(Value::Bool(true))
            }
            // ---- Process: the progress after each image of a batch goes to the page ----
            "process" => {
                let jobs = arg(0).as_array().cloned().unwrap_or_default();
                let fmt = args.get(1).and_then(Value::as_str).unwrap_or("WEBP").to_string();
                let progress = |i: usize, n: usize, r: &Value| {
                    self.js(&format!("window.onProcessProgress && window.onProcessProgress({i},{n},{r})"));
                };
                Ok(Value::Array(self.b.process(&jobs, &fmt, &arg(2), &progress)))
            }
            // ---- the back end's command table; these four answer a failure instead of raising (api.py) ----
            "load_stage" | "lossy_preview" => self.b.call(name, args).or_else(|e| {
                let req = if name == "load_stage" { arg(2) } else { arg(3) };
                Ok(json!({"ok": false, "req_id": req, "error": job_error(&e)}))
            }),
            "load_source" | "core_info" => {
                self.b.call(name, args).or_else(|e| Ok(json!({"ok": false, "error": job_error(&e)})))
            }
            _ => self.b.call(name, args).map_err(|e| job_error(&e)),
        }
    }

    fn collect(&self, paths: impl IntoIterator<Item = PathBuf>) -> Vec<String> {
        app::collect(&paths.into_iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>())
    }

    /// Ctrl+V: the images on the clipboard. Files copied in Explorer -> those images (folders expanded);
    /// a copied image (screenshot, from a browser…) has no file: saved as PNG in the paste folder (alpha
    /// kept when it has any), then opened. {"ok", "paths", "saved"} or {"ok": false, "error"}.
    fn paste_clipboard(&self) -> Value {
        let mut cb = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => return json!({"ok": false, "error": format!("Clipboard: {e}")}),
        };
        if let Ok(files) = cb.get().file_list() {
            if !files.is_empty() {
                let paths = self.collect(files);
                if paths.is_empty() {
                    return json!({"ok": false, "error": "No images among the copied files"});
                }
                return json!({"ok": true, "paths": paths, "saved": null});
            }
        }
        let Ok(img) = cb.get_image() else {
            return json!({"ok": false, "error": "No image on the clipboard"});
        };
        match save_pasted(&self.paste_dir, img.width, img.height, &img.bytes, &stamp()) {
            Ok(p) => {
                let p = p.to_string_lossy().into_owned();
                json!({"ok": true, "paths": [p], "saved": p})
            }
            Err(e) => json!({"ok": false, "error": format!("Saving the pasted image: {e}")}),
        }
    }

    /// The window's events the page hears about (bridge.js): a file drag over the window (its overlay),
    /// the files dropped, maximized / restored (the maximize button's icon).
    pub fn window_event(&self, ev: &WindowEvent) {
        match ev {
            WindowEvent::DragDrop(DragDropEvent::Enter { .. }) => self.js("window.app.dragState && window.app.dragState('enter')"),
            WindowEvent::DragDrop(DragDropEvent::Leave) => self.js("window.app.dragState && window.app.dragState('leave')"),
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                self.js("window.app.dragState && window.app.dragState('drop')");
                let found = self.collect(paths.iter().cloned());
                if !found.is_empty() {
                    self.js(&format!("window.app && window.app.addPaths({})", json!(found)));
                }
            }
            WindowEvent::Resized(_) => {
                if let Some(w) = self.window.get() {
                    let m = w.is_maximized().unwrap_or(false);
                    if self.maximized.swap(m, Ordering::Relaxed) != m {
                        self.js(&format!("window.app && window.app.winState({m})"));
                    }
                }
            }
            _ => {}
        }
    }
}

/// winchrome.js's edge names -> the resize loop's direction; None: the caption (a move).
fn resize_direction(edge: &str) -> Option<ResizeDirection> {
    Some(match edge {
        "left" => ResizeDirection::West,
        "right" => ResizeDirection::East,
        "top" => ResizeDirection::North,
        "bottom" => ResizeDirection::South,
        "topleft" => ResizeDirection::NorthWest,
        "topright" => ResizeDirection::NorthEast,
        "bottomleft" => ResizeDirection::SouthWest,
        "bottomright" => ResizeDirection::SouthEast,
        _ => return None,
    })
}

/// Show a file in Explorer (selected); the folder elsewhere.
fn reveal(path: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let p = path.replace('/', "\\");
        let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{p}\"")).spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(Path::new(path).parent().unwrap_or(Path::new("."))).spawn();
}

/// The local time as the pasted file's name has it: 2026-11-25 06-29-58.
fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string()
}

/// A copied image (RGBA from the clipboard) as "Pasted <stamp>.png" in `dir` ("… (2).png" when taken):
/// RGB when it is opaque (or its alpha is all zero: a 32-bit bitmap without alpha), RGBA otherwise.
pub fn save_pasted(dir: &Path, w: usize, h: usize, rgba: &[u8], stamp: &str) -> Result<PathBuf, String> {
    if rgba.len() != w * h * 4 || w == 0 || h == 0 {
        return Err("not an image".into());
    }
    let alpha = rgba.chunks_exact(4).map(|p| p[3]);
    let opaque = alpha.clone().all(|a| a == 255) || alpha.clone().all(|a| a == 0);
    let png = if opaque {
        let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        backend::encode::encode_png(backend::px::View::new(w, h, 3, &rgb), None)
    } else {
        backend::encode::encode_png(backend::px::View::new(w, h, 4, rgba), None)
    }
    .map_err(|e| e.message)?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut path = dir.join(format!("Pasted {stamp}.png"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Pasted {stamp} ({n}).png"));
        n += 1;
    }
    std::fs::write(&path, png).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_images() {
        let d = std::env::temp_dir().join(format!("fae-app-paste-{}", std::process::id()));
        let opaque: Vec<u8> = (0..6).flat_map(|i| [i * 40, 10, 200, 255]).collect();
        let p1 = save_pasted(&d, 3, 2, &opaque, "2026-01-02 03-04-05").unwrap();
        let p2 = save_pasted(&d, 3, 2, &opaque, "2026-01-02 03-04-05").unwrap();
        assert!(p1.ends_with("Pasted 2026-01-02 03-04-05.png") && p2.ends_with("Pasted 2026-01-02 03-04-05 (2).png"));
        let mut half = opaque.clone();
        half[3] = 128;
        let p3 = save_pasted(&d, 3, 2, &half, "x").unwrap();
        // PNG colour type: 2 = RGB, 6 = RGBA
        assert_eq!(std::fs::read(&p1).unwrap()[25], 2);
        assert_eq!(std::fs::read(&p3).unwrap()[25], 6);
        assert!(save_pasted(&d, 3, 3, &opaque, "y").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn edges() {
        assert!(resize_direction("caption").is_none());
        assert!(matches!(resize_direction("bottomright"), Some(ResizeDirection::SouthEast)));
    }
}
