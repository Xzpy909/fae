# Fae: what's next

Written after 0.7.0 (2026-11-25). The Rust back end (B0–B7) and the GPU core's waves G0–G5 and G7 are done:
Fae.exe is a Tauri app with no Python at run time. This file lists what is still open, in the order it is
worth doing. Back to the [README](../README.md); the details of each area stay in their own docs
(`redesign-right-panel.md`, `gpu-core-plan.md`, `filter-kit.md`, `development.md`).

Priorities: **P1** blocks a good release or will break the build, **P2** is the next visible work,
**P3** is clean-up that makes later work cheaper.

## Summary

| # | Item | Priority | Size |
|---|------|----------|------|
| 1 | Threaded WebAssembly build: the `atomics` warning becomes an error | P1 | 1 session |
| 2 | AVIF files that don't open (grids, premultiplied alpha) | P1 | 1–2 sessions |
| 3 | Visible leftovers of the Python app (`JPEG (Pillow)`) | P1 | small |
| 4 | Release packaging: installer, settings folder, licences | P1 | 2 sessions |
| 5 | Edit presets and applying edits to other images (redesign 1.4) | P2 | 2–3 sessions |
| 6 | Check the redesign on Windows (redesign 1.3) | P2 | owner check |
| 7 | Settings: output folder and file name pattern first | P2 | 1–2 sessions |
| 8 | The rest of the redesign plan (alternative slider, crop in presets, …) | P2 / later | — |
| 9 | Tests: retire the Python ports in `tests/pyref`, keep the references that still matter | P3 | 1–2 sessions |
| 10 | Automatic test runs (GitHub Actions) | P3 | 1 session |
| 11 | Docs that no longer match the code | P3 | small |
| 12 | Smaller clean-ups (versions, spike page, build without Python, icon) | P3 | small |

## P1 — before the next release

### 1. The `atomics` warning (threaded WebAssembly)
The CPU core's WebAssembly build (`core/build_core.bat`, `fastedit-wasm`) is multithreaded
(wasm-bindgen-rayon): it rebuilds the standard library with `-C target-feature=+atomics` and
`-Z build-std` under `RUSTC_BOOTSTRAP=1`. Newer compilers warn that this will become a hard error, so Rust
is pinned to 1.100.0 in `core/rust-toolchain.toml`. Today that works, but every Rust update (new crates
needing a newer compiler, security fixes) is blocked by it.

What has changed since: the live preview runs on the GPU core whenever WebGPU is there. The CPU
WebAssembly build is left for the no-GPU fallback (a reduced copy, see `development.md`), crop mode and the preset /
edit-preset thumbnails, which are small images.

Options:
- **a. Make the CPU WebAssembly build single-threaded (suggested).** No atomics, no build-std, no
  `RUSTC_BOOTSTRAP`, the toolchain pin can follow stable Rust. Crop mode and thumbnails barely notice;
  the no-GPU preview gets slower. Measure that first (`Fae.exe --no-gpu`, a 24 MP photo, dragging
  Sharpen and Mid/Detail); if it is still usable, this is the simplest way out. The GPU WebAssembly build is
  already single-threaded (`build_gpu.bat`). A side gain: the cross-origin isolation headers are only
  needed for SharedArrayBuffer, so they may go too (check the GPU build doesn't need them).
- **b. Build only the WebAssembly part with a pinned nightly.** Keeps the threads; atomics is a supported
  nightly feature. Costs a second toolchain and nightly's churn.
- **c. Stay on 1.100.0.** Nothing to do now; the problem returns with the first crate that needs newer Rust.

Done when: `build_core.bat` builds with no warning on a current stable Rust, the parity tests pass
(`parity.py check --impl wasm`), and the no-GPU preview is timed before and after.

### 2. AVIF files the Rust decoder can't open
Since Pillow left the app (B7 part 2), `avifdec.rs` answers "unsupported" for these and nothing else opens
them:
- **Grid images** (`grid` derived item: the picture is cut into tiles, each an AV1 image; cameras and some
  phones write them for large photos). Decode every tile with rav1d and put them together at the grid's
  size, cropped to the output width / height.
- **Premultiplied alpha** (`prem` reference): decode as now, then divide the colour by alpha (and round as
  libavif does).
- Also unsupported today: an alpha plane of another size or depth than the colour, and image sequences
  without a primary item (take the first frame). Lower priority; worth doing while the code is open.

Tests: make sample files with `avifenc` (grid: `--grid`; premultiplied: `--premultiply`) into
`tests/stage/inputs/`, compare against Pillow (still in the test requirements) in `encoders_check.py`.
The `development.md` "Not yet" line goes when this is done.

### 3. Visible leftovers of the Python app
- Without `cjpegli.exe` the JPEG encoder is libjpeg-turbo, but the back end still names it `"pillow"`
  (`encode.rs` `encode_jpeg`), so the export popover says **JPEG · Pillow** and the compare label
  **JPEG (Pillow)** (`controls.js`, `compare.js`). Rename to `libjpeg-turbo` in both (keep accepting the old
  name where saved settings could hold it).
- The AVIF choice `aom` is shown as `rav1e` but stored as `aom` (`controls.js`, `init.js`, `avif.rs`'s
  `aom_*` options). Internal only; rename when the export code is next touched, reading old settings.
- Comments that still speak of Python doing the work (`chain-presets.js`: "a new stage from Python";
  `edit-math.js`: "shared with Python"). Comments only.

### 4. Release packaging
Today a release is a zip of `dist\Fae` (Fae.exe, `ui\`, `encoders\`, `heif\`). Still open from B7:
- **Installer.** Tauri can build one (`cargo tauri build`, NSIS or MSI; `tauri.conf.json` has
  `"bundle": {"active": false}` now). Per-user install (no admin), Start menu entry, uninstaller, and an
  "Open with Fae" entry for image files (optional). Keep the zip as the portable version.
- **Where settings live.** `fae_settings.json` / `fae_presets.json` are written next to Fae.exe; an installed
  copy in `Program Files` can't write there. Installed: `%APPDATA%\Fae`; portable (zip): next to Fae.exe as
  now (e.g. a `portable` marker file next to the exe). `paths.rs` is the one place to change.
- **Licences.** The repository has no LICENSE of its own, and the release ships third-party code that
  asks for its notice to be included: libheif / libde265 (LGPL, loaded as DLLs, which suits the LGPL),
  rav1e / rav1d, libjpeg-turbo, libwebp, jpegli, SVT-AV1, the fonts (OFL, already there) and the emoji
  (already there). A `THIRD-PARTY.txt` in the zip / installer, generated where possible
  (`cargo about` for the Rust crates). Choosing Fae's own licence is the owner's decision.
- **Code signing (later).** Unsigned exes get Windows SmartScreen's warning on first run. A certificate
  costs money; worth a look only if the app is shared widely.
- **Auto-update (later).** Tauri's updater needs signed update files and a place to host them (GitHub
  releases work). Not needed while releases are occasional.

## P2 — the next visible work

### 5. Edit presets and applying edits (redesign 1.4, decided 2026-11-25)
Not built yet. In short (the full spec is in `redesign-right-panel.md` 1.4):
- Preset cards get an always-visible **gear** (and right-click) with Rename / Duplicate / Delete
  (+ Update with current edits).
- The **first card is a shimmering "+"**: a click makes a new card from the current edits with its name
  field focused; Enter saves, Esc drops it; an existing name asks "Replace …?".
- Copy edits / Paste edits / Paste edits to… become **Apply current edits to…** in two forms: single file
  (click a card, done) and multiple files (select, Apply to all / Apply to N, the per-card Apply pill).
  No clipboard step: the source is always the image on screen.

Still to settle while building (from the same section): Apply preset to… in the same two forms; a clearer
name for whole-edit presets (`Recipes`?); hover not previewing Resize / Color balance and presets
replacing the whole edit (now said only in the footer); the `Yours` tag while there are no built-in
presets; what Ctrl+Shift+C / Ctrl+Shift+V do afterwards.

Suggested order: the gear menu and the "+" card first (self-contained, in `presets.js`), then the two
Apply windows (`batch-apply.js`), then Apply preset to… on top of the same window.

### 6. Check the redesign on Windows (redesign 1.3)
Built and tested in headless Chrome only. On the PC: process an image with stacked tools (two Film Effects,
two Curves) and check the file shows both; use presets / edit presets, the film list, stacking, tiles and
crop straighten on real photos. If this has been done since, strike it from the redesign doc.

### 7. Settings
Preferences has two entries (tool dock, next image after process). For a batch tool the most useful next
ones are about output:
- **Output folder** (next to the source, or a chosen folder) and **file name pattern** (`name_WxH.ext` now).
- **When the file exists** (new name as now, overwrite, skip).
- Then: start new images with a chosen edit preset; default format / encoder settings.
The other candidates (preview performance, UI scale, encoder paths, preset import / export) can wait.

### 8. The rest of the redesign plan
- **Alternative slider** (1.1): only when there is a place that should use it.
- **Preview speed between fit and 1:1** (1.2): solved by the GPU core; the notes in 1.2 can go. Without a
  GPU the CPU preview renders a reduced copy (2560 px per `development.md`; check the code still does) and zooming in shows it scaled up, which the "what you see is
  what you get" rule doesn't allow; acceptable as a fallback, but it should say so in the status bar (check
  it does).
- **Crop in edit presets** (2, parked): aspect-locked crops; the open questions are listed there.
- **Built-in presets** (2, parked): none until there are presets worth shipping.
- **Film Effect** Brightness / Contrast may be removed (2, parked).

## P3 — clean-up

### 9. Tests: the Python reference code
`tests/pyref/` and `tests/fastedit/` are the Python app's code kept for the tests. They do three
different jobs:
- **`pyref/core.py`** is not old code: it runs the current cores (`fastedit_core`, `fae_gpu`) from Python
  for the parity, GPU, bench and process tests. Keep it; move it out of `pyref` (e.g. `tests/lib/cores.py`)
  so the name says what it is.
- **`tests/fastedit/`** (backend.py, config.py, engine.py) wraps `fae_backend` the way the old shell did and
  keeps Pillow's decode as an independent reference. Keep while the stage and command tests use it; a
  rename (`tests/lib/`) would end the confusion with the old app folder.
- **`pyref/avif.py, jpeg.py, output.py, proc.py, app.py`** are the Python versions the Rust back end
  replaced. They were the gates of each wave (Rust had to match them), and `encoders_check.py`,
  `app_check.py`, `commands_check.py` still compare against them. Now that the Rust versions are the app,
  record their outputs as goldens (as the stage tests already do with `--record-missing`) and compare
  against those; then delete these files. Pillow and NumPy stay in `tests/requirements.txt` as the
  independent decoder / maths reference.

### 10. Automatic test runs
There is no CI: every check runs by hand. A GitHub Actions workflow on a Windows runner could run
`cargo test` (back end, fae-app), build the Python extensions and run `parity.py check`, `stage_check.py`,
`commands_check.py`, `encoders_check.py` (without the encoder exes it skips those parts). GPU tests need a
GPU; WARP (`bench_native.py --warp`) may do for correctness. Start with `cargo test` + parity, the cheapest.

### 11. Docs that no longer match
- `gpu-core-plan.md` still says "Status: planned"; G0–G5 and G7 are done and G6 is mostly done (the
  preview worker remains as the no-GPU fallback). Update the status line; mark each wave.
- `development.md` calls the GPU core "in progress".
- `redesign-right-panel.md` 2 lists "Mid/Detail on more than one Adjustments" as parked, but
  `features.md` says every Adjustments copy has its own Mid/Detail. Remove it if done. Its 1.2 notes are
  obsolete (see 8).
- `rust-backend-plan.md` is finished: shorten to a status + history note, or move with the GPU plan to
  `docs/history/`.

### 12. Smaller clean-ups
- **Versions:** the back end says 0.7.0, the GPU core 0.7.2. One version for the app (the splash shows
  it) and the crates following it, or say in `development.md` which one is the app's.
- **`ui/gpu-spike.html`, `--gpu-spike`:** the G0 feasibility page. Remove unless it is still used to check a
  new PC.
- **Building without Python:** Python is still needed to build the app (maturin for the test extensions,
  and `copy_heif.py` takes libheif from pillow-heif). The app build could fetch a pinned libheif instead;
  only worth it if someone else is to build Fae.
- **`preview 1.webp`** in the repository root: move to `docs/images/` with the screenshots the README
  wants (`<!-- more screenshots go here -->`).
- **Icon at small sizes:** checked the 16, 24 and 32 px images in `icon.ico`: the wings read well; the
  body line is faint at 16 px but fine. Nothing left to fix. On a light taskbar the light tile has
  little edge; a thin darker outline at 16–24 px only would help if it bothers.
