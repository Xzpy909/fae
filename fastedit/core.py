"""The Rust core (core/fastedit-py, built by core/build_core.bat): all edit math after the stage.

Process and the lossy compare preview run render(); the live preview runs the WebAssembly build of
the same core (ui/wasm/). There is no NumPy fallback: without the extension the app shows how to
build it (app.py) instead of starting.
"""
from __future__ import annotations

import numpy as np

REQUIRED = (0, 8)          # extension version this file needs (bump with the API)
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


def _args(edit: dict) -> tuple:
    """The edit payload (jobFor in ui/js/process.js) as the extension takes it.
    A missing / None / empty entry means the step is off."""
    luts = [np.asarray(l, dtype=np.uint8) for l in edit.get("luts") or []]
    if len(luts) != 3:
        luts = [np.arange(256, dtype=np.uint8)] * 3
    luts = np.ascontiguousarray(np.stack(luts))
    gains = edit.get("gains")
    gains = None if gains is None else [float(v) for v in np.asarray(gains, dtype=np.float64).reshape(3)]
    wb = edit.get("wb")
    wb = None if wb is None else _f64(wb).reshape(3, 3)
    con = edit.get("con")
    con = None if con is None else _f64(con)
    tones = [(_f64(t["T"]), bool(t.get("ratio"))) for t in (edit.get("tones") or [])]
    boost = edit.get("boost")
    boost = None if boost is None else _f64(boost)
    sat = edit.get("sat")
    sat = None if sat is None else float(sat)
    bw = edit.get("bw")
    if bw is not None:
        w = bw.get("w")
        ps = float(bw["ps"]) if bw.get("ps") else 0.0
        bw = (float(bw["s"]), float(bw["op"]), [float(v) for v in w] if w else None,
              ps, _f64(bw["psk"]) if ps else None)
    film = edit.get("film")
    if film:
        d, t1 = film.get("d"), film.get("t1")
        film = (int(film["n"]), _f64(d) if d else None, _f64(t1) if t1 else None, float(film.get("op", 1)))
    else:
        film = None
    sc = edit.get("sc")
    if sc:
        dyn = float(sc.get("dyn", 0) or 0)
        shl = sc.get("shl")
        sc = (float(sc.get("cast", 0)), float(sc.get("cc", 0)), dyn, float(sc.get("op", 1)),
              _f64(sc["dk"]) if dyn > 0 else None, _f64(shl) if shl else None)
    else:
        sc = None
    return (luts, bool(edit.get("point")), float(edit.get("k", 1.0)), float(edit.get("o", 0.0)),
            gains, wb, con, tones, boost, sat, bw, film, sc)


def render(rgb: np.ndarray, edit: dict | None, hist: bool = False):
    """The full edit as Process writes it (Mid/Detail, then the chain). With hist=True also the
    UI's histograms: (image, hin, hout, hmx, hmn)."""
    fc = _need()
    rgb = np.ascontiguousarray(rgb)
    md = float((edit or {}).get("md") or 0) or None
    sharp = (edit or {}).get("sharp") or {}
    return fc.apply_edit(rgb, *_args(edit or {}), md, hist, structure=float(sharp.get("st") or 0),
                         local_contrast=float(sharp.get("lc") or 0))


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
    out = render(a, edit)
    ok = out.shape == a.shape and out.dtype == np.uint8 and not np.array_equal(out, a)
    return {"ok": bool(ok), **info(), **({} if ok else {"error": "unexpected result"})}
