"""Timing of the stage in the Rust back end (docs/rust-backend-plan.md) on one big synthetic photo-sized
image (pixels: random, so nothing compresses or caches away), next to Pillow where Pillow still does the
same job (resize, thumbnails). The Python versions of the other steps (OpenCV, NumPy) were removed in B3;
their times then: docs/rust-backend-plan.md. Single steps include handing the pixels to Python and back;
"load_stage" is the command the UI calls (stage + preview pixels), from a cached decode.

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
sys.path.insert(0, os.path.join(ROOT, "tests"))           # pyref: the Python versions the back end replaced


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
    from fastedit import backend
    from fastedit.api import Api
    from pyref import app as pyapp
    b = backend.shared()
    w = int(round((args.mp * 1e6 * 3 / 2) ** 0.5))
    h = int(round(w * 2 / 3))
    a = np.random.default_rng(1).integers(0, 256, (h, w, 3), dtype=np.uint8)
    im, raw = Image.fromarray(a), a.tobytes()
    print(f"{w} x {h} RGB ({w * h / 1e6:.1f} MP), fae_backend {fb.version()}, {os.cpu_count()} CPU threads")
    cases = [
        ("quarter turn", dict(rot_q=1), None),
        ("flip + crop", dict(flip=True, crop_w=w * 0.8, crop_h=h * 0.8), None),
        ("angle 2.5 + crop", dict(angle=2.5, crop_w=w * 0.88, crop_h=h * 0.88), None),
        ("resize 1080p", dict(resize_on=True, mode="preset", preset_res=1080), (1920, 1080)),
    ]
    for name, kw, pil in cases:
        f = fb.fit if kw.get("resize_on") else fb.geometry
        tr = best(lambda: f(raw, w, h, 3, json.dumps(kw)))
        line = f"  {name:18s} rust {tr:7.0f} ms"
        if pil:     # Pillow's Lanczos straight to the size (the cover crop's resize step)
            sw = round(w * max(pil[0] / w, pil[1] / h))
            line += f"   pillow {best(lambda: im.resize((sw, round(h * sw / w)), Image.Resampling.LANCZOS)):7.0f} ms"
        print(line)
    for name, run in (("white-patch", lambda: fb.white_patch(raw, w, h, 3, 99.0, 1.0)),
                      ("gray-edge", lambda: fb.gray_edge(raw, w, h, 3, 1, 2.0, 1.0))):
        print(f"  {name:18s} rust {best(run):7.0f} ms")
    with tempfile.TemporaryDirectory() as d:
        for fmt in ("jpg", "png"):
            p = os.path.join(d, "big." + fmt)
            im.save(p, quality=90) if fmt == "jpg" else im.save(p, compress_level=1)
            tp = best(lambda: pyapp.make_thumb_pillow(p))
            tr = best(lambda: b.thumb(p, 224))
            print(f"  {'thumbnail ' + fmt:18s} rust {tr:7.0f} ms   pillow {tp:7.0f} ms")
        p = os.path.join(d, "big.png")
        api = Api()
        for name, kw in (("load_stage", {}), ("load_stage 1080p+wp", dict(resize_on=True, preset_res=1080, wp_on=True))):
            api.load_stage(p, kw, 1, 0)
            print(f"  {name:18s} rust {best(lambda: api.load_stage(p, kw, 1, 0)):7.0f} ms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
