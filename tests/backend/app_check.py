"""B6 (docs/rust-backend-plan.md): the app plumbing in the Rust back end (fae-backend app.rs), against the
Python it replaces (engine.py collect and app.py's HTTP handler before B6, kept in tests/pyref/app.py).

    python tests/backend/app_check.py

- file lists: initial_files / expand_paths (and Backend.collect, which the dialogs, Ctrl+V and drag-drop
  use) give the same list as collect() did: folders walked in order, extensions in any case, files that
  aren't images, missing paths, links, non-ASCII names;
- the server's answers: every UI file (bytes and type), index, 404s, paths that try to leave the UI
  folder, preview pixels, thumbnails (the bytes the B2 thumbnail gives; the shell's decoder for a file
  Rust can't open; a changed file is made again), a thumbnail that fails;
- the real server on a port: status lines, headers (Content-Length, Cache-Control, the cross-origin
  isolation ones on and off), only GET, many requests at once.
"""
from __future__ import annotations

import concurrent.futures as cf
import http.client
import os
import shutil
import sys
import tempfile
import time
from urllib.parse import quote

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "tests"))           # fastedit (the back end from Python), pyref (the Python versions it replaced)

from PIL import Image  # noqa: E402

import fae_backend  # noqa: E402
from fastedit import backend  # noqa: E402
from fastedit.config import UI_DIR  # noqa: E402
from pyref import app as ref  # noqa: E402

INPUTS = os.path.join(ROOT, "tests", "stage", "inputs")

fails: list[str] = []


def check(name, ok, extra=""):
    print(f"  {'ok  ' if ok else 'FAIL'} {name}{('  ' + extra) if extra else ''}")
    if not ok:
        fails.append(name)


def make_tree(d):
    """Folders and files to collect: nested, mixed case, not images, non-ASCII, links."""
    def touch(*parts, data=b"x"):
        p = os.path.join(d, *parts)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data)
        return p
    touch("Starfall", "b.png")
    touch("Starfall", "A.JPG")
    touch("Starfall", "a.jpeg")
    touch("Starfall", "notes.txt")
    touch("Starfall", ".png")                       # no extension: not an image
    touch("Starfall", "noext")
    touch("Starfall", "page 10.webp")
    touch("Starfall", "page 2.webp")
    touch("Starfall", "Ch 1", "01.avif")
    touch("Starfall", "ch 0", "z.Tiff")
    touch("Starfall", "_x", "y.gif")
    touch("Starfall", "Zeta", "deep", "deeper", "k.bmp")
    touch("Moonforge", "Mirelle é ü 星.png")
    touch("Moonforge", "Ärger.HEIC")
    touch("Moonforge", "cover.jxl")
    touch("Moonforge", "x.png.bak")
    os.makedirs(os.path.join(d, "Empty"), exist_ok=True)
    links = []
    try:    # links: os.walk lists a link to a folder but doesn't go into it; a link to a file is a file
        os.symlink(os.path.join(d, "Moonforge"), os.path.join(d, "Starfall", "link_dir"), target_is_directory=True)
        os.symlink(os.path.join(d, "Moonforge", "cover.jxl"), os.path.join(d, "Starfall", "link.png"))
        links = ["link_dir", "link.png"]
    except (OSError, NotImplementedError):
        pass
    return links


def check_collect(b, d):
    print("file lists (initial_files / expand_paths)")
    links = make_tree(d)
    sf, mf = os.path.join(d, "Starfall"), os.path.join(d, "Moonforge")
    cases = [
        ("a folder", [sf]),
        ("two folders and a file", [mf, sf, os.path.join(sf, "b.png")]),
        ("files, a text file, a missing path, empty", [os.path.join(sf, "A.JPG"), os.path.join(sf, "notes.txt"),
                                                     os.path.join(d, "missing.png"), "", os.path.join(mf, "Ärger.HEIC")]),
        ("the same folder twice", [mf, mf]),
        ("an empty folder", [os.path.join(d, "Empty")]),
        ("a folder with a trailing separator", [sf + os.sep]),
        ("the parent of everything", [d]),
        ("nothing", []),
    ]
    if links:
        cases.append(("links", [os.path.join(sf, "link_dir"), os.path.join(sf, "link.png")]))
    else:
        print("  (no links: this system doesn't let the test make them)")
    for name, paths in cases:
        want = ref.collect(paths)
        got = b.call("expand_paths", paths)
        check(f"expand_paths, {name}: {len(want)} images", got == want, "" if got == want else f"{got} != {want}")
    check("Backend.collect (dialogs, Ctrl+V, drag-drop) = expand_paths",
          b.collect([sf, None, mf]) == ref.collect([sf, mf]))
    check("expand_paths: entries that aren't text are left out",
          b.call("expand_paths", [sf, 3, None]) == ref.collect([sf]))
    check("initial_files before any: []", b.call("initial_files") == [])
    b.set_initial([sf, os.path.join(d, "missing.png")])
    check("initial_files: the command line's paths, collected", b.call("initial_files") == ref.collect([sf]))
    b.set_initial([])


def check_serve(b, d):
    print("the server's answers")
    tb = b._b

    def ref_thumb(path):
        return tb.thumb(path, 224)                  # engine.make_thumb before B6: the B2 thumbnail
    files = []
    for root, dirs, names in os.walk(UI_DIR):
        dirs.sort()
        files += [os.path.relpath(os.path.join(root, n), UI_DIR).replace(os.sep, "/") for n in sorted(names)]
    bad = [f for f in files if b.serve("/" + quote(f)) != ref.serve("/" + quote(f), UI_DIR, b.preview, ref_thumb)]
    check(f"every UI file ({len(files)}): same bytes and type", not bad, ", ".join(bad[:5]))
    targets = ["/", "", "/index.html", "/index.html?gpu=0", "/?gpu=0&fxtest=1", "/selftest.html", "/js/../index.html",
               "/nope.js", "/js", "/../README.md", "/js/../../README.md", "/%2e%2e/README.md",
               "/..%2fREADME.md", "/%2E%2E%5CREADME.md", "//etc/passwd", "/preview/" + "0" * 32,
               "/preview/", "/thumb", "/thumb?p=", "/index.html#top"]
    for t in targets:
        got, want = b.serve(t), ref.serve(t, UI_DIR, b.preview, ref_thumb)
        same = got[:3] == want[:3] and (want[0] == 500 or got[3] == want[3])     # an error's text may differ
        check(f"GET {t or '(empty)'}: {want[0]} {want[1]}", same, "" if same else f"{got[:3]} != {want[:3]}")
    for t in ("/../README.md", "/%2e%2e/README.md", "/..%2fREADME.md", "/%2E%2E%5CREADME.md"):
        check(f"nothing outside the UI folder: {t}", b.serve(t)[0] == 404)
    px = bytes(range(256)) * 4
    tok = b.put_preview(px)
    check("preview pixels by token", b.serve("/preview/" + tok) == (200, "application/octet-stream", False, px))
    # thumbnails: the B2 thumbnail's bytes, by a quoted path (spaces, non-ASCII, + and & in the name)
    for n in ("photo.jpg", "orient_6.jpg", "rgba.png", "rgb.avif", "palette.gif", "cmyk.jpg"):
        src = os.path.join(INPUTS, n)
        if not os.path.isfile(src):
            continue
        p = os.path.join(d, f"Mirelle é + & {n}")
        shutil.copy(src, p)
        for q in (quote(p), quote(p, safe="").replace("%20", "+")):
            got = b.serve("/thumb?v=1&p=" + q)
            want = ref.serve("/thumb?v=1&p=" + q, UI_DIR, b.preview, ref_thumb)
            check(f"thumbnail {n} ({'+' if '+' in q else '%20'} for spaces)", got == want and got[0] == 200)
    # a changed file is made again; the cache answers the same file
    p = os.path.join(d, "changes.png")
    Image.new("RGB", (300, 200), (200, 30, 30)).save(p)
    first = b.serve("/thumb?p=" + quote(p))[3]
    check("a thumbnail again: the cached bytes", b.serve("/thumb?p=" + quote(p))[3] == first)
    Image.new("RGB", (300, 200), (30, 30, 200)).save(p)
    st = os.stat(p)
    os.utime(p, ns=(st.st_atime_ns, st.st_mtime_ns + 2_000_000_000))
    second = b.serve("/thumb?p=" + quote(p))[3]
    check("a changed file: a new thumbnail", second != first and second == tb.thumb(p, 224))
    st, ct, cache, body = b.serve("/thumb?p=" + quote(os.path.join(d, "missing.png")))
    want = ref.serve("/thumb?p=" + quote(os.path.join(d, "missing.png")), UI_DIR, b.preview, ref_thumb)
    check(f"a missing file's thumbnail: 500 ({body.decode(errors='replace')})", (st, ct, cache) == want[:3])
    # a file Rust can't open: the shell's decoder (Pillow), then the same thumbnail as Rust's own pixels
    odd = os.path.join(d, "Mirelle.dat")
    with open(odd, "wb") as f:
        f.write(b"not an image at all")
    img = Image.radial_gradient("L").convert("RGB").resize((400, 300))
    png = os.path.join(d, "same.png")
    img.save(png)
    fb = fae_backend.Backend(settings=os.path.join(d, "s.json"), presets=os.path.join(d, "p.json"),
                             fallback=lambda path: (img.tobytes(), 400, 300, 3, None), ui_dir=UI_DIR)
    got = fb.serve("/thumb?p=" + quote(odd))
    check("a file Rust can't open: the shell's pixels, the B2 thumbnail", got == (200, "image/jpeg", True, fb.thumb(png, 224)))
    nofb = fae_backend.Backend(settings=os.path.join(d, "s.json"), presets=os.path.join(d, "p.json"))
    check(f"... without the shell's decoder: 500 ({nofb.serve('/thumb?p=' + quote(odd))[3].decode(errors='replace')})",
          nofb.serve("/thumb?p=" + quote(odd))[0] == 500)
    check("no UI folder given: 404", nofb.serve("/index.html")[0] == 404)


def get(port, target, method="GET"):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    c.request(method, target)
    r = c.getresponse()
    body = r.read()
    c.close()
    return r.status, {k.lower(): v for k, v in r.getheaders()}, body


def check_server(b, d):
    print("the real server")
    coi = {"cross-origin-opener-policy": "same-origin", "cross-origin-embedder-policy": "require-corp",
           "cross-origin-resource-policy": "same-origin"}
    port = b.start_server(True)
    st, h, body = get(port, "/")
    with open(os.path.join(UI_DIR, "index.html"), "rb") as f:
        index = f.read()
    check(f"GET / on port {port}: 200, index.html", st == 200 and body == index)
    ok = (h.get("content-type") == "text/html; charset=utf-8" and h.get("content-length") == str(len(index))
          and h.get("cache-control") == "no-store")
    check("  Content-Type, Content-Length, Cache-Control: no-store", ok, "" if ok else str(h))
    check("  the cross-origin isolation headers", all(h.get(k) == v for k, v in coi.items()))
    p = os.path.join(d, "served.jpg")
    shutil.copy(os.path.join(INPUTS, "photo.jpg"), p)
    st, h, body = get(port, "/thumb?p=" + quote(p))
    check("GET /thumb: 200 image/jpeg, max-age=3600, the bytes", st == 200 and h.get("content-type") == "image/jpeg"
          and h.get("cache-control") == "max-age=3600" and body == b._b.thumb(p, 224))
    tok = b.put_preview(b"\x01\x02\x03\x04" * 1000)
    st, h, body = get(port, "/preview/" + tok)
    check("GET /preview/<token>: the pixels", st == 200 and body == b"\x01\x02\x03\x04" * 1000)
    st, h, body = get(port, "/nope.js")
    check(f"GET /nope.js: 404 ({body!r})", st == 404 and body == b"not found")
    st, h, body = get(port, "/../README.md")
    check("GET /../README.md: 404", st == 404)
    st, h, body = get(port, "/", "POST")
    check(f"POST /: 501 ({body!r})", st == 501)
    js = [f for f in sorted(os.listdir(os.path.join(UI_DIR, "js"))) if f.endswith(".js")]
    t = time.perf_counter()
    with cf.ThreadPoolExecutor(16) as ex:
        res = list(ex.map(lambda f: get(port, "/js/" + f), js * 4))
    ok = all(r[0] == 200 and r[2] == open(os.path.join(UI_DIR, "js", f), "rb").read() for r, f in zip(res, js * 4))
    check(f"{len(res)} requests at once ({(time.perf_counter() - t) * 1000:.0f} ms)", ok)
    port2 = b.start_server(False)
    st, h, _ = get(port2, "/")
    check("FASTEDIT_COI=0: no isolation headers", st == 200 and not any(k in h for k in coi))


def main():
    if not backend.available():
        print(backend.error())
        return 1
    print(f"fae_backend {fae_backend.version()}")
    d = tempfile.mkdtemp(prefix="fae_app_")
    try:
        b = backend.Backend(os.path.join(d, "s.json"), os.path.join(d, "p.json"), ui_dir=UI_DIR)
        check_collect(b, os.path.join(d, "tree"))
        check_serve(b, d)
        check_server(b, d)
    finally:
        shutil.rmtree(d, ignore_errors=True)
    print("ALL OK" if not fails else f"{len(fails)} FAILED")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
