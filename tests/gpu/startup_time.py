"""Where the first full render's wait goes: loading fae_gpu, making the device, then the first and second
render of a small image on the GPU core (the first compiles the shaders it uses), the CPU core next to it.

    python tests/gpu/startup_time.py
"""
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))

import numpy as np  # noqa: E402

t = time.perf_counter()
from fastedit import core  # noqa: E402

print(f"import fastedit.core (loads fae_gpu)   {time.perf_counter() - t:7.2f} s")

rng = np.random.default_rng(7)
a = rng.integers(0, 256, (256, 256, 3), dtype=np.uint8)
edit = {"luts": [list(range(256))] * 3, "point": True, "k": 1.1, "o": -3.0, "gains": [1.02, 0.97, 1.0],
        "sat": 1.3, "md": 0.4, "sharp": {"as": 0.5, "bs": 0.3, "st": 0.4, "lc": 0.2, "fo": -0.3}}

t = time.perf_counter()
info = core.gpu_info()
print(f"GPU device                             {time.perf_counter() - t:7.2f} s  "
      f"{info.get('adapter') or info.get('name') or info.get('error') or ''}")
if info.get("ok"):
    for n in ("first", "second"):
        t = time.perf_counter()
        core.render(a, edit, engine="gpu")
        print(f"GPU render 256x256, {n:6}             {time.perf_counter() - t:7.2f} s")
    core.gpu_release()
    t = time.perf_counter()
    core.render(a, edit, engine="gpu")
    print(f"GPU render after release               {time.perf_counter() - t:7.2f} s")
for n in ("first", "second"):
    t = time.perf_counter()
    core.render(a, edit, engine="cpu")
    print(f"CPU render 256x256, {n:6}             {time.perf_counter() - t:7.2f} s")
