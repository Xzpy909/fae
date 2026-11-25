"""Full-resolution timings of the edit math (what Process and the lossy compare preview run).

  python tests/bench/bench_python.py                  # Rust core, 24 MP synthetic image
  python tests/bench/bench_python.py --mp 12 --image some.jpg
(live-preview timings: the self-test page, fae.py --selftest)

Cases: tests/bench/cases.json (ids from the parity cases, so every filter is covered).
Results also go to tests/parity/.work/bench_python.json (not committed).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "tests"))           # pyref: the Python versions the back end replaced
sys.path.insert(0, os.path.join(ROOT, "tests", "parity"))

import parity  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mp", type=float, default=24.0, help="megapixels of the synthetic image")
    ap.add_argument("--image", help="use this image instead (full size)")
    ap.add_argument("--runs", type=int, default=2, help="best of N")
    a = ap.parse_args()

    if a.image:
        rgb = np.asarray(Image.open(a.image).convert("RGB")).copy()
    else:
        h = int(round((a.mp * 1e6 / 1.5) ** 0.5))
        rgb = parity._grad(int(round(h * 1.5)), h, 1)
    h, w = rgb.shape[:2]
    cases, _ = parity.load_cases()
    by_id = {c["id"]: c for c in cases}
    with open(os.path.join(HERE, "cases.json"), encoding="utf-8") as f:
        ids = json.load(f)

    from pyref import core
    if not core.available():
        sys.exit(core.error())
    print(f"Rust core {core.info()}")
    run = lambda e: core.render(rgb, e, engine="cpu")
    print(f"core: {w}x{h} ({w * h / 1e6:.1f} MP), best of {a.runs}")
    res = {}
    for cid in ids:
        e = by_id[cid]["job"]
        best, skipped = float("inf"), False
        for _ in range(a.runs):
            t = time.perf_counter()
            out = run(e)
            dt = time.perf_counter() - t
            if out is None:
                skipped = True
                break
            best = min(best, dt)
        res[cid] = None if skipped else round(best * 1000, 1)
        print(f"  {cid:22s} {'-' if skipped else f'{best * 1000:9.0f} ms'}", flush=True)
    os.makedirs(parity.WORK, exist_ok=True)
    with open(os.path.join(parity.WORK, "bench_python.json"), "w") as f:
        json.dump({"w": w, "h": h, "ms": res}, f, indent=1)


if __name__ == "__main__":
    main()
