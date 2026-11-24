# GPU core (wgpu): plan

Status: **planned** (2026-11-23). Replaces the CPU core (`core/fastedit-core`, run as WebAssembly for the
live preview and as a Python extension for Process) with one GPU implementation. Built in waves; the app
keeps working after every wave.

## 1. Why now

- **Speed:** between fit and 1:1 on big photos the preview renders tens of megapixels per frame on the CPU
  (WebAssembly is ~2× slower than native; native is still limited by memory bandwidth). A desktop GPU does
  the same work 10–50× faster; dragging any slider at any zoom on a 50 MP photo becomes fluid.
- **Before the filter count grows:** every filter written for the CPU core now would have to be rewritten.
  Moving while there are ~10 tools costs the least it ever will.
- **Simpler app:** most of the preview machinery exists only because the CPU is slow — the small / full-
  resolution swap, zoomed-in drafts, the screen-resolution redraw workers, "Mid/Detail only on the first
  Adjustments". With a GPU most of it can go (wave G6).

## 2. How it fits together

- **Rust + wgpu = the host:** sets up the GPU, uploads the image, runs the chain as a list of GPU passes,
  reads results back. wgpu is a Rust library that talks to the graphics card through DirectX 12 / Vulkan
  natively, and through **WebGPU** inside the browser (WebView2 is Chromium, which has WebGPU).
- **WGSL shaders = the filter math:** each filter is one or more small GPU programs. The **same shader files**
  run in the preview and in Process, so preview = export by construction, as with the CPU core today.
- **One crate, two builds** (as now): `fae-gpu` compiled to WebAssembly for the preview (wgpu's WebGPU
  backend, draws straight into the page's canvas — no pixel copies through JavaScript) and to a native
  Python extension for Process (wgpu on DirectX 12, reads the result back for the encoders).
- **Stays where it is:** loading / decoding, geometry, resize, colour constancy (Python, the stage); the
  encoders; the UI and the table builders in `edit-math.js` (`computeEdit` keeps producing the steps and
  tables; the GPU core consumes the same edit object). Moving geometry / resize to the GPU is optional, late.
- **The CPU core** stays during the migration as the **reference** (tests compare against it) and as the
  fallback for edits that use a tool not ported yet. Retired at the end (kept in git history).
- **No GPU / WebGPU unavailable:** wgpu can run on Windows' software renderer (WARP) — slow but correct.
  Checked in G0; if WARP is too slow for Process, the CPU core stays as the Process fallback.

```
edit (computeEdit, unchanged) ─▶ steps + tables ─▶ fae-gpu: pass graph
   preview: wgpu → WebGPU (in WebView2) → canvas, histograms back to the UI
   Process: wgpu → DirectX 12 (Python extension) → read back → encoders
```

## 3. Decisions (made / to confirm in G0)

| Topic | Plan |
|---|---|
| Precision | WGSL has no f64. Math in **f32**; images between passes in **rgba16float** (enough for 8-bit output, half the memory of f32) — confirm in G0 by measuring banding / error vs the CPU core; rgba32float where a pass needs it. |
| Tests | Bit-exact goldens end. New rule: GPU vs CPU reference **max 1–2 levels, mean < 0.05**, plus a visual diff report; after the migration, GPU goldens with that tolerance. |
| Statistics | Histograms, grids, percentiles (Perceptual Saturation, Super Contrast, UI histograms) via compute shaders with **integer atomics** → exact and independent of thread order, like the CPU core's integer stats. |
| 3D tables (Film) | Manual trilinear in the shader (GPU texture filtering uses ~8-bit weights → visible steps). |
| Memory | A 50 MP image in rgba16float = 400 MB per buffer; a chain needs a few (ping-pong, source, guided-filter planes). Fine on 16 GB; **tiling** for big images on small GPUs (Process first). WebGPU's default limits (256 MB buffers, 8192 px textures) must be raised from the adapter's limits (G0: check 16384 px / ≥ 1 GB on the PC). |
| Preview resolution | Render the **full-resolution stage** (up to the source size, no 36 MP cap) when the GPU is fast enough, then a GPU pass resamples to exact screen pixels (Lanczos) — no small / full swap, no drafts. Fall back to a resolution pyramid only if G0 shows it is needed. |
| Host language in the browser | wgpu compiled to WebAssembly (one Rust host for both builds). Alternative if wgpu-in-wasm is a problem: plain JS WebGPU in the page with the same WGSL files. Decide in G0. |

## 4. Waves

Each wave ends with: the app working, tests green, a rebuild on the PC (`core\build_core.bat`), and a check
by you. Effort is a rough guess in working sessions like today's.

### G0 — Feasibility spike (1 session) — **go / no-go**
- In Fae on the PC: is WebGPU available in WebView2 (`navigator.gpu`, adapter name, limits: max texture
  size, max buffer size)? Same with `--debug` off.
- `fae-gpu` skeleton: Curves as one WGSL shader, built for WebAssembly (preview) and native (Python).
  Compare with the CPU core (max / mean difference), time 24 MP and 50 MP on the PC.
- Draw a WebGPU texture into the app's canvas; time upload, render, present.
- WARP: run the native build on the software renderer; time it.
- Container testing route: headless Chrome with WebGPU on SwiftShader, native wgpu on Mesa (lavapipe) —
  correctness only, no speed.
- **Gate:** WebGPU works in WebView2, limits are enough (or tiling is acceptable), Curves within tolerance,
  speed ≥ 5× the CPU core. Otherwise: stop and take the pyramid + native-core route instead (redesign doc 1.2).

### G1 — Foundation (2 sessions)
- Device / adapter setup for both builds (limits raised, errors → clear message, fallback to the CPU core).
- Image upload (stage pixels → GPU once per stage), the **pass graph**: one GPU pass (or a few) per chain
  step in chain order, buffers reused between frames, parameters as uniform buffers, tables as storage
  buffers / textures.
- **Preview output:** a final pass resamples the visible part to exact screen pixels (Lanczos) and draws it
  into the canvas; replaces putImageData + the screen-resolution workers for edits the GPU can do.
- **Hybrid switch:** the preview uses the GPU when every step of the edit is ported, else the CPU core as now.
- Tolerance test runner: `tests/gpu/` renders the parity cases on both cores and reports max / mean / count of
  pixels > 1 level, per case, plus a diff image for failures.

### G2 — Per-pixel tools (2 sessions)
- Point correction, Adjustments (without Mid/Detail), Curves, Perceptual Saturation without its perceptual
  mode, Film Effect (manual trilinear), plus the UI histograms (in / out / min / max) via atomics.
- After G2 most simple edits run fully on the GPU; time them on the PC.

### G3 — Whole-image and neighbourhood tools (3 sessions)
- Mid/Detail (guided filter: box filters via prefix sums or separable passes) — **per Adjustments
  instance** (the one-smoothing limit goes).
- Perceptual Saturation (grid, blur), Super Contrast (channel histograms, percentiles, Dynamic Contrast
  grid / maps). Two-phase passes: statistics pass → small reduce → apply pass, as in the CPU core.

### G4 — Sharpen (2 sessions)
- `sharp.rs`: Adaptive Sharpening, Balance, Structure (fast guided filters), Local Contrast, Focus (blur in
  the gamma domain). Radii stay relative to the image / scaled for the preview as now.
- After G4 every edit runs on the GPU in the preview; the CPU core is only the reference and Process.

### G5 — Process on the GPU (2 sessions)
- The native build in the Python extension: render the full stage, read back, hand to the encoders; the
  encoded compare preview uses it too. Tiling for images that don't fit.
- Process results checked against the CPU core with the tolerance runner on real photos; then the CPU core
  is retired from Process (WARP or the CPU core as fallback, per G0).

### G6 — Simplify the preview (1–2 sessions)
- Remove what the GPU makes unnecessary: small / full-resolution swap (`checkHires`, `S.loImg`), zoomed-in
  drafts (`run_region`, `MdCache` prefix tricks), the screen-resolution workers (`sharp-view.js`), the WASM
  preview worker and its thread pool, the 36 MP preview cap.
- Crop view, preset / edit-preset thumbnails and the film list through the GPU.
- Update the README and the redesign rules ("what you see is what you get" stays; the means change).

### G7 — Filter kit — DONE (2026-11-24): see docs/filter-kit.md
The reason to do this now: adding a filter should be small and uniform.
Built: a filter = `core/gpu/fae-gpu/filters/<id>.json` (descriptor + UI spec) + `<id>.wgsl`; three
templates (pixel, neighbourhood with the shared blur, statistics with the shared histograms);
`tests/gpu/fx_check.py` makes the cases from the descriptor. Kit filters run on the GPU only (skipped
without one). Box / guided-filter / pyramid blocks and grid statistics come when a filter needs them.
Originally planned:
- A filter = a WGSL file + a short descriptor (parameters, passes, which building blocks it uses) + its UI
  spec (sliders as today). Templates for the three kinds: **per-pixel**, **neighbourhood** (uses shared
  blur / box / guided-filter / pyramid blocks), **statistics** (uses shared histogram / grid reductions).
- Tests come from the descriptor (parameter sweeps on the synthetic images, tolerance against stored
  results). A short how-to in `docs/`.

## 5. Risks

- **WebGPU in WebView2 / pywebview** not enabled or limited on some setups → G0 checks first; the CPU core
  stays as fallback until then.
- **Precision:** f32 instead of f64 changes results slightly (tolerance tests, visual checks); 3D-table and
  curve lookups need care to avoid banding.
- **GPU memory** on big images / small GPUs → tiling (Process first, preview if needed).
- **Driver differences** between DirectX 12 (Process) and WebGPU (preview, translated by the browser): small
  numeric differences are possible → the preview / Process comparison becomes a tolerance test.
- **Debugging and testing** are harder than on the CPU; the container can check correctness on software
  GPUs, speed only on the PC.
- **Build:** wgpu adds build time and size; the WebAssembly build already needs the pinned toolchain.
