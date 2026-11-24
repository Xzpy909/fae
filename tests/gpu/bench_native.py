"""GPU core speed, native (Python extension fae_gpu), against the CPU core (fastedit_core).

    python tests/gpu/bench_native.py               # DX12 and Vulkan, 1 / 24 / 50 MP
    python tests/gpu/bench_native.py --warp        # also Windows' software renderer (slow)
    python tests/gpu/bench_native.py --small       # 1 MP only

Three edits: Curves alone; a per-pixel chain from the parity cases (combo_26: Point, Adjustments,
Curves, the Perceptual Saturation filter, Film); a whole-image chain (combo_19: Adjustments with
Mid/Detail, Perceptual Saturation with its perceptual mode, Film, Super Contrast with Dynamic Contrast);
Sharpen alone with every slider on ("sharp": Adaptive + Balance, Structure, Local Contrast, Focus).
Times: upload of the stage, one run of the chain (from 20 in one submit), read back; CPU core = the
same edit through fastedit_core.
"""
from __future__ import annotations

import os
import sys
import time

import numpy as np

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, ROOT)

try:
    import fae_gpu
except ImportError as ex:
    sys.exit(f"fae_gpu is not installed ({ex}): run core\\gpu\\build_gpu.bat")
from fastedit import core  # noqa: E402
sys.path.insert(0, os.path.join(ROOT, "tests", "parity"))
import parity  # noqa: E402

SIZES = [(1001, 999), (6000, 4000), (8660, 5773)]
SHARP = {"sharp": {"as": 0.6, "bs": 0.4, "st": 0.5, "lc": 0.3, "fo": 0.2}}


def test_image(w, h, seed):
    rng = np.random.default_rng(seed)
    fx = np.linspace(0, 1, w, dtype=np.float32)[None, :]
    fy = np.linspace(0, 1, h, dtype=np.float32)[:, None]
    a = np.empty((h, w, 3), np.uint8)
    noise = rng.integers(0, 16, (h, w, 3), dtype=np.uint8)
    a[..., 0] = (fx * 230).astype(np.uint8) + noise[..., 0]
    a[..., 1] = (fy * 230).astype(np.uint8) + noise[..., 1]
    a[..., 2] = ((1 - fx * fy) * 230).astype(np.uint8) + noise[..., 2]
    return a


def random_luts(seed):
    rng = np.random.default_rng(seed)
    x = np.arange(256) / 255
    return np.stack([np.clip(np.round(255 * x ** (0.6 + rng.random())) + rng.integers(0, 4, 256), 0, 255)
                     for _ in range(3)]).astype(np.uint8)


def best(n, f):
    t = float("inf")
    for _ in range(n):
        t0 = time.perf_counter()
        f()
        t = min(t, time.perf_counter() - t0)
    return t * 1000


def run(label, out, sizes, **kw):
    try:
        g = fae_gpu.Gpu(**kw)
    except Exception as ex:
        out.append(f"FAIL  {label}: no device: {ex}")
        return
    i = g.info()
    out.append(f"INFO  {label}: {i['name']} via {i['backend']} ({i['driver']} {i['driver_info']}), "
               f"max {i['max_texture_2d']} px, up to {i['max_pixels'] / 1e6:.0f} MP")
    cases = {c["id"]: c["job"] for c in parity.load_cases()[0]}
    edits = (("curves", None), ("chain", cases["combo_26"]), ("whole", cases["combo_19"]), ("sharp", SHARP))
    for (w, h), (what, job0) in [(sz, e) for sz in sizes for e in edits]:
        name = f"{label} {w}x{h} ({w * h / 1e6:.1f} MP) {what:6s}"
        img = test_image(w, h, w + h)
        job = job0 or {"steps": [{"t": "curves", "luts": random_luts(w * 7 + h).tolist()}]}
        steps = core._steps(job)
        sp = job.get("sharp")
        if sp:
            steps.append(("sharp", sp["as"], sp["bs"], sp["st"], sp["lc"], sp["fo"], 1.0))
        try:
            g.set_steps(steps)
            t_up = best(3, lambda: g.upload(img))
            t_pass = best(2, lambda: g.run(20)) / 20
            r = {}
            t_read = best(2, lambda: r.__setitem__("o", g.readback()))
        except Exception as ex:
            out.append(f"WARN  {name}: not run: {ex}")
            continue
        ref = {}
        t_cpu = best(3, lambda: ref.__setitem__("o", core.render(img, job, engine="cpu")))
        d = np.abs(r["o"].astype(np.int16) - ref["o"].astype(np.int16))
        state = "OK  " if d.mean() < 0.05 and (d.max(axis=2) > 2).mean() <= 1e-4 else "FAIL"
        out.append(f"{state}  {name}: max diff {int(d.max())}, mean {d.mean():.4f} · upload {t_up:.1f} ms · chain {t_pass:.2f} ms · "
                   f"read back {t_read:.1f} ms · CPU core {t_cpu:.1f} ms ({t_cpu / t_pass:.0f}x the chain)")


def main():
    sizes = SIZES[:1] if "--small" in sys.argv else SIZES
    out = [f"INFO  fae_gpu {fae_gpu.version()}, CPU core {core.info()}"]
    run("DX12", out, sizes, backends="dx12") if sys.platform == "win32" else None
    run("Vulkan", out, sizes, backends="vulkan")
    if "--warp" in sys.argv:
        run("WARP", out, sizes, fallback=True)
    print("\n".join(out))


if __name__ == "__main__":
    main()
