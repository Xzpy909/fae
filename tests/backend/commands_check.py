"""The commands moved to the Rust back end so far, against the Python behaviour they replace
(claude/rust-backend-plan.md). Each wave adds its commands here.

    python tests/backend/commands_check.py

B0: get_settings / save_settings / get_presets / save_presets. The reference is api.py's code before B0
(kept below): the same files must come back the same, edge cases included (missing, broken, not an
object, non-ASCII text, key order, a file written by the old code read by the new and back).
"""
from __future__ import annotations

import json
import os
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, ROOT)

from fastedit import backend  # noqa: E402


# ---- the Python behaviour before B0 (api.py) ------------------------------------------------------------
def py_get_settings(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return {}


def py_save(path, data):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)


def py_get_presets(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


SAMPLES = [
    {},
    {"fmt": "AVIF", "secs": ["adjust", "curves", "fx_test_pixel"], "jpeg_q": 90, "jpeg_d": 1.25, "split": True,
     "stage": {"resize_on": True, "mode": "Mpix", "mpix": 2.0, "prio": "Balanced"}, "collapsed": []},
    {"z_last": 1, "a_first": 2, "m": None, "nested": {"y": [1, 2.5, -3e-7, 1e21], "x": "Mirelle ✓ “quoted” \\ / \u0001"}},
]
PRESETS = [
    {"adjust": [{"name": "Starfall", "values": {"adj_sat": 62, "adj_contrast": 1.15}}],
     "_chain": [{"name": "Moonforge", "chain": [{"id": "adjust", "t": "adjust"}], "values": {}, "off": {}}]},
    {"f_film": [{"name": "Ä é 漢字", "values": {"flt_film_preset": "none"}}]},
]


def main() -> int:
    fails = []

    def check(label, cond):
        print(f"  {'ok  ' if cond else 'FAIL'} {label}")
        if not cond:
            fails.append(label)

    d = tempfile.mkdtemp()
    sp, pp = os.path.join(d, "fae_settings.json"), os.path.join(d, "fae_presets.json")
    b = backend.Backend(sp, pp)
    check("missing settings -> {}", b.call("get_settings") == py_get_settings(sp) == {})
    check("missing presets -> {}", b.call("get_presets") == py_get_presets(pp) == {})
    for i, s in enumerate(SAMPLES):
        check(f"settings {i}: save -> true", b.call("save_settings", s) is True)
        got = b.call("get_settings")
        check(f"settings {i}: read back equal, key order kept", got == s and list(got) == list(s))
        check(f"settings {i}: the old code reads the new file", py_get_settings(sp) == s)
        py_save(sp, s)
        check(f"settings {i}: the new code reads the old file", b.call("get_settings") == s)
    for i, p in enumerate(PRESETS):
        check(f"presets {i}: save -> true", b.call("save_presets", p) is True)
        check(f"presets {i}: read back equal", b.call("get_presets") == p == py_get_presets(pp))
        check(f"presets {i}: no .tmp left", not os.path.exists(pp + ".tmp"))
    for bad, label in ((b"{broken", "broken JSON"), (b"", "empty file"), (b"\xff\xfe\x00", "not UTF-8"), (b"[1, 2]", "a list")):
        for path, get, pyget, name in ((sp, "get_settings", py_get_settings, "settings"), (pp, "get_presets", py_get_presets, "presets")):
            with open(path, "wb") as f:
                f.write(bad)
            check(f"{name}, {label}: same as before ({pyget(path)!r})", b.call(get) == pyget(path))
    # an unwritable place: save_settings still says true (as before), save_presets says false
    b2 = backend.Backend(os.path.join(d, "no", "such", "dir", "s.json"), os.path.join(d, "no", "such", "dir", "p.json"))
    check("save_settings into a missing folder -> true", b2.call("save_settings", {"a": 1}) is True)
    check("save_presets into a missing folder -> false", b2.call("save_presets", {"a": 1}) is False)
    try:
        b.call("load_stage")
        check("a command not moved yet raises", False)
    except RuntimeError as ex:
        check(f"a command not moved yet raises ({ex})", str(ex).startswith("unknown:"))
    print("ALL OK" if not fails else f"{len(fails)} FAILED")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
