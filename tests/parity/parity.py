"""Parity tests for the Rust core: the Python extension (Process) and the WebAssembly build
(live preview) must give exactly the same pixels and histograms as the stored goldens.

  python tests/parity/parity.py record                 # (re)write the goldens, from the Python core
  python tests/parity/parity.py record --keep-cases    # same inputs, new goldens (the core's math changed)
  python tests/parity/parity.py check --impl core      # Rust core, Python extension
  python tests/parity/parity.py check --impl wasm      # Rust core, WebAssembly (in node, single thread)
  python tests/parity/parity.py tables                 # do this machine's UI tables match the stored cases?
  options: -k <text> only cases whose id contains <text>; -v list every failing image

Each result is the image plus the four histograms the UI draws (hin, hout, hmx, hmn); manifest.json
stores a hash of both per test image x case. The NumPy / JS implementations the goldens first came
from are gone, so re-record only on purpose (a deliberate change of the math), and look at the
images in .work/golden/ before and after.

Cases come from the UI's own code (gen_cases.js: random slider values -> computeEdit -> jobFor).
`record` freezes them into cases.json.gz (film tables by preset name), so every machine checks the
same inputs: the JS math behind the tables (Math.pow, Math.exp) can differ slightly between node
versions. Node is needed for record, tables and the wasm check. Test images are synthetic
(made here, deterministic). Your own images can go into tests/parity/images/ (not committed);
their goldens go to .work/manifest.local.json. Images that changed since `record` are reported as
stale, not as failures.
"""
from __future__ import annotations

import argparse
import copy
import gzip
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
WORK = os.path.join(HERE, ".work")
GOLD = os.path.join(WORK, "golden")
MANIFEST = os.path.join(HERE, "manifest.json")
CASES = os.path.join(HERE, "cases.json.gz")
MANIFEST_LOCAL = os.path.join(WORK, "manifest.local.json")
LOCAL_DIR = os.path.join(HERE, "images")
LOCAL_MAX = 1600            # own images are scaled down to this long side
sys.path.insert(0, ROOT)

from fastedit import core  # noqa: E402


# ---------------------------------------------------------------------------
# test images
# ---------------------------------------------------------------------------
def _grad(w, h, seed):
    rng = np.random.default_rng(seed)
    x = np.linspace(0, 1, w)[None, :]
    y = np.linspace(0, 1, h)[:, None]
    hue = x * 6.0
    v = 0.15 + 0.85 * (1 - y)
    c = np.stack([np.clip(np.abs(((hue + o) % 6) - 3) - 1, 0, 1) for o in (0, 4, 2)], axis=-1)
    img = (c * 0.8 + 0.2) * v[..., None] * 255 + rng.integers(-6, 7, (h, w, 3))
    return np.clip(img, 0, 255).astype(np.uint8)


def _chart(w, h, seed):
    rng = np.random.default_rng(seed)
    pal = rng.integers(0, 256, (48, 3))
    pal[:8] = np.linspace(0, 255, 8)[:, None].astype(int)          # grey ramp
    img = np.empty((h, w, 3), np.uint8)
    img[:] = (96, 110, 128)                                         # large surround
    for i, col in enumerate(pal):
        r, c = divmod(i, 8)
        y0, x0 = 20 + r * (h - 40) // 6, 20 + c * (w - 40) // 8
        img[y0:y0 + (h - 40) // 6 - 6, x0:x0 + (w - 40) // 8 - 6] = col
    return img


def _noise(w, h, seed, lo=0, hi=256):
    return np.random.default_rng(seed).integers(lo, hi, (h, w, 3), dtype=np.uint8)


def synthetic_images():
    yield "grad", _grad(640, 427, 1)
    yield "chart", _chart(480, 320, 2)
    yield "noise_tall", _noise(301, 700, 3)          # taller than one 512-row strip, short last strip
    yield "grad_tall", _grad(150, 1100, 8)           # three strips: catches state leaking between strips
    yield "dark", np.clip(_grad(400, 300, 4).astype(int) // 4 + _noise(400, 300, 5, 0, 12), 0, 255).astype(np.uint8)
    yield "tiny", _noise(7, 5, 6)
    yield "thin", _grad(997, 3, 7)


def local_images():
    if not os.path.isdir(LOCAL_DIR):
        return
    for name in sorted(os.listdir(LOCAL_DIR)):
        if os.path.splitext(name)[1].lower() not in (".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff"):
            continue
        with Image.open(os.path.join(LOCAL_DIR, name)) as im:
            im = im.convert("RGB")
            if max(im.size) > LOCAL_MAX:
                s = LOCAL_MAX / max(im.size)
                im = im.resize((max(1, round(im.width * s)), max(1, round(im.height * s))), Image.Resampling.LANCZOS)
            yield "local_" + os.path.splitext(name)[0], np.asarray(im).copy()


# ---------------------------------------------------------------------------
# cases, reference, hashing
# ---------------------------------------------------------------------------
def generate_cases():
    """Fresh cases from this machine's UI code (node)."""
    os.makedirs(WORK, exist_ok=True)
    path = os.path.join(WORK, "cases.json")
    subprocess.run(["node", os.path.join(HERE, "gen_cases.js"), ROOT, path], check=True, stdout=subprocess.DEVNULL)
    with open(path, encoding="utf-8") as f:
        return json.load(f)


_FILM = {}


def film_table(key):
    """A Film preset's 3D table, read from ui/js/film/<key>.js (plain number literals)."""
    if key not in _FILM:
        with open(os.path.join(ROOT, "ui", "js", "film", key + ".js"), encoding="utf-8") as f:
            s = f.read()
        m = re.search(r"addFilmPreset\(\s*'([^']+)'\s*,\s*'[^']*'\s*,\s*\[", s)
        if not m or m.group(1) != key:
            raise ValueError(f"film preset file for {key} not understood")
        _FILM[key] = json.loads(s[m.end() - 1:s.rindex("]") + 1])
    return _FILM[key]


def _films(job):
    """The Film Effect entries of a job: the flat one and those of its steps (a reordered chain)."""
    out = [job.get("film")] + [st.get("film") for st in job.get("steps") or [] if st.get("t") == "film"]
    return [f for f in out if f]


def freeze(cases):
    """Film tables replaced by their preset name (they are 90 % of the size and fixed data)."""
    out = copy.deepcopy(cases)
    for c in out:
        for film in _films(c["job"]):
            if film.get("d"):
                key = c["state"]["flt_film_preset"]
                if film["d"] != film_table(key):
                    raise ValueError(f"{c['id']}: film table is not preset {key}")
                film["d"] = {"preset": key}
    return out


def thaw(cases):
    for c in cases:
        for film in _films(c["job"]):
            if isinstance(film.get("d"), dict):
                film["d"] = film_table(film["d"]["preset"])
    return cases


def write_cases(cases):
    data = json.dumps(freeze(cases), separators=(",", ":"), sort_keys=True).encode()
    with open(CASES, "wb") as f:                         # mtime=0: same bytes for the same cases
        f.write(gzip.compress(data, 9, mtime=0))


def load_cases():
    """The frozen cases (what manifest.json was recorded from) and the file's path for node."""
    if not os.path.exists(CASES):
        sys.exit("no cases.json.gz: run  parity.py record  first")
    with gzip.open(CASES, "rt", encoding="utf-8") as f:
        return thaw(json.load(f)), CASES


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()[:24]


def img_sha(a: np.ndarray) -> str:
    return sha(repr(a.shape).encode() + a.tobytes())


def case_sha(c) -> str:
    return sha(json.dumps(c["job"], sort_keys=True).encode())


def hist_bytes(h) -> bytes:
    """hin, hout, hmx, hmn as one little-endian uint32 blob (what run_js.js writes too)."""
    return b"".join(np.asarray(x, dtype="<u4").tobytes() for x in h)


def result_key(out, hb: bytes) -> str:
    return img_sha(out) + ":" + sha(hb)


def reference(rgb, edit):
    """What Process writes (api._render_full) after the stage, and the UI's histograms of it."""
    out, *h = core.render(rgb, edit, hist=True)
    return out, hist_bytes(h)


def run_safe(fn, *a):
    try:
        out = fn(*a)
        return out, None
    except Exception as ex:
        return None, f"{type(ex).__name__}: {ex}"


# ---------------------------------------------------------------------------
# implementations under test: {case id: (image, histogram bytes) | "error: ..."}
# ---------------------------------------------------------------------------
def need_core():
    if not core.available():
        sys.exit(core.error())


def impl_core(name, rgb, cases):
    need_core()
    res = {}
    for c in cases:
        out, err = run_safe(reference, rgb, c["job"])
        res[c["id"]] = out if err is None else "error: " + err
    return res


_RUN = None          # temp folder for the node runs of one check (removed afterwards)


def impl_wasm(name, rgb, cases, cases_path):
    h, w = rgb.shape[:2]
    tmp = os.path.join(_RUN, "wasm", name)
    os.makedirs(tmp)
    img = os.path.join(_RUN, f"{name}.rgb")
    with open(img, "wb") as f:
        f.write(np.ascontiguousarray(rgb).tobytes())
    ids = os.path.join(_RUN, f"wasm_{name}_ids.json")
    with open(ids, "w") as f:
        json.dump([c["id"] for c in cases], f)
    subprocess.run(["node", os.path.join(HERE, "run_js.js"), ROOT, cases_path, ids, img, str(w), str(h), tmp], check=True)
    res = {}
    for c in cases:
        cid = c["id"]
        if os.path.exists(os.path.join(tmp, cid + ".err")):
            with open(os.path.join(tmp, cid + ".err")) as f:
                res[cid] = "error: " + f.read().splitlines()[0]
        else:
            with open(os.path.join(tmp, cid + ".rgb"), "rb") as f:
                out = np.frombuffer(f.read(), np.uint8).reshape(h, w, 3)
            with open(os.path.join(tmp, cid + ".h32"), "rb") as f:
                res[cid] = (out, f.read())
    return res


IMPLS = {"core": impl_core, "wasm": impl_wasm}


# ---------------------------------------------------------------------------
# record / check
# ---------------------------------------------------------------------------
def all_images():
    for name, rgb in synthetic_images():
        yield name, rgb, False
    for name, rgb in local_images():
        yield name, rgb, True


def record(args):
    if args.k:
        sys.exit("record always writes every case (no -k)")
    need_core()
    if not args.keep_cases:            # --keep-cases: new goldens for the stored inputs (no node needed)
        write_cases(generate_cases())
    cases, _ = load_cases()
    man = {False: {"cases": {}, "images": {}, "results": {}}, True: {"cases": {}, "images": {}, "results": {}}}
    for m in man.values():
        m["cases"] = {c["id"]: case_sha(c) for c in cases}
    t0 = time.perf_counter()
    for name, rgb, local in all_images():
        m = man[local]
        m["images"][name] = {"sha": img_sha(rgb), "w": int(rgb.shape[1]), "h": int(rgb.shape[0])}
        os.makedirs(os.path.join(GOLD, name), exist_ok=True)
        for c in cases:
            r, err = run_safe(reference, rgb, c["job"])
            key = f"{name}/{c['id']}"
            if err:
                m["results"][key] = "error: " + err
                continue
            out, hb = r
            m["results"][key] = result_key(out, hb)
            Image.fromarray(out).save(os.path.join(GOLD, name, c["id"] + ".png"), compress_level=1)
        print(f"  {name:14s} {rgb.shape[1]}x{rgb.shape[0]}  {len(cases)} cases")
    for local, path in ((False, MANIFEST), (True, MANIFEST_LOCAL)):
        if local and not man[True]["images"]:
            continue
        with open(path, "w", encoding="utf-8") as f:
            json.dump(man[local], f, indent=0, sort_keys=True)
    errs = sum(v.startswith("error") for m in man.values() for v in m["results"].values())
    print(f"recorded {len(cases)} cases in {time.perf_counter() - t0:.1f} s"
          + (f"; the core raised on {errs} (stored as expected errors)" if errs else ""))


def _diff(name, cid, out):
    p = os.path.join(GOLD, name, cid + ".png")
    if not os.path.exists(p):
        return "differs (no golden PNG to measure by how much: run record)"
    g = np.asarray(Image.open(p))
    if g.shape != out.shape:
        return f"shape {out.shape} != {g.shape}"
    d = np.abs(g.astype(np.int16) - out.astype(np.int16))
    px = int((d.max(axis=2) > 0).sum())
    y, x = np.unravel_index(int(np.argmax(d.max(axis=2))), d.shape[:2])
    return f"max diff {int(d.max())}, {px} px ({100 * px / d.shape[0] / d.shape[1]:.3f} %), worst at x={x} y={y}"


def check(args):
    global _RUN
    _RUN = tempfile.mkdtemp(prefix="fastedit-parity-")
    try:
        return _check(args)
    finally:
        shutil.rmtree(_RUN, ignore_errors=True)


def _check(args):
    cases, cases_path = load_cases()
    if args.k:
        cases = [c for c in cases if args.k in c["id"]]
    mans = {}
    for local, path in ((False, MANIFEST), (True, MANIFEST_LOCAL)):
        if os.path.exists(path):
            with open(path, encoding="utf-8") as f:
                mans[local] = json.load(f)
    if False not in mans:
        sys.exit("no manifest.json: run  parity.py record  first")
    fn = IMPLS[args.impl]
    n = {"pass": 0, "fail": 0, "stale": 0}
    fails = []
    t0 = time.perf_counter()
    for name, rgb, local in all_images():
        m = mans.get(local)
        if m is None or name not in m["images"]:
            print(f"  {name}: not recorded (run record)")
            continue
        if m["images"][name]["sha"] != img_sha(rgb):
            print(f"  {name}: input image changed since record -> stale")
            n["stale"] += len(cases)
            continue
        res = fn(name, rgb, cases, cases_path) if args.impl == "wasm" else fn(name, rgb, cases)
        for c in cases:
            cid, got = c["id"], res[c["id"]]
            want = m["results"].get(f"{name}/{cid}")
            if want is None or m["cases"].get(cid) != case_sha(c):
                n["stale"] += 1
            elif isinstance(got, str) or want.startswith("error"):
                ok = isinstance(got, str) and want.startswith("error")
                n["pass" if ok else "fail"] += 1
                if not ok:
                    fails.append((name, cid, f"got {got if isinstance(got, str) else 'an image'}, golden: {want}"))
            elif result_key(*got) == want:
                n["pass"] += 1
            else:
                n["fail"] += 1
                out = got[0]
                if img_sha(out) == want.split(":")[0]:
                    fails.append((name, cid, "same pixels, histograms differ"))
                else:
                    fails.append((name, cid, _diff(name, cid, out)))
    dt = time.perf_counter() - t0
    by_case = {}
    for name, cid, msg in fails:
        by_case.setdefault(cid, []).append((name, msg))
    for cid, items in sorted(by_case.items()):
        shown = items if args.v else items[:1]
        for name, msg in shown:
            print(f"  FAIL {cid:28s} {name:14s} {msg}")
        if len(items) > len(shown):
            print(f"       {'':28s} (+{len(items) - len(shown)} more images)")
    print(f"{args.impl}: {n['pass']} pass, {n['fail']} fail, {n['stale']} stale  [{dt:.1f} s]")
    if n["stale"]:
        print("stale = image or case changed since record: re-run record if the change was intended")
    return 1 if n["fail"] else 0


def tables(args):
    """Compare this machine's freshly generated cases with the frozen ones, field by field."""
    frozen, _ = load_cases()
    fresh = {c["id"]: c for c in generate_cases()}
    per_field, n_diff = {}, 0
    for c in frozen:
        f = fresh.get(c["id"])
        if f is None:
            print(f"  {c['id']}: not generated here")
            continue
        changed = False
        for key in sorted(set(c["job"]) | set(f["job"])):
            a, b = c["job"].get(key), f["job"].get(key)
            if json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True):
                continue
            changed = True
            fa, fb = _flat(a), _flat(b)
            st = per_field.setdefault(key, {"cases": 0, "max": 0.0})
            st["cases"] += 1
            if len(fa) == len(fb):
                st["max"] = max(st["max"], max((abs(x - y) for x, y in zip(fa, fb)), default=0.0))
            else:
                st["max"] = float("inf")
        n_diff += changed
    if not per_field:
        print(f"tables: all {len(frozen)} cases identical to cases.json.gz")
        return 0
    print(f"tables: {n_diff} of {len(frozen)} cases differ from cases.json.gz (this is the UI's JS math on this "
          "machine, not the core; checks use the stored cases)")
    for key, st in sorted(per_field.items()):
        print(f"  {key:6s} in {st['cases']:3d} cases, max abs difference {st['max']:.3g}")
    return 0


def _flat(v):
    if isinstance(v, bool) or v is None:
        return [float(bool(v))]
    if isinstance(v, (int, float)):
        return [float(v)]
    if isinstance(v, dict):
        return [x for k in sorted(v) for x in _flat(v[k])]
    if isinstance(v, (list, tuple)):
        return [x for e in v for x in _flat(e)]
    return []


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cmd", choices=["record", "check", "tables"])
    ap.add_argument("--impl", default="core", choices=sorted(IMPLS))
    ap.add_argument("-k", help="only cases whose id contains this text")
    ap.add_argument("-v", action="store_true", help="list every failing image, not just the first per case")
    ap.add_argument("--keep-cases", action="store_true",
                    help="record: keep cases.json.gz, only re-record the results (a change in the core's math)")
    a = ap.parse_args()
    sys.exit({"record": record, "check": check, "tables": tables}[a.cmd](a))


if __name__ == "__main__":
    main()
