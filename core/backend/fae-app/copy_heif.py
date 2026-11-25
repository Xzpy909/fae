"""build_app.bat: copy libheif (+ libde265 and the other libraries next to it) from pillow-heif into the
app's heif folder, where Fae.exe looks for them (HEIC). Nothing to copy: HEIC files can't be opened.

    python core/backend/fae-app/copy_heif.py <dest>
"""
import os
import shutil
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
sys.path.insert(0, ROOT)
from fastedit.backend import heif_dir  # noqa: E402  (pillow-heif's libraries, as the pywebview shell finds them)

src, dest = heif_dir(), sys.argv[1]
if not src:
    print("HEIC: libheif not found (it comes with pillow-heif: pip install pillow-heif); HEIC files can't be opened")
    sys.exit(0)
libs = [n for n in os.listdir(src) if n.lower().endswith((".dll", ".dylib")) or ".so" in n]
os.makedirs(dest, exist_ok=True)
for n in libs:
    shutil.copy2(os.path.join(src, n), os.path.join(dest, n))
print(f"HEIC: {len(libs)} libraries from pillow-heif copied")
