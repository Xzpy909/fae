"""
Fast Image Editor - pywebview front end, numpy/OpenCV/Pillow back end.

What it does (per image, nothing is written until you press Process):
    Output     : 720p cover-crop or target-Mpix resize (with priority).
                 WEBP (lossless or lossy), PNG, JPEG via encoders/cjpegli.exe (Pillow fallback),
                 AVIF 10-bit 4:4:4 via encoders/SvtAv1EncApp.exe (--tune 3), muxed here;
                 Pillow's libavif/aom as fallback. Lossy formats get a split-view preview.
    Color      : Percentile White-Patch, Gray-Edge color constancy
    Points     : Black / neutral-gray / white eyedroppers (stay active - keep clicking)
    Curves     : Master + R/G/B curves with live histogram
    Alt (hold) : clipping / threshold view that helps you find where to click

Pipeline order:
    resize -> White-Patch -> Gray-Edge -> point correction -> curves

Requirements:
    pip install pywebview pillow numpy opencv-python

Run:
    python fast_image_editor.py [images or folders...]   (--debug opens dev tools)
"""

from __future__ import annotations

import io
import json
import math
import os
import struct
import subprocess
import sys
import tempfile
import threading
import uuid
from collections import OrderedDict
from dataclasses import dataclass, fields
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse

import cv2
import numpy as np
from PIL import Image, ImageOps, features

APP_DIR = os.path.dirname(os.path.abspath(__file__))
UI_DIR = os.path.join(APP_DIR, "ui")
SETTINGS_PATH = os.path.join(APP_DIR, "fast_image_editor_settings.json")
ENCODERS_DIR = os.path.join(APP_DIR, "encoders")
_EXE = ".exe" if sys.platform.startswith("win") else ""
CJPEGLI = os.path.join(ENCODERS_DIR, "cjpegli" + _EXE)
SVTAV1 = os.path.join(ENCODERS_DIR, "SvtAv1EncApp" + _EXE)
JPEG_Q_DEFAULT = 90
JPEG_D_DEFAULT = 1.0
AVIF_CRF_DEFAULT = 25.0
AVIF_PRESET_DEFAULT = 4
WEBP_Q_DEFAULT = 90
FORMATS = ("WEBP", "PNG", "JPEG", "AVIF")
EXTS = {"WEBP": ".webp", "PNG": ".png", "JPEG": ".jpg", "AVIF": ".avif"}

PREVIEW_MAX = 2560          # longest side of the image sent to the live preview
THUMB_SIZE = 112

TARGET_W, TARGET_H = 1280, 720
MODE_720 = "720p"
MODE_MPIX = "Mpix"
PRIO_MPIX = "Mpix priority"
PRIO_BALANCED = "Balanced"
PRIO_RATIO = "Ratio error priority"
IMAGE_EXTS = {
    ".jpg", ".jpeg", ".png", ".webp", ".bmp", ".gif", ".tif", ".tiff",
    ".avif", ".heic", ".ico", ".ppm", ".pgm", ".tga",
}


@dataclass(frozen=True)
class StageParams:
    """Settings that change the image Python hands to the live preview."""
    resize_on: bool = True
    mode: str = MODE_720
    mpix: float = 2.0
    prio: str = PRIO_BALANCED
    wp_on: bool = False
    wp_pct: float = 99.0
    wp_strength: float = 1.0
    ge_on: bool = False
    ge_p: int = 1
    ge_sigma: float = 2.0
    ge_strength: float = 1.0
    # geometry (applied to the source before resizing); crop is in source pixels,
    # centre-relative, in the rotated frame. crop_w == 0 means "no crop".
    rot_q: int = 0
    angle: float = 0.0
    flip: bool = False
    crop_x: float = 0.0
    crop_y: float = 0.0
    crop_w: float = 0.0
    crop_h: float = 0.0

    @classmethod
    def from_dict(cls, d: dict | None) -> "StageParams":
        d = d or {}
        ref = cls()
        kw = {}
        for f in fields(cls):
            if f.name in d and d[f.name] is not None:
                typ = type(getattr(ref, f.name))
                v = d[f.name]
                kw[f.name] = bool(v) if typ is bool else typ(v)
        return cls(**kw)

    def fit_key(self):
        if not self.resize_on:
            f = ("none",)
        else:
            f = (self.mode, self.mpix, self.prio) if self.mode == MODE_MPIX else (self.mode,)
        return (self.geom_key(), f)

    def geom_key(self):
        return (self.rot_q % 4, round(self.angle, 4), self.flip,
                round(self.crop_x, 3), round(self.crop_y, 3), round(self.crop_w, 3), round(self.crop_h, 3))

    def has_geom(self):
        return self.rot_q % 4 != 0 or self.angle != 0 or self.flip or self.crop_w > 0


# ---------------------------------------------------------------------------
# Resizing (same behaviour as image_to_720p_all.py)
# ---------------------------------------------------------------------------
def prepare(img: Image.Image) -> Image.Image:
    """EXIF rotation, then RGB or RGBA (grayscale is promoted to RGB for editing)."""
    img = ImageOps.exif_transpose(img)
    has_alpha = "A" in img.getbands() or "transparency" in img.info
    want = "RGBA" if has_alpha else "RGB"
    if img.mode != want:
        img = img.convert(want)
    return img


def resize_cover_crop(img: Image.Image, tw: int = TARGET_W, th: int = TARGET_H) -> Image.Image:
    w, h = img.size
    scale = max(tw / w, th / h)
    new_w = max(tw, round(w * scale))
    new_h = max(th, round(h * scale))
    if (new_w, new_h) != (w, h):
        img = img.resize((new_w, new_h), Image.Resampling.LANCZOS)
    left = (new_w - tw) // 2
    top = (new_h - th) // 2
    return img.crop((left, top, left + tw, top + th))


def mpix_size(w: int, h: int, mpix: float, priority: str = PRIO_BALANCED) -> tuple[int, int]:
    target_px = mpix * 1_000_000
    scale = math.sqrt(target_px / (w * h))
    ideal_w = w * scale
    ratio = w / h
    if priority == PRIO_RATIO:
        lo = max(1, int(ideal_w * 0.99) - 3)
        hi = int(ideal_w * 1.01) + 5
    else:
        lo = max(1, int(ideal_w) - 3)
        hi = int(ideal_w) + 5
    best = None
    for cw in range(lo, hi):
        ch = max(1, round(cw / ratio))
        ratio_err = round(abs(cw / ch - ratio) / ratio, 6)
        px_err = abs(cw * ch - target_px) / target_px
        key = (px_err, ratio_err) if priority == PRIO_MPIX else (ratio_err, px_err)
        if best is None or key < best[0]:
            best = (key, cw, ch)
    return best[1], best[2]


def resize_to_mpix(img: Image.Image, mpix: float, priority: str = PRIO_BALANCED) -> Image.Image:
    w, h = img.size
    new_w, new_h = mpix_size(w, h, mpix, priority)
    if (new_w, new_h) == (w, h):
        return img
    return img.resize((new_w, new_h), Image.Resampling.LANCZOS)


def fit_image(img: Image.Image, P: StageParams) -> Image.Image:
    if not P.resize_on:
        return img
    if P.mode == MODE_MPIX:
        return resize_to_mpix(img, P.mpix, P.prio)
    return resize_cover_crop(img)



# ---------------------------------------------------------------------------
# Geometry: flip -> rotate (90° steps + fine angle) -> crop   (matches ui/app.js crop mode)
# Positive angles rotate clockwise on screen.
# ---------------------------------------------------------------------------
_QUARTER = {1: Image.Transpose.ROTATE_270, 2: Image.Transpose.ROTATE_180, 3: Image.Transpose.ROTATE_90}


def apply_geometry(img: Image.Image, P: StageParams) -> Image.Image:
    if not P.has_geom():
        return img
    if P.flip:
        img = ImageOps.mirror(img)
    q = P.rot_q % 4
    W, H = img.size
    if P.crop_w <= 0 or P.crop_h <= 0:
        if P.angle == 0:
            return img.transpose(_QUARTER[q]) if q else img
        # no crop given for a fine angle: fall back to the full (unrotated-size) frame
        Wq, Hq = (H, W) if q % 2 else (W, H)
        cx, cy, cw, ch = 0.0, 0.0, float(Wq), float(Hq)
    else:
        cx, cy, cw, ch = P.crop_x, P.crop_y, P.crop_w, P.crop_h
    ow, oh = max(1, int(round(cw))), max(1, int(round(ch)))

    if P.angle == 0:
        # lossless path: quarter turn + integer crop
        if q:
            img = img.transpose(_QUARTER[q])
        Wq, Hq = img.size
        left = int(round(Wq / 2 + cx - ow / 2))
        top = int(round(Hq / 2 + cy - oh / 2))
        left = min(max(0, left), max(0, Wq - ow))
        top = min(max(0, top), max(0, Hq - oh))
        return img.crop((left, top, left + min(ow, Wq), top + min(oh, Hq)))

    th = math.radians(q * 90 + P.angle)
    c, s = math.cos(th), math.sin(th)
    x0 = cx - ow / 2 + 0.5
    y0 = cy - oh / 2 + 0.5
    M = np.array([
        [c, s, c * x0 + s * y0 + W / 2 - 0.5],
        [-s, c, -s * x0 + c * y0 + H / 2 - 0.5],
    ], dtype=np.float64)
    arr = np.asarray(img)
    out = cv2.warpAffine(arr, M, (ow, oh), flags=cv2.INTER_LANCZOS4 | cv2.WARP_INVERSE_MAP,
                         borderMode=cv2.BORDER_REPLICATE)
    return Image.fromarray(out, img.mode)

# ---------------------------------------------------------------------------
# sRGB <-> linear helpers
# ---------------------------------------------------------------------------
def _srgb_to_linear(u8: np.ndarray) -> np.ndarray:
    s = u8.astype(np.float32) / 255.0
    return np.where(s <= 0.04045, s / 12.92, ((s + 0.055) / 1.055) ** 2.4).astype(np.float32)


def _linear_to_srgb8(lin: np.ndarray) -> np.ndarray:
    lin = np.clip(lin, 0.0, 1.0)
    s = np.where(lin <= 0.0031308, lin * 12.92, 1.055 * np.power(lin, 1.0 / 2.4) - 0.055)
    return np.clip(s * 255.0 + 0.5, 0, 255).astype(np.uint8)


def _finish_linear(corrected: np.ndarray, alpha) -> Image.Image:
    """Soft highlight protection, back to sRGB, re-attach alpha."""
    peak = float(np.percentile(corrected, 99.9))
    if peak > 0.98:
        corrected = corrected * (0.98 / peak)
    result = Image.fromarray(_linear_to_srgb8(corrected))
    if alpha is not None:
        result.putalpha(alpha)
    return result


# ---------------------------------------------------------------------------
# Gray-Edge color constancy
# ---------------------------------------------------------------------------
def gray_edge_enhance(img: Image.Image, p: int = 1, sigma: float = 2.0,
                      strength: float = 1.0) -> Image.Image:
    strength = float(np.clip(strength, 0.0, 1.0))
    if strength == 0.0:
        return img.copy()
    alpha = img.getchannel("A") if "A" in img.getbands() else None
    lin = _srgb_to_linear(np.asarray(img.convert("RGB")))
    if sigma > 0:
        ksize = int(round(sigma * 4)) | 1
        smoothed = cv2.GaussianBlur(lin, (ksize, ksize), sigma)
    else:
        smoothed = lin
    gains = []
    for c in range(3):
        channel = smoothed[:, :, c]
        sx = cv2.Sobel(channel, cv2.CV_32F, 1, 0, ksize=3)
        sy = cv2.Sobel(channel, cv2.CV_32F, 0, 1, ksize=3)
        mag = np.sqrt(sx ** 2 + sy ** 2).astype(np.float64)
        norm = np.sum(mag) if p == 1 else np.sum(mag ** p) ** (1.0 / p)
        gains.append(norm)
    gains = np.maximum(np.array(gains, dtype=np.float64), 1e-6)
    computed = gains.max() / gains
    final = (1.0 + strength * (computed - 1.0)).astype(np.float32).reshape(1, 1, 3)
    return _finish_linear(lin * final, alpha)


# ---------------------------------------------------------------------------
# Percentile White-Patch
# ---------------------------------------------------------------------------
def white_patch_enhance(img: Image.Image, percentile: float = 99.0,
                        strength: float = 1.0) -> Image.Image:
    strength = float(np.clip(strength, 0.0, 1.0))
    if strength == 0.0:
        return img.copy()
    alpha = img.getchannel("A") if "A" in img.getbands() else None
    lin = _srgb_to_linear(np.asarray(img.convert("RGB")))
    ref = np.percentile(lin.reshape(-1, 3), percentile, axis=0).astype(np.float32)
    ref = np.maximum(ref, 1e-6)
    gains = ref.max() / ref
    gains = (1.0 + strength * (gains - 1.0)).reshape(1, 1, 3)
    return _finish_linear(lin * gains, alpha)


# ---------------------------------------------------------------------------
# Point correction + curves (must match computeEdit/render in ui/app.js)
# ---------------------------------------------------------------------------
def apply_edit(rgb: np.ndarray, edit: dict | None, strip_rows: int = 512) -> np.ndarray:
    """
    rgb  : HxWx3 uint8
    edit : {"point": bool, "k": float, "o": float, "gains": [3] | None,
            "luts": [[256], [256], [256]]}   (computed by the UI)
    Per pixel:  lum' = clip(lum*k + o)  ->  rgb *= lum'/max(lum,1)  ->  rgb *= gains
                ->  clip, truncate to uint8  ->  per-channel LUT (master curve folded in)
    """
    if not edit:
        return rgb
    luts = [np.asarray(l, dtype=np.uint8) for l in edit.get("luts") or []]
    if len(luts) != 3:
        luts = [np.arange(256, dtype=np.uint8)] * 3
    point = bool(edit.get("point"))
    k = float(edit.get("k", 1.0))
    o = float(edit.get("o", 0.0))
    gains = edit.get("gains")
    gains = None if gains is None else np.asarray(gains, dtype=np.float64).reshape(1, 1, 3)

    out = np.empty_like(rgb)
    for y0 in range(0, rgb.shape[0], strip_rows):
        src = rgb[y0:y0 + strip_rows]
        if point:
            f = src.astype(np.float64)
            if k != 1.0 or o != 0.0:
                lum = 0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2]
                bl = np.clip(lum * k + o, 0, 255)
                f *= (bl / np.maximum(lum, 1.0))[..., None]
            if gains is not None:
                f *= gains
            idx = np.clip(f, 0, 255).astype(np.uint8)
        else:
            idx = src
        dst = out[y0:y0 + strip_rows]
        for c in range(3):
            dst[..., c] = luts[c][idx[..., c]]
    return out


# ---------------------------------------------------------------------------
# Image cache + stage pipeline
# ---------------------------------------------------------------------------
class Entry:
    def __init__(self, path: str):
        self.path = path
        self.mtime = os.path.getmtime(path)
        with Image.open(path) as im:
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
            while len(self.previews) > 4:
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


# ---------------------------------------------------------------------------
# JPEG (jpegli via encoders/cjpegli.exe, Pillow/libjpeg-turbo as fallback)
# ---------------------------------------------------------------------------
def _no_window() -> dict:
    # pythonw has no console; without this every cjpegli call would flash one
    return {"creationflags": 0x08000000} if sys.platform.startswith("win") else {}


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
            r = subprocess.run([CJPEGLI, src, dst, *args], capture_output=True, **_no_window())
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


# ---------------------------------------------------------------------------
# AVIF via SvtAv1EncApp: RGB -> 10-bit full-range BT.709 YUV 4:4:4 -> SVT (OBU) -> AVIF container
# ---------------------------------------------------------------------------


class _Bits:
    def __init__(self, data: bytes):
        self.d, self.p = data, 0

    def f(self, n: int) -> int:
        v = 0
        for _ in range(n):
            byte = self.d[self.p >> 3]
            v = (v << 1) | ((byte >> (7 - (self.p & 7))) & 1)
            self.p += 1
        return v

    def uvlc(self) -> int:
        lz = 0
        while not self.f(1):
            lz += 1
            if lz >= 32:
                return (1 << 32) - 1
        return (1 << lz) - 1 + self.f(lz)


def _leb128(d: bytes, p: int) -> tuple[int, int]:
    v = 0
    for i in range(8):
        b = d[p + i]
        v |= (b & 0x7F) << (7 * i)
        if not b & 0x80:
            return v, p + i + 1
    raise ValueError("bad leb128")


def split_obus(data: bytes) -> list[tuple[int, bytes]]:
    """[(obu_type, full obu bytes incl. header)], requires obu_has_size_field."""
    out, p = [], 0
    while p < len(data):
        start = p
        h = data[p]
        typ, ext, has_size = (h >> 3) & 0xF, (h >> 2) & 1, (h >> 1) & 1
        p += 1 + ext
        if not has_size:
            raise ValueError("OBU without size field")
        size, p = _leb128(data, p)
        p += size
        out.append((typ, data[start:p]))
    return out


def parse_seq_header(obu: bytes) -> dict:
    h = obu[0]
    p = 1 + ((h >> 2) & 1)
    _, p = _leb128(obu, p)
    b = _Bits(obu[p:])
    prof = b.f(3)
    b.f(1)  # still_picture
    reduced = b.f(1)
    tier = 0
    if reduced:
        level = b.f(5)
    else:
        timing = b.f(1)
        dmi = 0
        bdl = 0
        if timing:
            b.f(32); b.f(32)
            if b.f(1):
                b.uvlc()
            dmi = b.f(1)
            if dmi:
                bdl = b.f(5) + 1
                b.f(32); b.f(5); b.f(5)
        idd = b.f(1)
        cnt = b.f(5) + 1
        level = None
        for i in range(cnt):
            b.f(12)
            lv = b.f(5)
            t = b.f(1) if lv > 7 else 0
            if i == 0:
                level, tier = lv, t
            if dmi and b.f(1):
                b.f(bdl); b.f(bdl); b.f(1)
            if idd and b.f(1):
                b.f(4)
    wb, hb = b.f(4) + 1, b.f(4) + 1
    b.f(wb); b.f(hb)
    if not reduced and b.f(1):
        b.f(4); b.f(3)
    b.f(1); b.f(1); b.f(1)
    if not reduced:
        b.f(1); b.f(1); b.f(1); b.f(1)
        order_hint = b.f(1)
        if order_hint:
            b.f(1); b.f(1)
        force_sct = 2 if b.f(1) else b.f(1)
        if force_sct > 0:
            if not b.f(1):
                b.f(1)
        if order_hint:
            b.f(3)
    b.f(1); b.f(1); b.f(1)
    # color_config
    hbd = b.f(1)
    twelve = b.f(1) if (prof == 2 and hbd) else 0
    mono = 0 if prof == 1 else b.f(1)
    cp = tc = mc = 2
    if b.f(1):
        cp, tc, mc = b.f(8), b.f(8), b.f(8)
    csp = 0
    if mono:
        sx = sy = 1
    elif cp == 1 and tc == 13 and mc == 0:
        sx = sy = 0
    else:
        b.f(1)
        if prof == 0:
            sx = sy = 1
        elif prof == 1:
            sx = sy = 0
        elif twelve:
            sx = b.f(1)
            sy = b.f(1) if sx else 0
        else:
            sx, sy = 1, 0
        if sx and sy:
            csp = b.f(2)
    return {"profile": prof, "level": level, "tier": tier, "high_bitdepth": hbd, "twelve_bit": twelve,
            "mono": mono, "sx": sx, "sy": sy, "csp": csp,
            "depth": 12 if twelve else (10 if hbd else 8)}


def _box(typ: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", 8 + len(payload)) + typ + payload


def _fbox(typ: bytes, ver: int, flags: int, payload: bytes) -> bytes:
    return _box(typ, struct.pack(">I", (ver << 24) | flags) + payload)


def mux_avif(obu_stream: bytes, width: int, height: int, icc: bytes | None = None,
             nclx=(1, 13, 1, 1)) -> bytes:
    """Wrap a single-frame AV1 OBU stream into a still-image AVIF file."""
    obus = split_obus(obu_stream)
    seq = next((o for t, o in obus if t == 1), None)
    if seq is None:
        raise ValueError("no sequence header in AV1 stream")
    item = b"".join(o for t, o in obus if t not in (2, 15))   # drop temporal delimiters / padding
    s = parse_seq_header(seq)
    av1c = bytes([
        0x81,
        (s["profile"] << 5) | (s["level"] & 0x1F),
        (s["tier"] << 7) | (s["high_bitdepth"] << 6) | (s["twelve_bit"] << 5) | (s["mono"] << 4)
        | (s["sx"] << 3) | (s["sy"] << 2) | (s["csp"] & 3),
        0,
    ]) + seq
    ch = 1 if s["mono"] else 3
    props = [
        _fbox(b"ispe", 0, 0, struct.pack(">II", width, height)),
        _fbox(b"pixi", 0, 0, bytes([ch] + [s["depth"]] * ch)),
        _box(b"av1C", av1c),
        _box(b"colr", b"nclx" + struct.pack(">HHHB", nclx[0], nclx[1], nclx[2], (nclx[3] & 1) << 7)),
    ]
    if icc:
        props.append(_box(b"colr", b"prof" + icc))
    assoc = [(1, False), (2, False), (3, True), (4, False)] + ([(5, False)] if icc else [])
    ipma = _fbox(b"ipma", 0, 0, struct.pack(">IHB", 1, 1, len(assoc))
                 + bytes(((0x80 if ess else 0) | idx) for idx, ess in assoc))
    iprp = _box(b"iprp", _box(b"ipco", b"".join(props)) + ipma)
    ftyp = _box(b"ftyp", b"avif" + struct.pack(">I", 0) + b"avifmif1miaf")
    hdlr = _fbox(b"hdlr", 0, 0, struct.pack(">I", 0) + b"pict" + bytes(12) + b"\0")
    pitm = _fbox(b"pitm", 0, 0, struct.pack(">H", 1))
    iinf = _fbox(b"iinf", 0, 0, struct.pack(">H", 1)
                 + _fbox(b"infe", 2, 0, struct.pack(">HH", 1, 0) + b"av01" + b"\0"))

    def build(offset: int) -> bytes:
        iloc = _fbox(b"iloc", 0, 0, bytes([0x44, 0x00]) + struct.pack(">HHHHII", 1, 1, 0, 1, offset, len(item)))
        return _fbox(b"meta", 0, 0, hdlr + pitm + iloc + iinf + iprp)

    meta = build(0)
    offset = len(ftyp) + len(meta) + 8      # data starts right after the mdat header
    meta = build(offset)
    return ftyp + meta + _box(b"mdat", item)


def rgb_to_yuv444p10(rgb) -> bytes:
    """uint8 HxWx3 sRGB -> planar 10-bit little-endian Y, Cb, Cr (full range, BT.709 matrix)."""
    f = rgb.astype(np.float32) * (1.0 / 255.0)
    r, g, b_ = f[..., 0], f[..., 1], f[..., 2]
    y = 0.2126 * r + 0.7152 * g + 0.0722 * b_
    cb = (b_ - y) / 1.8556
    cr = (r - y) / 1.5748
    planes = [y * 1023.0, cb * 1023.0 + 512.0, cr * 1023.0 + 512.0]
    return b"".join(np.clip(np.rint(pl), 0, 1023).astype("<u2").tobytes() for pl in planes)


def ivf_payload(data: bytes) -> bytes:
    """IVF container -> concatenated OBUs of the first frame (raw OBU input passes through)."""
    if data[:4] != b"DKIF":
        return data
    hdr = struct.unpack("<H", data[6:8])[0]
    size = struct.unpack("<I", data[hdr:hdr + 4])[0]
    return data[hdr + 12:hdr + 12 + size]


# ---------------------------------------------------------------------------
# AVIF encoding: SVT-AV1 (preferred) or Pillow's libavif/aom (fallback, also used for alpha)
# ---------------------------------------------------------------------------
def svt_available() -> bool:
    return os.path.isfile(SVTAV1)


def avif_available() -> bool:
    """Pillow AVIF support: needed to decode the preview and for the aom fallback."""
    try:
        return bool(features.check("avif"))
    except Exception:
        return False


def _clampi(v, lo, hi, default):
    try:
        return int(min(hi, max(lo, round(float(v)))))
    except (TypeError, ValueError):
        return default


def avif_opts(o) -> dict:
    o = o if isinstance(o, dict) else {}
    try:
        crf = float(o.get("crf", AVIF_CRF_DEFAULT))
    except (TypeError, ValueError):
        crf = AVIF_CRF_DEFAULT
    crf = round(min(70.0, max(1.0, crf)) * 4) / 4          # SVT takes 0.25 steps
    return {"crf": crf, "preset": _clampi(o.get("preset"), 0, 8, AVIF_PRESET_DEFAULT)}


def encode_avif_svt(img: Image.Image, o: dict, icc: bytes | None = None) -> bytes:
    rgb = np.ascontiguousarray(np.asarray(img.convert("RGB")))
    h, w = rgb.shape[:2]
    with tempfile.TemporaryDirectory(prefix="fie_avif_") as td:
        src, dst = os.path.join(td, "in.yuv"), os.path.join(td, "out.ivf")
        with open(src, "wb") as f:
            f.write(rgb_to_yuv444p10(rgb))
        cmd = [SVTAV1, "-i", src, "-w", str(w), "-h", str(h),
               "--input-depth", "10", "--color-format", "444", "-n", "1", "--avif", "1",
               "--tune", "3", "--crf", f"{o['crf']:g}", "--preset", str(o["preset"]),
               "--color-primaries", "1", "--transfer-characteristics", "13",
               "--matrix-coefficients", "1", "--color-range", "1",
               "--progress", "0", "-b", dst]
        r = subprocess.run(cmd, capture_output=True, **_no_window())
        if r.returncode != 0 or not os.path.isfile(dst) or os.path.getsize(dst) == 0:
            msg = (r.stderr or r.stdout or b"").decode(errors="replace").strip()
            raise RuntimeError(f"SvtAv1EncApp failed ({r.returncode}): {msg[-400:]}")
        with open(dst, "rb") as f:
            stream = ivf_payload(f.read())
    return mux_avif(stream, w, h, icc, nclx=(1, 13, 1, 1))


def encode_avif(img: Image.Image, opts=None, icc: bytes | None = None) -> tuple[bytes, str]:
    o = avif_opts(opts)
    if img.mode not in ("RGB", "RGBA"):
        img = img.convert("RGB")
    if svt_available() and img.mode == "RGB":
        return encode_avif_svt(img, o, icc), "svt"
    if not avif_available():
        raise RuntimeError("no AVIF encoder: SvtAv1EncApp.exe missing and Pillow has no AVIF "
                           "(pip install -U pillow, 11.2 or newer)")
    # aom fallback (also for images with alpha): map CRF/preset onto libavif quality/speed
    q = int(round(100 - min(63.0, o["crf"]) * 100 / 63))
    kw = dict(quality=q, speed=min(10, o["preset"] + 2), subsampling="4:4:4", range="full")
    if icc:
        kw["icc_profile"] = icc
    buf = io.BytesIO()
    try:
        img.save(buf, "AVIF", codec="aom", **kw)
        enc = "aom"
    except Exception:
        # aom missing from this build: let libavif pick (svt would be 4:2:0 only)
        buf = io.BytesIO()
        img.save(buf, "AVIF", **kw)
        enc = "avif"
    return buf.getvalue(), enc


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
    """Returns (bytes, encoder name, normalised settings) for JPEG / AVIF / lossy WEBP."""
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
    fmt = norm_fmt(fmt)
    if is_lossy(fmt, opts):
        data, _, _ = encode_lossy(img, fmt, opts, icc)
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


def make_thumb(path: str) -> bytes:
    with Image.open(path) as im:
        im.draft("RGB", (THUMB_SIZE * 2, THUMB_SIZE * 2))
        im = ImageOps.exif_transpose(im).convert("RGB")
        im.thumbnail((THUMB_SIZE * 2, THUMB_SIZE * 2), Image.Resampling.BILINEAR)
        buf = io.BytesIO()
        im.save(buf, "JPEG", quality=82)
        return buf.getvalue()


# ---------------------------------------------------------------------------
# JS API (window.pywebview.api.*)
# ---------------------------------------------------------------------------
class Api:
    def __init__(self, initial_paths=None):
        self._window = None
        self._initial = collect(initial_paths or [])

    def set_window(self, w):
        self._window = w

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
    def load_stage(self, path, params, req_id):
        try:
            P = StageParams.from_dict(params)
            e, img = ENGINE.stage(path, P)
            rgb = np.ascontiguousarray(np.asarray(img.convert("RGB")))
            w, h = img.size
            pv = img.convert("RGB")
            if max(w, h) > PREVIEW_MAX:
                s = PREVIEW_MAX / max(w, h)
                pv = pv.resize((max(1, round(w * s)), max(1, round(h * s))), Image.Resampling.LANCZOS)
            pw, ph = pv.size
            rgba = np.empty((ph, pw, 4), dtype=np.uint8)
            rgba[..., :3] = np.asarray(pv)
            rgba[..., 3] = 255
            token = ENGINE.put_preview(rgba.tobytes(), pw, ph)
            with ENGINE.lock:
                if req_id >= ENGINE.latest_req:
                    ENGINE.latest_req = req_id
                    ENGINE.current = {"path": path, "rgb": rgb, "req": req_id}
            return {
                "ok": True, "req_id": req_id, "token": token,
                "w": w, "h": h, "pw": pw, "ph": ph,
                "src_w": e.base.size[0], "src_h": e.base.size[1],
                "alpha": img.mode == "RGBA",
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
        out = Image.fromarray(apply_edit(rgb, job.get("edit")))
        if alpha is not None:
            out.putalpha(alpha)
        return e, out

    def encoder_info(self):
        return {"jpegli": jpegli_available(), "avif": avif_available(), "svt": svt_available()}

    def lossy_preview(self, job, fmt, opts, req_id):
        """Encode the fully edited image with the lossy format, decode it again, hand back preview pixels."""
        try:
            fmt = norm_fmt(fmt)
            e, out = self._render_full(job)
            data, enc, used = encode_lossy(out, fmt, opts, e.icc)
            with Image.open(io.BytesIO(data)) as jim:
                jim.load()
                pv = jim.convert("RGB")
            w, h = pv.size
            if max(w, h) > PREVIEW_MAX:
                s = PREVIEW_MAX / max(w, h)
                pv = pv.resize((max(1, round(w * s)), max(1, round(h * s))), Image.Resampling.LANCZOS)
            pw, ph = pv.size
            rgba = np.empty((ph, pw, 4), dtype=np.uint8)
            rgba[..., :3] = np.asarray(pv)
            rgba[..., 3] = 255
            token = ENGINE.put_preview(rgba.tobytes(), pw, ph)
            return {"ok": True, "req_id": req_id, "token": token, "pw": pw, "ph": ph,
                    "bytes": len(data), "encoder": enc, "fmt": fmt, "opts": used}
        except Exception as ex:
            return {"ok": False, "req_id": req_id, "error": f"{type(ex).__name__}: {ex}"}

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


# ---------------------------------------------------------------------------
# Local HTTP server: UI files, raw preview pixels, thumbnails
# ---------------------------------------------------------------------------
MIME = {".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8",
        ".css": "text/css; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png"}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, body: bytes, ctype: str, cache=False):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "max-age=3600" if cache else "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        u = urlparse(self.path)
        p = unquote(u.path)
        try:
            if p.startswith("/preview/"):
                item = ENGINE.previews.get(p[len("/preview/"):])
                if item is None:
                    return self._send(404, b"gone", "text/plain")
                return self._send(200, item[0], "application/octet-stream")
            if p == "/thumb":
                path = parse_qs(u.query).get("p", [""])[0]
                return self._send(200, make_thumb(path), "image/jpeg", cache=True)
            rel = "index.html" if p in ("/", "") else p.lstrip("/")
            full = os.path.normpath(os.path.join(UI_DIR, rel))
            if not full.startswith(os.path.normpath(UI_DIR)) or not os.path.isfile(full):
                return self._send(404, b"not found", "text/plain")
            with open(full, "rb") as f:
                body = f.read()
            return self._send(200, body, MIME.get(os.path.splitext(full)[1], "application/octet-stream"))
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as ex:
            try:
                self._send(500, str(ex).encode(), "text/plain")
            except Exception:
                pass


def start_server() -> int:
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    srv.daemon_threads = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv.server_address[1]


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------
def main():
    import webview

    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    debug = "--debug" in sys.argv
    api = Api(args)
    port = start_server()
    window = webview.create_window(
        "Fast Image Editor", f"http://127.0.0.1:{port}/", js_api=api,
        width=1560, height=940, min_size=(1100, 680), background_color="#0e0f11",
    )
    api.set_window(window)

    def bind_drop():
        try:
            from webview.dom import DOMEventHandler
        except Exception:
            return

        def on_drop(e):
            files = (e.get("dataTransfer") or {}).get("files") or []
            paths = [f.get("pywebviewFullPath") for f in files if f.get("pywebviewFullPath")]
            found = collect(paths)
            if found:
                window.evaluate_js(f"window.app && window.app.addPaths({json.dumps(found)})")

        doc = window.dom.document
        doc.events.dragenter += DOMEventHandler(lambda e: None, True, True)
        doc.events.dragover += DOMEventHandler(lambda e: None, True, True)
        doc.events.drop += DOMEventHandler(on_drop, True, True)

    window.events.loaded += bind_drop
    webview.start(debug=debug)


if __name__ == "__main__":
    main()
