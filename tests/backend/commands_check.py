"""The commands moved to the Rust back end so far, against the Python behaviour they replace
(docs/rust-backend-plan.md). Each wave adds its commands here.

    python tests/backend/commands_check.py

B0: get_settings / save_settings / get_presets / save_presets. The reference is api.py's code before B0
(kept below): the same files must come back the same, edge cases included (missing, broken, not an
object, non-ASCII text, key order, a file written by the old code read by the new and back).

B3: load_stage / load_source / sample. The reference is api.py's code before B3 (kept below, on the
Python stage in engine.py): the same answers (sizes, scale, alpha, ...), the same preview pixels, the
same samples; AVIF (opened by Pillow until B4) goes through give_source.
"""
from __future__ import annotations

import json
import os
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, ROOT)

from fastedit import backend  # noqa: E402


# ---- the Python behaviour before B0 (api.py) ------------------------------------------------------------
def py_get_settings(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return {}


def py_save(path, data):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)


def py_get_presets(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


# ---- the Python behaviour before B3 (api.py, on engine.py's Python stage) -------------------------------
def py_load_stage(path, params, req_id, full=0):
    import numpy as np
    from PIL import Image
    from fastedit.api import _preview_rgba, _pv_size
    from fastedit.config import StageParams
    from fastedit.engine import ENGINE
    P = StageParams.from_dict(params)
    e, img = ENGINE.stage(path, P)
    w, h = img.size
    pw, ph = _pv_size(w, h, full)
    rgba = _preview_rgba(img, (pw, ph), Image.Resampling.LANCZOS)
    ENGINE.current = {"rgb": np.ascontiguousarray(np.asarray(img.convert("RGB")))}
    return {
        "ok": True, "req_id": req_id,
        "w": w, "h": h, "pw": pw, "ph": ph, "full": float(full or 0),
        "src_w": e.size[0], "src_h": e.size[1],
        "alpha": img.mode == "RGBA",
        "scale": round(backend.shared().fit_scale(e.geo_size(P), P), 4),
        "geo_w": e.geo_size(P)[0], "geo_h": e.geo_size(P)[1],
    }, rgba


def py_load_source(path):
    from PIL import Image
    from fastedit.api import _preview_rgba
    from fastedit.config import PREVIEW_MAX
    from fastedit.engine import ENGINE
    img = ENGINE.entry(path).base
    W, H = img.size
    pw, ph = W, H
    if max(W, H) > PREVIEW_MAX:
        s = PREVIEW_MAX / max(W, H)
        pw, ph = max(1, round(W * s)), max(1, round(H * s))
    return {"ok": True, "W": W, "H": H, "pw": pw, "ph": ph, "alpha": img.mode == "RGBA"}, \
        _preview_rgba(img, (pw, ph), Image.Resampling.BILINEAR)


def py_sample(nx, ny):
    import numpy as np
    from fastedit.engine import ENGINE
    a = ENGINE.current["rgb"]
    h, w = a.shape[:2]
    x = int(min(w - 1, max(0, nx * w)))
    y = int(min(h - 1, max(0, ny * h)))
    patch = a[max(0, y - 1):min(h, y + 2), max(0, x - 1):min(w, x + 2)]
    return [int(v) for v in np.median(patch.reshape(-1, 3), axis=0)]


INPUTS = os.path.join(ROOT, "tests", "stage", "inputs")
STAGES = [
    ("photo.jpg", {}, 0),
    ("photo.jpg", {"resize_on": True, "mode": "preset", "preset_res": 240}, 0),
    ("photo.jpg", {"angle": 3.5, "wp_on": True, "ge_on": True, "ge_sigma": 1.5}, 50000),
    ("photo.jpg", {"rot_q": 1, "crop_x": 10.0, "crop_y": -5.0, "crop_w": 200.0, "crop_h": 150.0, "resize_on": True,
                   "mode": "Mpix", "mpix": 0.05}, 0),
    ("orient_6.jpg", {"flip": True, "wp_on": True, "wp_pct": 97.5, "wp_strength": 0.6}, 0),
    ("rgba.png", {"ge_on": True, "ge_p": 2, "ge_sigma": 0.0}, 3000),
    ("rgba.png", {"crop_x": 0.0, "crop_y": 0.0, "crop_w": 300.0, "crop_h": 300.0, "crop_free": True}, 0),
    ("rgb.avif", {"resize_on": True, "mode": "preset", "preset_res": 120}, 0),
]


def check_stage(check, b, d):
    from PIL import Image
    import numpy as np
    big = os.path.join(d, "big.png")
    rng = np.random.default_rng(3)
    a = rng.integers(0, 256, (30, 40, 4), dtype=np.uint8)
    Image.fromarray(a).resize((3000, 2250), Image.Resampling.BICUBIC).save(big)
    for i, (name, params, full) in enumerate(STAGES + [(big, {"wp_on": True}, 0)]):
        path = name if os.path.isabs(name) else os.path.join(INPUTS, name)
        label = f"load_stage {os.path.basename(path)} {i}"
        want, want_px = py_load_stage(path, params, i, full)
        got = b.stage_call("load_stage", path, params, i, full)
        token = got.pop("token", None)
        check(f"{label}: same answer", got == want or print("   ", got, "\n   ", want))
        check(f"{label}: same preview pixels", token is not None and b.preview(token) == want_px)
        for nx, ny in ((0.5, 0.5), (0.0, 0.0), (0.999, 0.999), (0.31, 0.77), (1.5, -1.0)):
            check(f"{label}: sample({nx}, {ny})", b.call("sample", nx, ny) == py_sample(nx, ny))
    for name in ("photo.jpg", "orient_6.jpg", "rgba.png", "rgb.avif", big):
        path = name if os.path.isabs(name) else os.path.join(INPUTS, name)
        want, want_px = py_load_source(path)
        got = b.stage_call("load_source", path)
        token = got.pop("token", None)
        check(f"load_source {os.path.basename(path)}: same answer", got == want or print("   ", got, "\n   ", want))
        check(f"load_source {os.path.basename(path)}: same preview pixels", b.preview(token) == want_px)
    r = b.stage_call("load_stage", os.path.join(d, "missing.jpg"), {}, 99, 0)
    check(f"load_stage of a missing file: ok false ({r.get('error')})", r.get("ok") is False and r.get("req_id") == 99)
    check("an old preview token is gone", b.preview("0" * 32) is None)
    from fastedit.config import StageParams
    from fastedit.engine import Entry
    path = os.path.join(INPUTS, "icc.png")
    raw, icc = b.stage(path, StageParams())
    check("stage image (Process): the source's ICC profile", icc is not None and icc == Entry(path).icc)
    check("stage image (Process): the pixels as decoded", raw == Entry(path).raw)


SAMPLES = [
    {},
    {"fmt": "AVIF", "secs": ["adjust", "curves", "fx_test_pixel"], "jpeg_q": 90, "jpeg_d": 1.25, "split": True,
     "stage": {"resize_on": True, "mode": "Mpix", "mpix": 2.0, "prio": "Balanced"}, "collapsed": []},
    {"z_last": 1, "a_first": 2, "m": None, "nested": {"y": [1, 2.5, -3e-7, 1e21], "x": "Mirelle ✓ “quoted” \\ / \u0001"}},
]
PRESETS = [
    {"adjust": [{"name": "Starfall", "values": {"adj_sat": 62, "adj_contrast": 1.15}}],
     "_chain": [{"name": "Moonforge", "chain": [{"id": "adjust", "t": "adjust"}], "values": {}, "off": {}}]},
    {"f_film": [{"name": "Ä é 漢字", "values": {"flt_film_preset": "none"}}]},
]


def main() -> int:
    fails = []

    def check(label, cond):
        print(f"  {'ok  ' if cond else 'FAIL'} {label}")
        if not cond:
            fails.append(label)

    d = tempfile.mkdtemp()
    sp, pp = os.path.join(d, "fae_settings.json"), os.path.join(d, "fae_presets.json")
    b = backend.Backend(sp, pp)
    check("missing settings -> {}", b.call("get_settings") == py_get_settings(sp) == {})
    check("missing presets -> {}", b.call("get_presets") == py_get_presets(pp) == {})
    for i, s in enumerate(SAMPLES):
        check(f"settings {i}: save -> true", b.call("save_settings", s) is True)
        got = b.call("get_settings")
        check(f"settings {i}: read back equal, key order kept", got == s and list(got) == list(s))
        check(f"settings {i}: the old code reads the new file", py_get_settings(sp) == s)
        py_save(sp, s)
        check(f"settings {i}: the new code reads the old file", b.call("get_settings") == s)
    for i, p in enumerate(PRESETS):
        check(f"presets {i}: save -> true", b.call("save_presets", p) is True)
        check(f"presets {i}: read back equal", b.call("get_presets") == p == py_get_presets(pp))
        check(f"presets {i}: no .tmp left", not os.path.exists(pp + ".tmp"))
    for bad, label in ((b"{broken", "broken JSON"), (b"", "empty file"), (b"\xff\xfe\x00", "not UTF-8"), (b"[1, 2]", "a list")):
        for path, get, pyget, name in ((sp, "get_settings", py_get_settings, "settings"), (pp, "get_presets", py_get_presets, "presets")):
            with open(path, "wb") as f:
                f.write(bad)
            check(f"{name}, {label}: same as before ({pyget(path)!r})", b.call(get) == pyget(path))
    # an unwritable place: save_settings still says true (as before), save_presets says false
    b2 = backend.Backend(os.path.join(d, "no", "such", "dir", "s.json"), os.path.join(d, "no", "such", "dir", "p.json"))
    check("save_settings into a missing folder -> true", b2.call("save_settings", {"a": 1}) is True)
    check("save_presets into a missing folder -> false", b2.call("save_presets", {"a": 1}) is False)
    check("sample before any stage -> None", b.call("sample", 0.5, 0.5) is None)
    check_stage(check, b, d)
    try:
        b.call("encoder_info")
        check("a command not moved yet raises", False)
    except RuntimeError as ex:
        check(f"a command not moved yet raises ({ex})", str(ex).startswith("unknown:"))
    print("ALL OK" if not fails else f"{len(fails)} FAILED")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
