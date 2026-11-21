"""Resizing and crop / rotate / flip geometry."""
from __future__ import annotations

import math
import cv2
import numpy as np
from PIL import Image, ImageOps
from .config import LEGACY_720, MODE_CUSTOM, MODE_MPIX, PRIO_BALANCED, PRIO_MPIX, PRIO_RATIO, StageParams, TARGET_H, TARGET_W


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


def preset_size(ar: str, res: int) -> tuple[int, int]:
    """'16:9', 1080 -> (1920, 1080). The p-number is the short side; the long side is rounded to even."""
    try:
        a, b = (float(x) for x in str(ar).split(":"))
        r = a / b if a > 0 and b > 0 else 16 / 9
    except ValueError:
        r = 16 / 9
    res = max(16, int(res))
    if r >= 1:
        return max(2, int(round(res * r / 2)) * 2), res
    return res, max(2, int(round(res / r / 2)) * 2)


def fit_target(P: StageParams) -> tuple[int, int, str] | None:
    """(w, h, 'cover'|'stretch') for preset / custom modes, None for Mpix / off."""
    if not P.resize_on or P.mode == MODE_MPIX:
        return None
    if P.mode == MODE_CUSTOM:
        w, h = max(1, min(32768, int(P.cw))), max(1, min(32768, int(P.ch)))
        return w, h, ("stretch" if P.cmode == "stretch" else "cover")
    if P.mode == LEGACY_720:
        return TARGET_W, TARGET_H, "cover"
    w, h = preset_size(P.preset_ar, P.preset_res)
    return w, h, "cover"


def fit_scale(size: tuple[int, int], P: StageParams) -> float:
    """Largest per-axis scale factor the resize applies (> 1 means upscaling)."""
    w, h = size
    if not P.resize_on:
        return 1.0
    if P.mode == MODE_MPIX:
        nw, nh = mpix_size(w, h, P.mpix, P.prio)
        return max(nw / w, nh / h)
    tw, th, _ = fit_target(P)
    return max(tw / w, th / h)


def fit_image(img: Image.Image, P: StageParams) -> Image.Image:
    if not P.resize_on:
        return img
    if P.mode == MODE_MPIX:
        return resize_to_mpix(img, P.mpix, P.prio)
    tw, th, how = fit_target(P)
    if how == "stretch":
        return img if img.size == (tw, th) else img.resize((tw, th), Image.Resampling.LANCZOS)
    return resize_cover_crop(img, tw, th)



# ---------------------------------------------------------------------------
# Geometry: flip -> rotate (90° steps + fine angle) -> crop   (matches ui/js/crop.js)
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
