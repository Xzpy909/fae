"""Checks of the filter kit's filters (docs/filter-kit.md): the cases come from each filter's
descriptor (core/gpu/fae-gpu/filters/<id>.json), the GPU core renders them, the results are compared
with stored ones (tests/gpu/fx_results/<id>.zip, PNGs). Kit filters exist only on the GPU, so there is no
CPU reference: the stored results are what the filter looked like when it was recorded (look at them
first: --record also writes them as images to tests/gpu/.work/fx/<id>/).

    python tests/gpu/fx_check.py                   # every filter
    python tests/gpu/fx_check.py -k test_blur -v   # filters whose id contains "test_blur", every case
    python tests/gpu/fx_check.py --record test_blur    # (re)store a filter's results (after a change
                                                       # to it that is meant to change its look)
    python tests/gpu/fx_check.py --backend vulkan  # (default: dx12 on Windows)

Cases per filter, on three synthetic images (colour ramps, colour patches, a tall striped one where
big blurs run on a shrunk copy):
- neutral: every field at its default -> the input exactly (the kit's rule: defaults = no change);
  also at Opacity 0 with every parameter moved;
- every parameter alone at its minimum and its maximum (a select: each option), the others at their
  defaults;
- all parameters at seeded random values, at Opacity 100 % and 37 %.
A case passes when the result is identical to the stored one or within the per-step rule of
tolerance.py: at most 0.01 % of the values (at least 2) more than 2 levels off, at most 1 % of the
values different, mean < 0.05 (another GPU, another driver: f32 results can round the other way).
A filter without stored results is reported as "not recorded" (fails).
"""
from __future__ import annotations

import argparse
import glob
import io
import json
import os
import sys
import time
import zipfile

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
FILTERS = os.path.join(ROOT, "core", "gpu", "fae-gpu", "filters")
RESULTS = os.path.join(HERE, "fx_results")
WORK = os.path.join(HERE, ".work", "fx")

MAX_LEVELS = 2
MAX_MEAN = 0.05
MAX_OUTLIERS = 1e-4      # share of the values more than MAX_LEVELS off (at least MIN_OUTLIERS)
MIN_OUTLIERS = 2
MAX_OFF = 1e-2           # share of the values that may differ at all
RANDOM_SETS = 2


# ---- the descriptor ------------------------------------------------------------------------------
def load(path: str) -> dict:
    """A descriptor with what the cases need: params = [(key, kind, default, values...)] in the
    order the GPU core takes them (the fields without Opacity), opacity = its key or None."""
    with open(path, encoding="utf-8") as f:
        d = json.load(f)
    d["id"] = os.path.splitext(os.path.basename(path))[0]
    op = d.get("opacity")
    params = []
    for a in d["fields"]:
        if isinstance(a, list):
            key, _label, default, lo, hi, step = a[:6]
            if key != op:
                params.append({"key": key, "select": False, "def": float(default), "min": float(lo),
                               "max": float(hi), "step": float(step)})
        elif isinstance(a, dict) and a.get("type") == "select":
            opts = [o[0] for o in a["options"]]
            params.append({"key": a["key"], "select": True, "def": float(opts.index(a["def"])), "n": len(opts)})
    d["params"] = params
    return d


def snap(p: dict, v: float) -> float:
    """v on the slider's grid (what the UI can send)."""
    return float(min(p["max"], max(p["min"], round((v - p["min"]) / p["step"]) * p["step"] + p["min"])))


def cases(d: dict) -> list[tuple[str, list[float], float]]:
    """(name, params, opacity) for the descriptor's filter."""
    P = d["params"]
    base = [p["def"] for p in P]
    out = [("neutral", base, 1.0)]
    for i, p in enumerate(P):
        if p["select"]:
            vals = [float(j) for j in range(p["n"]) if j != p["def"]]
        else:
            vals = [v for v in dict.fromkeys([p["min"], p["max"]]) if v != p["def"]]
        for v in vals:
            q = list(base)
            q[i] = v
            out.append((f"{p['key']}={v:g}", q, 1.0))
    rng = np.random.default_rng(sum(map(ord, d["id"])))
    for r in range(RANDOM_SETS):
        q = [float(rng.integers(0, p["n"])) if p["select"] else snap(p, rng.uniform(p["min"], p["max"])) for p in P]
        out.append((f"random{r}", q, 1.0))
        out.append((f"random{r}_op37", q, 0.37))
    if P:
        out.append(("opacity0", out[-1][1], 0.0))
    return out


# ---- the images ----------------------------------------------------------------------------------
def images() -> dict[str, np.ndarray]:
    """Synthetic, deterministic, smooth enough that the stored results stay small."""
    y, x = np.mgrid[0:200, 0:320].astype(np.float64)
    ramps = np.stack([x / 319 * 255, (1 - x / 319) * 255 * (y / 199), y / 199 * 255], -1)
    ramps[150:, :] = (x[150:, :, None] / 319 * 255).repeat(3, -1)              # a grey ramp at the bottom
    pal = np.array([[115, 82, 68], [194, 150, 130], [98, 122, 157], [87, 108, 67], [133, 128, 177], [103, 189, 170],
                    [214, 126, 44], [80, 91, 166], [193, 90, 99], [94, 60, 108], [157, 188, 64], [224, 163, 46],
                    [56, 61, 150], [70, 148, 73], [175, 54, 60], [231, 199, 31], [187, 86, 149], [8, 133, 161],
                    [243, 243, 242], [200, 200, 200], [160, 160, 160], [122, 122, 121], [85, 85, 85], [52, 52, 52]])
    patches = np.zeros((180, 240, 3))
    for i, c in enumerate(pal):
        r, k = divmod(i, 6)
        patches[r * 45:(r + 1) * 45, k * 40:(k + 1) * 40] = c
    patches[:, :, :] += ((np.arange(240)[None, :, None] + np.arange(180)[:, None, None]) % 7 - 3) * 1.5
    ty, tx = np.mgrid[0:720, 0:96].astype(np.float64)
    tall = np.stack([127.5 + 120 * np.sin(ty / 23.0), 127.5 + 110 * np.sin(tx / 9.0 + ty / 61.0),
                     np.where((ty // 90 + tx // 32) % 2 == 0, 40.0, 215.0)], -1)
    return {k: np.ascontiguousarray(np.clip(np.round(v), 0, 255).astype(np.uint8))
            for k, v in (("ramps", ramps), ("patches", patches), ("tall", tall))}


# ---- the rule ------------------------------------------------------------------------------------
def compare(a: np.ndarray, b: np.ndarray) -> tuple[bool, str]:
    if a.shape != b.shape:
        return False, f"shape {a.shape} vs {b.shape}"
    d = np.abs(a.astype(np.int16) - b.astype(np.int16))
    n = d.size
    far = int((d > MAX_LEVELS).sum())
    off = int((d > 0).sum())
    ok = far <= max(MIN_OUTLIERS, MAX_OUTLIERS * n) and off <= MAX_OFF * n and d.mean() < MAX_MEAN
    return ok, f"max {int(d.max())}, {off / n * 100:.4f} % off, {far} > {MAX_LEVELS}, mean {d.mean():.5f}"


def key(img: str, name: str) -> str:
    return f"{img}|{name}"


def png(a: np.ndarray) -> bytes:
    from PIL import Image
    b = io.BytesIO()
    Image.fromarray(a).save(b, "PNG", optimize=True)
    return b.getvalue()


def entry(k: str) -> str:
    """the zip member of result k ("image|case")"""
    img, name = k.split("|")
    return f"{img}/{name.replace('=', '_')}.png"


def read_results(path: str) -> dict[str, np.ndarray]:
    from PIL import Image
    out = {}
    with zipfile.ZipFile(path) as z:
        for n in z.namelist():
            img, case = n[:-4].split("/")
            out[n] = np.asarray(Image.open(io.BytesIO(z.read(n))).convert("RGB"))
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-k", default="", help="only filters whose id contains this")
    ap.add_argument("-v", action="store_true", help="list every case")
    ap.add_argument("--record", nargs="*", metavar="ID", help="store the results of these filters (none: of every filter)")
    ap.add_argument("--backend", default=None)
    args = ap.parse_args()

    import fae_gpu
    dev = fae_gpu.Gpu(backends=args.backend)
    info = dev.info()
    print(f"fae_gpu {fae_gpu.version()} on {info['name']} ({info['backend']})")
    built = {f["id"]: f for f in fae_gpu.filters()}
    descs = [load(p) for p in sorted(glob.glob(os.path.join(FILTERS, "*.json")))]
    descs = [d for d in descs if args.k in d["id"]]
    imgs = images()
    bad = 0
    for d in descs:
        fid = d["id"]
        if fid not in built:
            print(f"{fid}: not in this build of fae_gpu (run core\\gpu\\build_gpu.bat)")
            bad += 1
            continue
        if built[fid]["params"] != len(d["params"]):
            print(f"{fid}: the build has {built[fid]['params']} parameters, the descriptor {len(d['params'])}: rebuild")
            bad += 1
            continue
        record = args.record is not None and (not args.record or fid in args.record)
        store_path = os.path.join(RESULTS, f"{fid}.zip")
        stored = read_results(store_path) if not record and os.path.exists(store_path) else {}
        results, fails, t0 = {}, [], time.perf_counter()
        cs = cases(d)
        for img_name, img in imgs.items():
            for name, p, op in cs:
                out = dev.render(img, [("fx", fid, p, op)])
                k = key(img_name, name)
                results[k] = out
                if name in ("neutral", "opacity0"):
                    ok = np.array_equal(out, img)
                    msg = "identical to the input" if ok else "changes the image (must be the input exactly)"
                elif record:
                    ok, msg = True, "recorded"
                elif entry(k) not in stored:
                    ok, msg = False, "not recorded (--record " + fid + ")"
                else:
                    ok, msg = compare(out, stored[entry(k)])
                if not ok:
                    fails.append(k)
                if args.v or not ok:
                    print(f"  {'ok  ' if ok else 'FAIL'} {fid} {img_name:8s} {name:24s} {msg}")
        dt = time.perf_counter() - t0
        if record:
            os.makedirs(RESULTS, exist_ok=True)
            # the PNGs once: stored in the zip as they are (no second compression), and for looking at
            with zipfile.ZipFile(store_path, "w", zipfile.ZIP_STORED) as z:
                for k, v in results.items():
                    data = png(v)
                    if not k.endswith(("|neutral", "|opacity0")):
                        z.writestr(zipfile.ZipInfo(entry(k), date_time=(2026, 1, 1, 0, 0, 0)), data)
                    path = os.path.join(WORK, fid, entry(k).replace("/", "_"))
                    os.makedirs(os.path.dirname(path), exist_ok=True)
                    with open(path, "wb") as f:
                        f.write(data)
            print(f"{fid}: recorded {len(results)} results ({os.path.getsize(store_path) // 1024} KB); "
                  f"images in {os.path.relpath(os.path.join(WORK, fid), ROOT)}")
        print(f"{fid} ({d['kind']}): {len(results) - len(fails)}/{len(results)} ok  [{dt:.1f} s]")
        bad += bool(fails)
    print("ALL OK" if not bad and descs else f"{bad} filter(s) failed" if bad else "no filters")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
