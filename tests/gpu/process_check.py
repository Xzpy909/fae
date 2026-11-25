"""Process on the GPU core (gpu-core plan, G5): what Process writes, rendered by the GPU core and by
the CPU core (pyref.core.render with engine="gpu" / "cpu"), on photos at their full size.

    python tests/gpu/process_check.py              # own photos in tests/parity/images + 2 / 24 MP synthetic ones
    python tests/gpu/process_check.py --max-mp 12  # skip photos above 12 MP
    python tests/gpu/process_check.py -k sharp     # only edits whose name contains "sharp"

Edits: parity cases that use every tool (whole-image ones, a Film preset, a long chain) and the same
with Sharpen on top. Each image must pass the chain part of tests/gpu/tolerance.py's rule (mean <
0.05 levels, at most 0.1 % of the pixels more than 2 levels off; the per-step part is
tolerance.py's job). Times are what Process sees for the edit: the GPU core with upload and read
back, the CPU core as before. Also checks that render() falls back to the CPU core when the GPU
can't take an image (too big) or is turned off. Failures get a diff image in tests/gpu/.work/process/
(GPU | CPU | difference x16).
"""
from __future__ import annotations

import argparse
import os
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "tests"))           # pyref: the Python versions the back end replaced
sys.path.insert(0, os.path.join(ROOT, "tests", "parity"))
sys.path.insert(0, HERE)

from PIL import Image  # noqa: E402

from pyref import core  # noqa: E402
import parity  # noqa: E402
import tolerance  # noqa: E402

WORK = os.path.join(HERE, ".work", "process")
BASES = ("combo_19", "chain_2", "sc_all", "film_k_portra_160vc", "adj_middetail_max", "combo_26")
SHARP = {"as": 0.6, "bs": 0.4, "st": 0.5, "lc": 0.3, "fo": 0.2}
MAX_MEAN = tolerance.MAX_MEAN
MAX_OUTLIERS = tolerance.MAX_OUTLIERS


def photos(max_mp: float):
    """Own photos at full size (tests/parity/images, not committed), then synthetic 2 and 24 MP ones."""
    d = parity.LOCAL_DIR
    if os.path.isdir(d):
        for name in sorted(os.listdir(d)):
            if os.path.splitext(name)[1].lower() not in (".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff"):
                continue
            with Image.open(os.path.join(d, name)) as im:
                if im.width * im.height > max_mp * 1e6:
                    print(f"skip  {name}: {im.width}x{im.height} is above {max_mp} MP")
                    continue
                yield os.path.splitext(name)[0], np.ascontiguousarray(np.asarray(im.convert("RGB")))
    yield "synthetic_2mp", tolerance.tall(1800, 1200, 10)
    if max_mp >= 24:
        yield "synthetic_24mp", tolerance.tall(6000, 4000, 11)


def edits(k: str):
    by_id = {c["id"]: c["job"] for c in parity.load_cases()[0]}
    out = []
    for b in BASES:
        if b in by_id:
            out.append((b, by_id[b]))
            out.append((b + "+sharp", dict(by_id[b], sharp=SHARP)))
    out.append(("sharp_only", {"sharp": SHARP}))
    return [(n, e) for n, e in out if k in n]


def timed(f):
    t0 = time.perf_counter()
    r = f()
    return r, (time.perf_counter() - t0) * 1000


def fallbacks(rgb) -> list[str]:
    """render() with engine="auto" gives the CPU core's result when the GPU can't take the image."""
    bad = []
    job = {"sharp": SHARP}
    cpu = core.render(rgb, job, engine="cpu")
    g = core._gpu
    keep = dict(g["info"])
    try:
        g["info"]["max_pixels"] = rgb.shape[0] * rgb.shape[1] - 1          # "too big for this GPU"
        if not np.array_equal(core.render(rgb, job), cpu):
            bad.append("too big: auto didn't give the CPU core's result")
        try:
            core.render(rgb, job, engine="gpu")
            bad.append("too big: engine='gpu' didn't refuse")
        except RuntimeError:
            pass
    finally:
        g["info"].clear()
        g["info"].update(keep)
    why = g["why"]
    try:
        g["why"] = "turned off (--no-gpu)"
        if not np.array_equal(core.render(rgb, job), cpu):
            bad.append("--no-gpu: auto didn't give the CPU core's result")
        if core.gpu_info()["ok"]:
            bad.append("--no-gpu: gpu_info still says ok")
    finally:
        g["why"] = why
    return bad


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-k", default="", help="only edits whose name contains this")
    ap.add_argument("--max-mp", type=float, default=60.0, help="skip photos above this many megapixels")
    a = ap.parse_args()
    if not core.available():
        sys.exit(core.error())
    gi = core.gpu_info()
    if not gi["ok"]:
        sys.exit(f"the GPU core can't run: {gi['error']}")
    print(f"Process engine: GPU core {gi['version']} on {gi['name']} via {gi['backend']}; "
          f"CPU core {core.info().get('version')}")
    E = edits(a.k)
    failed, n, t_gpu, t_cpu = [], 0, 0.0, 0.0
    first = None
    for name, rgb in photos(a.max_mp):
        first = rgb if first is None else first
        h, w = rgb.shape[:2]
        print(f"{name}: {w}x{h} ({w * h / 1e6:.1f} MP)")
        core.render(rgb, E[0][1] if E else {}, engine="gpu")          # warm up this size
        for en, job in E:
            gpu, tg = timed(lambda: core.render(rgb, job, engine="gpu"))
            cpu, tc = timed(lambda: core.render(rgb, job, engine="cpu"))
            d = np.abs(gpu.astype(np.int16) - cpu.astype(np.int16))
            over = int((d.max(axis=2) > tolerance.MAX_LEVELS).sum())
            ok = d.mean() < MAX_MEAN and over <= max(tolerance.MIN_OUTLIERS, w * h * MAX_OUTLIERS)
            n += 1
            t_gpu += tg
            t_cpu += tc
            print(f"  {'ok  ' if ok else 'FAIL'}  {en:28s} max {int(d.max()):3d}  mean {d.mean():.4f}  px>2: {over:5d}  "
                  f"GPU {tg:7.1f} ms  CPU {tc:7.1f} ms  ({tc / max(tg, 1e-6):.1f}x)")
            if not ok:
                failed.append(f"{name}/{en}")
                tolerance.diff_image(os.path.join(WORK, name, en + ".png"), gpu, cpu)
        core.gpu_release()
    fb = fallbacks(first[:512, :512].copy()) if first is not None else []
    for f in fb:
        print(f"FAIL  fallback: {f}")
    print(f"\n{n} renders: {n - len(failed)} ok, {len(failed)} failed; fallbacks {'ok' if not fb else 'FAILED'}; "
          f"GPU {t_gpu / 1000:.1f} s vs CPU {t_cpu / 1000:.1f} s in total")
    if failed:
        print(f"diff images: {WORK}")
    sys.exit(1 if failed or fb else 0)


if __name__ == "__main__":
    main()
