# Redesign — what's left

The redesign is built (right panel with re-orderable islands, export block, unified window header, standard
slider, per-tool presets, edit presets, film list, stackable tools incl. Curves, Filters tiles with
drag-to-place). See the README for how it works. This file keeps only what is still open, and the rules
later work has to follow.

## 1. Still to do

### 1.1 Alternative slider (design first)
The standard slider (white capsule thumb) is used everywhere. An alternative was planned (gradient tracks
or a downward triangle thumb) for places that are chosen later. `FILTER_GRADS` (state.js) and the jog-wheel
code (`makeJog`, `spec.wheel` / `spec.grad` in `makeField`) are kept for it. Only worth doing once there is
a place that should use it.

### 1.2 Preview speed between fit and 1:1 on big photos
**Decided 2026-11-23: move the core to the GPU (wgpu), see `docs/gpu-core-plan.md`.** The notes below
apply until then (and become unnecessary in its wave G6).

Zoomed between ~30 % and 1:1 on a large image, the full-resolution preview is on screen and most of it is
visible, so dragging a slider renders several megapixels per frame (drafts render the visible part only).
The fix is a faster core at these sizes, **never** a lower-quality image (see rules).
- Zoomed-in drafts are a first version: tune the idle time (200 ms), the margin (128 px for Sharpen) and
  when a draft is used.
- Carry the Mid/Detail smoothing over between the small and the full-resolution preview, so the first sharp
  render after zooming in doesn't start from nothing.

### 1.3 Check on Windows
Built and tested in headless Chrome only (no Python core there):
- Process an image with stacked tools (e.g. two Film Effects, two Curves) and check the file shows both.
- Use the new parts on real photos: presets / edit presets, film list, stacking, tiles, crop straighten.

## 2. Later (parked)

- **Crop in edit presets.** A crop is stored in the source image's pixels, so a preset needs a meaning that
  carries over. Idea: a preset with a crop becomes **aspect-ratio locked** (aspect bubble such as `3:2` on
  its card), the crop stored relative to the frame. To settle first: on another aspect, **skip the crop and
  apply the rest** with a notice (suggested) or block the preset; a tolerance (~0.5 %); landscape 3:2 vs
  portrait 2:3 (rotate the crop or count as a mismatch); compare against the oriented source before any
  crop; whether rotation / flip / angle travel with the crop. Like Resize it applies on click (hover and
  thumbnails show the image uncropped), and it clears point-correction marker positions.
- **Built-in presets** (tool and edit presets): none for now, only the user's own.
- **Film Effect:** Brightness / Contrast may be removed.
- **Mid/Detail on more than one Adjustments:** the core keeps one smoothing (`MdCache`), so only the first
  Adjustments has Mid/Detail. Would need a smoothing per step.

### 2.1 Settings (started)
The gear in the title bar opens a small Preferences popover (commands.js, `Ctrl+,`), stored in
`fae_settings.json` with the rest. Built so far: **Show tool dock** (`dock`) and **Next image after process**.
Further candidates (none decided): Candidates (none decided):
- **Output:** output folder (next to the source or a chosen folder), file name pattern (`name_WxH.ext` now),
  what to do when the file exists.
- **Defaults for new images:** start with a chosen edit preset; default format / encoder settings.
- **Preview / performance:** screen-resolution redraw on / off, its delay, preview threads.
- **Presets:** where `fae_presets.json` lives, import / export.
- **Interface:** UI scale, window header (custom or Windows frame, now `--native-frame`), language later.
- **Encoders:** paths to `cjpegli.exe` / `SvtAv1EncApp.exe` if not in `encoders/`.

## 3. Rules to keep

- **What you see while dragging is what you get:** drafts are always at the preview's own resolution; speed
  comes from rendering less (the visible part) or a faster core, never from a smaller copy scaled up
  (tried and reverted). Idle, the image is never shown scaled up from the small preview (swap at 1:1).
- **Islands:** every tool is an island with an on / off switch, the circular-arrow reset (no "Clear" /
  "Reset" text) and X; text inside islands is for control instructions only (no explanations of how a
  filter works); no per-slider reset (double-click the slider).
- **Every slider is the standard slider** unless the alternative is chosen for it.
- **Chain:** Crop, Resize, Color balance run first and Sharpen last (pinned); the rest run in panel order,
  added in the order they are opened. Point correction and Sharpen don't stack; the others do, up to 8.
  A stackable tool's button / tile adds another copy once the tool is open (a copy is removed with its X);
  the other tool buttons open / close.
- **Fonts / colours (direction E):** Inter for the UI, JetBrains Mono for numbers; professional grey, no blur:
  ground `#232326`, title bar `#1C1C1F`, panels `#1F1F22`, image area `#161618`, lines `#333338` / `#404046`,
  text `#E6E6E6`, muted `#9A9AA0`, light grey `#D4D4D8` for what is on. Switches: `#171717` capsule, knob
  `#999999` off / `#E64545` on, shaded centre on hover. Sliders: name and value on one line, full-width track
  below (`#3A3A40`, fill `#8E8E95`, white upright thumb).
- Rust core changes are rebuilt on the Windows PC with `core\build_core.bat` when asked.
