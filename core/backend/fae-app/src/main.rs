//! Fae.exe: the Tauri shell around the Rust back end (B7, docs/rust-backend-plan.md). It does what the
//! pywebview shell (fastedit/app.py, api.py, winchrome.py) did, nothing more:
//! - the window (frameless on Windows: the UI's top bar is the title bar; --native-frame keeps the frame);
//! - the page and its pixels: the `fae` protocol answers as the B6 local server did (Backend::serve: the
//!   UI files, /preview/<token>, /thumb; the same headers, cross-origin isolation included);
//! - the commands: one Tauri command, `call(name, args)`, runs the back end's command table; the shell's own
//!   commands (window buttons, dialogs, Ctrl+V, reveal in Explorer, Process with its progress) are here;
//! - the shell's calls into the page go through the globals bridge.js defines (window.app.addPaths, …).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod paths;
mod shell;

use std::sync::Arc;
use tauri::http;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

fn main() {
    let opts = paths::Options::from_args(std::env::args().skip(1));
    let dirs = paths::Dirs::find();
    dirs.adopt_old_settings();
    let b = Arc::new(backend::Backend::new(backend::Config {
        settings: dirs.settings(),
        presets: dirs.presets(),
        heif_dir: dirs.heif(),
        encoders_dir: Some(dirs.encoders()),
        no_gpu: opts.no_gpu,
        ui_dir: Some(dirs.ui()),
    }));
    b.set_initial(&opts.files);
    b.warm_gpu(); // the GPU core's shaders, in the background
    let sh = Arc::new(shell::Shell::new(b.clone(), &opts, dirs.paste()));
    let coi = opts.coi;

    let proto = b.clone();
    let events = sh.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(sh.clone())
        .register_asynchronous_uri_scheme_protocol("fae", move |_ctx, req, responder| {
            let b = proto.clone();
            let target = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
            let method = req.method().clone();
            std::thread::spawn(move || {
                let r = if method == http::Method::GET { b.serve(&target) } else { backend::app::Response::text(501, "only GET") };
                let mut res = http::Response::builder().status(r.status);
                for (k, v) in r.headers(coi) {
                    res = res.header(k, v);
                }
                responder.respond(res.body(r.body.to_vec()).unwrap());
            });
        })
        .invoke_handler(tauri::generate_handler![shell::call])
        .setup(move |app| {
            let url = page_url(&opts.page)?;
            let w = WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol(url))
                .title("Fae")
                .inner_size(1560.0, 940.0)
                .min_inner_size(1240.0, 680.0)
                .decorations(!sh.frameless())
                .background_color(tauri::window::Color(0x23, 0x23, 0x26, 0xff))
                .build()?;
            if opts.debug {
                w.open_devtools();
            }
            sh.set_window(app.get_webview_window("main").expect("the main window"));
            Ok(())
        })
        .on_window_event(move |_w, ev| events.window_event(ev))
        .run(tauri::generate_context!())
        .expect("Fae: the window could not start");
}

/// The page on the `fae` protocol. WebView2 (Windows) and Android reach custom protocols as
/// http://<name>.localhost/, the others as <name>://localhost/.
fn page_url(page: &str) -> Result<tauri::Url, Box<dyn std::error::Error>> {
    let base = if cfg!(any(windows, target_os = "android")) { "http://fae.localhost/" } else { "fae://localhost/" };
    Ok(tauri::Url::parse(base)?.join(page)?)
}
