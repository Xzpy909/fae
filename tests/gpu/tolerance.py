"""Tolerance runner for the GPU core (claude/gpu-core-plan.md, rule from G1 on): the parity cases
(tests/parity/cases.json.gz, the UI's own edits) on the parity test images, rendered by the GPU core
(fae_gpu) and by the CPU core (fastedit_core, the reference).

    python tests/gpu/tolerance.py               # every case the GPU can run
    python tests/gpu/tolerance.py -k curves -v  # only cases whose id contains "curves", list each
    python tests/gpu/tolerance.py --backend vulkan   (default: dx12 on Windows)
    python tests/gpu/tolerance.py --trace       # for outlier pixels: the step where they first differ
    python tests/gpu/tolerance.py --steps       # how often each step kind on its own makes values differ

A case passes when, on every image, the mean difference is < 0.05 levels and at most 0.01 % of the
pixels (at least 2) are more than 2 levels off. Why outliers are allowed: the GPU computes in f32,
the CPU core in f64, and every step truncates to whole levels, so a value within ~1e-5 of a whole
level can land one level apart; a later steep table (Curves, Contrast, Film) can turn that single
level into several. That happens to a handful of pixels per million, invisibly; anything systematic
shows up in the mean or the outlier count. Cases using a tool the GPU doesn't run yet are skipped
(counted per tool). Failures get a diff image in tests/gpu/.work/<image>/<case>.png:
GPU | CPU | difference x16.

Software GPUs (lavapipe) round every float operation on its own; NVIDIA fuses multiply-adds and
divides with a reciprocal, so some problems only show there. To reproduce them elsewhere, build
fae_gpu with `maturin build --features emulate` and run with FAE_GPU_EMULATE=fma,div
(core/gpu/fae-gpu/src/emulate.rs rewrites the shaders that way).
"""
from __future__ import annotations

import argparse
import os
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
WORK = os.path.join(HERE, ".work")
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "tests", "parity"))

from fastedit import core  # noqa: E402
import parity  # noqa: E402  (cases and test images)

MAX_LEVELS = 2          # a pixel further off than this is an outlier
MAX_MEAN = 0.05
MAX_OUTLIERS = 1e-4     # share of the pixels, at least MIN_OUTLIERS
MIN_OUTLIERS = 2


def sharp_on(job: dict) -> bool:
    s = job.get("sharp") or {}
    return any(float(s.get(k) or 0) for k in ("as", "bs", "st", "lc", "fo"))


def unported(job: dict, ported) -> str | None:
    """The first tool in the job the GPU can't run, or None."""
    if sharp_on(job):
        return "sharp"
    for st in core._steps(job):
        if st[0] not in ported:
            return st[0]
    return None


def cpu_chain(rgb, steps):
    return core._need().apply_chain(np.ascontiguousarray(rgb), steps, False, adaptive=0.0, balance=0.0,
                                    structure=0.0, local_contrast=0.0, focus=0.0)


def trace(g, rgb, job, mask, out: dict, label=""):
    """For up to 5 pixels of `mask`: the first step where GPU and CPU differ. Each step prefix of the
    chain runs on the whole image on both cores (Super Contrast, Perceptual Saturation and Mid/Detail
    take statistics from the whole image, so a single pixel would not do). The first pixel is
    printed step by step."""
    steps = core._steps(job)
    pts = np.argwhere(mask)[:5]
    rows = [[] for _ in pts]
    first = [None] * len(pts)
    for k in range(1, len(steps) + 1):
        cpu = cpu_chain(rgb, steps[:k])
        gpu = g.render(rgb, steps[:k])
        for i, (y, x) in enumerate(pts):
            rows[i].append(f"{steps[k - 1][0]:7s} cpu {cpu[y, x].tolist()} gpu {gpu[y, x].tolist()}")
            if first[i] is None and not np.array_equal(cpu[y, x], gpu[y, x]):
                first[i] = steps[k - 1][0]
    for f in first:
        if f:
            out[f] = out.get(f, 0) + 1
    if len(pts) and first[0]:
        y, x = pts[0]
        print(f"      {label} pixel ({x}, {y}) in {rgb[y, x].tolist()}: " + " | ".join(rows[0]))


def isolated(g, rgb, steps, acc: dict):
    """Every step on its own: both cores get the CPU core's input for that step, so a difference is
    made by that step, not carried in or amplified. acc[kind] = [values, off by 1, off by more, max]."""
    x = rgb
    for st in steps:
        a = cpu_chain(x, [st])
        b = g.render(x, [st])
        d = np.abs(a.astype(np.int16) - b.astype(np.int16))
        t = acc.setdefault(st[0], [0, 0, 0, 0])
        t[0] += d.size
        t[1] += int((d == 1).sum())
        t[2] += int((d > 1).sum())
        t[3] = max(t[3], int(d.max()))
        x = a


def diff_image(path, gpu, cpu):
    from PIL import Image
    d = np.clip(np.abs(gpu.astype(np.int16) - cpu.astype(np.int16)) * 16, 0, 255).astype(np.uint8)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    Image.fromarray(np.concatenate([gpu, cpu, d], axis=1)).save(path)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-k", default="", help="only cases whose id contains this")
    ap.add_argument("-v", action="store_true", help="one line per case")
    ap.add_argument("--backend", default=None, help="dx12 | vulkan (default: the GPU core's default)")
    ap.add_argument("--fallback", action="store_true", help="software adapter (WARP)")
    ap.add_argument("--trace", action="store_true", help="for up to 5 outlier pixels per case and image: the "
                    "first step at which GPU and CPU differ (each step prefix run on that one pixel)")
    ap.add_argument("--steps", action="store_true", help="also run every step on its own (both cores get "
                    "the CPU core's input for it): how often each step kind itself makes a value differ")
    a = ap.parse_args()

    if not core.available():
        sys.exit(core.error())
    try:
        import fae_gpu
    except ImportError as ex:
        sys.exit(f"fae_gpu is not installed ({ex}): run core\\gpu\\build_gpu.bat")
    g = fae_gpu.Gpu(fallback=a.fallback, backends=a.backend)
    i = g.info()
    print(f"GPU core {fae_gpu.version()} on {i['name']} via {i['backend']}; CPU core {core.info().get('version')}")
    ported = set(fae_gpu.PORTED)

    cases, _ = parity.load_cases()
    cases = [c for c in cases if a.k in c["id"]]
    images = [(n, rgb) for n, rgb, _ in parity.all_images()]
    skipped: dict[str, int] = {}
    run, failed, with_outliers = [], [], []
    outliers = 0
    tally = [0, 0, 0]          # values identical, 1 level off, all
    traced: dict[str, int] = {}
    iso: dict[str, list] = {}
    t0 = time.perf_counter()
    for c in cases:
        why = unported(c["job"], ported)
        if why:
            skipped[why] = skipped.get(why, 0) + 1
            continue
        worst = (0, 0.0, 0)            # max, mean, pixels > 1 level (over all images)
        bad_imgs = []
        for name, rgb in images:
            ref = core.render(rgb, c["job"])
            try:
                out = g.render(rgb, core._steps(c["job"]))
            except ValueError as ex:          # a step kind the GPU refuses: treat as unported
                skipped[str(ex)] = skipped.get(str(ex), 0) + 1
                break
            d = np.abs(out.astype(np.int16) - ref.astype(np.int16))
            mx, mean = int(d.max()), float(d.mean())
            tally[0] += int((d == 0).sum()); tally[1] += int((d == 1).sum()); tally[2] += int(d.size)
            over = int((d.max(axis=2) > MAX_LEVELS).sum())
            worst = (max(worst[0], mx), max(worst[1], mean), worst[2] + over)
            n_px = rgb.shape[0] * rgb.shape[1]
            if a.steps:
                isolated(g, rgb, core._steps(c["job"]), iso)
            if a.trace and over:
                trace(g, rgb, c["job"], d.max(axis=2) > MAX_LEVELS, traced, f"{c['id']}/{name}")
            if mean >= MAX_MEAN or over > max(MIN_OUTLIERS, n_px * MAX_OUTLIERS):
                bad_imgs.append(name)
                diff_image(os.path.join(WORK, name, c["id"] + ".png"), out, ref)
        else:
            run.append(c["id"])
            line = f"max {worst[0]:3d}  mean {worst[1]:.4f}  px>{MAX_LEVELS}: {worst[2]}"
            outliers += worst[2]
            if worst[0] > MAX_LEVELS:
                with_outliers.append(c["id"])
            if bad_imgs:
                failed.append(c["id"])
                print(f"FAIL  {c['id']:28s} {line}  ({', '.join(bad_imgs)})")
            elif a.v:
                print(f"ok    {c['id']:28s} {line}")
    dt = time.perf_counter() - t0
    sk = ", ".join(f"{k} {v}" for k, v in sorted(skipped.items(), key=lambda kv: -kv[1]))
    print(f"\n{len(run)} cases x {len(images)} images on the GPU in {dt:.1f} s: "
          f"{len(run) - len(failed)} ok, {len(failed)} failed (mean < {MAX_MEAN}, "
          f"pixels > {MAX_LEVELS} levels off <= {MAX_OUTLIERS:.2%})")
    if tally[2]:
        print(f"values: {tally[0] / tally[2]:.4%} identical, {tally[1] / tally[2]:.4%} 1 level off, "
              f"{(tally[2] - tally[0] - tally[1]) / tally[2]:.5%} more")
    print(f"outliers (> {MAX_LEVELS} levels): {outliers} pixels in {len(with_outliers)} cases"
          + (f"  [{', '.join(with_outliers)}]" if with_outliers else ""))
    if a.trace:
        print("outliers first differ at: " + (", ".join(f"{k} {v}" for k, v in sorted(traced.items(), key=lambda kv: -kv[1]))
                                              or "(none traced)"))
    if a.steps:
        print("each step on its own (values made different by the step itself):")
        for k, (n, one, more, mx) in sorted(iso.items(), key=lambda kv: -(kv[1][1] + kv[1][2]) / kv[1][0]):
            print(f"  {k:7s} {n:11d} values: {one / n:.5%} 1 level off, {more / n:.5%} more (max {mx})")
    print(f"skipped (tool not on the GPU yet): {sum(skipped.values())}" + (f"  [{sk}]" if sk else ""))
    if failed:
        print(f"diff images: {WORK}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
