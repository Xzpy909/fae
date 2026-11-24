"""Opening images with Pillow (the formats the Rust back end doesn't decode yet: AVIF until B4), file
lists and thumbnails. The stage itself (decode, geometry, resize, colour balance, its cache and the
previews) runs in the Rust back end since B3 (fae-backend stage.rs)."""
from __future__ import annotations

import io
import os
import threading
from collections import OrderedDict
from PIL import Image, ImageOps, UnidentifiedImageError
from .config import IMAGE_EXTS, PLUGIN_HINT, THUMB_SIZE
from . import backend


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
    """Pillow's decode, for the formats the Rust back end doesn't open yet (AVIF until B4): ((pixels, w,
    h, channels): upright RGB / RGBA, its ICC profile)."""
    with open_image(path) as im:
        im.load()
        img = prepare(im)
        return (img.tobytes(), img.width, img.height, len(img.getbands())), im.info.get("icc_profile")


def collect(paths) -> list[str]:
    files = []
    for p in paths or []:
        if not p:
            continue
        if os.path.isdir(p):
            for root, dirs, names in os.walk(p):
                dirs.sort()
                for n in sorted(names):
                    if os.path.splitext(n)[1].lower() in IMAGE_EXTS:
                        files.append(os.path.join(root, n))
        elif os.path.isfile(p) and os.path.splitext(p)[1].lower() in IMAGE_EXTS:
            files.append(p)
    return files



_THUMB_SEM = threading.Semaphore(2)
_THUMBS: "OrderedDict[tuple, bytes]" = OrderedDict()


def make_thumb(path: str) -> bytes:
    try:
        key = (os.path.normcase(os.path.abspath(path)), os.path.getmtime(path))
    except OSError:
        key = None
    if key in _THUMBS:
        _THUMBS.move_to_end(key)
        return _THUMBS[key]
    with _THUMB_SEM:
        data = _make_thumb(path)
    if key is not None:
        _THUMBS[key] = data
        while len(_THUMBS) > 1000:
            _THUMBS.popitem(last=False)
    return data


def _make_thumb(path: str) -> bytes:
    """The Rust back end's thumbnail (B2: the same pixels as Pillow's below), Pillow for the formats not
    moved yet (AVIF)."""
    data = backend.shared().thumb(path, THUMB_SIZE * 2)
    return data if data is not None else make_thumb_pillow(path)


def make_thumb_pillow(path: str) -> bytes:
    """Pillow's thumbnail (AVIF until B4; the stage tests' reference)."""
    with open_image(path) as im:
        im.draft("RGB", (THUMB_SIZE * 2, THUMB_SIZE * 2))
        im = ImageOps.exif_transpose(im).convert("RGB")
        im.thumbnail((THUMB_SIZE * 2, THUMB_SIZE * 2), Image.Resampling.BILINEAR)
        buf = io.BytesIO()
        im.save(buf, "JPEG", quality=82)
        return buf.getvalue()
