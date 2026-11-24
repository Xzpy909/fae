"""The Rust back end (core/backend, built by core/backend/build_backend.bat; docs/rust-backend-plan.md).

The work of fastedit/ moves into it wave by wave, behind one command table: api.py forwards the commands
it runs (fae_backend.commands()) and keeps the rest in Python until their wave. Required, like the CPU
core: without it the app shows how to build it (app.py) instead of starting.
"""
from __future__ import annotations

import dataclasses
import importlib.util
import json
import os
import sys

REQUIRED = (0, 5)           # extension version this file needs (bump with the API)
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

    def __init__(self, settings: str, presets: str, encoders: str | None = None):
        if _fb is None:
            raise RuntimeError(_ERR)
        self._b = _fb.Backend(settings=settings, presets=presets, heif_dir=heif_dir(), encoders_dir=encoders)

    def call(self, name: str, *args):
        return json.loads(self._b.call(name, json.dumps(list(args))))

    def _with_source(self, path: str, run):
        """run(); when the back end can't open `path` (a format not in Rust yet: AVIF until B4), open it
        with Pillow, hand the pixels over (give_source) and run again."""
        try:
            return run()
        except RuntimeError as ex:
            if not str(ex).startswith("unknown:"):
                raise
        from .engine import decode_pillow
        (px, w, h, ch), icc = decode_pillow(path)
        self._b.give_source(path, px, w, h, ch, icc)
        return run()

    def stage_call(self, name: str, path: str, *args):
        """load_stage / load_source (B3): the command, with Pillow opening the formats not in Rust yet."""
        return self._with_source(path, lambda: self.call(name, path, *args))

    def stage(self, path: str, P):
        """The stage image at full resolution (B3; Process / the encoded compare until B5):
        ((pixels, w, h, channels), icc)."""
        px, w, h, ch, icc = self._with_source(path, lambda: self._b.stage(path, _params(P)))
        return (px, w, h, ch), icc

    def preview(self, token: str):
        """The preview pixels (RGBA bytes) under `token`, None when gone: served at /preview/<token>."""
        return self._b.preview(token)

    def put_preview(self, rgba: bytes) -> str:
        """Store preview pixels made in Python (the encoded compare until B5); their token."""
        return self._b.put_preview(rgba)

    def decode(self, path: str):
        """Open an image in Rust (B1): (pixels, w, h, channels, icc, format), upright; None when the
        format isn't in the back end yet (the caller opens it with Pillow)."""
        try:
            return self._b.decode(path)
        except RuntimeError as ex:
            if str(ex).startswith("unknown:"):
                return None
            raise


    def thumb(self, path: str, size: int):
        """A file-list thumbnail (B2): JPEG bytes; None when the format isn't in the back end yet."""
        try:
            return self._b.thumb(path, size)
        except RuntimeError as ex:
            if str(ex).startswith("unknown:"):
                return None
            raise

    def encode(self, img, fmt: str, opts, icc: bytes | None = None, req: int | None = None):
        """JPEG / AVIF / lossy WebP in Rust (B4; output.py encode_lossy): (bytes, encoder, settings used).
        `img`: a Pillow image (RGB / RGBA); `req`: the encoded compare's request (proc.Cancelled when a
        newer one began). None when the encoder isn't in Rust yet (the caller uses its own)."""
        px, w, h, ch = _raw(img)
        try:
            data, enc, used = self._b.encode(px, w, h, ch, fmt, json.dumps(opts or {}), icc or None, req)
        except RuntimeError as ex:
            return _encode_error(ex)
        return data, enc, json.loads(used)

    def save(self, img, src_path: str, fmt: str, icc: bytes | None = None, opts=None):
        """Encode and write next to the source (B4; output.py save_image): the path written. None when
        the encoder isn't in Rust yet."""
        px, w, h, ch = _raw(img)
        try:
            return self._b.save(px, w, h, ch, src_path, fmt, json.dumps(opts or {}), icc or None)
        except RuntimeError as ex:
            return _encode_error(ex)

    def preview_begin(self, req: int):
        """A new encoded-compare request: the encoder runs of older ones are killed (proc.preview_begin)."""
        self._b.preview_begin(int(req))

    @staticmethod
    def geometry(raw, P):
        """Flip / rotate / crop (B2): raw = (pixels, w, h, channels) in and out; P = StageParams."""
        return _fb.geometry(*raw, _params(P))

    @staticmethod
    def fit(raw, P):
        """The resize step (B2): raw in and out."""
        return _fb.fit(*raw, _params(P))

    @staticmethod
    def fit_scale(size, P) -> float:
        """The largest per-axis factor the resize applies to an image of `size` (> 1: upscaling)."""
        return _fb.fit_scale(size[0], size[1], _params(P))


def _raw(img):
    """A Pillow image as (pixels, w, h, channels), RGB or RGBA."""
    if img.mode not in ("RGB", "RGBA"):
        img = img.convert("RGBA" if "A" in img.getbands() else "RGB")
    return img.tobytes(), img.width, img.height, len(img.getbands())


def _encode_error(ex: RuntimeError):
    msg = str(ex)
    if msg.startswith("unknown:"):
        return None
    if msg.startswith("cancelled:"):
        from .proc import Cancelled
        raise Cancelled() from None
    raise RuntimeError(msg.split(": ", 1)[-1]) from None


def _params(P) -> str:
    return json.dumps(dataclasses.asdict(P))


_SHARED = None


def shared() -> Backend:
    """The app's one back end (its stage cache and previews are shared by api.py and app.py)."""
    global _SHARED
    if _SHARED is None:
        from .config import ENCODERS_DIR, PRESETS_PATH, SETTINGS_PATH
        _SHARED = Backend(SETTINGS_PATH, PRESETS_PATH, ENCODERS_DIR)
    return _SHARED
