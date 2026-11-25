"""Local HTTP server and program entry point."""
from __future__ import annotations

import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse
from .config import UI_DIR
from .engine import collect, make_thumb
from .api import Api
from . import backend


# ---------------------------------------------------------------------------
# Local HTTP server: UI files, raw preview pixels, thumbnails
# ---------------------------------------------------------------------------
MIME = {".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8",
        ".css": "text/css; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png",
        ".wasm": "application/wasm"}

# Cross-origin isolation: lets the page use SharedArrayBuffer, which the multithreaded WebAssembly
# preview needs. Everything is served from this server, so nothing cross-origin gets blocked.
# FASTEDIT_COI=0 turns it off (the self-test page then shows "not isolated").
COI = os.environ.get("FASTEDIT_COI", "1") != "0"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, body: bytes, ctype: str, cache=False):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "max-age=3600" if cache else "no-store")
        if COI:
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
            self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        u = urlparse(self.path)
        p = unquote(u.path)
        try:
            if p.startswith("/preview/"):
                data = backend.shared().preview(p[len("/preview/"):])
                if data is None:
                    return self._send(404, b"gone", "text/plain")
                return self._send(200, data, "application/octet-stream")
            if p == "/thumb":
                path = parse_qs(u.query).get("p", [""])[0]
                return self._send(200, make_thumb(path), "image/jpeg", cache=True)
            rel = "index.html" if p in ("/", "") else p.lstrip("/")
            full = os.path.normpath(os.path.join(UI_DIR, rel))
            if not full.startswith(os.path.normpath(UI_DIR)) or not os.path.isfile(full):
                return self._send(404, b"not found", "text/plain")
            with open(full, "rb") as f:
                body = f.read()
            return self._send(200, body, MIME.get(os.path.splitext(full)[1], "application/octet-stream"))
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as ex:
            try:
                self._send(500, str(ex).encode(), "text/plain")
            except Exception:
                pass


def start_server() -> int:
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    srv.daemon_threads = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv.server_address[1]


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------
def main():
    import webview

    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    debug = "--debug" in sys.argv
    selftest = "--selftest" in sys.argv
    page = "selftest.html" if selftest else ""     # Rust core / WASM / isolation checks
    if "--gpu-spike" in sys.argv:                  # GPU core feasibility page (gpu-core-plan, G0)
        selftest, page = True, "gpu-spike.html"
    elif not selftest:
        q = []
        if "--no-gpu" in sys.argv:                 # CPU core only: preview (gpu-view.js) and Process (backend.py)
            q.append("gpu=0")
        if "--test-filters" in sys.argv:           # the filter kit's hidden test filters in the Filters list
            q.append("fxtest=1")
        page = "?" + "&".join(q) if q else ""
    missing = backend.error()                      # the Rust back end (with both cores since B5)
    if missing and not selftest:                   # no Python fallback: say how to build it
        msg = missing.replace("&", "&amp;").replace("<", "&lt;")
        webview.create_window("Fae", html=(
            "<body style='background:#0e0f11;color:#ddd;font:15px system-ui;margin:40px'>"
            "<h2 style='font-weight:600'>The Rust core is missing</h2>"
            f"<p>{msg}</p><p style='color:#888'>Then start the editor again.</p></body>"),
            width=760, height=300, background_color="#0e0f11")
        webview.start()
        return
    api = Api(args)
    backend.shared().warm_gpu()                    # the GPU core's shaders, in the background
    port = start_server()
    # unified header: on Windows the window has no frame and the UI's top bar is the title bar
    # (fastedit/winchrome.py); --native-frame keeps the normal Windows frame
    from .winchrome import IS_WIN, Chrome
    frameless = IS_WIN and "--native-frame" not in sys.argv and not selftest
    window = webview.create_window(
        "Fae", f"http://127.0.0.1:{port}/{page}", js_api=api,
        width=1560, height=940, min_size=(1240, 680), background_color="#232326",
        frameless=frameless, easy_drag=False,
    )
    api.set_window(window)
    if frameless:
        api._chrome = Chrome(window)
        window.events.shown += api._chrome.setup

    def bind_drop():
        try:
            from webview.dom import DOMEventHandler
        except Exception:
            return

        def on_drop(e):
            files = (e.get("dataTransfer") or {}).get("files") or []
            paths = [f.get("pywebviewFullPath") for f in files if f.get("pywebviewFullPath")]
            found = collect(paths)
            if found:
                window.evaluate_js(f"window.app && window.app.addPaths({json.dumps(found)})")

        # pywebview serialises the whole event (deep DOM copy of target/currentTarget) for every
        # event it forwards. Bound to `document` that meant the entire page per event - and
        # dragover fires continuously. So: only `drop`, and only on an empty overlay (#dropZone)
        # that the UI shows while a file drag is over the window. dragenter/dragover
        # preventDefault is done in plain JS (ui/js/tools.js).
        zone = window.dom.get_element("#dropZone")
        if zone is not None:
            zone.events.drop += DOMEventHandler(on_drop, True, False)

    window.events.loaded += bind_drop
    webview.start(debug=debug)


if __name__ == "__main__":
    main()
