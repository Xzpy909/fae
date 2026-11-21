"""JPEG output: jpegli (encoders/cjpegli.exe) or Pillow/libjpeg-turbo."""
from __future__ import annotations

import io
import os
import tempfile
from PIL import Image
from .config import CJPEGLI, JPEG_D_DEFAULT, JPEG_Q_DEFAULT
from .proc import run_encoder


# ---------------------------------------------------------------------------
# JPEG
# ---------------------------------------------------------------------------
def jpegli_available() -> bool:
    return os.path.isfile(CJPEGLI)


def flatten_rgb(img: Image.Image) -> Image.Image:
    """JPEG has no alpha: composite transparent images onto white."""
    if img.mode == "RGBA":
        bg = Image.new("RGB", img.size, (255, 255, 255))
        bg.paste(img, mask=img.getchannel("A"))
        return bg
    return img if img.mode == "RGB" else img.convert("RGB")


def jpeg_opts(o) -> dict:
    """Normalise UI options: {"mode": "q"|"d", "q": 1..100, "d": 0..25}."""
    if not isinstance(o, dict):
        o = {"q": o} if o is not None else {}
    mode = "d" if str(o.get("mode", "q")).lower().startswith("d") else "q"
    q = int(min(100, max(1, round(float(o.get("q", JPEG_Q_DEFAULT))))))
    d = round(min(25.0, max(0.0, float(o.get("d", JPEG_D_DEFAULT)))), 3)
    return {"mode": mode, "q": q, "d": d}


def encode_jpeg(img: Image.Image, opts=None, icc: bytes | None = None) -> tuple[bytes, str]:
    """Returns (jpeg bytes, encoder name)."""
    img = flatten_rgb(img)
    o = jpeg_opts(opts)
    q = o["q"]
    if jpegli_available():
        args = [f"--distance={o['d']:g}"] if o["mode"] == "d" else [f"--quality={q}"]
        with tempfile.TemporaryDirectory(prefix="fie_jpeg_") as td:
            src, dst = os.path.join(td, "in.png"), os.path.join(td, "out.jpg")
            kw = {"compress_level": 1}
            if icc:
                kw["icc_profile"] = icc
            img.save(src, "PNG", **kw)
            r = run_encoder([CJPEGLI, src, dst, *args])
            if r.returncode != 0 or not os.path.isfile(dst):
                msg = (r.stderr or r.stdout or b"").decode(errors="replace").strip()
                raise RuntimeError(f"cjpegli failed ({r.returncode}): {msg[-300:]}")
            with open(dst, "rb") as f:
                return f.read(), "jpegli"
    buf = io.BytesIO()
    # Pillow fallback: no butteraugli distance, so distance mode uses the quality value
    kw = dict(quality=q, optimize=True, progressive=True)
    if icc:
        kw["icc_profile"] = icc
    img.save(buf, "JPEG", **kw)
    return buf.getvalue(), "pillow"
