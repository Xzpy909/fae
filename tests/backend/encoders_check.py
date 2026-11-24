"""B4: the encoders in the Rust back end against the Python ones they replace (docs/rust-backend-plan.md).

    python tests/backend/encoders_check.py          # synthetic images + the stage test inputs
    python tests/backend/encoders_check.py --own    # also own photos in tests/parity/images/ (slower)

The reference is fastedit/output.py, jpeg.py and avif.py (encode_lossy_py, save_image_py, ...), kept until
the owner's check on Windows. What has to match:
- WebP lossless / lossy (libwebp, the same version as Pillow's): the same bytes;
- JPEG through cjpegli (when encoders/cjpegli.exe is there) and the Pillow fallback (libjpeg-turbo): the
  same bytes;
- AVIF through SvtAv1EncApp (when there): the same bytes; always: the 10-bit YUV conversion and the muxer
  byte for byte (the muxer on AV1 streams taken from Pillow's AVIF files);
- PNG: the same pixels and ICC profile (Pillow's filters are ported exactly; zlib is zlib-rs instead of
  Pillow's zlib-ng, so the bytes and size differ a little: the sizes are printed);
- the options as the UI sends them (odd values too) give the same settings used, or both fail;
- output names (_<w>x<h>, _1, _2 ... when taken), save_image's files;
- the encoded compare's cancellation: a newer request kills a running SvtAv1EncApp / cjpegli.
"""
from __future__ import annotations

import argparse
import io
import json
import os
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, ROOT)

import numpy as np  # noqa: E402
from PIL import Image  # noqa: E402

import fae_backend as fb  # noqa: E402
from fastedit import avif as A, backend, output  # noqa: E402
from fastedit.config import ENCODERS_DIR, IMAGE_EXTS  # noqa: E402
from fastedit.jpeg import flatten_rgb, jpeg_opts, jpegli_available  # noqa: E402
from fastedit.proc import Cancelled, _TL, preview_begin  # noqa: E402

INPUTS = os.path.join(ROOT, "tests", "stage", "inputs")
OWN = os.path.join(ROOT, "tests", "parity", "images")
FAILS: list[str] = []


def check(label, cond, note=""):
    print(f"  {'ok  ' if cond else 'FAIL'} {label}{('  ' + note) if note else ''}")
    if not cond:
        FAILS.append(label)


def raw(img):
    return img.tobytes(), img.width, img.height, len(img.getbands())


def synthetic():
    rng = np.random.RandomState(7)
    h, w = 181, 263
    y, x = np.mgrid[0:h, 0:w]
    grad = np.stack([x * 255 // (w - 1), y * 255 // (h - 1), (x + y) * 255 // (w + h - 2)], -1).astype(np.uint8)
    noise = rng.randint(0, 256, (h, w, 3), dtype=np.uint8)
    alpha = ((np.hypot(x - w / 2, y - h / 2) < 70) * 255 + (x % 32 < 16) * 60).clip(0, 255).astype(np.uint8)
    out = [
        ("gradient", Image.fromarray(grad, "RGB"), None),
        ("noise", Image.fromarray(noise, "RGB"), None),
        ("gradient+alpha", Image.fromarray(np.dstack([grad, alpha]), "RGBA"), None),
        ("noise+alpha", Image.fromarray(np.dstack([noise, alpha]), "RGBA"), None),
        ("1x1", Image.new("RGB", (1, 1), (12, 200, 99)), None),
        ("wide 3001x2", Image.fromarray(np.resize(noise, (2, 3001, 3)), "RGB"), None),
    ]
    # an ICC profile from a test input (the bytes only have to travel unchanged)
    with Image.open(os.path.join(INPUTS, "icc.png")) as im:
        icc = im.info.get("icc_profile")
    out.append(("gradient+icc", out[0][1], icc))
    out.append(("alpha+big icc", out[2][1], (icc or b"x") * 60))       # > 64 KB: several JPEG APP2 markers
    return out


def inputs(own: bool):
    paths = [os.path.join(INPUTS, n) for n in ("photo.jpg", "icc.png", "rgb.png", "palette_transparent.png", "icc.webp")]
    if own and os.path.isdir(OWN):
        paths += sorted(os.path.join(OWN, n) for n in os.listdir(OWN) if os.path.splitext(n)[1].lower() in IMAGE_EXTS)
    out = []
    for p in paths:
        if not os.path.isfile(p):
            continue
        try:
            with Image.open(p) as im:
                im.load()
                icc = im.info.get("icc_profile")
                img = im.convert("RGBA" if im.has_transparency_data else "RGB")
        except Exception as ex:
            print(f"  skip {os.path.basename(p)}: {ex}")
            continue
        out.append((os.path.basename(p), img, icc))
    return out


def pillow(img, fmt, icc, **kw):
    b = io.BytesIO()
    if icc:
        kw["icc_profile"] = icc
    img.save(b, fmt, **kw)
    return b.getvalue()


def check_image(name, img, icc, b, has_jpegli, has_svt):
    px, w, h, ch = raw(img)
    # PNG
    want = pillow(img, "PNG", icc, compress_level=9, optimize=True)
    got = fb.encode_png(px, w, h, ch, icc)
    with Image.open(io.BytesIO(got)) as g:
        g.load()
        same_px = g.mode == img.mode and np.array_equal(np.asarray(g), np.asarray(img))
        same_icc = g.info.get("icc_profile") == icc
    check(f"{name}: PNG pixels + ICC", same_px and same_icc,
          f"{'same bytes' if got == want else 'size %d vs Pillow %d (%+.2f %%)' % (len(got), len(want), 100 * (len(got) / len(want) - 1))}")
    # WebP
    want = pillow(img, "WEBP", icc, lossless=True, quality=100, method=6, exact=True)
    check(f"{name}: WebP lossless bytes", fb.encode_webp(px, w, h, ch, True, 100, 6, True, icc) == want)
    for q in (0, 75, 90):
        r = b.encode(img, "WEBP", {"webp": {"lossless": False, "q": q}}, icc)
        want = output.encode_lossy_py(img, "WEBP", {"webp": {"lossless": False, "q": q}}, icc)
        check(f"{name}: WebP q{q} bytes", r == want)
    # JPEG: Pillow fallback (always), cjpegli (when there)
    for q in (1, 30, 90, 100):
        want = pillow(flatten_rgb(img), "JPEG", icc, quality=q, optimize=True, progressive=True)
        check(f"{name}: JPEG libjpeg-turbo q{q} bytes", fb.encode_jpeg_pillow(px, w, h, ch, q, icc) == want)
    if has_jpegli:
        for o in ({"mode": "q", "q": 85}, {"mode": "d", "d": 1.5}):
            want = output.encode_lossy_py(img, "JPEG", {"jpeg": o}, icc)
            got = b.encode(img, "JPEG", {"jpeg": o}, icc)
            check(f"{name}: JPEG cjpegli {o} bytes", got == want)
    # AVIF: the YUV conversion, SvtAv1EncApp (when there, RGB only)
    rgb = np.ascontiguousarray(np.asarray(img.convert("RGB")))
    check(f"{name}: AVIF 10-bit YUV", fb.yuv444p10(px, w, h, ch) == A.rgb_to_yuv444p10(rgb))
    if has_svt and img.mode == "RGB" and w * h >= 64:
        o = {"avif": {"enc": "svt", "crf": 30, "preset": 8}}
        want, got = output.encode_lossy_py(img, "AVIF", o, icc), b.encode(img, "AVIF", o, icc)
        check(f"{name}: AVIF SvtAv1EncApp bytes", got == want and got[1] == "svt", f"{len(got[0])} bytes" if got else "")


def check_mux(cases):
    """The muxer on AV1 streams from Pillow's AVIF files (needs Pillow with AVIF)."""
    if not A.avif_available():
        print("  skip muxer: Pillow has no AVIF")
        return
    for name, img, icc in cases:
        if img.mode != "RGB" or img.width * img.height < 64:
            continue
        for kw in ({"quality": 60}, {"quality": 90, "subsampling": "4:4:4"}):
            d = pillow(img, "AVIF", None, **kw)
            stream = d[d.index(b"mdat") + 4:]
            ok = fb.avif_mux(stream, img.width, img.height, icc) == A.mux_avif(stream, img.width, img.height, icc)
            check(f"{name}: AVIF muxer {kw}", ok)


def check_options():
    """The UI's options, odd ones too: the settings used are the same, or both fail."""
    odd = {
        "jpeg": (jpeg_opts, [None, {}, 55, "70", {"q": 33.5}, {"q": 34.5}, {"mode": "D", "d": 0.1875},
                             {"mode": "dist", "d": "2.25"}, {"mode": None, "q": 101, "d": -3}, {"mode": 5, "q": True},
                             {"d": 12.3456}, {"q": "abc"}, {"q": None}, {"d": [1]}]),
        "webp": (output.webp_opts, [None, {}, {"lossless": False, "q": 50.5}, {"lossless": False, "q": "x"},
                                    {"lossless": 0, "q": 300}, {"lossless": None, "q": 2.5}, {"q": -1}, {"q": "nan"}]),
        "avif": (A.avif_opts, [None, {}, {"crf": 25.13}, {"crf": "33.4", "preset": 9.5, "aom_q": "7", "aom_speed": -2},
                               {"crf": None, "preset": None, "enc": "aom"}, {"crf": 0, "enc": "AOM"},
                               {"crf": 100, "preset": 2.5}, {"crf": 25.125}, {"crf": 25.375}, {"aom_q": [3]}]),
    }
    for key, (fn, cases) in odd.items():
        fmt = {"jpeg": "JPEG", "webp": "WEBP", "avif": "AVIF"}[key]
        for o in cases:
            try:
                want = ("ok", fn(o))
            except Exception as ex:
                want = ("error", type(ex).__name__)
            try:
                got = ("ok", json.loads(fb.encoder_opts(fmt, json.dumps({key: o}))))
            except Exception as ex:
                got = ("error", type(ex).__name__)
            same = got == want or (want[0] == got[0] == "error")
            check(f"options: {key} {json.dumps(o)}", same, "" if same else f"{got} vs {want}")


_FAKE = {}


def _fake_backend():
    """A back end whose encoders folder has stand-ins (Unix only): SvtAv1EncApp writes a fixed AV1 stream,
    cjpegli copies a fixed JPEG. Checks the plumbing (arguments, files, muxing) without the real encoders."""
    if os.name == "nt" or not A.avif_available():
        return None
    if "b" not in _FAKE:
        d = tempfile.mkdtemp(prefix="fae_enc_")
        av1 = pillow(Image.new("RGB", (16, 16), (1, 2, 3)), "AVIF", None, quality=50)
        stream = av1[av1.index(b"mdat") + 4:]
        ivf = b"DKIF" + bytes(2) + (32).to_bytes(2, "little") + bytes(24) + len(stream).to_bytes(4, "little") + bytes(8) + stream
        with open(os.path.join(d, "stream.ivf"), "wb") as f:
            f.write(ivf)
        svt = os.path.join(d, "SvtAv1EncApp")
        with open(svt, "w") as f:
            f.write('#!/bin/sh\nout=""\nwhile [ $# -gt 0 ]; do [ "$1" = "-b" ] && out="$2"; shift; done\n'
                    f'[ -n "$SLOW" ] && sleep 5\ncp "{d}/stream.ivf" "$out"\n')
        os.chmod(svt, 0o755)
        _FAKE["dir"] = d
        _FAKE["b"] = backend.Backend(os.path.join(d, "s.json"), os.path.join(d, "p.json"), d)
    return _FAKE["b"]


def check_fake_svt():
    b2 = _fake_backend()
    if b2 is None:
        return
    import fastedit.avif as avif_mod
    img = Image.new("RGB", (40, 30), (200, 10, 50))
    old = avif_mod.SVTAV1
    avif_mod.SVTAV1 = os.path.join(_FAKE["dir"], "SvtAv1EncApp")
    try:
        o = {"avif": {"crf": 22.25, "preset": 6}}
        want = output.encode_lossy_py(img, "AVIF", o, b"icc!")
        got = b2.encode(img, "AVIF", o, b"icc!")
        check("stand-in SvtAv1EncApp: same file as avif.py", got == want)
    finally:
        avif_mod.SVTAV1 = old


def check_cancel(b, has_svt):
    """A newer encoded-compare request kills the running encoder (the real SvtAv1EncApp, or the stand-in)."""
    big = Image.fromarray(np.random.RandomState(1).randint(0, 256, (2000, 3000, 3), dtype=np.uint8), "RGB")
    if has_svt:
        bb, opts = b, {"avif": {"enc": "svt", "crf": 20, "preset": 0}}
    elif _fake_backend() is not None:
        os.environ["SLOW"] = "1"
        bb, opts = _fake_backend(), {"avif": {"enc": "svt"}}
    else:
        print("  skip cancellation: no SvtAv1EncApp")
        return
    res = {}

    def job():
        bb.preview_begin(1000)
        try:
            res["r"] = bb.encode(big, "AVIF", opts, None, 1000)
        except Cancelled:
            res["r"] = "cancelled"

    t = threading.Thread(target=job)
    t.start()
    time.sleep(1.0)
    t0 = time.time()
    bb.preview_begin(1001)
    t.join(30)
    os.environ.pop("SLOW", None)
    check("cancellation: a newer request stops the encoder", res.get("r") == "cancelled", f"{time.time() - t0:.2f} s")
    try:
        bb.encode(Image.new("RGB", (8, 8)), "WEBP", {"webp": {"lossless": False}}, None, 999)
        check("cancellation: an old request doesn't start", False)
    except Cancelled:
        check("cancellation: an old request doesn't start", True)


def check_names_and_save(b, has_jpegli):
    d = tempfile.mkdtemp(prefix="fae_out_")
    src = os.path.join(d, "Moonforge.v2.png")
    img = Image.new("RGBA", (40, 30), (10, 20, 30, 128))
    for fmt, opts in (("PNG", None), ("WEBP", None), ("WEBP", {"webp": {"lossless": False, "q": 70}}),
                      ("JPEG", {"jpeg": {"q": 80}}), ("BMP", None)):
        p_py = output.save_image_py(img, src, fmt, b"icc", opts)
        p_rs = b.save(img, src, fmt, b"icc", opts)
        with open(p_py, "rb") as f1, open(p_rs, "rb") as f2:
            a, c = f1.read(), f2.read()
        same = a == c if fmt != "PNG" else np.array_equal(np.asarray(Image.open(io.BytesIO(a))), np.asarray(Image.open(io.BytesIO(c))))
        base = os.path.join(d, "Moonforge.v2_40x30")
        check(f"save {fmt}: names {os.path.basename(p_py)} / {os.path.basename(p_rs)}, same file",
              same and p_rs.startswith(base) and p_py != p_rs)
    check("out_path: next free name", fb.out_path(src, 40, 30, ".png") == os.path.join(d, "Moonforge.v2_40x30_2.png"))
    check("out_path: no extension", fb.out_path(os.path.join(d, ".hidden"), 1, 2, ".webp") == os.path.join(d, ".hidden_1x2.webp"))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--own", action="store_true", help="also own photos in tests/parity/images/")
    a = ap.parse_args()
    print(f"fae_backend {fb.version()}; encoders: {ENCODERS_DIR}")
    b = backend.shared()
    has_jpegli = jpegli_available()
    has_svt = A.svt_available()
    print(f"cjpegli: {'yes' if has_jpegli else 'no (JPEG checks: the Pillow fallback only)'}; "
          f"SvtAv1EncApp: {'yes' if has_svt else 'no (AVIF checks: YUV and muxer only)'}")
    nojpegli = backend.Backend(*(os.path.join(tempfile.mkdtemp(), n) for n in ("s.json", "p.json")), None)
    r = nojpegli.encode(Image.new("RGB", (8, 8)), "JPEG", {"jpeg": {"mode": "d", "q": 70}})
    check("no cjpegli: the libjpeg-turbo fallback", r[1] == "pillow" and r[0] == pillow(Image.new("RGB", (8, 8)), "JPEG", None, quality=70, optimize=True, progressive=True))
    cases = synthetic() + inputs(a.own)
    for name, img, icc in cases:
        print(f"{name} ({img.width} x {img.height} {img.mode}{', ICC' if icc else ''})")
        check_image(name, img, icc, b, has_jpegli, has_svt)
    print("muxer")
    check_mux(cases)
    print("options")
    check_options()
    print("save / names")
    check_names_and_save(b, has_jpegli)
    print("stand-in encoders / cancellation")
    check_fake_svt()
    check_cancel(b, has_svt)
    print("ALL OK" if not FAILS else f"{len(FAILS)} FAILED")
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
