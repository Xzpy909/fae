# Fae: building, tests and internals

How to build Fae.exe, run the tests, and how the cores fit together. Back to the [README](../README.md).

## Requirements

- Windows 10/11 with the WebView2 runtime (part of Windows; the bundled encoders are Windows builds)
- To build: Rust from https://rustup.rs (MSVC build tools) and Python 3.10+ (the WebAssembly build script, and
  pillow-heif's libheif for HEIC): `pip install -r requirements.txt`
- The tests: `pip install -r tests/requirements.txt` (Pillow and NumPy, the references)

HEIC / HEIF open through the libheif that comes with `pillow-heif` (`build.bat` copies it into `heif\` next to
`Fae.exe`); JPEG XL, AVIF and the rest are built in.

### Encoders

The encoder binaries are **not in the repository**. They come with the [release](../../../releases) zip. Put them in the `encoders/` folder:

```
encoders/
├── cjpegli.exe        # jpegli, from libjxl
└── SvtAv1EncApp.exe   # SVT-AV1 (SVT-AV1-HDR fork)
```

Both are optional:
- Without `cjpegli.exe`, JPEG uses libjpeg-turbo, which has no distance mode.
- Without `SvtAv1EncApp.exe`, AVIF uses rav1e (built in).

The app shows which encoder is in use.

## Building and running

```
build.bat                             # -> dist\Fae\Fae.exe (with ui\, encoders\, heif\ next to it); zip dist\Fae
dist\Fae\Fae.exe [images or folders...]
dist\Fae\Fae.exe --debug              # opens the web inspector
dist\Fae\Fae.exe --selftest           # checks the window (cross-origin isolation, WASM, threads) and the cores
dist\Fae\Fae.exe --native-frame       # normal Windows title bar instead of the app's own header
dist\Fae\Fae.exe --no-gpu             # live preview and Process without the GPU core
core\backend\target\release\Fae.exe   # a build also runs from the repository: its ui\ and settings
```

Settings are saved to `fae_settings.json` next to `Fae.exe`, your presets to `fae_presets.json`. Not yet: the
few AVIF files the Rust decoder can't open (grid images, premultiplied alpha).

## Rust core

All edit math runs in one Rust crate (`core/`), built twice: linked into the Rust back end (Process, lossy
compare preview; the Python extension `fastedit_core` is the tests' reference) and as WebAssembly (`ui/wasm/`, the live preview, multithreaded in
its own worker: `ui/js/preview-worker.js`). The UI only computes the tables the sliders stand for
(`ui/js/edit-math.js`); there is no NumPy or JS fallback, so the core has to be built:

```
core\build_core.bat        # needs Rust from https://rustup.rs (MSVC build tools)
```

`build.bat` runs it too. The Rust version is pinned in `core/rust-toolchain.toml` (the threaded
WebAssembly build rebuilds Rust's standard library; the script adds the `rust-src` component itself).
Without `ui/wasm/` the preview shows the unedited image and says so. `FASTEDIT_COI=0` turns off the cross-origin isolation headers the
threaded WebAssembly needs.

### Tests

Both builds must give exactly the stored pixels and histograms (needs node). Run from the project
folder:

```
python tests/parity/parity.py check --impl core   # Python extension
python tests/parity/parity.py check --impl wasm   # WebAssembly (single-threaded in node)
python tests/parity/parity.py record              # only after a deliberate change of the math
python tests/bench/bench_python.py                # full-resolution timings
Fae.exe --selftest                  # WebView2 checks + live-preview timings
```

Cases are random slider values run through the UI's own `computeEdit` / `jobFor`, stored in
`tests/parity/cases.json.gz` by `record` so every machine checks the same inputs (`parity.py tables`
shows whether this machine's JS computes the tables differently). The test images are synthetic.
Own images can go into `tests/parity/images/` (not committed).

### Rust back end

The Python back end moved to Rust wave by wave (`core/backend/`, plan in `docs/rust-backend-plan.md`): one
table of commands, run by the app (`core/backend/fae-app`, the Tauri shell: window, dialogs, drag-drop,
Ctrl+V) and by the tests through the Python extension `fae_backend` (`tests/fastedit/`). `ui/js/bridge.js` is
the only UI file that knows the shell. In Rust: settings, presets, opening images (with the same pixels
Pillow gave, AVIF through rav1d; HEIC through the libheif that comes with pillow-heif; with
[nasm](https://nasm.us) installed, the builds use libjpeg-turbo's faster SIMD code), crop / rotate / flip,
resizing, the file-list thumbnails, White-Patch / Gray-Edge and the whole stage with its cache and previews
(Pillow's, OpenCV's and NumPy's maths ported, multi-threaded), the encoders (PNG, WebP, JPEG, AVIF through
SvtAv1EncApp or rav1e, saving), Process and the encoded compare (the back end links the CPU core and the GPU
core as crates), file lists and serving the page. Before a part moved, its Python results were stored as
goldens; the Rust version must match them (exactly, or within the tolerance rule for the parts that
re-implement OpenCV maths). The Python versions it replaced stay in `tests/pyref/` as the references:

```
core\backend\build_backend.bat           # Python extension fae_backend, for the tests
cd core\backend && cargo test && cargo test -p fae-app
python tests/stage/stage_check.py        # decode, geometry, resize, colour balance, the stage vs the goldens
                                         #   (--impl python: Pillow's decode / thumbnails; --own: own photos too)
python tests/backend/commands_check.py   # the commands vs their Python behaviour
python tests/backend/encoders_check.py   # the encoders and the AVIF decoder vs Pillow / the Python versions
python tests/backend/process_check.py    # Process and the encoded compare vs the Python path (tests/pyref)
python tests/backend/app_check.py        # file lists and the local HTTP server vs the Python versions (tests/pyref)
python tests/stage/stage_bench.py        # timing of the stage in Rust (Pillow next to it where it still does the job)
```

Plan, status and how the waves were done: `docs/rust-backend-plan.md`.

### GPU core (in progress)

The edit math is moving to the GPU (wgpu + WGSL shaders, `core/gpu/`), in waves. The GPU core runs
every tool (Point correction, Adjustments with Mid/Detail, Curves, Perceptual Saturation, Film Effect,
Super Contrast, Sharpen) and counts the histograms. The live preview uses it whenever WebGPU is
available (`ui/js/gpu-view.js`; the status bar says which core draws): it renders the image at full
resolution and draws it at exactly the screen's pixels on every change, pan and zoom, and the
histograms follow a dragged slider. Without a GPU the CPU core renders a copy of at most 2560 px on
the long side (zooming in shows it scaled up); crop mode and the preset thumbnails always use the
CPU core (small images). The GPU core compiles all its shaders once at start-up, behind the loading screen
(`ui/js/startup.js`; `Fae.exe --debug` logs how long each start-up step took). Process (and the encoded compare preview) renders on the GPU core too
(built into the Rust back end) when there is a device; otherwise, with `--no-gpu`, for an image too big for the GPU, or after a GPU error,
the CPU core renders it, with the same result within the tolerance rule. The GPU computes in f32 (the CPU core in f64): 99.99 % of the
values match exactly, the rest are 1 level off with rare outliers (`tests/gpu/tolerance.py` explains
the rule).

New filters are made with the filter kit (`docs/filter-kit.md`): a descriptor and a shader in
`core/gpu/fae-gpu/filters/`; the island, the Filters tile, the chain, presets, the preview and Process
follow from them. Kit filters run on the GPU only: without one they are skipped, and the status bar
and Process say so.

```
core\gpu\build_gpu.bat             # Python extension fae_gpu + WebAssembly in ui/gpu/
python tests/gpu/tolerance.py       # GPU vs CPU core on the parity cases + Sharpen cases (--trace: where outliers
                                    #   start, --steps: how often each step on its own differs)
python tests/gpu/bench_native.py    # speed, DirectX 12 and Vulkan (--warp: software renderer)
python tests/gpu/process_check.py   # Process on the GPU vs the CPU core, own photos at full size
Fae.exe --gpu-spike                 # WebGPU in the app window: limits, speed of the WebAssembly build
Fae.exe --no-gpu                    # live preview and Process on the CPU core only
python tests/gpu/fx_check.py        # the filter kit's filters against their stored results (--record ID)
Fae.exe --test-filters              # the kit's hidden test filters in the Filters list
```

## Project layout

```
ui/                    # the app's page
├── index.html
├── style.css
└── js/                # bridge (the shell), state, film-tables, film/*, edit-math, wasm-preview, render, compare, stage,
    │                  # view, gpu-view, curves, controls, files, tools, presets, instances, chain-presets, process,
    │                  # winchrome, init, crop
    └── film/          # one file per Film Effect preset (33×33×33 colour table)
core/                  # Rust core: fastedit-core (math), fastedit-py (Python, the tests), fastedit-wasm (live preview)
core/gpu/              # GPU core: fae-gpu (wgpu + WGSL), its Python extension and WebAssembly build
core/backend/          # Rust back end: fae-backend (every command; rav1e from github.com/Nukvah/rav1e-fae),
                       #   fae-backend-py (Python extension fae_backend, the tests)
core/backend/fae-app/  # Fae.exe: the Tauri shell (window, the fae protocol for the page, dialogs, drag-drop, Ctrl+V)
tests/fastedit/        # the back end from Python (paths, Pillow's decode as its fallback) for the tests
tests/pyref/           # the Python versions the Rust back end replaced: the tests' references
tests/parity/          # bit-exactness tests of both core builds against stored goldens
tests/stage/, tests/backend/, tests/gpu/, tests/bench/
encoders/              # cjpegli.exe, SvtAv1EncApp.exe (from the release)
build.bat              # dist\Fae\Fae.exe
requirements.txt       # building and the tests (the app needs no Python)
```
