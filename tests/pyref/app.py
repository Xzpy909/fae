"""The app plumbing as it was in Python before B6 (engine.py collect and the Pillow thumbnail): the
reference for tests/backend/app_check.py and the stage tests. The app itself uses fae-backend app.rs."""
from __future__ import annotations

import io
import os
from PIL import Image, ImageOps
from fastedit.config import IMAGE_EXTS, THUMB_SIZE
from fastedit.engine import open_image


def collect(paths) -> list[str]:
    files = []
    for p in paths or []:
        if not p:
            continue
        if os.path.isdir(p):
            for root, dirs, names in os.walk(p):
                dirs.sort()
                for n in sorted(names):
                    if os.path.splitext(n)[1].lower() in IMAGE_EXTS:
                        files.append(os.path.join(root, n))
        elif os.path.isfile(p) and os.path.splitext(p)[1].lower() in IMAGE_EXTS:
            files.append(p)
    return files


def make_thumb_pillow(path: str) -> bytes:
    """Pillow's file-list thumbnail (the Rust back end makes the same pixels since B2)."""
    with open_image(path) as im:
        im.draft("RGB", (THUMB_SIZE * 2, THUMB_SIZE * 2))
        im = ImageOps.exif_transpose(im).convert("RGB")
        im.thumbnail((THUMB_SIZE * 2, THUMB_SIZE * 2), Image.Resampling.BILINEAR)
        buf = io.BytesIO()
        im.save(buf, "JPEG", quality=82)
        return buf.getvalue()


# ---- app.py's HTTP handler before B6 (do_GET), as a function: (status, content type, cacheable, body) ----
MIME = {".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8",
        ".css": "text/css; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png",
        ".wasm": "application/wasm"}


def serve(target: str, ui_dir: str, preview, thumb):
    """`preview(token)` -> bytes or None, `thumb(path)` -> JPEG bytes (raises on failure)."""
    from urllib.parse import parse_qs, unquote, urlparse
    u = urlparse(target)
    p = unquote(u.path)
    try:
        if p.startswith("/preview/"):
            data = preview(p[len("/preview/"):])
            if data is None:
                return 404, "text/plain", False, b"gone"
            return 200, "application/octet-stream", False, data
        if p == "/thumb":
            path = parse_qs(u.query).get("p", [""])[0]
            return 200, "image/jpeg", True, thumb(path)
        rel = "index.html" if p in ("/", "") else p.lstrip("/")
        full = os.path.normpath(os.path.join(ui_dir, rel))
        if not full.startswith(os.path.normpath(ui_dir)) or not os.path.isfile(full):
            return 404, "text/plain", False, b"not found"
        with open(full, "rb") as f:
            body = f.read()
        return 200, MIME.get(os.path.splitext(full)[1], "application/octet-stream"), False, body
    except Exception as ex:
        return 500, "text/plain", False, str(ex).encode()
