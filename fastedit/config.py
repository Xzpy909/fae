"""Paths, defaults and the per-image stage parameters."""
from __future__ import annotations

import os
import sys
from dataclasses import dataclass, fields

# Optional decoders (input only). Both register themselves with Pillow.
try:
    from pillow_heif import register_heif_opener
    register_heif_opener()
    HAS_HEIF = True
except ImportError:
    HAS_HEIF = False
try:
    import pillow_jxl  # noqa: F401
    HAS_JXL = True
except ImportError:
    HAS_JXL = False


PLUGIN_HINT = {
    ".heic": (HAS_HEIF, "pillow-heif"), ".heif": (HAS_HEIF, "pillow-heif"), ".hif": (HAS_HEIF, "pillow-heif"),
    ".jxl": (HAS_JXL, "pillow-jxl-plugin"),
}

# Frozen (PyInstaller) build: settings + encoders/ live next to the .exe, ui/ is bundled inside.
FROZEN = getattr(sys, "frozen", False)
APP_DIR = os.path.dirname(os.path.abspath(sys.executable if FROZEN else os.path.dirname(__file__)))
UI_DIR = os.path.join(getattr(sys, "_MEIPASS", APP_DIR), "ui")
SETTINGS_PATH = os.path.join(APP_DIR, "fae_settings.json")
PRESETS_PATH = os.path.join(APP_DIR, "fae_presets.json")   # the user's tool presets
# Settings from before the rename to Fae: adopt them once.
_OLD_SETTINGS = os.path.join(APP_DIR, "fast_image_editor_settings.json")
if not os.path.exists(SETTINGS_PATH) and os.path.exists(_OLD_SETTINGS):
    try:
        os.replace(_OLD_SETTINGS, SETTINGS_PATH)
    except OSError:
        pass
ENCODERS_DIR = os.path.join(APP_DIR, "encoders")
_EXE = ".exe" if sys.platform.startswith("win") else ""
CJPEGLI = os.path.join(ENCODERS_DIR, "cjpegli" + _EXE)
SVTAV1 = os.path.join(ENCODERS_DIR, "SvtAv1EncApp" + _EXE)
JPEG_Q_DEFAULT = 90
JPEG_D_DEFAULT = 1.0
AVIF_CRF_DEFAULT = 25.0
AVIF_PRESET_DEFAULT = 4
AOM_Q_DEFAULT = 80                # Pillow/libavif quality 0..100
AOM_SPEED_DEFAULT = 6             # Pillow/libavif speed 0 (slowest) .. 10
WEBP_Q_DEFAULT = 90
FORMATS = ("WEBP", "PNG", "JPEG", "AVIF")
EXTS = {"WEBP": ".webp", "PNG": ".png", "JPEG": ".jpg", "AVIF": ".avif"}

PREVIEW_MAX = 2560          # longest side of the live preview on the CPU core (the GPU gets full resolution)
THUMB_SIZE = 112

TARGET_W, TARGET_H = 1280, 720
MODE_PRESET = "preset"
MODE_MPIX = "Mpix"
MODE_CUSTOM = "custom"
LEGACY_720 = "720p"               # old settings: same as preset 16:9 / 720p
PRIO_MPIX = "Mpix priority"
PRIO_BALANCED = "Balanced"
PRIO_RATIO = "Ratio error priority"
IMAGE_EXTS = {
    ".jpg", ".jpeg", ".png", ".webp", ".bmp", ".gif", ".tif", ".tiff",
    ".avif", ".heic", ".heif", ".hif", ".jxl", ".ico", ".ppm", ".pgm", ".tga",
}


@dataclass(frozen=True)
class StageParams:
    """Settings that change the image Python hands to the live preview."""
    resize_on: bool = False
    mode: str = MODE_PRESET
    mpix: float = 2.0
    prio: str = PRIO_BALANCED
    preset_ar: str = "16:9"
    preset_res: int = 720
    cw: int = 1920
    ch: int = 1080
    cmode: str = "crop"           # custom: "crop" (cover) or "stretch"
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
            f = (self.mode, self.mpix, self.prio, self.preset_ar, self.preset_res, self.cw, self.ch, self.cmode)
        return (self.geom_key(), f)

    def geom_key(self):
        return (self.rot_q % 4, round(self.angle, 4), self.flip,
                round(self.crop_x, 3), round(self.crop_y, 3), round(self.crop_w, 3), round(self.crop_h, 3))

    def has_geom(self):
        return self.rot_q % 4 != 0 or self.angle != 0 or self.flip or self.crop_w > 0
