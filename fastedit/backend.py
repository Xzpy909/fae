"""The Rust back end (core/backend, built by core/backend/build_backend.bat; claude/rust-backend-plan.md).

The work of fastedit/ moves into it wave by wave, behind one command table: api.py forwards the commands
it runs (fae_backend.commands()) and keeps the rest in Python until their wave. Required, like the CPU
core: without it the app shows how to build it (app.py) instead of starting.
"""
from __future__ import annotations

import json

REQUIRED = (0, 1)           # extension version this file needs (bump with the API)
BUILD_HINT = r"run core\backend\build_backend.bat (needs Rust from https://rustup.rs)"

try:
    import fae_backend as _fb
    if tuple(int(v) for v in _fb.version().split(".")[:2]) < REQUIRED:
        raise ImportError(f"fae_backend {_fb.version()} is out of date (need {'.'.join(map(str, REQUIRED))})")
    _ERR = None
except ImportError as ex:
    _fb = None
    _ERR = f"Rust back end: {ex}: {BUILD_HINT}"


def available() -> bool:
    return _fb is not None


def error() -> str | None:
    """Why the back end can't be used, or None."""
    return _ERR


class Backend:
    """One back end for the app: call(name, *args) runs a command of the table (JSON values in and
    out, as the UI sends and receives them). RuntimeError when the command fails."""

    def __init__(self, settings: str, presets: str):
        if _fb is None:
            raise RuntimeError(_ERR)
        self._b = _fb.Backend(settings=settings, presets=presets)

    def call(self, name: str, *args):
        return json.loads(self._b.call(name, json.dumps(list(args))))
