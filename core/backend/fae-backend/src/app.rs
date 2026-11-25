//! App plumbing (wave B6), as fastedit/engine.py `collect` / `make_thumb` and fastedit/app.py's local HTTP
//! server did it:
//! - [`collect`]: paths → the image files in them (folders walked in sorted order, as `os.walk`);
//! - [`Thumbs`]: the file list's thumbnails, cached by path and modification time, two made at a time;
//! - [`serve`]: the shell's HTTP requests → (status, headers, body): the UI files, `/preview/<token>`,
//!   `/thumb?p=<path>`: Fae.exe's `fae` protocol answers with it; [`start_server`] serves the same on
//!   127.0.0.1 (the pywebview shell loaded the UI from it until B7; the tests still use it).

use crate::Backend;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

/// config.py IMAGE_EXTS: the files the file list takes.
pub const IMAGE_EXTS: &[&str] = &[
    ".jpg", ".jpeg", ".png", ".webp", ".bmp", ".gif", ".tif", ".tiff", ".avif", ".heic", ".heif", ".hif", ".jxl",
    ".ico", ".ppm", ".pgm", ".tga",
];

/// config.py THUMB_SIZE × 2: the thumbnails' longest side.
pub const THUMB_MAX: u32 = 224;
const THUMB_CACHE: usize = 1000;

fn is_image(name: &str) -> bool {
    // os.path.splitext: the extension starts at the last dot that isn't leading
    let stem = name.trim_start_matches('.');
    match stem.rfind('.') {
        Some(i) => {
            let ext = stem[i..].to_lowercase();
            IMAGE_EXTS.contains(&ext.as_str())
        }
        None => false,
    }
}

/// os.path.join(root, name)
fn join(root: &str, name: &str) -> String {
    let seps: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    if root.is_empty() || root.ends_with(seps) || (cfg!(windows) && root.len() == 2 && root.ends_with(':')) {
        format!("{root}{name}")
    } else {
        format!("{root}{}{name}", std::path::MAIN_SEPARATOR)
    }
}

/// os.walk(top) in sorted order (dirs.sort(), sorted(names)), without following links to folders.
fn walk(top: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(top) else { return };
    let (mut dirs, mut files) = (vec![], vec![]);
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // os.walk: a folder (or a link to one) goes to dirs, everything else to names
        let is_dir = std::fs::metadata(e.path()).map(|m| m.is_dir()).unwrap_or(false);
        let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        if is_dir {
            if !is_link {
                dirs.push(name);
            }
        } else {
            files.push(name);
        }
    }
    files.sort();
    dirs.sort();
    for n in files {
        if is_image(&n) {
            out.push(join(top, &n));
        }
    }
    for d in dirs {
        walk(&join(top, &d), out);
    }
}

/// engine.py `collect`: files → themselves when they are images, folders → the images in them (all
/// levels, sorted), anything else left out.
pub fn collect(paths: &[String]) -> Vec<String> {
    let mut files = vec![];
    for p in paths {
        if p.is_empty() {
            continue;
        }
        let m = std::fs::metadata(p);
        if m.as_ref().is_ok_and(|m| m.is_dir()) {
            walk(p, &mut files);
        } else if m.is_ok_and(|m| m.is_file()) && is_image(Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).as_deref().unwrap_or("")) {
            files.push(p.clone());
        }
    }
    files
}

/// The paths of a JSON value: a list of strings (anything else left out).
pub fn paths_of(v: &serde_json::Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// The thumbnail cache: (path key, mtime) → JPEG bytes, the most recent last; and the two-at-a-time gate.
#[derive(Default)]
pub struct Thumbs {
    cache: Mutex<(HashMap<(String, Option<SystemTime>), Arc<Vec<u8>>>, VecDeque<(String, Option<SystemTime>)>)>,
    busy: Mutex<usize>,
    free: Condvar,
}

impl Thumbs {
    /// engine.py `make_thumb`: cached by (normcase(abspath), mtime); at most two made at a time.
    pub fn get(&self, path: &str, make: impl FnOnce() -> Result<Vec<u8>, crate::Error>) -> Result<Arc<Vec<u8>>, crate::Error> {
        let key = std::fs::metadata(path).and_then(|m| m.modified()).ok().map(|t| (path_key(path), Some(t)));
        if let Some(k) = &key {
            let mut c = self.cache.lock().unwrap();
            if let Some(d) = c.0.get(k).cloned() {
                c.1.retain(|x| x != k);
                c.1.push_back(k.clone());
                return Ok(d);
            }
        }
        let data = {
            let mut b = self.busy.lock().unwrap();
            while *b >= 2 {
                b = self.free.wait(b).unwrap();
            }
            *b += 1;
            drop(b);
            let r = make();
            *self.busy.lock().unwrap() -= 1;
            self.free.notify_one();
            Arc::new(r?)
        };
        if let Some(k) = key {
            let mut c = self.cache.lock().unwrap();
            if c.0.insert(k.clone(), data.clone()).is_none() {
                c.1.push_back(k);
            }
            while c.1.len() > THUMB_CACHE {
                if let Some(old) = c.1.pop_front() {
                    c.0.remove(&old);
                }
            }
        }
        Ok(data)
    }
}

/// os.path.normcase(os.path.abspath(path))
fn path_key(path: &str) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
    let s = abs.to_string_lossy().into_owned();
    if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s }
}

// ---- the HTTP handlers ------------------------------------------------------------------------------

/// app.py MIME
fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("json") => "application/json",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// urllib's unquote (UTF-8, bad bytes replaced); `plus`: "+" is a space too (parse_qs).
pub fn unquote(s: &str, plus: bool) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(if plus && b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A response: status, content type, cacheable (max-age=3600, else no-store), body.
pub struct Response {
    pub status: u16,
    pub ctype: &'static str,
    pub cache: bool,
    pub body: Arc<Vec<u8>>,
}

impl Response {
    /// A plain-text answer (404s, errors).
    pub fn text(status: u16, msg: &str) -> Response {
        text(status, msg)
    }

    /// The headers app.py's handler sent (Content-Length aside): the type, Cache-Control (an hour for
    /// thumbnails, nothing else kept), and with `coi` the cross-origin isolation ones that let the page use
    /// SharedArrayBuffer (the threaded WebAssembly preview). The local server and the Tauri shell's
    /// protocol both send these.
    pub fn headers(&self, coi: bool) -> Vec<(&'static str, &'static str)> {
        let mut h = vec![("Content-Type", self.ctype), ("Cache-Control", if self.cache { "max-age=3600" } else { "no-store" })];
        if coi {
            h.extend([
                ("Cross-Origin-Opener-Policy", "same-origin"),
                ("Cross-Origin-Embedder-Policy", "require-corp"),
                ("Cross-Origin-Resource-Policy", "same-origin"),
            ]);
        }
        h
    }
}

fn text(status: u16, msg: &str) -> Response {
    Response { status, ctype: "text/plain", cache: false, body: Arc::new(msg.as_bytes().to_vec()) }
}

/// A UI file: `rel` inside `ui_dir` (".." may not leave it), None when it isn't there.
fn ui_file(ui_dir: &Path, rel: &str) -> Option<PathBuf> {
    let mut parts: Vec<&std::ffi::OsStr> = vec![];
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(p) => parts.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None, // a root or a drive: not inside the UI folder
        }
    }
    let full = parts.iter().fold(ui_dir.to_path_buf(), |p, c| p.join(c));
    full.is_file().then_some(full)
}

impl Backend {
    /// app.py's Handler.do_GET: the response to GET `target` (path + query, as the request line has it).
    pub fn serve(&self, target: &str) -> Response {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let path = path.split_once('#').map_or(path, |(p, _)| p);
        let p = unquote(path, false);
        if let Some(token) = p.strip_prefix("/preview/") {
            return match self.preview(token) {
                Some(d) => Response { status: 200, ctype: "application/octet-stream", cache: false, body: d },
                None => text(404, "gone"),
            };
        }
        if p == "/thumb" {
            let file = query.split('&').find_map(|kv| kv.strip_prefix("p=")).map(|v| unquote(v, true)).unwrap_or_default();
            return match self.thumb(&file) {
                Ok(d) => Response { status: 200, ctype: "image/jpeg", cache: true, body: d },
                Err(e) => text(500, &e.message),
            };
        }
        let Some(ui) = &self.cfg.ui_dir else { return text(404, "not found") };
        let rel = if p.is_empty() || p == "/" { "index.html" } else { p.trim_start_matches('/') };
        match ui_file(ui, rel).map(|f| (std::fs::read(&f), f)) {
            Some((Ok(body), f)) => Response { status: 200, ctype: mime(&f), cache: false, body: Arc::new(body) },
            Some((Err(e), _)) => text(500, &e.to_string()),
            None => text(404, "not found"),
        }
    }

    /// The file list's thumbnail of `path` (B2's pixels; cached; the shell's decoder for what Rust can't open).
    pub fn thumb(&self, path: &str) -> Result<Arc<Vec<u8>>, crate::Error> {
        self.thumbs.get(path, || match self.thumb_file(Path::new(path), THUMB_MAX) {
            Err(e) if e.kind == crate::ErrorKind::Unknown && self.fallback.is_some() => {
                let (img, _) = (self.fallback.as_ref().unwrap())(Path::new(path))?;
                let t = crate::thumb::thumbnail(img, (THUMB_MAX as usize, THUMB_MAX as usize));
                crate::thumb::encode_jpeg(&t, 82)
            }
            r => r,
        })
    }
}

fn status_text(s: u16) -> &'static str {
    match s {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        501 => "Not Implemented",
        _ => "Internal Server Error",
    }
}

fn handle(b: &Backend, mut s: TcpStream, coi: bool) -> std::io::Result<()> {
    let mut head = vec![];
    let mut buf = [0u8; 4096];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut buf)?;
        if n == 0 || head.len() > 64 * 1024 {
            return Ok(());
        }
        head.extend_from_slice(&buf[..n]);
    }
    let line = String::from_utf8_lossy(&head[..head.iter().position(|&c| c == b'\r').unwrap_or(0)]).into_owned();
    let mut it = line.split(' ');
    let (method, target) = (it.next().unwrap_or(""), it.next().unwrap_or("/"));
    let r = if method == "GET" { b.serve(target) } else { text(501, "only GET") };
    let mut h = format!("HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n", r.status, status_text(r.status), r.body.len());
    for (k, v) in r.headers(coi) {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    s.write_all(h.as_bytes())?;
    s.write_all(&r.body)?;
    s.flush()
}

/// app.py `start_server`: serve [`Backend::serve`] on 127.0.0.1 at a free port (returned), one thread per
/// connection, for as long as the process runs. `coi`: the cross-origin isolation headers (FASTEDIT_COI=0
/// turns them off in the shell).
pub fn start_server(b: Arc<Backend>, coi: bool) -> std::io::Result<u16> {
    let l = TcpListener::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    std::thread::Builder::new().name("fae-http".into()).spawn(move || {
        for s in l.incoming().flatten() {
            let b = b.clone();
            let _ = std::thread::Builder::new().name("fae-http-conn".into()).spawn(move || {
                let _ = handle(&b, s, coi);
            });
        }
    })?;
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquoting() {
        assert_eq!(unquote("/a%20b/%C3%A9+x", false), "/a b/é+x");
        assert_eq!(unquote("a+b%2Bc%", true), "a b+c%");
        assert_eq!(unquote("%zz%4", false), "%zz%4");
    }

    #[test]
    fn exts_and_ui_paths() {
        assert!(is_image("Starfall.JPG") && is_image("a.b.webp") && !is_image(".png") && !is_image("x.txt"));
        let d = std::env::temp_dir().join(format!("fae-app-test-{}", std::process::id()));
        std::fs::create_dir_all(d.join("js")).unwrap();
        std::fs::write(d.join("js").join("a.js"), "x").unwrap();
        assert!(ui_file(&d, "js/a.js").is_some());
        assert!(ui_file(&d, "js/../js/a.js").is_some());
        assert!(ui_file(&d, "../x").is_none());
        assert!(ui_file(&d, "/etc/passwd").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn collecting() {
        let d = std::env::temp_dir().join(format!("fae-collect-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for p in ["b/2.png", "b/1.JPG", "a/z.webp", "a/sub/q.gif", "top.txt", "top.tif"] {
            let f = d.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, "x").unwrap();
        }
        let s = d.to_string_lossy().into_owned();
        let sep = std::path::MAIN_SEPARATOR;
        let got: Vec<String> = collect(&[s.clone(), format!("{s}{sep}top.tif"), "".into(), format!("{s}{sep}nope.png")])
            .into_iter()
            .map(|p| p[s.len()..].replace(sep, "/"))
            .collect();
        assert_eq!(got, vec!["/top.tif", "/a/z.webp", "/a/sub/q.gif", "/b/1.JPG", "/b/2.png", "/top.tif"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
