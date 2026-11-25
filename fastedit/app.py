"""Program entry point: the window. The local HTTP server it loads runs in the Rust back end (B6)."""
from __future__ import annotations

import json
import os
import sys
from .api import Api
from . import backend


# Cross-origin isolation: lets the page use SharedArrayBuffer, which the multithreaded WebAssembly
# preview needs. Everything is served from the back end's local server (UI files, preview pixels,
# thumbnails: fae-backend app.rs, B6), so nothing cross-origin gets blocked. FASTEDIT_COI=0 turns it off
# (the self-test page then shows "not isolated").
COI = os.environ.get("FASTEDIT_COI", "1") != "0"


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
    port = backend.shared().start_server(COI)
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
            found = backend.shared().collect(paths)
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
