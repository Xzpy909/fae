"""JS API exposed to the UI (window.pywebview.api.*)."""
from __future__ import annotations

import io
import json
import os
import subprocess
import sys
import time
import numpy as np
from PIL import Image
from .config import IMAGE_EXTS, PASTE_DIR, PRESETS_PATH, PREVIEW_MAX, SETTINGS_PATH, StageParams
from .geometry import fit_scale
from . import backend, core
from .engine import ENGINE, Entry, collect
from .proc import Cancelled, _PREV, _TL, check_cancel, preview_begin
from .jpeg import jpegli_available
from .avif import avif_available, svt_available
from .output import encode_lossy, norm_fmt, save_image


# ---------------------------------------------------------------------------
# JS API (window.pywebview.api.*)
# ---------------------------------------------------------------------------
def _pv_size(w, h, full=0):
    """Preview size: full resolution when the GPU draws the preview (`full` = the most pixels its
    device takes, the image is scaled down only beyond that), else PREVIEW_MAX on the long side."""
    if full:
        s = min(1.0, (float(full) / (w * h)) ** 0.5)
    else:
        s = min(1.0, PREVIEW_MAX / max(w, h))
    return (max(1, round(w * s)), max(1, round(h * s))) if s < 1 else (w, h)


def _preview_rgba(img: Image.Image, size, resample) -> bytes:
    """The preview's pixels: RGB at `size`, alpha the image's own (255 without one). RGB is resized as it
    always was (the cores edit RGB; transparency only decides what shows through), alpha on its own."""
    rgb = img.convert("RGB")
    if rgb.size != tuple(size):
        rgb = rgb.resize(size, resample)
    pw, ph = rgb.size
    rgba = np.empty((ph, pw, 4), dtype=np.uint8)
    rgba[..., :3] = np.asarray(rgb)
    if img.mode == "RGBA":
        a = img.getchannel("A")
        rgba[..., 3] = np.asarray(a if a.size == (pw, ph) else a.resize((pw, ph), resample))
    else:
        rgba[..., 3] = 255
    return rgba.tobytes()


_LOSSY = {"key": None}      # last lossy round trip, so a preview-size change doesn't re-encode


class Api:
    def __init__(self, initial_paths=None):
        self._window = None
        self._chrome = None          # winchrome.Chrome when the window is frameless (Windows); private: not exposed to JS
        self._initial = collect(initial_paths or [])
        self._b = backend.Backend(SETTINGS_PATH, PRESETS_PATH)      # the Rust back end (private: not exposed to JS)

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

    def initial_files(self):
        return self._initial

    # ---- files ------------------------------------------------------------
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
        return collect(list(res or []))

    def pick_folder(self):
        res = self._dialog("folder")
        return collect(list(res or []))

    def expand_paths(self, paths):
        return collect(paths)

    def paste_clipboard(self):
        """Ctrl+V: the images on the clipboard. Files copied in Explorer -> those images (folders expanded);
        a copied image (screenshot, from a browser...) has no file: saved as PNG in PASTE_DIR (alpha kept
        when the clipboard has it), then opened. {"ok", "paths", "saved"} or {"ok": False, "error"}."""
        try:
            from PIL import ImageGrab
            clip = ImageGrab.grabclipboard()
        except Exception as ex:
            return {"ok": False, "error": f"Clipboard: {type(ex).__name__}: {ex}"}
        if isinstance(clip, list):
            paths = collect(clip)
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

    # ---- stage / preview --------------------------------------------------
    def load_stage(self, path, params, req_id, full=0):
        try:
            P = StageParams.from_dict(params)
            e, img = ENGINE.stage(path, P)
            rgb = np.ascontiguousarray(np.asarray(img.convert("RGB")))
            w, h = img.size
            pw, ph = _pv_size(w, h, full)
            # RGBA: the alpha (transparent images) only shows; the cores edit RGB
            token = ENGINE.put_preview(_preview_rgba(img, (pw, ph), Image.Resampling.LANCZOS), pw, ph)
            with ENGINE.lock:
                if req_id >= ENGINE.latest_req:
                    ENGINE.latest_req = req_id
                    ENGINE.current = {"path": path, "rgb": rgb, "req": req_id}
            return {
                "ok": True, "req_id": req_id, "token": token,
                "w": w, "h": h, "pw": pw, "ph": ph, "full": float(full or 0),
                "src_w": e.base.size[0], "src_h": e.base.size[1],
                "alpha": img.mode == "RGBA",
                "scale": round(fit_scale(e.geometried(P).size, P), 4),
                "geo_w": e.geometried(P).size[0], "geo_h": e.geometried(P).size[1],
            }
        except Exception as ex:
            return {"ok": False, "req_id": req_id, "error": f"{type(ex).__name__}: {ex}"}

    def load_source(self, path):
        """Oriented, un-cropped source for crop mode (downscaled for display)."""
        try:
            e = ENGINE.entry(path)
            img = e.base
            W, H = img.size
            pw, ph = W, H
            if max(W, H) > PREVIEW_MAX:
                s = PREVIEW_MAX / max(W, H)
                pw, ph = max(1, round(W * s)), max(1, round(H * s))
            token = ENGINE.put_preview(_preview_rgba(img, (pw, ph), Image.Resampling.BILINEAR), pw, ph)
            return {"ok": True, "token": token, "W": W, "H": H, "pw": pw, "ph": ph, "alpha": img.mode == "RGBA"}
        except Exception as ex:
            return {"ok": False, "error": f"{type(ex).__name__}: {ex}"}

    def sample(self, nx, ny):
        """3x3 median of the full-resolution stage image (input to point correction)."""
        cur = ENGINE.current
        if not cur:
            return None
        a = cur["rgb"]
        h, w = a.shape[:2]
        x = int(min(w - 1, max(0, nx * w)))
        y = int(min(h - 1, max(0, ny * h)))
        patch = a[max(0, y - 1):min(h, y + 2), max(0, x - 1):min(w, x + 2)]
        return [int(v) for v in np.median(patch.reshape(-1, 3), axis=0)]

    # ---- processing -------------------------------------------------------
    @staticmethod
    def _render_full(job, skipped: list | None = None) -> tuple[Entry, Image.Image]:
        """The stage + the whole edit at full resolution (what Process writes): the GPU core when it
        can, else the CPU core (core.render; it adds the filter kit's filters it left out to `skipped`)."""
        P = StageParams.from_dict(job.get("stage"))
        e, img = ENGINE.stage(job.get("path"), P)
        alpha = img.getchannel("A") if img.mode == "RGBA" else None
        rgb = np.ascontiguousarray(np.asarray(img.convert("RGB")))
        out = Image.fromarray(core.render(rgb, job.get("edit"), skipped=skipped))
        if alpha is not None:
            out.putalpha(alpha)
        return e, out

    def core_info(self):
        """Rust core status + a quick NumPy comparison (self-test page)."""
        try:
            return core.selftest()
        except Exception as ex:
            return {"ok": False, "error": f"{type(ex).__name__}: {ex}"}

    def encoder_info(self):
        return {"jpegli": jpegli_available(), "avif": avif_available(), "svt": svt_available()}

    def lossy_cancel(self, req_id):
        """Stop the running lossy preview encode (the encoded compare was turned off)."""
        preview_begin(int(req_id))
        return True

    def lossy_preview(self, job, fmt, opts, req_id, full=0):
        """Encode the fully edited image with the lossy format, decode it again, hand back preview pixels."""
        preview_begin(req_id)
        _TL.preview_req = req_id
        try:
            check_cancel()
            fmt = norm_fmt(fmt)
            key = json.dumps([job, fmt, opts], sort_keys=True, default=str)
            if _LOSSY["key"] == key:
                dec, nbytes, enc, used = _LOSSY["val"]
            else:
                try:
                    e, out = self._render_full(job)
                finally:
                    core.gpu_release()          # the GPU memory of a full-size image isn't kept
                check_cancel()
                data, enc, used = encode_lossy(out, fmt, opts, e.icc)
                check_cancel()
                with Image.open(io.BytesIO(data)) as jim:
                    jim.load()      # the encoded file's own alpha (JPEG: none, the white it was written on)
                    dec = jim.convert("RGBA" if "A" in jim.getbands() or "transparency" in jim.info else "RGB")
                nbytes = len(data)
                _LOSSY.update(key=key, val=(dec, nbytes, enc, used))
            w, h = dec.size
            pw, ph = _pv_size(w, h, full)
            token = ENGINE.put_preview(_preview_rgba(dec, (pw, ph), Image.Resampling.LANCZOS), pw, ph)
            return {"ok": True, "req_id": req_id, "token": token, "pw": pw, "ph": ph,
                    "bytes": nbytes, "encoder": enc, "fmt": fmt, "opts": used}
        except Cancelled:
            return {"ok": False, "req_id": req_id, "cancelled": True}
        except Exception as ex:
            if req_id < _PREV["latest"]:
                return {"ok": False, "req_id": req_id, "cancelled": True}
            return {"ok": False, "req_id": req_id, "error": f"{type(ex).__name__}: {ex}"}
        finally:
            _TL.preview_req = None

    def process(self, jobs, fmt="WEBP", opts=None):
        fmt = norm_fmt(fmt)
        results = []
        n = len(jobs or [])
        for i, job in enumerate(jobs or []):
            path = job.get("path")
            try:
                skipped = []
                e, out = self._render_full(job, skipped)
                out_path = save_image(out, path, fmt, e.icc, opts)
                r = {"path": path, "ok": True, "out": out_path}
                if skipped:                      # no GPU: the filter kit's filters were left out
                    r["skipped"] = skipped
            except Exception as ex:
                r = {"path": path, "ok": False, "error": f"{type(ex).__name__}: {ex}"}
            results.append(r)
            if self._window is not None and n > 1:
                try:
                    self._window.evaluate_js(
                        f"window.onProcessProgress && window.onProcessProgress({i + 1},{n},{json.dumps(r)})")
                except Exception:
                    pass
        core.gpu_release()                       # same-size images reused it; now give it back
        return results
