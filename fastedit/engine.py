"""Image loading, stage cache and thumbnails."""
from __future__ import annotations

import io
import os
import threading
import uuid
from collections import OrderedDict
from PIL import Image, ImageOps, UnidentifiedImageError
from .config import IMAGE_EXTS, PLUGIN_HINT, StageParams, THUMB_SIZE
from .geometry import prepare
from .colour import gray_edge_enhance, white_patch_enhance
from . import backend


# ---------------------------------------------------------------------------
# Image cache + stage pipeline
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


def decode(path: str) -> tuple[tuple, bytes | None]:
    """An image file -> ((pixels, w, h, channels): upright RGB / RGBA, its ICC profile): the Rust back
    end's decoders (B1, the same pixels Pillow gave), Pillow for the formats not moved yet (AVIF)."""
    r = backend.shared().decode(path)
    if r is not None:
        px, w, h, ch, icc, _fmt = r
        return (px, w, h, ch), icc
    with open_image(path) as im:
        im.load()
        img = prepare(im)
        return (img.tobytes(), img.width, img.height, len(img.getbands())), im.info.get("icc_profile")


class Entry:
    """One opened image: its pixels as decoded (raw: pixels, w, h, channels), the geometry result for
    the last geometry settings, the resized results for the last few resize settings. Geometry and
    resizing run in the Rust back end (B2); Pillow images only where the Python stage still needs them."""

    def __init__(self, path: str):
        self.path = path
        self.mtime = os.path.getmtime(path)
        self.raw, self.icc = decode(path)
        self._base = None
        self.fits: dict = {}
        self.geo = (None, None)

    @property
    def size(self) -> tuple[int, int]:
        return self.raw[1], self.raw[2]

    @property
    def alpha(self) -> bool:
        return self.raw[3] == 4

    @property
    def base(self) -> Image.Image:
        """The decoded image as a Pillow image (crop mode's source), made on first use."""
        if self._base is None:
            self._base = to_pil(self.raw)
        return self._base

    def geometried(self, P: StageParams) -> tuple:
        """The raw pixels after flip / rotation / crop."""
        key = P.geom_key()
        if self.geo[0] != key:
            self.geo = (key, backend.shared().geometry(self.raw, P) if P.has_geom() else self.raw)
        return self.geo[1]

    def geo_size(self, P: StageParams) -> tuple[int, int]:
        g = self.geometried(P)
        return g[1], g[2]

    def fitted(self, P: StageParams) -> Image.Image:
        key = P.fit_key()
        img = self.fits.get(key)
        if img is None:
            if len(self.fits) > 3:
                self.fits.clear()
            g = self.geometried(P)
            img = self.fits[key] = to_pil(backend.shared().fit(g, P) if P.resize_on else g)
        return img


class Engine:
    def __init__(self):
        self.lock = threading.RLock()
        self.entries: "OrderedDict[str, Entry]" = OrderedDict()
        self.previews: "OrderedDict[str, tuple[bytes, int, int]]" = OrderedDict()
        self.current = None        # {"path","rgb","req"}
        self.latest_req = -1

    def entry(self, path: str) -> Entry:
        with self.lock:
            key = os.path.normcase(os.path.abspath(path))
            e = self.entries.get(key)
            if e is not None and e.mtime != os.path.getmtime(path):
                e = None
            if e is None:
                e = Entry(path)
                self.entries[key] = e
                while len(self.entries) > 4:
                    self.entries.popitem(last=False)
            self.entries.move_to_end(key)
            return e

    def stage(self, path: str, P: StageParams) -> tuple[Entry, Image.Image]:
        with self.lock:
            e = self.entry(path)
            out = e.fitted(P)
            if P.wp_on:
                out = white_patch_enhance(out, P.wp_pct, P.wp_strength)
            if P.ge_on:
                out = gray_edge_enhance(out, P.ge_p, P.ge_sigma, P.ge_strength)
            return e, out

    def put_preview(self, rgba: bytes, w: int, h: int) -> str:
        token = uuid.uuid4().hex
        with self.lock:
            self.previews[token] = (rgba, w, h)
            while len(self.previews) > 8:   # image + Mid/Detail base per load
                self.previews.popitem(last=False)
        return token


ENGINE = Engine()


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
