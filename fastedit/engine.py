"""Image loading, stage cache and thumbnails."""
from __future__ import annotations

import io
import os
import threading
import uuid
from collections import OrderedDict
from PIL import Image, ImageOps, UnidentifiedImageError
from .config import IMAGE_EXTS, PLUGIN_HINT, StageParams, THUMB_SIZE
from .geometry import apply_geometry, fit_image, prepare
from .colour import gray_edge_enhance, white_patch_enhance


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


class Entry:
    def __init__(self, path: str):
        self.path = path
        self.mtime = os.path.getmtime(path)
        with open_image(path) as im:
            im.load()
            self.icc = im.info.get("icc_profile")
            self.base = prepare(im)
        self.fits: dict = {}
        self.geo = (None, None)

    def geometried(self, P: StageParams) -> Image.Image:
        key = P.geom_key()
        if self.geo[0] != key:
            self.geo = (key, apply_geometry(self.base, P))
        return self.geo[1]

    def fitted(self, P: StageParams) -> Image.Image:
        key = P.fit_key()
        img = self.fits.get(key)
        if img is None:
            if len(self.fits) > 3:
                self.fits.clear()
            img = self.fits[key] = fit_image(self.geometried(P), P)
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
    with open_image(path) as im:
        im.draft("RGB", (THUMB_SIZE * 2, THUMB_SIZE * 2))
        im = ImageOps.exif_transpose(im).convert("RGB")
        im.thumbnail((THUMB_SIZE * 2, THUMB_SIZE * 2), Image.Resampling.BILINEAR)
        buf = io.BytesIO()
        im.save(buf, "JPEG", quality=82)
        return buf.getvalue()
