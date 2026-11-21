"""Colour constancy: sRGB <-> linear helpers, Gray-Edge, Percentile White-Patch."""
from __future__ import annotations

import cv2
import numpy as np
from PIL import Image


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
