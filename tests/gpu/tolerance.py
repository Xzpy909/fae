"""Tolerance runner for the GPU core (claude/gpu-core-plan.md, rule from G1 on): the parity cases
(tests/parity/cases.json.gz, the UI's own edits) on the parity test images, rendered by the GPU core
(fae_gpu) and by the CPU core (fastedit_core, the reference).

    python tests/gpu/tolerance.py               # every case the GPU can run
    python tests/gpu/tolerance.py -k curves -v  # only cases whose id contains "curves", list each
    python tests/gpu/tolerance.py --backend vulkan   (default: dx12 on Windows)
    python tests/gpu/tolerance.py --trace       # for outlier pixels: the step where they first differ
    python tests/gpu/tolerance.py --steps       # also print how often each step kind on its own differs

A case passes when, on every image, both parts of the rule hold (since G4):
- every step on its own (both cores get the CPU core's input for that step, so the difference is made
  by that step, not carried in from earlier steps or amplified by later ones): at most 0.01 % of the
  values (at least 2) more than 2 levels off, at most 1 % of the values different, mean < 0.05. This
  is the strict part: a mistake in a shader makes many values differ and shows up here. (Single values
  further off are allowed: Adjustments runs Contrast, Boost and Saturation inside one step, so one f32
  flip can grow there too; one of the chart's patch colours lands on .5 in Super Contrast: 0.43 %.)
- the whole chain: mean < 0.05 levels, at most 0.1 % of the pixels (at least 2) more than 2 levels off.
Why chain outliers are allowed: the GPU computes in f32, the CPU core in f64, and every step truncates
to whole levels, so a value within ~1e-5 of a whole level can land one level apart; a later steep
table (Curves, Contrast, Film) or Sharpen (each part sharpens a 1-level difference like any other
detail: Adaptive -> Structure -> Local Contrast -> Focus turn 1 level into 3-4) makes that single
level several. On contrast-stretched noise that reaches a few hundred pixels per million, invisibly.
Sharpen's parts count as steps of their own here (sharp.as, sharp.st, sharp.lc, sharp.fo). Cases using a tool the GPU doesn't run yet are skipped
(counted per tool). Sharpen isn't in the parity cases (the UI's generator leaves it off), so this
runner adds its own Sharpen cases (sharp_*): each slider alone, together, on top of chain cases,
at Process scale and at preview scales, also on two taller synthetic images (Structure's filters
on a shrunk copy). Failures get a diff image in tests/gpu/.work/<image>/<case>.png:
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
sys.path.insert(0, os.path.join(ROOT, "tests"))           # fastedit (the back end from Python), pyref (the Python versions it replaced)
sys.path.insert(0, os.path.join(ROOT, "tests", "parity"))

from pyref import core  # noqa: E402
import parity  # noqa: E402  (cases and test images)

MAX_LEVELS = 2          # a pixel further off than this is an outlier
MAX_MEAN = 0.05
MAX_OUTLIERS = 1e-3     # whole chain: share of the pixels, at least MIN_OUTLIERS
MIN_OUTLIERS = 2
STEP_MAX_OUTLIERS = 1e-4   # every step on its own: share of the values more than MAX_LEVELS off,
STEP_MAX_OFF = 1e-2        # share of the values that may differ at all


SHARP_KEYS = ("as", "bs", "st", "lc", "fo")


def sharp_on(job: dict) -> bool:
    s = job.get("sharp") or {}
    return any(float(s.get(k) or 0) for k in SHARP_KEYS if k != "bs")


def unported(job: dict, ported) -> str | None:
    """The first tool in the job the GPU can't run, or None."""
    if sharp_on(job) and "sharp" not in ported:
        return "sharp"
    for st in core._steps(job):
        if st[0] not in ported:
            return st[0]
    return None


def steps_of(case: dict) -> list:
    """The case as GPU steps: core._steps, then ("sharp", as, bs, st, lc, fo, scale) when on."""
    st = core._steps(case["job"])
    if sharp_on(case["job"]):
        s = case["job"]["sharp"]
        st.append(("sharp",) + tuple(float(s.get(k) or 0) for k in SHARP_KEYS) + (float(case.get("scale", 1.0)),))
    return st


def cpu_chain(rgb, steps):
    """The CPU core on GPU-style steps; trailing "sharp" steps (one, or one per part, see parts())
    run one after the other, as the CPU core's Sharpen runs its parts."""
    n = len(steps)
    while n and steps[n - 1][0] == "sharp":
        n -= 1
    sharps = [tuple(s[1:]) for s in steps[n:]] or [(0.0,) * 5 + (1.0,)]
    x = np.ascontiguousarray(rgb)
    for i, sh in enumerate(sharps):
        x = core._need().apply_chain(x, steps[:n] if i == 0 else [], False, adaptive=sh[0], balance=sh[1],
                                     structure=sh[2], local_contrast=sh[3], focus=sh[4], sharp_scale=sh[5])
    return x


def parts(steps):
    """Sharpen split into its parts (Adaptive with Balance, Structure, Local Contrast, Focus), each a
    "sharp" step of its own (same result: the parts run one after the other on 8-bit pixels), so
    --steps and --trace can tell them apart. Names: steps[i][0] stays "sharp"; part_name() labels."""
    out = []
    for st in steps:
        if st[0] != "sharp":
            out.append(st)
            continue
        a, b, s, l, f, sc = st[1:]
        for one in ((a, b, 0, 0, 0), (0, 0, s, 0, 0), (0, 0, 0, l, 0), (0, 0, 0, 0, f)):
            if one[0] > 0 or any(one[2:]):
                out.append(("sharp",) + tuple(float(v) for v in one) + (sc,))
    return out


def part_name(st) -> str:
    if st[0] != "sharp":
        return st[0]
    return "sharp." + ("as" if st[1] > 0 else "st" if st[3] else "lc" if st[4] else "fo")


def sharp_cases(cases: list) -> list:
    """Sharpen cases: each slider alone (both signs, Balance both ways), all together, at preview
    scales (the blurs are a fixed size in output pixels), and on top of a few chain cases."""
    S = lambda **k: {"as": 0, "bs": 0, "st": 0, "lc": 0, "fo": 0, **k}
    alone = [("as25", S(**{"as": 0.25})), ("as100", S(**{"as": 1})), ("as60_bal_p", S(**{"as": 0.6, "bs": 0.8})),
             ("as90_bal_m", S(**{"as": 0.9, "bs": -1})), ("st_p", S(st=1)), ("st_m", S(st=-0.6)), ("st_p35", S(st=0.35)),
             ("lc_p", S(lc=1)), ("lc_m", S(lc=-0.5)), ("fo_p", S(fo=1)), ("fo_m", S(fo=-1)), ("fo_m30", S(fo=-0.3)),
             ("all", S(**{"as": 0.6, "bs": 0.4, "st": 0.5, "lc": 0.3, "fo": 0.2})),
             ("all_m", S(**{"as": 0.4, "bs": -0.5, "st": -0.5, "lc": -0.3, "fo": -0.6}))]
    out = []
    for n, sp in alone:
        out.append({"id": f"sharp_{n}", "job": {"sharp": sp}, "scale": 1.0})
    for n, sp in alone[-2:] + [alone[7], alone[10]]:
        for sc in (0.5, 0.2, 2.0):
            out.append({"id": f"sharp_{n}_x{sc:g}", "job": {"sharp": sp}, "scale": sc})
    by_id = {c["id"]: c for c in cases}
    for base in ("combo_3", "chain_2", "sc_all", "adj_middetail_max"):
        if base in by_id:
            out.append({"id": f"sharp_on_{base}", "job": dict(by_id[base]["job"], sharp=alone[12][1]), "scale": 1.0})
    return out


def tall(w, h, seed):
    """A synthetic photo-like image (soft waves, checks, noise) for Structure's shrunk filters."""
    r = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:h, 0:w]
    v = (128 + 90 * np.sin(xx / 37.0) * np.cos(yy / 53.0))[..., None] + r.normal(0, 12, (h, w, 3))
    v[(xx // 64 + yy // 64) % 2 == 0] += 25
    return np.clip(v, 0, 255).astype(np.uint8)


def trace(g, rgb, case, mask, out: dict, label=""):
    """For up to 5 pixels of `mask`: the first step where GPU and CPU differ. Each step prefix of the
    chain runs on the whole image on both cores (Super Contrast, Perceptual Saturation and Mid/Detail
    take statistics from the whole image, so a single pixel would not do). The first pixel is
    printed step by step."""
    steps = parts(steps_of(case))
    pts = np.argwhere(mask)[:5]
    rows = [[] for _ in pts]
    first = [None] * len(pts)
    for k in range(1, len(steps) + 1):
        cpu = cpu_chain(rgb, steps[:k])
        gpu = g.render(rgb, steps[:k])
        for i, (y, x) in enumerate(pts):
            rows[i].append(f"{part_name(steps[k - 1]):8s} cpu {cpu[y, x].tolist()} gpu {gpu[y, x].tolist()}")
            if first[i] is None and not np.array_equal(cpu[y, x], gpu[y, x]):
                first[i] = part_name(steps[k - 1])
    for f in first:
        if f:
            out[f] = out.get(f, 0) + 1
    if len(pts) and first[0]:
        y, x = pts[0]
        print(f"      {label} pixel ({x}, {y}) in {rgb[y, x].tolist()}: " + " | ".join(rows[0]))


def isolated(g, rgb, steps, acc: dict) -> list:
    """Every step on its own: both cores get the CPU core's input for that step, so a difference is
    made by that step, not carried in or amplified. acc[kind] = [values, off by 1, off by more, max].
    Returns [(kind, share of values off, values > MAX_LEVELS off, max, mean, values)] for this image."""
    x = rgb
    out = []
    for st in steps:
        a = cpu_chain(x, [st])
        b = g.render(x, [st])
        d = np.abs(a.astype(np.int16) - b.astype(np.int16))
        t = acc.setdefault(part_name(st), [0, 0, 0, 0])
        t[0] += d.size
        t[1] += int((d == 1).sum())
        t[2] += int((d > 1).sum())
        t[3] = max(t[3], int(d.max()))
        out.append((part_name(st), float((d > 0).mean()), int((d > MAX_LEVELS).sum()), int(d.max()), float(d.mean()), d.size))
        x = a
    return out


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
    ap.add_argument("--steps", action="store_true", help="print the totals of the per-step check (every step "
                    "on its own, both cores get the CPU core's input for it): how often each step kind differs")
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

    every, _ = parity.load_cases()
    cases = [c for c in every + sharp_cases(every) if a.k in c["id"]]
    images = [(n, rgb) for n, rgb, _ in parity.all_images()]
    big = [("tall_1500", tall(900, 1500, 3)), ("tall_2600", tall(500, 2600, 4))]
    skipped: dict[str, int] = {}
    run, failed, with_outliers = [], [], []
    outliers = 0
    tally = [0, 0, 0]          # values identical, 1 level off, all
    traced: dict[str, int] = {}
    iso: dict[str, list] = {}
    worst_step = [0.0, ""]
    t0 = time.perf_counter()
    for c in cases:
        why = unported(c["job"], ported)
        if why:
            skipped[why] = skipped.get(why, 0) + 1
            continue
        worst = (0, 0.0, 0)            # max, mean, pixels > 1 level (over all images)
        bad_imgs = []
        for name, rgb in images + (big if c["id"].startswith("sharp_") else []):
            ref = cpu_chain(rgb, steps_of(c))
            try:
                out = g.render(rgb, steps_of(c))
            except ValueError as ex:          # a step kind the GPU refuses: treat as unported
                skipped[str(ex)] = skipped.get(str(ex), 0) + 1
                break
            d = np.abs(out.astype(np.int16) - ref.astype(np.int16))
            mx, mean = int(d.max()), float(d.mean())
            tally[0] += int((d == 0).sum()); tally[1] += int((d == 1).sum()); tally[2] += int(d.size)
            over = int((d.max(axis=2) > MAX_LEVELS).sum())
            worst = (max(worst[0], mx), max(worst[1], mean), worst[2] + over)
            n_px = rgb.shape[0] * rgb.shape[1]
            step_bad = []
            for kind, off, far, smx, smean, nv in isolated(g, rgb, parts(steps_of(c)), iso):
                if off > worst_step[0]:
                    worst_step[:] = [off, f"{kind} in {c['id']}/{name}"]
                if far > max(MIN_OUTLIERS, nv * STEP_MAX_OUTLIERS) or off > STEP_MAX_OFF or smean >= MAX_MEAN:
                    step_bad.append(f"{kind}: {far} values > {MAX_LEVELS} off (max {smx}), {off:.4%} off")
            if a.trace and over:
                trace(g, rgb, c, d.max(axis=2) > MAX_LEVELS, traced, f"{c['id']}/{name}")
            if step_bad or mean >= MAX_MEAN or over > max(MIN_OUTLIERS, n_px * MAX_OUTLIERS):
                bad_imgs.append(name + (" - step " + "; ".join(step_bad) if step_bad else ""))
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
          f"{len(run) - len(failed)} ok, {len(failed)} failed (each step: values > {MAX_LEVELS} levels off "
          f"<= {STEP_MAX_OUTLIERS:.2%}, values off <= {STEP_MAX_OFF:.0%}; chain: mean < {MAX_MEAN}, pixels > {MAX_LEVELS} levels off "
          f"<= {MAX_OUTLIERS:.1%})")
    if worst_step[1]:
        print(f"most values off by one step on its own: {worst_step[0]:.4%} ({worst_step[1]})")
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
            print(f"  {k:8s} {n:11d} values: {one / n:.5%} 1 level off, {more / n:.5%} more (max {mx})")
    print(f"skipped (tool not on the GPU yet): {sum(skipped.values())}" + (f"  [{sk}]" if sk else ""))
    if failed:
        print(f"diff images: {WORK}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
