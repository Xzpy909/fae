"""build.bat: copy libheif (+ libde265 and the other libraries next to it) from pillow-heif into the
app's heif folder, where Fae.exe looks for them (HEIC). Nothing to copy: HEIC files can't be opened.

    python core/backend/fae-app/copy_heif.py <dest>
"""
import importlib.util
import os
import shutil
import sys


def heif_dir():
    """pillow-heif's libraries: Linux / macOS wheels keep them in site-packages/pillow_heif.libs, the
    Windows wheel puts them straight into site-packages."""
    try:
        spec = importlib.util.find_spec("pillow_heif")
    except (ImportError, ValueError):
        return None
    if not spec or not spec.origin:
        return None
    site = os.path.dirname(os.path.dirname(spec.origin))
    for d in (os.path.join(site, "pillow_heif.libs"), site):
        try:
            if any(n.lower().startswith(("libheif", "heif")) for n in os.listdir(d)):
                return d
        except OSError:
            pass
    return None


src, dest = heif_dir(), sys.argv[1]
if not src:
    print("HEIC: libheif not found (it comes with pillow-heif: pip install pillow-heif); HEIC files can't be opened")
    sys.exit(0)
libs = [n for n in os.listdir(src) if n.lower().endswith((".dll", ".dylib")) or ".so" in n]
os.makedirs(dest, exist_ok=True)
for n in libs:
    shutil.copy2(os.path.join(src, n), os.path.join(dest, n))
print(f"HEIC: {len(libs)} libraries from pillow-heif copied")
