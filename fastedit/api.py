"""JS API exposed to the UI (window.pywebview.api.*)."""
from __future__ import annotations

import io
import json
import os
import subprocess
import sys
import numpy as np
from PIL import Image
from .config import HIRES_MAX_MP, IMAGE_EXTS, PRESETS_PATH, PREVIEW_MAX, SETTINGS_PATH, StageParams
from .geometry import fit_scale
from . import core
from .engine import ENGINE, Entry, collect
from .proc import Cancelled, _PREV, _TL, check_cancel, preview_begin
from .jpeg import jpegli_available
from .avif import avif_available, svt_available
from .output import encode_lossy, norm_fmt, save_image


# ---------------------------------------------------------------------------
# JS API (window.pywebview.api.*)
# ---------------------------------------------------------------------------
def _pv_size(w, h, full=False):
    """Preview size: PREVIEW_MAX on the long side, or full resolution (capped at HIRES_MAX_MP) when zoomed in."""
    if full:
        s = min(1.0, (HIRES_MAX_MP / (w * h)) ** 0.5)
    else:
        s = min(1.0, PREVIEW_MAX / max(w, h))
    return (max(1, round(w * s)), max(1, round(h * s))) if s < 1 else (w, h)


_LOSSY = {"key": None}      # last lossy round trip, so a preview-size change doesn't re-encode


class Api:
    def __init__(self, initial_paths=None):
        self._window = None
        self._chrome = None          # winchrome.Chrome when the window is frameless (Windows); private: not exposed to JS
        self._initial = collect(initial_paths or [])

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

    # ---- settings ---------------------------------------------------------
    def get_settings(self):
        try:
            with open(SETTINGS_PATH, "r", encoding="utf-8") as f:
                return json.load(f)
        except Exception:
            return {}

    def save_settings(self, data):
        try:
            with open(SETTINGS_PATH, "w", encoding="utf-8") as f:
                json.dump(data, f, indent=2)
        except Exception:
            pass
        return True

    def get_presets(self):
        """The user's tool presets: {tool: [{name, values}]}."""
        try:
            with open(PRESETS_PATH, "r", encoding="utf-8") as f:
                data = json.load(f)
            return data if isinstance(data, dict) else {}
        except Exception:
            return {}

    def save_presets(self, data):
        try:
            tmp = PRESETS_PATH + ".tmp"           # write whole, then swap: a crash can't leave half a file
            with open(tmp, "w", encoding="utf-8") as f:
                json.dump(data, f, indent=2)
            os.replace(tmp, PRESETS_PATH)
            return True
        except Exception:
            return False

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
    def load_stage(self, path, params, req_id, full=False):
        try:
            P = StageParams.from_dict(params)
            e, img = ENGINE.stage(path, P)
            rgb = np.ascontiguousarray(np.asarray(img.convert("RGB")))
            w, h = img.size
            pv = img.convert("RGB")
            if _pv_size(w, h, full) != (w, h):
                pv = pv.resize(_pv_size(w, h, full), Image.Resampling.LANCZOS)
            pw, ph = pv.size
            rgba = np.empty((ph, pw, 4), dtype=np.uint8)
            rgba[..., :3] = np.asarray(pv)
            rgba[..., 3] = 255
            token = ENGINE.put_preview(rgba.tobytes(), pw, ph)   # the preview core does Mid/Detail itself
            with ENGINE.lock:
                if req_id >= ENGINE.latest_req:
                    ENGINE.latest_req = req_id
                    ENGINE.current = {"path": path, "rgb": rgb, "req": req_id}
            return {
                "ok": True, "req_id": req_id, "token": token,
                "w": w, "h": h, "pw": pw, "ph": ph, "lo_pw": _pv_size(w, h)[0], "full": bool(full),
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
            img = e.base.convert("RGB")
            W, H = img.size
            if max(W, H) > PREVIEW_MAX:
                s = PREVIEW_MAX / max(W, H)
                img = img.resize((max(1, round(W * s)), max(1, round(H * s))), Image.Resampling.BILINEAR)
            pw, ph = img.size
            rgba = np.empty((ph, pw, 4), dtype=np.uint8)
            rgba[..., :3] = np.asarray(img)
            rgba[..., 3] = 255
            token = ENGINE.put_preview(rgba.tobytes(), pw, ph)
            return {"ok": True, "token": token, "W": W, "H": H, "pw": pw, "ph": ph}
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
    def _render_full(job) -> tuple[Entry, Image.Image]:
        """Stage + point correction + curves at full resolution (what Process writes)."""
        P = StageParams.from_dict(job.get("stage"))
        e, img = ENGINE.stage(job.get("path"), P)
        alpha = img.getchannel("A") if img.mode == "RGBA" else None
        rgb = np.ascontiguousarray(np.asarray(img.convert("RGB")))
        out = Image.fromarray(core.render(rgb, job.get("edit")))     # Rust core where it can, else NumPy
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

    def lossy_preview(self, job, fmt, opts, req_id, full=False):
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
                e, out = self._render_full(job)
                check_cancel()
                data, enc, used = encode_lossy(out, fmt, opts, e.icc)
                check_cancel()
                with Image.open(io.BytesIO(data)) as jim:
                    jim.load()
                    dec = jim.convert("RGB")
                nbytes = len(data)
                _LOSSY.update(key=key, val=(dec, nbytes, enc, used))
            w, h = dec.size
            pv = dec if _pv_size(w, h, full) == (w, h) else dec.resize(_pv_size(w, h, full), Image.Resampling.LANCZOS)
            pw, ph = pv.size
            rgba = np.empty((ph, pw, 4), dtype=np.uint8)
            rgba[..., :3] = np.asarray(pv)
            rgba[..., 3] = 255
            token = ENGINE.put_preview(rgba.tobytes(), pw, ph)
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
                e, out = self._render_full(job)
                out_path = save_image(out, path, fmt, e.icc, opts)
                r = {"path": path, "ok": True, "out": out_path}
            except Exception as ex:
                r = {"path": path, "ok": False, "error": f"{type(ex).__name__}: {ex}"}
            results.append(r)
            if self._window is not None and n > 1:
                try:
                    self._window.evaluate_js(
                        f"window.onProcessProgress && window.onProcessProgress({i + 1},{n},{json.dumps(r)})")
                except Exception:
                    pass
        return results
