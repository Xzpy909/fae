"""WEBP output, lossy-format dispatch and saving.

B4 (docs/rust-backend-plan.md): encode_lossy and save_image run in the Rust back end; the Python versions
(encode_lossy_py, save_image_py, with jpeg.py and avif.py) stay as the reference of
tests/backend/encoders_check.py. B5: the app no longer uses this file (Process and the encoded compare run
in the Rust back end); it goes in the B5 clean-up.
"""
from __future__ import annotations

import io
import os
from PIL import Image
from .config import EXTS, FORMATS, WEBP_Q_DEFAULT
from .jpeg import encode_jpeg, jpeg_opts
from .avif import _clampi, avif_opts, encode_avif


# ---------------------------------------------------------------------------
# WEBP (lossless default, lossy optional)
# ---------------------------------------------------------------------------
def webp_opts(o) -> dict:
    o = o if isinstance(o, dict) else {}
    return {"lossless": o.get("lossless", True) is not False,
            "q": _clampi(o.get("q"), 0, 100, WEBP_Q_DEFAULT)}


def encode_webp_lossy(img: Image.Image, opts=None, icc: bytes | None = None) -> tuple[bytes, str]:
    o = webp_opts(opts)
    kw = dict(quality=o["q"], method=6)
    if icc:
        kw["icc_profile"] = icc
    buf = io.BytesIO()
    img.save(buf, "WEBP", **kw)
    return buf.getvalue(), "libwebp"


def norm_fmt(fmt) -> str:
    f = str(fmt or "").upper()
    return f if f in FORMATS else "WEBP"


def is_lossy(fmt: str, opts: dict | None) -> bool:
    if fmt in ("JPEG", "AVIF"):
        return True
    return fmt == "WEBP" and not webp_opts((opts or {}).get("webp"))["lossless"]


def encode_lossy(img: Image.Image, fmt: str, opts: dict | None, icc: bytes | None = None) -> tuple[bytes, str, dict]:
    """Returns (bytes, encoder name, normalised settings) for JPEG / AVIF / lossy WEBP: the Rust back end
    (B4), Python for what isn't in it yet."""
    from . import backend
    from .proc import _preview_req
    r = backend.shared().encode(img, norm_fmt(fmt), opts, icc, _preview_req())
    return r if r is not None else encode_lossy_py(img, fmt, opts, icc)


def encode_lossy_py(img: Image.Image, fmt: str, opts: dict | None, icc: bytes | None = None) -> tuple[bytes, str, dict]:
    """The Python encoders (before B4)."""
    opts = opts or {}
    if fmt == "JPEG":
        data, enc = encode_jpeg(img, opts.get("jpeg"), icc)
        return data, enc, jpeg_opts(opts.get("jpeg"))
    if fmt == "AVIF":
        data, enc = encode_avif(img, opts.get("avif"), icc)
        return data, enc, avif_opts(opts.get("avif"))
    data, enc = encode_webp_lossy(img, opts.get("webp"), icc)
    return data, enc, webp_opts(opts.get("webp"))


def _out_path(src_path: str, size: tuple[int, int], ext: str) -> str:
    base, _ = os.path.splitext(src_path)
    out_w, out_h = size
    out_path = f"{base}_{out_w}x{out_h}{ext}"
    n = 1
    while os.path.exists(out_path):
        out_path = f"{base}_{out_w}x{out_h}_{n}{ext}"
        n += 1
    return out_path


def save_image(img: Image.Image, src_path: str, fmt: str = "WEBP", icc: bytes | None = None,
               opts: dict | None = None) -> str:
    """Encode and write next to the source: the Rust back end (B4), Python for what isn't in it yet."""
    from . import backend
    r = backend.shared().save(img, src_path, norm_fmt(fmt), icc, opts)
    return r if r is not None else save_image_py(img, src_path, fmt, icc, opts)


def save_image_py(img: Image.Image, src_path: str, fmt: str = "WEBP", icc: bytes | None = None,
                  opts: dict | None = None) -> str:
    """The Python version (before B4)."""
    fmt = norm_fmt(fmt)
    if is_lossy(fmt, opts):
        data, _, _ = encode_lossy_py(img, fmt, opts, icc)
        out_path = _out_path(src_path, img.size, EXTS[fmt])
        with open(out_path, "wb") as f:
            f.write(data)
        return out_path
    out_path = _out_path(src_path, img.size, EXTS[fmt])
    params = (dict(lossless=True, quality=100, method=6, exact=True) if fmt == "WEBP"
              else dict(compress_level=9, optimize=True))
    if icc:
        params["icc_profile"] = icc
    img.save(out_path, fmt, **params)
    return out_path
