"""Where the first full render's wait goes, in the app's back end (fae_backend, with the GPU core built in):
loading it, making the GPU device, then the first and second render of a small image on the GPU core (the
first compiles the shaders it uses), the CPU core next to it.

    python tests/gpu/startup_time.py
"""
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))      # tests/: fastedit

import numpy as np  # noqa: E402

t = time.perf_counter()
from fastedit import backend  # noqa: E402

b = backend.shared()
print(f"load fae_backend                       {time.perf_counter() - t:7.2f} s")

rng = np.random.default_rng(7)
a = rng.integers(0, 256, (256, 256, 3), dtype=np.uint8).tobytes()
edit = {"luts": [list(range(256))] * 3, "point": True, "k": 1.1, "o": -3.0, "gains": [1.02, 0.97, 1.0],
        "sat": 1.3, "md": 0.4, "sharp": {"as": 0.5, "bs": 0.3, "st": 0.4, "lc": 0.2, "fo": -0.3}}

t = time.perf_counter()
info = b.gpu_info()
print(f"GPU device                             {time.perf_counter() - t:7.2f} s  "
      f"{info.get('name') or info.get('error') or ''}")
if info.get("ok"):
    for n in ("first", "second"):
        t = time.perf_counter()
        b.render(a, 256, 256, edit, "gpu")
        print(f"GPU render 256x256, {n:6}             {time.perf_counter() - t:7.2f} s")
    b.gpu_release()
    t = time.perf_counter()
    b.render(a, 256, 256, edit, "gpu")
    print(f"GPU render after release               {time.perf_counter() - t:7.2f} s")
for n in ("first", "second"):
    t = time.perf_counter()
    b.render(a, 256, 256, edit, "cpu")
    print(f"CPU render 256x256, {n:6}             {time.perf_counter() - t:7.2f} s")
