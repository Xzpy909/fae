"""B5 (docs/rust-backend-plan.md): Process and the encoded compare in the Rust back end, against the Python
path they replace (api.py's code before B5, kept below: the stage from the back end, fastedit/core.py's
render on the Python cores fastedit_core / fae_gpu, the B4 encoders, Pillow decoding the encoded file back).

    python tests/backend/process_check.py            # everything
    python tests/backend/process_check.py --quick    # fewer edits on the CPU core

- render: every parity case (tests/parity/cases.json.gz), alone and with Sharpen, on the CPU core:
  identical to core.render(engine="cpu"); on the GPU core (when there is one): identical to core.render(
  engine="gpu") (the same shaders on the same device); the kit filters left out on the CPU core;
- process: the files written (byte for byte), the answers, progress; RGB, RGBA, AVIF, rotated, resized,
  colour-balanced stages; WebP lossless / lossy, PNG, JPEG, AVIF; the same with the GPU turned off;
- lossy_preview: the answer (sizes, bytes, encoder, settings), the preview pixels, the cache (a size
  change doesn't encode again), cancellation;
- core_info, the shell's fallback decoder (a file Rust can't open).
Needs fastedit_core (core/build_core.bat); fae_gpu (core/gpu/build_gpu.bat) for the GPU comparison.
"""
from __future__ import annotations

import argparse
import io
import json
import os
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, ROOT)
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(ROOT, "tests", "parity"))

import numpy as np  # noqa: E402
from PIL import Image  # noqa: E402

from fastedit import backend, core  # noqa: E402
from fastedit.config import ENCODERS_DIR, PRESETS_PATH, SETTINGS_PATH, StageParams  # noqa: E402
from fastedit.engine import to_pil  # noqa: E402
from commands_check import preview_rgba, pv_size  # noqa: E402
import parity  # noqa: E402

INPUTS = os.path.join(ROOT, "tests", "stage", "inputs")
SHARP = {"as": 0.6, "bs": 0.4, "st": 0.5, "lc": 0.3, "fo": 0.2}

fails: list[str] = []


def check(name, ok, extra=""):
    print(f"  {'ok  ' if ok else 'FAIL'} {name}{('  ' + extra) if extra else ''}")
    if not ok:
        fails.append(name)


# ---- the Python path before B5 (api.py) ------------------------------------------------------------------
def py_render_full(job, skipped=None, engine="auto"):
    """api.py _render_full: (icc, image) - the stage from the back end, the edit by core.render."""
    P = StageParams.from_dict(job.get("stage"))
    raw, icc = backend.shared().stage(job.get("path"), P)
    img = to_pil(raw)
    alpha = img.getchannel("A") if img.mode == "RGBA" else None
    rgb = np.ascontiguousarray(np.asarray(img.convert("RGB")))
    out = Image.fromarray(core.render(rgb, job.get("edit"), skipped=skipped, engine=engine))
    if alpha is not None:
        out.putalpha(alpha)
    return icc, out


def py_process(jobs, fmt, opts, engine="auto"):
    """api.py process (save_image: the B4 encoders in Rust)."""
    from fastedit.output import norm_fmt
    fmt = norm_fmt(fmt)
    results = []
    for job in jobs:
        path = job.get("path")
        try:
            skipped = []
            icc, out = py_render_full(job, skipped, engine)
            r = {"path": path, "ok": True, "out": backend.shared().save(out, path, fmt, icc, opts)}
            if skipped:
                r["skipped"] = skipped
        except Exception as ex:
            r = {"path": path, "ok": False, "error": f"{type(ex).__name__}: {ex}"}
        results.append(r)
    core.gpu_release()
    return results


def py_lossy_preview(job, fmt, opts, req_id, full=0):
    """api.py lossy_preview without its cache and cancellation: (answer, preview RGBA)."""
    from fastedit.output import norm_fmt
    fmt = norm_fmt(fmt)
    icc, out = py_render_full(job)
    core.gpu_release()
    data, enc, used = backend.shared().encode(out, fmt, opts, icc)
    with Image.open(io.BytesIO(data)) as jim:
        jim.load()
        dec = jim.convert("RGBA" if "A" in jim.getbands() or "transparency" in jim.info else "RGB")
    w, h = dec.size
    pw, ph = pv_size(w, h, full)
    rgba = preview_rgba(dec, (pw, ph), Image.Resampling.LANCZOS)
    return {"ok": True, "req_id": req_id, "pw": pw, "ph": ph, "bytes": len(data), "encoder": enc, "fmt": fmt,
            "opts": used}, rgba


# ---- the checks ------------------------------------------------------------------------------------------
def cases(quick: bool):
    cs = parity.load_cases()[0]
    if quick:
        cs = cs[::6]
    out = [(c["id"], c["job"]) for c in cs]
    out += [(c["id"] + "+sharp", dict(c["job"], sharp=SHARP)) for c in cs[::4]]
    out.append(("sharp_only", {"sharp": SHARP}))
    return out


def fx_edit():
    """An edit with a kit filter between two steps (a filter fae_gpu lists; a made-up id without fae_gpu)."""
    fid, n = "starfall", 0
    try:
        import fae_gpu
        fs = sorted(fae_gpu.filters(), key=lambda f: f["hidden"])     # one of the Filters list, else a test one
        f = next(f for f in fs if f["kind"] == "pixel" or not f["hidden"])
        fid, n = f["id"], f["params"]
    except Exception:
        pass
    return {"steps": [{"t": "point", "point": True, "k": 1.1, "o": -2},
                      {"t": "fx", "fx": {"id": fid, "p": [0.5] * n, "op": 0.8}},
                      {"t": "sc", "sc": {"cast": 0.3, "cc": 20, "op": 1}}]}, fid


def check_render(b, quick):
    print("render: the edit on the CPU core (exact vs fastedit_core)")
    E = cases(quick)
    imgs = [(n, a) for n, a in parity.synthetic_images() if n in ("grad", "noise_tall", "tiny", "thin")]
    for name, rgb in imgs:
        h, w = rgb.shape[:2]
        bad = []
        for cid, job in E:
            ref = core.render(rgb, job, engine="cpu")
            out, sk = b.render(rgb.tobytes(), w, h, job, "cpu")
            if out != ref.tobytes() or sk:
                bad.append(cid)
        check(f"{name} {w}x{h}: {len(E)} edits identical", not bad, ", ".join(bad[:8]))
    rgb = imgs[0][1]
    h, w = rgb.shape[:2]
    fx, fid = fx_edit()
    sk_ref = []
    ref = core.render(rgb, fx, engine="cpu", skipped=sk_ref)
    out, sk = b.render(rgb.tobytes(), w, h, fx, "cpu")
    check(f"a kit filter on the CPU core: left out and reported ({sk})", out == ref.tobytes() and sk == sk_ref == [fid])
    for bad_edit in ({"steps": [{"t": "nope"}]}, {"wb": [1, 2, 3]}, {"film": {"n": 1, "op": 1}}):
        try:
            b.render(rgb.tobytes(), w, h, bad_edit, "cpu")
            ok = False
        except RuntimeError:
            ok = True
        check(f"a broken edit raises: {json.dumps(bad_edit)}", ok)

    gi, gp = b.gpu_info(), core.gpu_info()
    if not gi["ok"] or not gp["ok"]:
        print(f"render on the GPU core: skipped (back end: {gi.get('error') or 'ok'}; fae_gpu: {gp.get('error') or 'ok'})")
        return
    print(f"render: the edit on the GPU core ({gi['name']} via {gi['backend']}; exact vs fae_gpu)")
    for name, rgb in imgs[:2]:
        h, w = rgb.shape[:2]
        bad = []
        for cid, job in E[::3] + [("fx", fx)]:
            ref = core.render(rgb, job, engine="gpu")
            out, sk = b.render(rgb.tobytes(), w, h, job, "gpu")
            if out != ref.tobytes() or sk:
                bad.append(cid)
        check(f"{name} {w}x{h}: {len(E[::3]) + 1} edits identical", not bad, ", ".join(bad[:8]))
    core.gpu_release()
    b.gpu_release()


def inputs(d):
    """Test images in `d`: some of the stage inputs and a bigger synthetic photo."""
    names = ["photo.jpg", "rgba.png", "rgb.avif", "icc.png", "orient_6.jpg", "grey_alpha.png"]
    for n in names:
        shutil.copy(os.path.join(INPUTS, n), d)
    big = parity._grad(1500, 1000, 3)
    Image.fromarray(big).save(os.path.join(d, "Moonforge.png"))
    return [os.path.join(d, n) for n in names + ["Moonforge.png"]]


FORMATS = [("WEBP", {}), ("WEBP", {"webp": {"lossless": False, "q": 80}}), ("PNG", {}),
           ("JPEG", {"jpeg": {"q": 85}}), ("AVIF", {"avif": {"svt": False, "aom_q": 70, "aom_speed": 9}})]
STAGES = [{}, {"resize_on": True, "mode": "Mpix", "mpix": 0.3}, {"rot_q": 1, "angle": 3.5, "flip": True},
          {"wp_on": True, "ge_on": True}]


def jobs_for(paths, quick):
    by_id = {c["id"]: c["job"] for c in parity.load_cases()[0]}
    edits = [by_id.get("combo_19"), by_id.get("chain_2"), dict(by_id.get("sc_all") or {}, sharp=SHARP),
             by_id.get("film_k_portra_160vc"), fx_edit()[0], None]
    jobs = []
    for i, p in enumerate(paths):
        jobs.append({"path": p, "stage": STAGES[i % len(STAGES)], "edit": edits[i % len(edits)]})
    jobs.append({"path": os.path.join(os.path.dirname(paths[0]), "missing.png"), "stage": {}, "edit": None})
    return [jobs[i] for i in (0, 1, 2, 4)] + jobs[-1:] if quick else jobs     # 4: the kit filter


def same_files(a_res, b_res, a_dir, b_dir):
    """The answers equal apart from the folder; the files written byte for byte the same."""
    bad = []
    for ra, rb in zip(a_res, b_res):
        ra = {k: (os.path.relpath(v, a_dir) if k in ("path", "out") else v) for k, v in ra.items()}
        rb = {k: (os.path.relpath(v, b_dir) if k in ("path", "out") else v) for k, v in rb.items()}
        if ra.get("ok") != rb.get("ok") or ra.get("out") != rb.get("out") or ra.get("skipped") != rb.get("skipped"):
            bad.append(f"{os.path.basename(ra['path'])}: {ra} vs {rb}")
        elif ra["ok"]:
            with open(os.path.join(a_dir, ra["out"]), "rb") as f, open(os.path.join(b_dir, rb["out"]), "rb") as g:
                if f.read() != g.read():
                    bad.append(f"{os.path.basename(ra['path'])}: files differ")
    return bad if len(a_res) == len(b_res) else ["not as many answers"]


def check_process(tmp, quick):
    print("process: Rust vs the Python path (files byte for byte)")
    no_gpu = backend.Backend(SETTINGS_PATH, PRESETS_PATH, ENCODERS_DIR, no_gpu=True)
    for engine, b in (("auto", backend.shared()), ("cpu", no_gpu)):
        for fmt, opts in FORMATS[:3] if quick else FORMATS:
            tag = f"{fmt} {json.dumps(opts)} ({'GPU off' if engine == 'cpu' else 'GPU when there is one'})"
            a_dir, b_dir = tempfile.mkdtemp(dir=tmp), tempfile.mkdtemp(dir=tmp)
            ja, jb = jobs_for(inputs(a_dir), quick), jobs_for(inputs(b_dir), quick)
            ref = py_process(ja, fmt, opts, engine)
            prog = []
            got = b.process(jb, fmt, opts, lambda i, n, r: prog.append((i, n, r)))
            bad = same_files(ref, got, a_dir, b_dir)
            check(f"{tag}: {sum(r['ok'] for r in got)} written, 1 failed", not bad and sum(not r["ok"] for r in got) == 1,
                  "; ".join(bad[:3]))
            check(f"{tag}: progress", [(i, n) for i, n, _ in prog] == [(i + 1, len(jb)) for i in range(len(jb))]
                  and [r for _, _, r in prog] == got)
            if engine == "cpu" and fmt == "PNG":
                sk = [r.get("skipped") for r in got if r.get("skipped")]
                check(f"{tag}: the kit filter reported as skipped ({sk[:1]})", bool(sk))
    one = b.process([ja[0]], "PNG", {}, lambda *a: fails.append("progress for a single image"))
    check("a single image: no progress call", len(one) == 1 and one[0]["ok"])
    check("GPU off: gpu_info says so", no_gpu.gpu_info() == {"ok": False, "error": "turned off (--no-gpu)"})


def check_lossy(tmp, quick):
    print("lossy_preview: Rust vs the Python path (answer, preview pixels)")
    b = backend.shared()
    d = tempfile.mkdtemp(dir=tmp)
    paths = inputs(d)
    jobs = jobs_for(paths, quick)[:-1]
    req = 100
    for fmt, opts in FORMATS:
        if fmt == "PNG" or (fmt == "WEBP" and not opts):
            continue
        for job in jobs[:3] if quick else jobs:
            for full in (0, 50_000):
                req += 1
                ref, rgba = py_lossy_preview(job, fmt, opts, req, full)
                got = b.call("lossy_preview", job, fmt, opts, req, full)
                same = {k: got.get(k) for k in ref} == ref and b.preview(got.get("token", "")) == rgba
                name = f"{fmt} {os.path.basename(job['path'])} full={full}"
                check(f"{name}: same answer and pixels", same,
                      "" if same else json.dumps({k: (ref[k], got.get(k)) for k in ref if ref[k] != got.get(k)}))
    # the cache: a size change doesn't encode again (the same time is reported)
    job = jobs[0]
    a = b.call("lossy_preview", job, "JPEG", {}, 900, 0)
    c = b.call("lossy_preview", job, "JPEG", {}, 901, 20_000)
    check("a preview-size change reuses the encode", a["ok"] and c["ok"] and a["ms"] == c["ms"] and c["pw"] < a["pw"])
    # cancellation: a request older than the latest, lossy_cancel
    r = b.call("lossy_preview", job, "WEBP", {"webp": {"lossless": False}}, 899, 0)
    check("an older request is cancelled", r == {"ok": False, "req_id": 899, "cancelled": True})
    check("lossy_cancel", b.call("lossy_cancel", 950) is True
          and b.call("lossy_preview", job, "AVIF", {}, 949, 0).get("cancelled") is True)
    r = b.call("lossy_preview", dict(job, path=os.path.join(d, "missing.jpg")), "JPEG", {}, 960, 0)
    check(f"a missing file: an error ({r.get('error')})", r.get("ok") is False and "error" in r and r["req_id"] == 960)


def check_misc(tmp):
    print("core_info, the shell's fallback decoder")
    i = backend.shared().call("core_info")
    g = i.get("gpu") or {}
    check(f"core_info: CPU core {i.get('version')}, GPU core: {g.get('name') or g.get('error')}",
          i.get("ok") is True and i.get("version") == core.info()["version"] and (not g.get("ok") or g.get("same") is True))
    import fae_backend
    p = os.path.join(tmp, "Mirelle.dat")
    with open(p, "wb") as f:
        f.write(b"not an image at all")
    asked = []

    def fallback(path):
        asked.append(path)
        return bytes(range(36)), 4, 3, 3, b"icc"
    fb = fae_backend.Backend(settings=os.path.join(tmp, "s.json"), presets=os.path.join(tmp, "p.json"), fallback=fallback)
    r = json.loads(fb.call("load_stage", json.dumps([p, {}, 1, 0])))
    px, w, h, ch, icc = fb.stage(p, "{}")
    check("a file Rust can't open: the fallback's pixels", r.get("ok") and (r["w"], r["h"]) == (4, 3)
          and (px, w, h, ch, icc) == (bytes(range(36)), 4, 3, 3, b"icc") and asked[:1] == [p])
    nofb = fae_backend.Backend(settings=os.path.join(tmp, "s.json"), presets=os.path.join(tmp, "p.json"))
    r = json.loads(nofb.call("process", json.dumps([[{"path": p}]])))
    check(f"without a fallback: Process says why ({r[0].get('error')})", r[0]["ok"] is False)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--quick", action="store_true", help="fewer edits, formats and images")
    a = ap.parse_args()
    if not backend.available():
        sys.exit(backend.error())
    if not core.available():
        sys.exit(core.error())
    b = backend.shared()
    print(f"fae_backend {__import__('fae_backend').version()}; CPU core {core.info()['version']}; "
          f"GPU core: {b.gpu_info().get('name') or b.gpu_info().get('error')}")
    tmp = tempfile.mkdtemp(prefix="fae-process-")
    try:
        check_render(b, a.quick)
        check_process(tmp, a.quick)
        check_lossy(tmp, a.quick)
        check_misc(tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("ALL OK" if not fails else f"{len(fails)} FAILED: " + "; ".join(fails[:10]))
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
