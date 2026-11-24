"""Stage tests for the Rust back end (docs/rust-backend-plan.md): what fastedit/ does to an image before
the edit (decode, geometry, resize, colour balance, the whole stage, file-list thumbnails, the size
maths), recorded from the Python implementation **before** each part moves to Rust, then checked against
whichever implementation runs it.

    python tests/stage/stage_check.py                  # check the Rust back end against the stored results
    python tests/stage/stage_check.py --impl python    # Pillow's decode / thumbnails (the rest: no Python version
                                                       #   since B3, skipped)
    python tests/stage/stage_check.py -k resize -v     # only cases whose name contains "resize", list each
    python tests/stage/stage_check.py --record-missing # store only cases without a result (new inputs / cases;
                                                       #   from Python where it still exists, else from Rust)
    python tests/stage/stage_check.py --make-inputs    # (re)write tests/stage/inputs/ (once; committed)
    python tests/stage/stage_check.py --own            # also own photos in tests/parity/images/ (results in
                                                       #   tests/stage/.work/own_results.zip, not committed;
                                                       #   --record-missing --own stores them)

Inputs: small synthetic files in every format the app opens (tests/stage/inputs/, committed so every
machine decodes the same bytes) and two synthetic images made in memory (a 401 x 267 "scene", a
colour-cast one for the colour balance). Rules, per case:
- exact: decoding, quarter turns / flips / crops, every resize (Pillow's Lanczos is ported exactly), the
  size maths;
- known, deliberate differences (FIXED, META_DIFF below): the stored result is the fixed one; the
  Python implementation skips those cases;
- tolerance (the GPU core's chain rule: mean < 0.05 levels, at most 0.1 % of the pixels, at least 2,
  more than 2 levels off): free-angle rotation and the colour balance (OpenCV's maths re-implemented),
  thumbnails (JPEG), the whole stage when it uses those.
A failing case writes stored | got | difference x16 to tests/stage/.work/<case>.png.
"""
from __future__ import annotations

import argparse
import dataclasses
import glob
import hashlib
import io
import json
import os
import sys
import time
import zipfile

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
INPUTS = os.path.join(HERE, "inputs")
RESULTS = os.path.join(HERE, "results.zip")
WORK = os.path.join(HERE, ".work")
OWN_DIR = os.path.join(ROOT, "tests", "parity", "images")
OWN_RESULTS = os.path.join(WORK, "own_results.zip")
sys.path.insert(0, ROOT)

MAX_LEVELS = 2
MAX_MEAN = 0.05
MAX_OUTLIERS = 1e-3
MIN_OUTLIERS = 2


# ---- inputs -----------------------------------------------------------------------------------------
def scene(w=401, h=267) -> np.ndarray:
    """A deterministic photo-like image: smooth gradients, hard edges, thin lines, fine stripes, a
    noise patch (resize and rotation filters show on all of them)."""
    y, x = np.mgrid[0:h, 0:w].astype(np.float64)
    a = np.stack([90 + 80 * np.sin(x / 53.0) * np.cos(y / 41.0) + 60 * x / w,
                  60 + 150 * y / h,
                  200 - 120 * x / w + 30 * np.sin((x + y) / 17.0)], -1)
    a[(x - 0.3 * w) ** 2 + (y - 0.45 * h) ** 2 < (0.18 * h) ** 2] = [235, 70, 40]          # a red disc
    a[int(0.62 * h):int(0.8 * h), int(0.55 * w):int(0.9 * w)] = [25, 30, 35]            # a dark block
    a[:, int(0.75 * w)::7][..., 1] = 250                                                  # thin green lines
    a[int(0.1 * h):int(0.3 * h), int(0.6 * w):int(0.95 * w)] = np.where(
        (x[int(0.1 * h):int(0.3 * h), int(0.6 * w):int(0.95 * w), None] // 2) % 2 == 0, 240, 15)  # fine stripes
    rng = np.random.default_rng(11)
    a[h - 60:h - 10, 10:110] = rng.integers(0, 256, (50, 100, 3))                         # noise
    return np.clip(np.round(a), 0, 255).astype(np.uint8)


def cast() -> np.ndarray:
    """A scene with a warm cast and a bright highlight (White-Patch / Gray-Edge have work to do)."""
    a = scene(320, 214).astype(np.float64)
    a = a * np.array([1.0, 0.86, 0.68]) + np.array([12, 4, 0])
    a[20:40, 20:60] = [250, 238, 205]
    return np.clip(np.round(a), 0, 255).astype(np.uint8)


def make_inputs():
    """Write tests/stage/inputs/ (Pillow and the optional plugins; formats a plugin can't write are left out)."""
    os.makedirs(INPUTS, exist_ok=True)
    big = Image.fromarray(scene(400, 267))
    small = Image.fromarray(scene(120, 80))
    alpha = Image.fromarray(np.clip(np.mgrid[0:80, 0:120][1] * 2.1, 0, 255).astype(np.uint8))
    rgba = small.copy()
    rgba.putalpha(alpha)
    try:
        from PIL import ImageCms
        icc = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
    except Exception:
        icc = None
    out = {}

    def save(name, im, **kw):
        path = os.path.join(INPUTS, name)
        if os.path.exists(path):                  # existing inputs stay byte for byte (the goldens are theirs)
            out[name] = "kept"
            return
        try:
            im.save(path, **kw)
            out[name] = True
        except Exception as ex:                   # format not writable here: leave it out
            out[name] = f"not written: {type(ex).__name__}: {ex}"
            if os.path.exists(path):
                os.remove(path)

    save("photo.jpg", big, quality=90, **({"icc_profile": icc} if icc else {}))
    save("photo_444.jpg", big, quality=95, subsampling=0)
    save("photo_progressive.jpg", big, quality=85, progressive=True)
    save("grey.jpg", small.convert("L"), quality=90)
    save("cmyk.jpg", small.convert("CMYK"), quality=92)
    for o in range(2, 9):                         # EXIF orientations
        ex = Image.Exif()
        ex[0x0112] = o
        save(f"orient_{o}.jpg", small, quality=90, exif=ex.tobytes())
    save("rgb.png", small)
    save("rgba.png", rgba)
    save("grey.png", small.convert("L"))
    save("grey_alpha.png", rgba.convert("LA"))
    g16 = Image.fromarray((np.asarray(small.convert("L")).astype(np.uint16) * 257).astype(np.uint16))
    save("grey16.png", g16)
    pal = small.convert("P", palette=Image.Palette.ADAPTIVE, colors=64)
    pal.info["transparency"] = 0
    save("palette_transparent.png", pal, transparency=0)
    save("palette.gif", small.convert("P", palette=Image.Palette.ADAPTIVE, colors=32))
    save("rgb.webp", small, quality=85)
    save("rgba_lossless.webp", rgba, lossless=True)
    save("rgb.tif", small)
    save("rgba.tif", rgba, compression="tiff_lzw")
    save("rgb.bmp", small)
    save("rgb.ppm", small)
    save("grey.pgm", small.convert("L"))
    save("rgb.tga", small)
    save("rgb.ico", small.resize((64, 64)))
    try:
        from pillow_heif import register_heif_opener
        register_heif_opener()
        save("rgb.heic", small, quality=90)
    except ImportError:
        out["rgb.heic"] = "not written: pillow-heif missing"
    try:
        import pillow_jxl  # noqa: F401
        save("rgb.jxl", small, lossless=True)
    except ImportError:
        out["rgb.jxl"] = "not written: pillow-jxl-plugin missing"
    save("rgb.avif", small, quality=80)
    # B1: what the Rust decoders do beyond the basics
    rgb16 = np.asarray(small).astype(np.uint16) * 257 + np.arange(80)[:, None, None].astype(np.uint16)
    save("rgb16.png", _png16(rgb16, "RGB"))
    save("rgba16.png", _png16(np.concatenate([rgb16, (np.asarray(alpha).astype(np.uint16) * 257)[..., None]], -1), "RGBA"))
    ex6 = Image.Exif()
    ex6[0x0112] = 6
    save("exif6.png", small, exif=ex6.tobytes())
    save("exif6.webp", small, quality=90, exif=ex6.tobytes())
    if icc:
        save("icc.png", small, icc_profile=icc)
        save("icc.webp", small, quality=90, icc_profile=icc)
    big_icc = bytes(np.random.default_rng(5).integers(0, 256, 150_000, dtype=np.uint8))   # 3 APP2 chunks
    save("icc_3chunks.jpg", small, quality=90, icc_profile=big_icc)
    gt = small.convert("P", palette=Image.Palette.ADAPTIVE, colors=32)
    save("transparent.gif", gt, transparency=3)
    try:
        from pillow_heif import register_heif_opener
        register_heif_opener()
        save("rgba.heic", rgba, quality=90)
    except ImportError:
        out["rgba.heic"] = "not written: pillow-heif missing"
    # B2: thumbnails that use JPEG draft scaling (+ EXIF rotation) and Pillow's reduce step
    ex6 = Image.Exif()
    ex6[0x0112] = 6
    save("thumb_draft.jpg", Image.fromarray(scene(1000, 700)), quality=85, exif=ex6.tobytes())
    save("thumb_wide.jpg", Image.fromarray(scene(2000, 240)), quality=85)
    save("thumb_reduce.png", Image.fromarray(scene(960, 300)))
    for k, v in out.items():
        print(f"  {k:28s} {'ok' if v is True else v}")


def _png16(a, mode):
    """A 16-bit PNG from a uint16 array (Pillow doesn't write 16-bit RGB): a minimal writer."""
    import struct
    import zlib
    h, w, c = a.shape
    raw = b"".join(b"\0" + a[y].astype(">u2").tobytes() for y in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    ct = 2 if mode == "RGB" else 6
    png_bytes = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 16, ct, 0, 0, 0))
                 + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))
    return _Raw(png_bytes)


class _Raw:
    """Bytes that save() writes as they are."""
    def __init__(self, data):
        self.data = data

    def save(self, path, **_kw):
        with open(path, "wb") as f:
            f.write(self.data)


# ---- the cases ----------------------------------------------------------------------------------------
# geometry: StageParams fields; "exact" unless a free angle is used
GEOMETRY = {
    "q1": dict(rot_q=1), "q2": dict(rot_q=2), "q3": dict(rot_q=3), "flip": dict(flip=True),
    "flip_q1": dict(flip=True, rot_q=1),
    "crop": dict(crop_x=-40.5, crop_y=17.0, crop_w=201.0, crop_h=130.0),
    "crop_edge": dict(crop_x=150.0, crop_y=-100.0, crop_w=180.0, crop_h=99.0),
    "q1_crop": dict(rot_q=1, crop_x=10.0, crop_y=-20.0, crop_w=151.0, crop_h=203.0),
    "angle_7.5": dict(angle=7.5),
    "angle_-33_crop": dict(angle=-33.0, crop_x=12.0, crop_y=-8.0, crop_w=150.0, crop_h=100.0),
    "q3_flip_angle_2.25_crop": dict(rot_q=3, flip=True, angle=2.25, crop_x=0.0, crop_y=30.0, crop_w=201.0, crop_h=251.0),
}
# resize: StageParams with resize_on
RESIZE = {
    "preset_16x9_240": dict(mode="preset", preset_ar="16:9", preset_res=240),
    "preset_16x9_360_up": dict(mode="preset", preset_ar="16:9", preset_res=360),
    "preset_9x16_300": dict(mode="preset", preset_ar="9:16", preset_res=300),
    "preset_1x1_256": dict(mode="preset", preset_ar="1:1", preset_res=256),
    "preset_4x3_201": dict(mode="preset", preset_ar="4:3", preset_res=201),
    "mpix_0.05_balanced": dict(mode="Mpix", mpix=0.05, prio="Balanced"),
    "mpix_0.05_mpix": dict(mode="Mpix", mpix=0.05, prio="Mpix priority"),
    "mpix_0.05_ratio": dict(mode="Mpix", mpix=0.05, prio="Ratio error priority"),
    "mpix_0.16_up": dict(mode="Mpix", mpix=0.16),
    "custom_300x180_crop": dict(mode="custom", cw=300, ch=180, cmode="crop"),
    "custom_300x180_stretch": dict(mode="custom", cw=300, ch=180, cmode="stretch"),
    "custom_97x403_crop": dict(mode="custom", cw=97, ch=403, cmode="crop"),
}
COLOUR = {
    "wp_99_1": ("wp", 99.0, 1.0), "wp_95_0.5": ("wp", 95.0, 0.5), "wp_99.9_1": ("wp", 99.9, 1.0),
    "ge_p1_s2_1": ("ge", 1, 2.0, 1.0), "ge_p6_s0_0.7": ("ge", 6, 0.0, 0.7), "ge_p1_s5_1": ("ge", 1, 5.0, 1.0),
    "ge_p3_s1_1": ("ge", 3, 1.0, 1.0),
}
# the whole stage (Engine.stage) on inputs/photo.jpg: (params, exact?)
STAGE = {
    "default": ({}, True),
    "resize_preset_240": (dict(resize_on=True, mode="preset", preset_res=240), True),
    "q1_crop_mpix": (dict(rot_q=1, crop_x=0.0, crop_y=0.0, crop_w=200.0, crop_h=300.0, resize_on=True, mode="Mpix", mpix=0.04), True),
    "angle_wp_ge": (dict(angle=-4.0, crop_x=0.0, crop_y=0.0, crop_w=360.0, crop_h=230.0, wp_on=True, ge_on=True), False),
    "everything": (dict(flip=True, rot_q=2, angle=1.5, crop_x=5.0, crop_y=5.0, crop_w=380.0, crop_h=250.0, resize_on=True,
                        mode="custom", cw=320, ch=200, cmode="crop", wp_on=True, wp_pct=97.0, wp_strength=0.8,
                        ge_on=True, ge_p=2, ge_sigma=1.5, ge_strength=0.6), False),
}
SIZES_WH = [(641, 427), (4000, 3000), (6000, 4000), (3024, 4032), (1920, 1080), (1000, 1000), (7952, 5304), (17, 1031)]


# ---- deliberate differences from the Python implementation --------------------------------------------------
def _grey16_fixed():
    """16-bit grey PNG as the Rust back end opens it: the high byte (as Pillow does for 16-bit RGB)."""
    a = np.asarray(Image.open(os.path.join(INPUTS, "grey16.png"))).astype(np.uint16)
    g = (a >> 8).astype(np.uint8)
    return np.repeat(g[..., None], 3, -1)


# case -> (why, the stored result): the golden is the fixed result; the Python implementation skips the case
FIXED = {
    "decode/grey16.png": ("Pillow clipped 16-bit grey to 0..255, so such files opened almost white; the Rust "
                          "back end takes the high byte", _grey16_fixed),
}
# case -> (metadata key the Rust implementation may differ in, why)
META_DIFF = {
    "decode/rgb.jxl": ("icc", "pillow-jxl synthesizes an ICC profile from the file's colour description; "
                              "jxl-oxide keeps only an embedded one (none here: sRGB either way)"),
}


# ---- implementations ------------------------------------------------------------------------------------
class NotMoved(Exception):
    """this part isn't in the implementation yet"""


class PythonImpl:
    """The Python code as it was, where it still exists: Pillow's decode and thumbnails. Geometry, resize,
    colour balance and the whole stage lost their Python version in B3 (geometry.py, colour.py and the
    Python stage were removed once the Rust back end matched them on every case): the stored results are
    their reference now, and new cases of theirs are recorded from the Rust back end (--record-missing)."""
    name = "python"

    def __init__(self):
        from fastedit import engine
        self.engine = engine

    def decode(self, path):
        # Pillow itself (the stage opens files with the Rust back end since B1): the reference
        with self.engine.open_image(path) as im:
            im.load()
            icc = im.info.get("icc_profile")
            base = self.engine.prepare(im)
        return np.asarray(base), {"mode": base.mode, "icc": hashlib.sha1(icc).hexdigest()[:12] if icc else None}

    def geometry_(self, a, kw):
        raise NotMoved("geometry: Python version removed in B3")

    def resize(self, a, kw):
        raise NotMoved("resize: Python version removed in B3")

    def colour_(self, a, spec):
        raise NotMoved("colour balance: Python version removed in B3")

    def stage(self, path, kw):
        raise NotMoved("the stage: Python version removed in B3")

    def thumb(self, path):
        # Pillow itself (engine.make_thumb uses the Rust back end since B2): the reference
        return np.asarray(Image.open(io.BytesIO(self.engine.make_thumb_pillow(path))).convert("RGB"))

    def sizes(self):
        raise NotMoved("the size maths: Python version removed in B3")


class RustImpl:
    """The Rust back end (fae_backend). Parts arrive wave by wave (B1 decode, B2 geometry / resize /
    thumbnails / the size maths, B3 colour balance and the stage); until then they raise NotMoved and
    are skipped."""
    name = "rust"

    def __init__(self):
        import fae_backend
        from fastedit import backend
        self.fb = fae_backend
        self.b = backend.shared()

    def _fn(self, name):
        f = getattr(self.fb, name, None)
        if f is None:
            raise NotMoved(name)
        return f

    def _raw(self, name, a, kw):
        h, w, c = a.shape
        px, ow, oh, oc = self._fn(name)(np.ascontiguousarray(a).tobytes(), w, h, c, json.dumps(kw))
        return np.frombuffer(px, np.uint8).reshape(oh, ow, oc)

    def decode(self, path):
        r = self.b.decode(path)
        if r is None:
            raise NotMoved(f"{os.path.basename(path)}: format not in Rust yet")
        px, w, h, ch, icc, _fmt = r
        a = np.frombuffer(px, np.uint8).reshape(h, w, ch)
        return a, {"mode": "RGBA" if ch == 4 else "RGB", "icc": hashlib.sha1(icc).hexdigest()[:12] if icc else None}

    def geometry_(self, a, kw):
        return self._raw("geometry", a, kw)

    def resize(self, a, kw):
        return self._raw("fit", a, dict(resize_on=True, **kw))

    def colour_(self, a, spec):
        h, w, c = a.shape
        px = np.ascontiguousarray(a).tobytes()
        if spec[0] == "wp":
            r = self.fb.white_patch(px, w, h, c, float(spec[1]), float(spec[2]))
        else:
            r = self.fb.gray_edge(px, w, h, c, int(spec[1]), float(spec[2]), float(spec[3]))
        px, ow, oh, oc = r
        return np.frombuffer(px, np.uint8).reshape(oh, ow, oc)

    def stage(self, path, kw):
        from fastedit import config
        P = config.StageParams(**kw)
        px, w, h, ch, _icc = self.b._b.stage(path, json.dumps(dataclasses.asdict(P)))
        return np.frombuffer(px, np.uint8).reshape(h, w, ch)

    def thumb(self, path):
        from fastedit.config import THUMB_SIZE
        try:
            data = self.b._b.thumb(path, THUMB_SIZE * 2)
        except RuntimeError as ex:
            if str(ex).startswith("unknown:"):
                raise NotMoved(str(ex)) from None
            raise
        return np.asarray(Image.open(io.BytesIO(data)).convert("RGB"))

    def sizes(self):
        mp, ps = self._fn("mpix_size"), self._fn("preset_size")
        out = {}
        for w, h in SIZES_WH:
            for m in (0.05, 0.5, 2.0, 12.0):
                for pr in ("Balanced", "Mpix priority", "Ratio error priority"):
                    out[f"mpix {w}x{h} {m} {pr}"] = list(mp(w, h, m, pr))
        for ar in ("16:9", "4:3", "3:2", "1:1", "9:16", "2:3", "21:9", "bad", "0:5"):
            for res in (16, 240, 720, 1080, 2160):
                out[f"preset {ar} {res}"] = list(ps(ar, res))
        return out


# ---- results ------------------------------------------------------------------------------------------
def _img(r):
    return r[0] if isinstance(r, tuple) else r


def png(a: np.ndarray) -> bytes:
    b = io.BytesIO()
    Image.fromarray(a).save(b, "PNG", optimize=True)
    return b.getvalue()


def unpng(data: bytes) -> np.ndarray:
    return np.asarray(Image.open(io.BytesIO(data)))


def compare(got: np.ndarray, want: np.ndarray, exact: bool) -> tuple[bool, str]:
    if got.shape != want.shape:
        return False, f"shape {got.shape} vs stored {want.shape}"
    d = np.abs(got.astype(np.int16) - want.astype(np.int16))
    if d.max() == 0:
        return True, "identical"
    dp = d.max(axis=-1) if d.ndim == 3 else d
    far = int((dp > MAX_LEVELS).sum())
    msg = f"max {int(d.max())}, {(d > 0).mean() * 100:.4f} % of values off, {far} px > {MAX_LEVELS}, mean {d.mean():.5f}"
    if exact:
        return False, "not identical: " + msg
    return d.mean() < MAX_MEAN and far <= max(MIN_OUTLIERS, MAX_OUTLIERS * dp.size), msg


def diff_image(name: str, got: np.ndarray, want: np.ndarray):
    if got.shape != want.shape:
        return
    to3 = lambda a: np.repeat(a[..., None], 3, -1) if a.ndim == 2 else a[..., :3]
    g, w = to3(got), to3(want)
    d = np.clip(np.abs(g.astype(np.int16) - w.astype(np.int16)) * 16, 0, 255).astype(np.uint8)
    os.makedirs(WORK, exist_ok=True)
    Image.fromarray(np.concatenate([w, g, d], 1)).save(os.path.join(WORK, name.replace("/", "__") + ".png"))


def cases(own: bool):
    """(name, kind, run(impl) -> array | json, exact)"""
    sc, cc = scene(), cast()
    sc_rgba = np.concatenate([cast(), np.full(cast().shape[:2] + (1,), 200, np.uint8)], -1)
    out = []
    for p in sorted(glob.glob(os.path.join(INPUTS, "*"))):
        n = os.path.basename(p)
        out.append((f"decode/{n}", "img", lambda im, p=p: im.decode(p), True))
    for n, kw in GEOMETRY.items():
        out.append((f"geometry/{n}", "img", lambda im, kw=kw: im.geometry_(sc, kw), "angle" not in kw))
    out.append(("geometry/rgba_angle_5", "img", lambda im: im.geometry_(sc_rgba, dict(angle=5.0)), False))
    # transparent images: a centred object, transparent around it; free crop past the edges
    obj = sc_rgba.copy()
    obj[..., 3] = 0
    obj[40:170, 80:240, 3] = 255
    obj[60:150, 100:220, 3] = np.uint8(128)
    for n, kw in {"free_crop_past_edges": dict(crop_x=60.0, crop_y=-30.0, crop_w=420.0, crop_h=300.0, crop_free=True),
                  "free_crop_bounded_off": dict(crop_x=60.0, crop_y=-30.0, crop_w=420.0, crop_h=300.0),
                  "free_crop_q1": dict(rot_q=1, crop_x=0.0, crop_y=0.0, crop_w=400.0, crop_h=400.0, crop_free=True),
                  "free_crop_angle_12": dict(angle=12.0, crop_x=-20.0, crop_y=10.0, crop_w=380.0, crop_h=280.0, crop_free=True)}.items():
        out.append((f"geometry/rgba_{n}", "img", lambda im, kw=kw: im.geometry_(obj, kw), "angle" not in kw))
    for n, kw in RESIZE.items():
        out.append((f"resize/{n}", "img", lambda im, kw=kw: im.resize(sc, kw), True))
    out.append(("resize/rgba_preset_240", "img", lambda im: im.resize(sc_rgba, dict(mode="preset", preset_res=120)), True))
    for n, spec in COLOUR.items():
        out.append((f"colour/{n}", "img", lambda im, spec=spec: im.colour_(cc, spec), False))
    out.append(("colour/rgba_wp_ge", "img", lambda im: im.colour_(sc_rgba, ("ge", 1, 2.0, 1.0)), False))
    photo = os.path.join(INPUTS, "photo.jpg")
    for n, (kw, ex) in STAGE.items():
        out.append((f"stage/{n}", "img", lambda im, kw=kw: im.stage(photo, kw), ex))
    for n in ("photo.jpg", "rgba.png", "orient_6.jpg", "thumb_draft.jpg", "thumb_wide.jpg", "thumb_reduce.png"):
        out.append((f"thumb/{n}", "img", lambda im, n=n: im.thumb(os.path.join(INPUTS, n)), False))
    out.append(("sizes/all", "json", lambda im: im.sizes(), True))
    if own:
        for p in sorted(glob.glob(os.path.join(OWN_DIR, "*"))):
            n = os.path.basename(p)
            out.append((f"own/{n}/decode", "img", lambda im, p=p: im.decode(p), True))
            out.append((f"own/{n}/stage_1080", "img", lambda im, p=p: im.stage(p, dict(resize_on=True, mode="preset", preset_res=1080)), True))
            # B2 on real photos: the resize, a fine angle with a crop, the thumbnail
            out.append((f"own/{n}/fit_1080", "img", lambda im, p=p: im.resize(im.decode(p)[0], dict(mode="preset", preset_res=1080)), True))
            out.append((f"own/{n}/angle_3_crop", "img", lambda im, p=p: _own_angle(im, p), False))
            out.append((f"own/{n}/thumb", "img", lambda im, p=p: im.thumb(p), False))
    return out


def _own_angle(im, path):
    a = im.decode(path)[0]
    h, w = a.shape[:2]
    return im.geometry_(a, dict(angle=3.0, crop_x=w * 0.02, crop_y=-h * 0.01, crop_w=w * 0.85, crop_h=h * 0.85))


def record_missing(args) -> int:
    """Add the results of cases that have none to results.zip: the Python version's where it still exists,
    else the Rust back end's (geometry, resize, colour balance, the stage: identical to the removed Python
    code on every case when B3 removed it); every stored entry keeps its bytes."""
    py, rs = PythonImpl(), None

    def run_ref(run):
        nonlocal rs
        try:
            return run(py)
        except NotMoved:
            rs = rs or RustImpl()
            return run(rs)
    with zipfile.ZipFile(RESULTS) as z:
        items = {n: z.read(n) for n in z.namelist()}
    own = {}
    if args.own and os.path.exists(OWN_RESULTS):
        with zipfile.ZipFile(OWN_RESULTS) as z:
            own = {n: z.read(n) for n in z.namelist()}
    meta = json.loads(items.get("meta.json", b"{}"))
    added = []
    for name, kind, run, _exact in cases(args.own):
        key = name + (".json" if kind == "json" else ".png")
        if name.startswith("own/"):
            if key not in own and args.k in name:
                own[key] = png(np.ascontiguousarray(_img(run_ref(run))))
                added.append(name)
            continue
        if key in items or args.k not in name:
            continue
        if name in FIXED:
            items[key] = png(FIXED[name][1]())
        else:
            r = run_ref(run)
            if isinstance(r, tuple):
                r, info = r
                meta[name] = info
            items[key] = json.dumps(r, sort_keys=True).encode() if kind == "json" else png(np.ascontiguousarray(r))
        added.append(name)
    items["meta.json"] = json.dumps(meta, sort_keys=True, indent=1).encode()
    with zipfile.ZipFile(RESULTS, "w", zipfile.ZIP_STORED) as z:
        for k in sorted(items):
            z.writestr(zipfile.ZipInfo(k, date_time=(2026, 1, 1, 0, 0, 0)), items[k])
    if own:
        os.makedirs(WORK, exist_ok=True)
        with zipfile.ZipFile(OWN_RESULTS, "w", zipfile.ZIP_STORED) as z:
            for k in sorted(own):
                z.writestr(zipfile.ZipInfo(k, date_time=(2026, 1, 1, 0, 0, 0)), own[k])
    print(f"added {len(added)}: {', '.join(added) or '-'}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--impl", default="rust", choices=["python", "rust"])
    ap.add_argument("-k", default="")
    ap.add_argument("-v", action="store_true")
    ap.add_argument("--record", action="store_true", help="store the Python implementation's results")
    ap.add_argument("--record-missing", action="store_true", help="store results only for cases that have none")
    ap.add_argument("--make-inputs", action="store_true")
    ap.add_argument("--own", action="store_true", help="also own photos in tests/parity/images/")
    args = ap.parse_args()
    if args.make_inputs:
        make_inputs()
        return 0
    if args.record_missing:
        return record_missing(args)
    if args.record:
        print("--record would rewrite every result, and the Python version of geometry, resize, colour balance and the\n"
              "stage is gone since B3 (the stored results are their reference): use --record-missing")
        return 2
    im = PythonImpl() if args.impl == "python" else RustImpl()
    t0 = time.perf_counter()
    sel = [c for c in cases(args.own) if args.k in c[0]]
    stored = {}
    for path in (RESULTS, OWN_RESULTS):
        if not args.record and os.path.exists(path):
            with zipfile.ZipFile(path) as z:
                stored.update({n: z.read(n) for n in z.namelist()})
    new, new_own = {}, {}
    meta_stored = json.loads(stored.get("meta.json", b"{}"))
    meta = {}
    ok = bad = skipped = 0
    for name, kind, run, exact in sel:
        if name in FIXED and im.name == "python":
            if args.record:
                new[name + ".png"] = png(FIXED[name][1]())
            skipped += 1
            if args.v:
                print(f"  skip {name:44s} known difference: {FIXED[name][0]}")
            continue
        try:
            r = run(im)
        except NotMoved as ex:
            skipped += 1
            if args.v:
                print(f"  skip {name:44s} not in the {im.name} implementation yet ({ex})")
            continue
        info = None
        if isinstance(r, tuple):
            r, info = r
        target = new_own if name.startswith("own/") else new
        if kind == "json":
            key = name + ".json"
            data = json.dumps(r, sort_keys=True).encode()
            if args.record:
                target[key] = data
                good, msg = True, "recorded"
            elif key not in stored:
                good, msg = False, "not recorded"
            else:
                diff = [k for k, v in r.items() if json.loads(stored[key]).get(k) != v]
                good, msg = (not diff), ("identical" if not diff else f"{len(diff)} differ, e.g. {diff[0]}: {r[diff[0]]}")
        else:
            r = np.ascontiguousarray(r)
            key = name + ".png"
            if args.record:
                target[key] = png(r)
                if info:
                    meta[name] = info
                good, msg = True, "recorded"
            elif key not in stored:
                good, msg = False, "not recorded (--record-missing)"
            else:
                want = unpng(stored[key])
                good, msg = compare(r, want, exact)
                if info and name in meta_stored and info != meta_stored[name]:
                    ms = dict(meta_stored[name])
                    if im.name == "rust" and name in META_DIFF:      # an allowed difference
                        k, why = META_DIFF[name]
                        ms[k] = info.get(k)
                        msg += f" ({k} differs: {why.split(';')[0]})"
                    if info != ms:
                        good, msg = False, f"{msg}; metadata {info} vs stored {meta_stored[name]}"
                if not good:
                    diff_image(name, r, want)
        ok += good
        bad += not good
        if args.v or not good:
            print(f"  {'ok  ' if good else 'FAIL'} {name:44s} {'exact' if exact else 'tol  '} {msg}")
    if args.record:
        new["meta.json"] = json.dumps(meta, sort_keys=True, indent=1).encode()
        for path, d in ((RESULTS, new), (OWN_RESULTS, new_own)):
            if not d or (path == RESULTS and args.k):
                continue
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as z:
                for k in sorted(d):
                    z.writestr(zipfile.ZipInfo(k, date_time=(2026, 1, 1, 0, 0, 0)), d[k])
            print(f"recorded {len(d)} results in {os.path.relpath(path, ROOT)} ({os.path.getsize(path) // 1024} KB)")
        if args.k:
            print("(-k with --record: results.zip not rewritten; record without -k)")
    dt = time.perf_counter() - t0
    print(f"{im.name}: {ok} ok, {bad} failed, {skipped} not in this implementation yet  [{dt:.1f} s]")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
