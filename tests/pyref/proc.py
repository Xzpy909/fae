"""Running external encoders: hidden window, low priority, cancellable live-preview encodes."""
from __future__ import annotations

import subprocess
import sys
import threading


# ---------------------------------------------------------------------------
# JPEG (jpegli via encoders/cjpegli.exe, Pillow/libjpeg-turbo as fallback)
# ---------------------------------------------------------------------------
def _no_window() -> dict:
    # pythonw has no console; without this every cjpegli call would flash one
    return {"creationflags": 0x08000000} if sys.platform.startswith("win") else {}


# ---- live-preview encodes: low priority, and killed as soon as a newer preview is requested ----
from fastedit.backend import Cancelled  # noqa: E402  (the back end raises the same one)


_TL = threading.local()                    # .preview_req is set while a preview encode runs
_PREV_LOCK = threading.Lock()
_PREV = {"latest": -1, "procs": {}}


def _preview_req():
    return getattr(_TL, "preview_req", None)


def preview_begin(req: int):
    """Register a new preview request and kill encoder processes of older ones."""
    with _PREV_LOCK:
        if req > _PREV["latest"]:
            _PREV["latest"] = req
        for r, p in list(_PREV["procs"].items()):
            if r < _PREV["latest"]:
                try:
                    p.kill()
                except Exception:
                    pass


def check_cancel():
    r = _preview_req()
    if r is not None and r < _PREV["latest"]:
        raise Cancelled()


def run_encoder(cmd: list[str]):
    """subprocess.run replacement: preview encodes run at below-normal priority and are killable."""
    from types import SimpleNamespace
    req = _preview_req()
    kw = _no_window()
    if req is not None and sys.platform.startswith("win"):
        kw = {"creationflags": 0x08000000 | 0x00004000}     # CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS
    check_cancel()
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kw)
    if req is not None:
        with _PREV_LOCK:
            _PREV["procs"][req] = p
            if req < _PREV["latest"]:
                p.kill()
    try:
        out, err = p.communicate()
    finally:
        if req is not None:
            with _PREV_LOCK:
                _PREV["procs"].pop(req, None)
    check_cancel()
    return SimpleNamespace(returncode=p.returncode, stdout=out, stderr=err)
