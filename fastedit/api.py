"""JS API exposed to the UI (window.pywebview.api.*)."""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from .config import IMAGE_EXTS, PASTE_DIR
from . import backend


class Api:
    def __init__(self, initial_paths=None):
        self._window = None
        self._chrome = None          # winchrome.Chrome when the window is frameless (Windows); private: not exposed to JS
        self._b = backend.shared()      # the Rust back end (private: not exposed to JS; it also serves the UI)
        self._b.set_initial(initial_paths or [])

    def set_window(self, w):
        self._window = w

    # ---- unified header: window controls (frameless window on Windows) ----
    def win_info(self):
        return self._chrome.info() if self._chrome else {"frameless": False, "maximized": False}

    def win_begin(self, what):
        if self._chrome:
            self._chrome.begin(what)

    def win_toggle_max(self):
        if self._chrome:
            self._chrome.toggle_max()

    def win_minimize(self):
        if self._chrome:
            self._chrome.minimize()

    def win_close(self):
        if self._chrome:
            self._chrome.close()

    # ---- settings (Rust back end, fae-backend store.rs) -------------------
    def get_settings(self):
        return self._b.call("get_settings")

    def save_settings(self, data):
        return self._b.call("save_settings", data)

    def get_presets(self):
        """The user's tool presets: {tool: [{name, values}]} and the edit presets."""
        return self._b.call("get_presets")

    def save_presets(self, data):
        return self._b.call("save_presets", data)

    # ---- files (Rust back end, fae-backend app.rs, B6; the dialogs and the clipboard stay here) ----
    def initial_files(self):
        return self._b.call("initial_files")

    def _dialog(self, kind, multiple=False):
        import webview
        fd = getattr(webview, "FileDialog", None)
        if kind == "open":
            t = fd.OPEN if fd else webview.OPEN_DIALOG
            types = ("Images (" + ";".join(f"*{e}" for e in sorted(IMAGE_EXTS)) + ")", "All files (*.*)")
            return self._window.create_file_dialog(t, allow_multiple=multiple, file_types=types)
        t = fd.FOLDER if fd else webview.FOLDER_DIALOG
        return self._window.create_file_dialog(t)

    def pick_files(self):
        res = self._dialog("open", True)
        return self._b.collect(list(res or []))

    def pick_folder(self):
        res = self._dialog("folder")
        return self._b.collect(list(res or []))

    def expand_paths(self, paths):
        return self._b.call("expand_paths", paths or [])

    def paste_clipboard(self):
        """Ctrl+V: the images on the clipboard. Files copied in Explorer -> those images (folders expanded);
        a copied image (screenshot, from a browser...) has no file: saved as PNG in PASTE_DIR (alpha kept
        when the clipboard has it), then opened. {"ok", "paths", "saved"} or {"ok": False, "error"}."""
        try:
            from PIL import Image, ImageGrab
            clip = ImageGrab.grabclipboard()
        except Exception as ex:
            return {"ok": False, "error": f"Clipboard: {type(ex).__name__}: {ex}"}
        if isinstance(clip, list):
            paths = self._b.collect([str(p) for p in clip])
            if not paths:
                return {"ok": False, "error": "No images among the copied files"}
            return {"ok": True, "paths": paths, "saved": None}
        if isinstance(clip, Image.Image):
            try:
                clip.load()
                img = clip if clip.mode in ("RGB", "RGBA") else clip.convert(
                    "RGBA" if "A" in clip.getbands() or "transparency" in clip.info else "RGB")
                os.makedirs(PASTE_DIR, exist_ok=True)
                stamp = time.strftime("%Y-%m-%d %H-%M-%S")
                path = os.path.join(PASTE_DIR, f"Pasted {stamp}.png")
                n = 2
                while os.path.exists(path):
                    path = os.path.join(PASTE_DIR, f"Pasted {stamp} ({n}).png")
                    n += 1
                img.save(path, "PNG")
                return {"ok": True, "paths": [path], "saved": path}
            except Exception as ex:
                return {"ok": False, "error": f"Saving the pasted image: {type(ex).__name__}: {ex}"}
        return {"ok": False, "error": "No image on the clipboard"}

    def reveal(self, path):
        try:
            if sys.platform.startswith("win"):
                subprocess.Popen(["explorer", "/select,", os.path.normpath(path)])
            elif sys.platform == "darwin":
                subprocess.Popen(["open", "-R", path])
            else:
                subprocess.Popen(["xdg-open", os.path.dirname(path)])
        except Exception:
            pass
        return True

    # ---- stage / preview (Rust back end, fae-backend stage.rs, B3) -------
    def load_stage(self, path, params, req_id, full=0):
        try:
            return self._b.stage_call("load_stage", path, params, req_id, full or 0)
        except Exception as ex:
            return {"ok": False, "req_id": req_id, "error": f"{type(ex).__name__}: {ex}"}

    def load_source(self, path):
        """Oriented, un-cropped source for crop mode (downscaled for display)."""
        try:
            return self._b.stage_call("load_source", path)
        except Exception as ex:
            return {"ok": False, "error": f"{type(ex).__name__}: {ex}"}

    def sample(self, nx, ny):
        """3x3 median of the full-resolution stage image (input to point correction)."""
        return self._b.call("sample", nx, ny)

    # ---- processing (Rust back end, fae-backend render.rs, B5) -------------
    def core_info(self):
        """The cores' self-test (self-test page): the CPU core runs, the GPU core gives the same."""
        try:
            return self._b.call("core_info")
        except Exception as ex:
            return {"ok": False, "error": f"{type(ex).__name__}: {ex}"}

    def encoder_info(self):
        return self._b.call("encoder_info")             # B4: Rust (cjpegli / SvtAv1EncApp found, AVIF built in)

    def lossy_cancel(self, req_id):
        """Stop the running lossy preview encode (the encoded compare was turned off)."""
        return self._b.call("lossy_cancel", req_id)

    def lossy_preview(self, job, fmt, opts, req_id, full=0):
        """Encode the fully edited image with the lossy format, decode it again, hand back preview pixels."""
        try:
            return self._b.call("lossy_preview", job, fmt, opts, req_id, full or 0)
        except Exception as ex:
            return {"ok": False, "req_id": req_id, "error": f"{type(ex).__name__}: {ex}"}

    def process_cancel(self):
        """Stop the running Process batch: the images not written yet answer {"ok": False, "cancelled": True}."""
        return self._b.call("process_cancel")

    def process(self, jobs, fmt="WEBP", opts=None):
        def progress(i, n, r):
            if self._window is not None:
                try:
                    self._window.evaluate_js(
                        f"window.onProcessProgress && window.onProcessProgress({i},{n},{json.dumps(r)})")
                except Exception:
                    pass
        return self._b.process(jobs, fmt, opts, progress)
