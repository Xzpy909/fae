"""Opening images with Pillow: the back end's fallback for the files it can't open itself (AVIF grids,
premultiplied alpha; backend.py _pillow_source). Everything else (the stage, file lists, thumbnails)
runs in the Rust back end (fae-backend stage.rs since B3, app.rs since B6)."""
from __future__ import annotations

import os
from PIL import Image, ImageOps, UnidentifiedImageError
from .config import PLUGIN_HINT


# ---------------------------------------------------------------------------
# Opening images with Pillow
# ---------------------------------------------------------------------------
def open_image(path: str) -> Image.Image:
    """Image.open with a clear message when an optional decoder is missing."""
    try:
        return Image.open(path)
    except UnidentifiedImageError:
        ok, pkg = PLUGIN_HINT.get(os.path.splitext(path)[1].lower(), (True, ""))
        if not ok:
            raise RuntimeError(f"{os.path.basename(path)}: needs  pip install {pkg}") from None
        raise


def to_pil(raw) -> Image.Image:
    """(pixels, w, h, channels) -> a Pillow image (RGB / RGBA)."""
    px, w, h, ch = raw
    mode = "RGBA" if ch == 4 else "RGB"
    return Image.frombuffer(mode, (w, h), px, "raw", mode, 0, 1)


def prepare(img: Image.Image) -> Image.Image:
    """EXIF rotation, then RGB or RGBA (grayscale is promoted to RGB for editing): as decode.rs does."""
    img = ImageOps.exif_transpose(img)
    has_alpha = "A" in img.getbands() or "transparency" in img.info
    want = "RGBA" if has_alpha else "RGB"
    if img.mode != want:
        img = img.convert(want)
    return img


def decode_pillow(path: str) -> tuple[tuple, bytes | None]:
    """Pillow's decode, for the files the Rust back end can't open: ((pixels, w, h, channels): upright
    RGB / RGBA, its ICC profile)."""
    with open_image(path) as im:
        im.load()
        img = prepare(im)
        return (img.tobytes(), img.width, img.height, len(img.getbands())), im.info.get("icc_profile")
