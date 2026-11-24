"""The Rust back end (core/backend, built by core/backend/build_backend.bat; claude/rust-backend-plan.md).

The work of fastedit/ moves into it wave by wave, behind one command table: api.py forwards the commands
it runs (fae_backend.commands()) and keeps the rest in Python until their wave. Required, like the CPU
core: without it the app shows how to build it (app.py) instead of starting.
"""
from __future__ import annotations

import importlib.util
import json
import os
import sys

REQUIRED = (0, 2)           # extension version this file needs (bump with the API)
BUILD_HINT = r"run core\backend\build_backend.bat (needs Rust from https://rustup.rs)"

try:
    import fae_backend as _fb
    if tuple(int(v) for v in _fb.version().split(".")[:2]) < REQUIRED:
        raise ImportError(f"fae_backend {_fb.version()} is out of date (need {'.'.join(map(str, REQUIRED))})")
    _ERR = None
except ImportError as ex:
    _fb = None
    _ERR = f"Rust back end: {ex}: {BUILD_HINT}"


def available() -> bool:
    return _fb is not None


def error() -> str | None:
    """Why the back end can't be used, or None."""
    return _ERR


def heif_dir() -> str | None:
    """Where libheif (+ libde265) are, for HEIC: FAE_HEIF_DIR, a heif/ folder next to the app, or the
    libraries pillow-heif ships: Linux / macOS wheels keep them in site-packages/pillow_heif.libs, the
    Windows wheel puts them straight into site-packages (also inside a PyInstaller build)."""
    from .config import APP_DIR
    cands = [os.environ.get("FAE_HEIF_DIR"), os.path.join(APP_DIR, "heif")]
    if getattr(sys, "_MEIPASS", None):          # PyInstaller build: pillow-heif's DLLs in either place
        cands += [os.path.join(sys._MEIPASS, "pillow_heif.libs"), sys._MEIPASS]
    try:
        spec = importlib.util.find_spec("pillow_heif")
        if spec and spec.origin:
            site = os.path.dirname(os.path.dirname(spec.origin))
            cands += [os.path.join(site, "pillow_heif.libs"), site]
    except (ImportError, ValueError):
        pass
    def has_heif(d):
        try:
            return any(n.lower().startswith(("libheif", "heif")) for n in os.listdir(d))
        except OSError:
            return False
    return next((d for d in cands if d and os.path.isdir(d) and has_heif(d)), None)


class Backend:
    """One back end for the app: call(name, *args) runs a command of the table (JSON values in and
    out, as the UI sends and receives them). RuntimeError when the command fails."""

    def __init__(self, settings: str, presets: str):
        if _fb is None:
            raise RuntimeError(_ERR)
        self._b = _fb.Backend(settings=settings, presets=presets, heif_dir=heif_dir())

    def call(self, name: str, *args):
        return json.loads(self._b.call(name, json.dumps(list(args))))

    def decode(self, path: str):
        """Open an image in Rust (B1): (pixels, w, h, channels, icc, format), upright; None when the
        format isn't in the back end yet (the caller opens it with Pillow)."""
        try:
            return self._b.decode(path)
        except RuntimeError as ex:
            if str(ex).startswith("unknown:"):
                return None
            raise


_SHARED = None


def shared() -> Backend:
    """The back end for work that needs no settings paths (decoding)."""
    global _SHARED
    if _SHARED is None:
        from .config import PRESETS_PATH, SETTINGS_PATH
        _SHARED = Backend(SETTINGS_PATH, PRESETS_PATH)
    return _SHARED
