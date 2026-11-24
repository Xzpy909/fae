"""The edit math after the stage: the GPU core (core/gpu, built by core/gpu/build_gpu.bat) and the
CPU core (core/fastedit-py, built by core/build_core.bat).

Process and the lossy compare preview run render(): on the GPU core when it is installed and has a
device (G5 of the GPU core plan), otherwise on the CPU core, with the same result within the
tolerance rule of tests/gpu/tolerance.py. The CPU core is required (the reference, the histograms,
the fallback): without it the app shows how to build it (app.py) instead of starting. The live
preview runs the WebAssembly builds of both cores in the page (ui/gpu/, ui/wasm/).
"""
from __future__ import annotations

import os
import sys
import threading

import numpy as np

REQUIRED = (0, 16)          # extension version this file needs (bump with the API)
BUILD_HINT = r"run core\build_core.bat (needs Rust from https://rustup.rs)"

try:
    import fastedit_core as _fc
    if tuple(int(v) for v in _fc.version().split(".")[:2]) < REQUIRED:
        raise ImportError(f"fastedit_core {_fc.version()} is out of date (need "
                          f"{'.'.join(map(str, REQUIRED))})")
    _ERR = None
except ImportError as ex:          # not built / not installed / too old
    _fc = None
    _ERR = f"{ex}: {BUILD_HINT}"


# ---- the GPU core (optional) -------------------------------------------------------------------
GPU_REQUIRED = (0, 6)       # fae_gpu version this file needs (release())
GPU_BUILD_HINT = r"run core\gpu\build_gpu.bat"

try:
    import fae_gpu as _fg
    if tuple(int(v) for v in _fg.version().split(".")[:2]) < GPU_REQUIRED:
        raise ImportError(f"fae_gpu {_fg.version()} is out of date (need {'.'.join(map(str, GPU_REQUIRED))}): "
                          + GPU_BUILD_HINT)
    _GPU_ERR = None
except ImportError as ex:
    _fg = None
    _GPU_ERR = f"GPU core not installed ({ex})" if "out of date" not in str(ex) else str(ex)

# one device for the whole app, made on first use; Process and the compare preview share it
_gpu = {"dev": None, "info": None, "why": _GPU_ERR, "lock": threading.Lock()}
if "--no-gpu" in sys.argv or os.environ.get("FAE_NO_GPU"):
    _gpu["why"] = "turned off (--no-gpu)"


def _gpu_device():
    """The GPU core's device, or None (_gpu["why"] says why). Call with the lock held."""
    if _gpu["why"] is not None:          # turned off, or failed earlier
        return None
    if _gpu["dev"] is None:
        try:
            _gpu["dev"] = _fg.Gpu()
            _gpu["info"] = dict(_gpu["dev"].info())
        except Exception as ex:
            _gpu["why"] = f"no GPU device: {ex}"
    return _gpu["dev"]


def gpu_info() -> dict:
    """Which core Process uses: {"ok": True, "version", "name", "backend", "max_pixels", ...} or
    {"ok": False, "error"} (not installed, --no-gpu, no device, failed earlier)."""
    with _gpu["lock"]:
        dev = _gpu_device() if _fg is not None else None
        if dev is None:
            return {"ok": False, "error": _gpu["why"]}
        return {"ok": True, "version": _fg.version(), **_gpu["info"]}


def gpu_release():
    """Free the GPU memory of the last image (after a Process batch / a compare preview)."""
    with _gpu["lock"]:
        if _gpu["dev"] is not None:
            try:
                _gpu["dev"].release()
            except Exception as ex:
                _gpu["dev"], _gpu["why"] = None, f"failed: {ex}"


def _gpu_steps(edit: dict) -> list:
    """The edit as the GPU core's steps: the chain, then Sharpen (Process: scale 1)."""
    st = _steps(edit)
    sp = edit.get("sharp") or {}
    vals = [float(sp.get(k) or 0) for k in ("as", "bs", "st", "lc", "fo")]
    if vals[0] > 0 or any(vals[2:]):
        st.append(("sharp", *vals, 1.0))
    return st


def _gpu_render(rgb: np.ndarray, edit: dict):
    """The edit on the GPU core, or None when it can't (then the CPU core renders): not installed,
    turned off, no device, the image too big for this GPU, or an error. After an error the GPU core
    is not used again in this session (a lost device or a driver problem would fail every image)."""
    if _fg is None or _gpu["why"] is not None:
        return None
    with _gpu["lock"]:
        dev = _gpu_device()
        if dev is None:
            return None
        h, w = rgb.shape[:2]
        i = _gpu["info"]
        if w * h > i["max_pixels"] or max(w, h) > i["max_texture_2d"]:
            return None                     # too big for this GPU: the CPU core (no tiling yet)
        try:
            return dev.render(rgb, _gpu_steps(edit))
        except Exception as ex:
            _gpu["dev"], _gpu["why"] = None, f"failed: {type(ex).__name__}: {ex}"
            print(f"GPU core off for this session, Process uses the CPU core: {_gpu['why']}", file=sys.stderr)
            return None


# ---- the CPU core (required) --------------------------------------------------------------------
def available() -> bool:
    return _fc is not None


def error() -> str | None:
    """Why the core can't be used, or None."""
    return _ERR


def info() -> dict:
    if _fc is None:
        return {"ok": False, "error": _ERR}
    return {"ok": True, "version": _fc.version(), "threads": _fc.threads()}


def _need():
    if _fc is None:
        raise RuntimeError(_ERR)
    return _fc


def _f64(a) -> np.ndarray:
    return np.ascontiguousarray(np.asarray(a, dtype=np.float64))


# The tool pill order (docs/redesign-right-panel.md, 8.8): the chain's default when the edit has no
# explicit "steps" list. Crop, Resize and Color balance run before (the stage), Sharpen after.
DEFAULT_ORDER = ("point", "adjust", "curves", "bw", "film", "sc")


def _step(e: dict):
    """One step of the chain as apply_chain takes it, from a dict with the flat edit's entries
    (jobFor in ui/js/process.js) and "t" = the kind. None when the step does nothing."""
    t = e.get("t")
    if t == "point":
        if not e.get("point"):
            return None
        gains = e.get("gains")
        gains = None if gains is None else [float(v) for v in np.asarray(gains, dtype=np.float64).reshape(3)]
        return ("point", float(e.get("k", 1.0)), float(e.get("o", 0.0)), gains)
    if t == "adjust":
        md = float(e.get("md") or 0)
        wb, con, boost, sat = e.get("wb"), e.get("con"), e.get("boost"), e.get("sat")
        tones = [(_f64(x["T"]), bool(x.get("ratio"))) for x in (e.get("tones") or [])]
        if not md and wb is None and con is None and not tones and boost is None and sat is None:
            return None
        return ("adjust", md, None if wb is None else _f64(wb).reshape(3, 3), None if con is None else _f64(con),
                tones, None if boost is None else _f64(boost), None if sat is None else float(sat))
    if t == "curves":
        luts = [np.asarray(x, dtype=np.uint8) for x in e.get("luts") or []]
        if len(luts) != 3:
            return None
        luts = np.ascontiguousarray(np.stack(luts))
        if (luts == np.arange(256, dtype=np.uint8)).all():
            return None                                 # identity: nothing to do
        return ("curves", luts)
    if t == "bw":
        bw = e.get("bw")
        if not bw:
            return None
        w = bw.get("w")
        ps = float(bw["ps"]) if bw.get("ps") else 0.0
        return ("bw", float(bw["s"]), float(bw["op"]), [float(v) for v in w] if w else None,
                ps, _f64(bw["psk"]) if ps else None)
    if t == "film":
        film = e.get("film")
        if not film:
            return None
        d, t1 = film.get("d"), film.get("t1")
        return ("film", int(film["n"]), _f64(d) if d else None, _f64(t1) if t1 else None, float(film.get("op", 1)))
    if t == "sc":
        sc = e.get("sc")
        if not sc:
            return None
        dyn = float(sc.get("dyn", 0) or 0)
        shl = sc.get("shl")
        return ("sc", float(sc.get("cast", 0)), float(sc.get("cc", 0)), dyn, float(sc.get("op", 1)),
                _f64(sc["dk"]) if dyn > 0 else None, _f64(shl) if shl else None)
    raise ValueError(f"unknown step {t!r}")


def _steps(edit: dict) -> list:
    """The edit as the ordered list apply_chain takes. edit["steps"] lists the steps in panel order
    (each a dict with "t" and that tool's entries); without it the flat edit runs in DEFAULT_ORDER."""
    specs = edit.get("steps")
    if specs is None:
        specs = [dict(edit, t=k) for k in DEFAULT_ORDER]
    return [s for s in (_step(sp) for sp in specs) if s is not None]


def render(rgb: np.ndarray, edit: dict | None, hist: bool = False, engine: str = "auto"):
    """The full edit as Process writes it (the chain of steps, then Sharpen). engine: "auto" = the
    GPU core when it can (see _gpu_render), else the CPU core; "cpu" / "gpu" = only that one ("gpu"
    raises when the GPU core can't). With hist=True also the UI's histograms, from the CPU core:
    (image, hin, hout, hmx, hmn)."""
    rgb = np.ascontiguousarray(rgb)
    if not hist and engine != "cpu":
        out = _gpu_render(rgb, edit or {})
        if out is not None:
            return out
        if engine == "gpu":
            raise RuntimeError(f"GPU core can't render this: {_gpu['why'] or 'image too big for this GPU'}")
    fc = _need()
    sharp = (edit or {}).get("sharp") or {}
    return fc.apply_chain(rgb, _steps(edit or {}), hist, adaptive=float(sharp.get("as") or 0),
                          balance=float(sharp.get("bs") or 0), structure=float(sharp.get("st") or 0),
                          local_contrast=float(sharp.get("lc") or 0), focus=float(sharp.get("fo") or 0))


def selftest() -> dict:
    """The extension loads and runs (used by the self-test page)."""
    if _fc is None:
        return {"ok": False, "error": _ERR}
    rng = np.random.default_rng(7)
    a = rng.integers(0, 256, (257, 131, 3), dtype=np.uint8)
    edit = {"luts": [rng.integers(0, 256, 256).tolist() for _ in range(3)], "point": True, "k": 1.1, "o": -3.0,
            "gains": [1.02, 0.97, 1.0], "sat": 1.3, "md": 0.4,
            "bw": {"s": 1.2, "op": 0.7, "w": [1.05, 1.0, 0.95], "ps": 0.5, "psk": [0.25, 0.5, 0.25]},
            "sc": {"cast": 0.4, "cc": 35, "dyn": 0.5, "op": 0.8, "dk": [0.2, 0.6, 0.2], "shl": None}}
    edit["sharp"] = {"as": 0.5, "bs": 0.3, "st": 0.4, "lc": 0.2, "fo": -0.3}
    out = render(a, edit, engine="cpu")
    ok = out.shape == a.shape and out.dtype == np.uint8 and not np.array_equal(out, a)
    gpu = gpu_info()
    if gpu["ok"]:                   # the GPU core on the same edit, by the tolerance rule's chain part
        try:
            d = np.abs(render(a, edit, engine="gpu").astype(np.int16) - out.astype(np.int16))
            gpu.update(max_diff=int(d.max()), mean_diff=round(float(d.mean()), 4),
                       same=bool(d.mean() < 0.05 and (d.max(axis=2) > 2).mean() <= 1e-3))
        except Exception as ex:
            gpu = {"ok": False, "error": f"{type(ex).__name__}: {ex}"}
    return {"ok": bool(ok), **info(), "gpu": gpu, **({} if ok else {"error": "unexpected result"})}
