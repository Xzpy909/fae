# Rust back end: plan

Status: **B0, B1, B2, B3 done** (on main; geometry.py, colour.py, the Python stage and OpenCV removed).
**B4 in progress** (fae_backend 0.5.0): all encoders and AVIF decoding in Rust (rav1e / rav1d), checked by the
owner on Windows; left: the B4 clean-up (the Python versions listed under B4). Moves the Python back
end (`fastedit/`) into Rust, wave by wave, so the app keeps working after every wave and a later switch from
pywebview to **Tauri** is a small last step instead of a rewrite. Not a goal in itself: no visible change to
the editor; the gains are a smaller, faster-starting app (no NumPy / OpenCV / Pillow at run time), one
language for the engine, and an easy Tauri switch later.

## 0. Working on a wave

- **Build:** `core\backend\build_backend.bat` (maturin wheel of `core/backend/fae-backend-py`, pip install;
  prints the fae_backend version and how many commands are in Rust). Uses the pinned toolchain in
  `core/rust-toolchain.toml`. Optional: `nasm` on PATH for libjpeg-turbo's SIMD.
- **Layout:** `core/backend/fae-backend/src/` — `lib.rs` (command table `COMMANDS`, `Backend`, the wave each
  command moves in), `store.rs` (settings / presets, B0), `decode.rs` + `heif.rs` (B1), `px.rs` (`Px` owned /
  `View` borrowed pixels), `resample.rs` (Pillow's resize / reduce), `geometry.rs` (flip, quarter turns,
  crops, OpenCV's Lanczos-4 warp), `fit.rs` (StageParams, size maths, the resize step), `thumb.rs` (B2),
  `colour.rs` (White-Patch, Gray-Edge), `stage.rs` (entry cache, load_stage / load_source / sample,
  preview tokens) (B3).
  Python binding: `core/backend/fae-backend-py/src/lib.rs`; Python side: `fastedit/backend.py`
  (`REQUIRED` = the minimum fae_backend version; bump it with the crate version).
- **Tests** (run before and after every change):
  - `python tests\stage\stage_check.py` — the Python reference against the goldens in
    `tests/stage/results.zip`;
  - `python tests\stage\stage_check.py --impl rust` — the Rust back end against the same goldens (parts of
    later waves are skipped);
  - `--record-missing` records goldens for new cases from the Python version (keeps existing ones);
    `--own` adds your own photos from `tests/parity/images/` (results kept locally, not committed);
  - `python tests\backend\commands_check.py` — the commands moved so far vs api.py's old behaviour;
  - `python tests\stage\stage_bench.py` — Python vs Rust timings of the moved stage steps;
  - `cargo test --release` in `core/backend`.
- **Rules:** record goldens from the Python version *before* replacing it; exact where the same library or
  algorithm is ported, the tolerance rule where OpenCV maths is re-implemented (section 3); the Python code
  goes only after the Rust version passes on Windows. Bump the crate version (`core/backend/Cargo.toml`) and
  `REQUIRED` in backend.py together. Commit / privacy rules: `CLAUDE.md`.
- **B4 starting point:** the encoders are `fastedit/output.py` (PNG, WebP, the lossy dispatch, saving),
  `jpeg.py` (cjpegli.exe) and `avif.py` (SvtAv1EncApp.exe, the 10-bit 4:4:4 conversion, the OBU parser /
  AVIF muxer, Pillow's aom fallback); `proc.py` runs them cancellably. AVIF is also the one format the
  stage still opens with Pillow (engine.py `decode_pillow`, handed to Rust with `give_source`) and the
  file list's AVIF thumbnails (`make_thumb_pillow`): an AVIF decoder (dav1d) in Rust ends both. Decide
  the alpha AVIF question first (section 3). Record goldens from the Python encoders before replacing them.

## 1. Where things are

| Part | Where it runs | Python it replaces / uses |
|---|---|---|
| Decode + prepare | **Rust (B1)**: decode.rs, heif.rs (EXIF rotation, RGB / RGBA, ICC kept for output); AVIF: avifdec.rs (rav1d) since B4 | engine.py `decode`, geometry.py `prepare` (Pillow, pillow-heif, pillow-jxl-plugin) |
| Thumbnails (file list) | **Rust (B2)**: thumb.rs; AVIF still Pillow | engine.py `make_thumb_pillow` |
| Geometry | **Rust (B2)**: geometry.rs (quarter turns / flip, free angle as OpenCV's warpAffine Lanczos-4, crop, crop_free) | geometry.py `apply_geometry` (removed in B3) |
| Resize | **Rust (B2)**: resample.rs, fit.rs (Pillow's Lanczos, Mpix / preset / custom sizes) | geometry.py `fit_image` and helpers (removed in B3) |
| Color balance | **Rust (B3)**: colour.rs (White-Patch, Gray-Edge) | colour.py (removed in B3), NumPy, OpenCV |
| The stage and its cache | **Rust (B3)**: stage.rs (entry cache, load_stage / load_source / sample, preview tokens) | engine.py `Engine`, `Entry` (removed in B3) |
| The edit (Process) | core.py `render`: GPU core, else CPU core, skipped kit filters — **B5** | fae_gpu, fastedit_core (both Rust already) |
| Encoders | output.py PNG / WebP, jpeg.py (cjpegli.exe), avif.py (SvtAv1EncApp.exe + own OBU parser / AVIF muxer; Pillow aom fallback, also for alpha) — **B4** | Pillow, subprocess |
| Encoded compare | api.py `lossy_preview`: render, encode, decode back, size; cancellation (proc.py) — **B5** | all of the above |
| App plumbing | settings / presets in Rust (B0); the rest: api.py (files, dialogs, reveal), app.py (local HTTP server: UI, /preview tokens, /thumb), winchrome.py (frameless window), drag-drop — **B6** | pywebview, http.server |

The edit math (CPU core, GPU core) and the whole UI are already Rust / web: they don't move.

## 2. The shape at the end

```
ui/ (unchanged) ── bridge.js ──▶ shell: pywebview now, Tauri later (thin: window, dialogs, drag-drop)
                                   │  JSON commands + binary buffers
                                   ▼
                     fae-backend (Rust library): every command of api.py
                       ├─ stage: decode, geometry, resize, colour balance, thumbnails, entry cache
                       ├─ render: fae-gpu (native) / fastedit-core fallback, Process, compare
                       ├─ encode: PNG, WebP, JPEG (cjpegli.exe), AVIF (SvtAv1EncApp.exe + muxer)
                       └─ app: settings, presets, file lists, preview tokens, serving files
```

- **The boundary is a command table**: each api.py method becomes `fn(json args) -> json result` (+ byte
  buffers for pixels) in fae-backend. Under pywebview a ~100-line api.py forwards to it (one Python extension,
  `fae_backend`); under Tauri the same functions become `#[tauri::command]`s. Nothing else changes in a switch.
- **`ui/js/bridge.js`**: the only file that knows the shell (today `window.pywebview.api`, the local server
  URLs `/preview/<token>`, `/thumb`, the pywebviewready event). Tauri = rewrite this one file.
- **The local server's handlers** (UI files, /preview, /thumb) become Rust functions `(path) -> (status,
  headers, body)`: served by a small Rust HTTP server now, Tauri's custom protocol later (its config can set
  the cross-origin headers the threaded CPU preview needs).
- **Python stays as the test language** (parity.py, tolerance.py, fx_check, the stage tests) and as the
  pywebview shell until Tauri. At run time: Python + pywebview + one extension, no NumPy / OpenCV / Pillow.
- fae-backend links the two cores as Rust crates (native GPU for Process; the CPU core as the no-GPU fallback,
  as now). Its own Cargo workspace, like the GPU core's, so the CPU core's pinned WASM build is untouched.

## 3. Decisions

| Topic | Plan |
|---|---|
| Matching the Python results | **Exact where the same library or algorithm is ported**: JPEG decode (libjpeg-turbo, as Pillow), WebP (libwebp, as Pillow), Pillow's Lanczos resize (ported: fixed-point coefficients, deterministic), quarter turns / flips / crops, Mpix / preset size maths, the AVIF muxer (byte-exact). **Tolerance where OpenCV maths is re-implemented** (free-angle rotation, Gray-Edge's Gaussian / Sobel, White-Patch's percentile): the GPU core's chain rule (mean < 0.05, ≤ 0.1 % of pixels > 2 levels off). B2 result: the free-angle port (OpenCV's fixed-point coordinates and tables) and the thumbnails came out **identical** too — worth trying the same for B3's Gaussian / Sobel. |
| References | Before a part is replaced, its Python version records goldens (**stage parity**, like the CPU core's parity tests): synthetic images + own photos in `tests/parity/images/`. The Python code is deleted only after the Rust version passes on Windows. |
| HEIC / HEIF | **Decided: libheif** — loaded at run time from the DLLs pillow-heif ships (same decoder, no C build); later copied next to the app. |
| JPEG XL | jxl-oxide (pure Rust): lossless pixels exact; Pillow's synthesized sRGB ICC is not reproduced (listed difference). |
| AVIF with alpha / without SvtAv1EncApp (the aom fallback) | **Decided (B4): rav1e** (pure Rust) replaces Pillow's aom: alpha AVIFs, the UI's "aom" choice and no SvtAv1EncApp; a different encoder, so those files look and weigh differently (aom_q / aom_speed map onto rav1e's quality / speed). AVIF decoding (the stage, thumbnails, the encoded compare's decode-back): **rav1d** (pure Rust port of dav1d). Cargo-only build, no CMake. |
| Colour management | As now: the source's ICC profile is kept and embedded in the output, no conversion. |
| Threads | Decode / resize / colour balance multi-threaded (rayon). B2, 42 MP, Python → Rust: resize to 1080p 191 → 11 ms, 12 MP 251 → 23 ms, fine angle 190 → 93 ms, quarter turn 83 → 36 ms. |
| Toolchain | The pinned Rust 1.100; C libraries (libjpeg-turbo, libwebp, libheif) built by their -sys crates or shipped as DLLs — decided per wave, with build_backend.bat doing it in one go. |

## 4. Waves

Each wave ends with: the app working, the stage tests green, a rebuild on Windows
(`core\backend\build_backend.bat`), the owner's check in the app, then the replaced Python code and its
dependency removed.

### B0 — Groundwork — DONE (on main)
- `core/backend/`: fae-backend (library) + fae-backend-py (pyo3, `fae_backend`), build_backend.bat; the command
  table design (names, JSON shapes, binary buffers, errors, cancellation, progress callbacks).
- `ui/js/bridge.js`: every pywebview / server call of the UI goes through it (no behaviour change).
- **Stage tests** `tests/stage/stage_check.py`: the Python outputs (decode per format, EXIF orientations,
  alpha, geometry incl. free angles and crops, every resize mode, White-Patch / Gray-Edge) as goldens; check
  mode with the exact / tolerance rules above.
- First commands through the new path: settings and presets (store.rs).

### B1 — Decode, prepare — DONE (on main)
- JPEG (libjpeg-turbo via the mozjpeg crate; SIMD with nasm), PNG, WebP, TIFF, BMP, GIF, ICO, PNM, TGA; EXIF
  orientation; RGB / RGBA (grey → RGB); ICC kept; JPEG XL; HEIC (libheif). AVIF stays with Pillow until B4.
- Gate: exact pixels against Pillow for JPEG / PNG / WebP / TIFF; HEIC / JXL as decided. Deliberate
  differences (stage tests `FIXED`, `META_DIFF`): 16-bit grey PNG (Pillow clipped it), JPEG XL ICC.

### B2 — Geometry, resize, thumbnails — DONE (on main, e5f520a)
- Quarter turns, flip, crop; free-angle rotation (OpenCV's Lanczos-4 warp re-implemented exactly, border
  replicate for RGB, premultiplied with a transparent border for RGBA); crop past the edges (`crop_free`).
- Pillow's Lanczos port; cover-crop, Mpix (priorities), preset / custom sizes, fit modes.
- File-list thumbnails (Pillow's JPEG draft + BILINEAR thumbnail with its reduce step + JPEG q82).
- Result: every stage case identical to Python; engine.py's `Entry` keeps raw pixels and calls
  `backend.geometry` / `backend.fit`. geometry.py stays as the test reference until B3.

### B3 — Color balance → `load_stage` in Rust (1 session) — DONE (on main)
- White-Patch (percentile, strength), Gray-Edge (Minkowski p, sigma, strength) in linear sRGB.
- `load_stage` / `load_source` / `sample` move to fae-backend (with the entry cache: decoded pixels, the
  geometry result, the last few resize results); preview tokens served from Rust.
- Then: geometry.py, colour.py, most of engine.py, **OpenCV** removed.
- Result (colour.rs, stage.rs): every colour and stage case **identical** to Python (the tolerance rule
  wasn't needed). Both colour balances end in a per-channel gain, so the output is three 256-entry
  tables; the percentiles come from level histograms with NumPy's interpolation, Gray-Edge's gains from
  OpenCV's Gaussian kernel and 3 × 3 Sobel (float32, reflect-101). 12 MP: White-Patch 1.9 s → 27 ms,
  Gray-Edge 2.0 s → 0.28 s. The app keeps one back end (`backend.shared()`): api.py forwards the three
  commands, app.py serves `/preview/<token>` from it. Until B4 / B5: AVIF is opened by Pillow and handed
  over (`give_source`); Process and the encoded compare take the stage image from Rust (`Backend.stage`)
  and store their preview with `put_preview`. `tests/backend/commands_check.py` compares the three
  commands with api.py's old code (answers, preview pixels, samples, AVIF).
- Clean-up done after the owner's check: geometry.py, colour.py and engine.py's `Engine` / `Entry` deleted,
  opencv-python dropped (`prepare` lives next to engine.py's Pillow decode for AVIF). The stage tests'
  goldens are now the only reference for geometry, resize, colour balance and the stage; new cases of
  theirs are recorded from Rust (`--record-missing`), `--record` is refused. Python at run time: Pillow
  (AVIF decode, thumbnails' AVIF, encoders) and NumPy (core.py, avif.py) until B4 / B5.

### B4 — Encoders and output (2–3 sessions)
- PNG, WebP lossless / lossy (libwebp), JPEG through cjpegli.exe (quality / distance), AVIF through
  SvtAv1EncApp.exe with the muxer and 10-bit 4:4:4 conversion ported (byte-exact against avif.py), the aom
  path per the decision; ICC embedding everywhere; output names; running encoders without a console window,
  with cancellation.
- Gate: same file sizes / bytes where the encoder is the same (exe paths, libwebp), pixels decoded back equal.
- Part 1 (fae_backend 0.5.0): `encode.rs` (options as the Python normalised them, PNG, WebP, JPEG, saving,
  output names), `avif.rs` (10-bit YUV, IVF / OBU parsing, the muxer), `run.rs` (encoders without a
  console window, below-normal priority and killed by a newer request for the encoded compare; temp
  folders). output.py's `encode_lossy` / `save_image` call the back end (`Backend.encode` / `save`); the
  Python versions stay as `encode_lossy_py` / `save_image_py`, and for the AVIF cases not in Rust yet
  (Rust answers "unknown:"). `tests/backend/encoders_check.py` compares them: WebP (libwebp 1.6, as
  Pillow 12 / 13), the cjpegli and libjpeg-turbo JPEGs, the SvtAv1EncApp AVIF, the YUV conversion and the
  muxer byte for byte; PNG has Pillow's filter choice exactly but zlib-rs instead of Pillow's zlib-ng
  (same pixels and ICC, sizes within a few tenths of a percent, smaller on flat images); the options
  (odd values too), output names, cancellation.
- Part 2 (same version): **rav1e** for the old "aom" cases (alpha, the UI's choice, no SvtAv1EncApp): ravif's
  quality → quantizer and speed settings, colour as the SVT path's 10-bit 4:4:4 full-range BT.709, alpha as
  a 10-bit monochrome auxiliary item (`auxl` + `auxC`); the UI calls it "rav1e" (settings keys stay
  `aom_*`). **rav1d** opens AVIF (`avifdec.rs`: own HEIF box parser, irot / imir as Pillow maps them):
  libavif's choice between libyuv and its own float conversion is ported, with libyuv's integer YUV
  constants and chroma upsampling, so the pixels equal Pillow's in every case tried (8 / 10 / 12-bit,
  4:2:0 / 4:2:2 / 4:4:4, alpha, limited range, grey, identity matrix, odd sizes) and the thumbnails are
  byte-identical. Grids and premultiplied alpha answer "unknown:" and still go to Pillow. `encoder_info`
  is a command. The `simd` feature (build_backend.bat with nasm) also builds rav1e's asm; rav1d's asm
  is off (it doesn't link into the extension).
- rav1e speed on big images (owner's choice: patch rav1e rather than an AVIF grid): after its tiles,
  rav1e ran CDEF, loop restoration and the deblocking-level search over the whole frame on one thread;
  with 24 threads that was most of a 50 MP encode (speeds 3–8 all ≈ 7.5 s). rav1e 0.8.1 with those three steps in
  parallel lives in github.com/Nukvah/rav1e-fae (upstream crate as the first commit, then the patch; notes
  in its FAE-PATCH.md), pulled in by `[patch.crates-io]` pinned to a commit, output byte-identical; colour and alpha encode side by side.
- Kept until the owner's check, then removed (B4 clean-up): jpeg.py, avif.py, output.py's
  `encode_lossy_py` / `save_image_py`, proc.py's encoder runner, engine.py's `decode_pillow` /
  `make_thumb_pillow` and `give_source` (these three stay as the fallback for the AVIFs Rust doesn't open).

### B5 — Process and the encoded compare (1–2 sessions)
- core.py's job in Rust: GPU core first (one device, lock, release after a batch), CPU core fallback, size
  limits, "GPU off for the session" after an error, skipped kit filters; Process batches with progress;
  `lossy_preview` (render, encode, decode back, size, cancel).
- Then: core.py, output.py, jpeg.py, avif.py, proc.py, **Pillow and NumPy** leave the run-time dependencies
  (tests keep NumPy).

### B6 — App plumbing (1 session)
- Settings and presets files, file / folder collection, the HTTP handlers (UI, /preview, /thumb) in Rust.
- api.py = a forwarder over the command table; app.py = start the window. What Python still does at run time:
  pywebview, winchrome.py (frameless window), dialogs, reveal in Explorer, drag-drop paths.
- PyInstaller build size and start-up time measured before / after.

### B7 — Tauri (later, only when decided; 2–3 sessions)
- Tauri shell: the command table as commands, the HTTP handlers as a custom protocol (with the cross-origin
  headers), frameless window + header buttons, drag-drop, dialogs, installer, maybe auto-update.
- `bridge.js` rewritten for Tauri; winchrome.py, app.py, api.py and pywebview gone. Tests stay in Python
  through fae_backend.

## 5. Risks

- **Matching Pillow / OpenCV**: exact for what is ported or uses the same library; the OpenCV-based parts
  may shift slightly (tolerance rule, visual check). Every replaced part gets goldens first.
- **C libraries on Windows** (libjpeg-turbo, libwebp, libheif, maybe libavif / dav1d): build time and DLLs.
  Decided per wave; a pure-Rust choice is preferred where it gives the same pixels.
- **HEIC and alpha AVIF** are the two places where the current behaviour may not carry over 1:1 (decisions above).
- **No visible feature** during the move: each wave is checked by the stage tests and a short app check.
- **Two shells' quirks**: the frameless window and drag-drop are pywebview-specific until B7; bridge.js keeps
  the UI free of them.
