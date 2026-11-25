# Redesign — what's left

The redesign is built (right panel with re-orderable islands, export block, unified window header, standard
slider, per-tool presets, edit presets, film list, stackable tools incl. Curves, Filters tiles with
drag-to-place). See docs/features.md for how it works. This file keeps only what is still open, and the rules
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

### 1.4 Edit presets and applying edits to other images (decided 2026-11-25)
The Edit presets popover and the Copy / Paste edits commands are confusing. Decided by the owner:

**Preset cards: a gear menu instead of ⋯**
- Delete is not found today (it sits in the ⋯ that shows only on hover over the thumbnail). Each card gets a
  **gear icon in its top-right corner**, always visible; a click opens a small menu: **Rename preset**,
  **Duplicate preset**, **Delete preset**. **Right-click** on the card opens the same menu.
- Rename edits the name in place on the card (as now). Duplicate adds a copy right after the original,
  named `<name> copy` (`copy 2`, … on a clash), and starts renaming it. Delete keeps the click-again arm.
- The ⋯ entries that are not in the gear menu: *Apply to images…* moves to the batch window below;
  *Update with current edits* goes in the gear menu too, under the three (to settle when building).

**New preset: a "+" card instead of Save current edits**
- The `Save current edits` button and the separate name row go. The **first card is always a big "+"**
  (card-sized, shimmering effect) also when there are no presets (it replaces the "No edit presets yet" text).
- A click turns it into a new card with the current edits (thumbnail, tool strip) and the cursor already in
  the card's name field; **Enter saves** it, Esc / clicking away drops it. A name that exists asks
  "Replace <name>?" on the card before overwriting.

**Copy edits / Paste edits / Paste edits to… become two commands**
- **Apply current edits to… (single file):** a window with the image cards (as Paste edits to… now); a click
  on a card applies the current image's edits to that image and closes. No selection, no pills.
- **Apply current edits to… (multiple files):** the same window, a click selects a card (Shift+click a range,
  Ctrl+A all, as now); the head shows **Apply to all** from the start (not only once something is selected),
  and Apply to N once there is a selection. The Apply pill on each card stays (applies to that
  image alone and shows Applied, quicker than selecting for 2-3 images).
- No clipboard step: the source is always the image on screen (its name shown in the head, "Edits from
  game_a.tif" style). The current image is shown but can't be picked. Crop still goes only to images of the
  same source size (notice as now). Ctrl+Shift+C / Ctrl+Shift+V go away or move to these two (to settle).
- Each image keeps its own undo step (as now).

**Still open**
- Edit › *Apply preset to…*: give it the same two forms (single / multiple files) after Select preset, so
  presets and current edits apply the same way.
- Naming: the top-bar button says `Presets`, the tool rows have preset buttons too, the window says
  `Edit presets`. A clearer name for whole-edit presets (e.g. `Recipes`) is worth a look.
- Hover previews the card on the image, but Resize and Color balance only show after a click; and applying
  replaces the whole edit (a preset with only Sharpen resets Adjustments). Both say so only in the footer.
- `Yours` on every card says nothing while there are no built-in presets.

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
