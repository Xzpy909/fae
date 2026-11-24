"""Timing of the stage steps moved to Rust (docs/rust-backend-plan.md): the Python version (Pillow /
OpenCV) against the Rust back end on one big synthetic photo-sized image (pixels: random, so nothing
compresses or caches away). Rust times of the single steps include handing the pixels to Python and
back; the whole stage (B3) keeps them in Rust: "load_stage" is the command the UI calls (stage +
preview pixels), from a cached decode.

    python tests/stage/stage_bench.py            # 42 MP (7952 x 5304)
    python tests/stage/stage_bench.py --mp 24    # another size
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
import time

import numpy as np
from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, ROOT)


def best(f, n=3):
    ts = []
    for _ in range(n):
        t = time.perf_counter()
        f()
        ts.append(time.perf_counter() - t)
    return min(ts) * 1000


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--mp", type=float, default=42.2)
    args = ap.parse_args()
    import fae_backend as fb
    from fastedit import colour, config, engine, geometry
    from fastedit.api import Api, _preview_rgba, _pv_size
    w = int(round((args.mp * 1e6 * 3 / 2) ** 0.5))
    h = int(round(w * 2 / 3))
    a = np.random.default_rng(1).integers(0, 256, (h, w, 3), dtype=np.uint8)
    im, raw = Image.fromarray(a), a.tobytes()
    print(f"{w} x {h} RGB ({w * h / 1e6:.1f} MP), fae_backend {fb.version()}, {os.cpu_count()} CPU threads")
    cases = [
        ("quarter turn", dict(rot_q=1)),
        ("flip + crop", dict(flip=True, crop_w=w * 0.8, crop_h=h * 0.8)),
        ("angle 2.5 + crop", dict(angle=2.5, crop_w=w * 0.88, crop_h=h * 0.88)),
        ("resize 1080p", dict(resize_on=True, mode="preset", preset_res=1080)),
        ("resize 12 MP", dict(resize_on=True, mode="Mpix", mpix=12.0)),
    ]
    for name, kw in cases:
        P = config.StageParams(**kw)
        if P.resize_on:
            tp = best(lambda: geometry.fit_image(im, P))
            tr = best(lambda: fb.fit(raw, w, h, 3, json.dumps(kw)))
        else:
            tp = best(lambda: geometry.apply_geometry(im, P))
            tr = best(lambda: fb.geometry(raw, w, h, 3, json.dumps(kw)))
        print(f"  {name:18s} python {tp:7.0f} ms   rust {tr:7.0f} ms")
    for name, py, rs in (
            ("white-patch", lambda: colour.white_patch_enhance(im, 99.0, 1.0), lambda: fb.white_patch(raw, w, h, 3, 99.0, 1.0)),
            ("gray-edge", lambda: colour.gray_edge_enhance(im, 1, 2.0, 1.0), lambda: fb.gray_edge(raw, w, h, 3, 1, 2.0, 1.0))):
        print(f"  {name:18s} python {best(py):7.0f} ms   rust {best(rs):7.0f} ms")
    with tempfile.TemporaryDirectory() as d:
        for fmt in ("jpg", "png"):
            p = os.path.join(d, "big." + fmt)
            im.save(p, quality=90) if fmt == "jpg" else im.save(p, compress_level=1)
            tp = best(lambda: engine.make_thumb_pillow(p))
            tr = best(lambda: engine._make_thumb(p))
            print(f"  {'thumbnail ' + fmt:18s} python {tp:7.0f} ms   rust {tr:7.0f} ms")
        p = os.path.join(d, "big.png")
        api = Api()
        for name, kw in (("load_stage", {}), ("load_stage 1080p+wp", dict(resize_on=True, preset_res=1080, wp_on=True))):
            P = config.StageParams(**kw)

            def py_load():
                _, img = engine.ENGINE.stage(p, P)
                pw, ph = _pv_size(*img.size)
                _preview_rgba(img, (pw, ph), Image.Resampling.LANCZOS)
            py_load()
            api.load_stage(p, kw, 1, 0)
            tp = best(py_load)
            tr = best(lambda: api.load_stage(p, kw, 1, 0))
            print(f"  {name:18s} python {tp:7.0f} ms   rust {tr:7.0f} ms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
