# Redesign: right panel, presets, sliders, window header and stackable filters

Status: **in progress** — built in waves, see the progress list in section 7.

## 1. Right panel layout — collapsible tool rows (approved: design C)

The panel keeps two fixed islands and a re-orderable list between them:

| Position | Island | Re-orderable | Preset button |
|---|---|---|---|
| Top (fixed) | Histogram **block** (docked, not an island) | no | no |
| Middle | every tool / filter the user has open | **yes** | yes, where it makes sense (see 3.3) |
| Bottom (fixed) | Export **block** (see 1.4) | no | no |

### 1.1 Collapsed row

Each tool in the middle list collapses to one frosted row (~40 px high), left to right:

1. **Drag grip** (6 dots) — drag to reorder.
2. **Order number** (small round badge, 1, 2, 3 …) — the position in the processing chain. The expanded / active tool's badge is accent-filled.
3. **On / off switch** — same switch as today's islands; off keeps the values but leaves the tool out (row title dims).
4. **Title** (sentence case, 12.5 px, medium weight).
5. **Preset button** — shows the current preset name (`Default`, a preset name, or `Custom` once values differ from the picked preset); opens the preset dropdown (section 3). Hidden for tools without presets.
6. **Expand chevron** — `›` collapsed, `⌄` expanded.

### 1.2 Expanded row

The row grows into a full island: the same header line, then the tool's controls (sliders etc.) indented under it. The expanded island gets a faint accent border. Several rows may be open at once (same rule as today's islands).

### 1.3 Reordering

- Drag by the grip. The dragged row lifts (slight tilt, stronger shadow, accent border, grabbing cursor) and a **glowing accent drop line** with small ring ends shows where it will land.
- The order is the **processing order**: the preview and Process apply tools top to bottom. Order numbers update live after a drop.
- Reordering is an edit: it goes into undo / redo, is stored per image like other edits, and is included in Copy edits / Paste.
- Histogram and Export cannot be dragged and nothing can be dropped above Histogram or below Export.

### 1.4 Export block (approved: design C — compact, fixed height)

The export part is **no longer an island**: it is a flat block docked to the bottom of the panel, full panel width, square edges, solid background a step darker than the islands (`#00282B`), 1 px top border and a soft upward shadow. The tool list scrolls behind its top edge with a short fade. Its **height never changes** (≈ 130 px for every format), so nothing has to be resized when the format or encoder changes, and the list keeps the space when there are many filters.

Rows:
1. **Encoding compare** button (split icon) — split view *lossless | encoded*; greyed out for lossless formats (PNG, lossless WEBP). The top bar's split is always *original | edited*; only one of the two splits is on at a time. Then the **format picker** (WEBP · PNG · JPEG · AVIF, segmented, fills the row) + a **settings chip** on the right showing the current setting in mono (`q 90`, `d 1.00`, `crf 25`, `lossless`; PNG: no chip or a disabled one). The chip opens the **encoder popover**.
2. **Process** (Gradient 1, `Ctrl+S` hint, takes the remaining width) + **Process all** (ghost button) beside it.
3. **Next image after process** checkbox on the left, output size readout on the right in mono (`3840×2160 → 1920×1080`).

**Encoder popover** — opens upward from the chip, frosted glass, inset 15 px from the panel edges, the block's width:
- header: format + encoder (`JPEG · jpegli`, `AVIF · SVT-AV1 4:4:4`) and the estimated file size for lossy formats,
- the format's controls as today: Quality / Distance switch + slider (JPEG), Lossy / Lossless + Quality (WEBP), SVT-AV1 / aom + CRF + Preset (AVIF),
- standard sliders; closes on Esc, click outside, or the chip again.

The progress bar for Process / Process all runs along the block's top edge.

### 1.5 What stays from the current panel

- The X (delete: reset + close) and reset buttons keep their behaviour; in collapsed rows they move into the expanded header (or a row hover) so the collapsed row stays clean.
- The Filters popover (Filters pill at the end of the tool pill row over the image, tile grid) still adds filters; a newly added filter appears at the end of the chain and opens expanded.

## 2. Live preview of presets

- **Thumbnails are rendered from the current image**, run through that preset — not stock pictures. Render them at thumbnail size with the WASM preview path so they are cheap; re-render when the image or the earlier part of the chain changes.
- **Hover = preview**: hovering a preset (per-tool or chain) temporarily shows it on the main image, with a small chip in the viewport: `Previewing <name>`. Leaving the dropdown restores the real state. **Click = apply** (one undo step).

## 3. Per-tool presets (approved design)

### 3.1 Dropdown

Opens under the row's preset button, anchored to the panel's right edge, frosted glass, ~270 px wide:

- One line per preset: **thumbnail (≈54×36, rounded)**, name, and a small meta line (`Built-in`, `Yours`, `No changes` for Default).
- The active preset has an accent-tinted background, accent border and a check mark on the right.
- Divider, then:
  - `+ Save current as preset…` (accent coloured) — asks for a name, saves the tool's current values.
  - `Rename / delete presets` — manage the user's own presets (built-in ones can't be deleted).
- Long lists scroll inside the dropdown.

### 3.2 Behaviour

- A preset stores only that tool's values (e.g. Film Effect: film table, Brightness, Contrast, Opacity).
- Picking a preset sets the tool's values; moving any slider afterwards turns the button label into `Custom`.
- `Default` is always first and resets the tool to its neutral values.

### 3.3 Which tools get a preset button

Not every tool gets presets — for some they make no sense. Decide tool by tool when implementing; tools without presets simply have no preset button in their row (no empty placeholder).

## 4. Chain presets (approved design — "Edit presets")

A **Presets** button in the top bar, directly before **Copy edits** (`Presets ▾ | Copy edits | Paste | Reset`). It opens a large frosted dropdown (~620 px) under the button:

- Header: `EDIT PRESETS` label with the hint `whole chain, in order`, a **search** box, and a primary **Save current edits** button (accent gradient).
- **Grid of cards, 3 per row.** Each card:
  - thumbnail of the **current image** through the whole chain (≈ 176×108),
  - name,
  - a **chain strip**: small icon chips of the tools in the preset, in order, joined by `›`,
  - meta on the right: `Yours` / `Built-in`.
- Hover on a card: accent border, `Apply` button and `⋯` menu (rename, delete, update with current edits) over the thumbnail; the main image previews the chain.
- Footer note: *Applying replaces the filters and their order; crop and output settings stay as they are.*

Open questions (decide before implementing):
- Ship some built-in presets, or start with only the user's own?
- Should chain presets include Resize, or leave it like crop and output?
- Stored format: one JSON file per preset or one presets file, kept next to the settings file.

## 5. Sliders — shared standard slider (DONE: design A "Capsule", built)

Design canvas: thumb options A / B / C, states at 2×. **A · Capsule** was picked.

Spec (as built in `ui/style.css` → "shared slider", `makeField` in `ui/js/controls.js`):

- **Row**: label 140 px | track (fills the rest) | value box 58 px; gap 10 px; islands are **420 px wide** (panel column 450 px — the standard width). **No per-slider reset button** — double-click the slider resets it.
- **Track**: 4 px, rounded, empty part `#0b4b4f` (`--sl-base`), fill **Gradient 1** `#00EBFF → #00FFAA` (`--sl-fill`), spread over the whole track and revealed up to the value.
- **Centre-anchored** when the range crosses zero, or at the neutral value via `spec.center` (Adjustments: Saturation 50, Contrast 1.00): fill grows from the anchor either way, with a small tick (2×10 px, 30 % white) at the anchor.
- **Thumb**: white capsule 20×12, radius 6, soft drop shadow + 1 px dark rim.
  - hover: 4 px mint glow ring (`rgba(0,255,170,.18)`)
  - dragging: 6 px ring (`.26`) + scale 1.08
  - keyboard focus: 2 px gap + 2 px `#00FFAA` ring
- **Changed from default**: label goes from `#9dbdba` to `#E6E6E6` with a 5 px mint dot; value box text white with a mint border. At default everything stays quiet.
- **Value box**: monospace, tabular digits, `rgba(0,30,33,.75)` background, 1 px 10 % white border, radius 6; focus = mint border. Drag-scrub, typing and wheel behave as before.
- **Island off**: track greys out (`#103a3d` + grey fill), thumb `#9fb6b4`.
- Double-click resets to default (as before).

**Every slider in the app is the standard slider now** — filter sliders (gradient tracks dropped) and Adjustments (jog wheels dropped) included. The alternative slider is used only where chosen, once it is designed; `FILTER_GRADS` (state.js) and the jog-wheel code are kept for it.

### 5.1 Island rules (apply to every island, now and in the new panel)

- **Island reset**: one icon for "reset the whole island" — the circular arrow (no clock hands), top-right next to the X. Used by Crop, Point correction, filters, Adjustments, Curves (current channel) and Sharpen; no "Clear" / "Reset" text links.
- **Point correction**: each picked point (black / gray / white) gets its own small reset icon on its tile (shown only while that point is set), so one correction can be removed without clearing the others.
- **Text inside islands is for control instructions only** (how to use the controls, like Point correction's "keep clicking to refine, hold Alt…", or the curve's "click to add · drag…"). No explanations of how a filter works or how it was matched — those belong in the README / docs.

Fonts: **Inter** for the UI and **JetBrains Mono** for every number / value / `kbd` (both bundled in `ui/fonts`, SIL OFL).

Colour note: the app's base palette (`#061113` background, flat `#00f0be` accent) has drifted from the colour scheme (Mint background `#001E21`, Dark mint `#003D40`, Gradient 1 `#00EBFF → #00FFAA`, Gradient 2 `#E0BDFF → #00FFAC`, text `#E6E6E6`, Inter). Re-align it during the panel rework.

## 6. Unified window header (custom title bar)

Goal: **one header row** instead of the Windows title bar plus the app's own top bar, to save working space (about 32 px of height) and give the app one clean, branded strip.

### 6.1 Layout (left to right, one 46 px row)

`[FAE logo + name] [file name + size]  ·  [Zoom: Fit  31%]  ·  [Compare] [Split] | [Undo] [Redo] | [Presets ▾] [Copy edits] [Paste] [Reset]  [— ▢ ✕]`

- Logo, file info, zoom (centred: **Fit** + the zoom **percentage, which is the 100% button** — no separate 1:1), view and edit controls stay as in today's top bar.
- **Window buttons** on the far right: minimize, maximize / restore (icon swaps with the state), close. Same height as the bar, about 46 px wide each, no gap to the window edge. Hover: soft light background, **close turns red** (`#c42b1c`, white ✕) like native Windows.
- Empty space in the bar (and the logo / file name) is the **drag area**; buttons and other controls are not.

### 6.2 Behaviour to implement

- Window created **frameless** (pywebview `frameless=True`); the startup "Rust core is missing" window keeps the normal Windows frame.
- Python API: `win_minimize`, `win_toggle_maximize`, `win_close`, `win_is_maximized`; the UI keeps the maximize / restore icon in sync from window events.
- **Drag** the window from the bar through a native drag (Win32: release capture + `WM_NCLBUTTONDOWN` with `HTCAPTION`) rather than pywebview's emulated drag, so **Aero Snap** works: drag to the screen edges to snap, drag to the top to maximize, drag down off a maximized window to restore.
- **Double-click** the drag area = maximize / restore.
- **Resize by the edges and corners**: thin invisible strips (≈ 5 px, 10 px corners) at the window edges start a native resize (`WM_NCLBUTTONDOWN` with `HTLEFT` / `HTTOPRIGHT` / …). Hidden while maximized. `min_size` stays 1100×680.
- **Shadow and Windows 11 rounded corners** restored through DWM (`DwmExtendFrameIntoClientArea`, `DWMWA_WINDOW_CORNER_PREFERENCE`).
- **Maximized window must not cover the taskbar** (clamp to the monitor's work area if needed).
- Keyboard: Alt+F4, Win+arrows and Win+Z keep working.
- Not available with pywebview: the Snap Layouts flyout when hovering the maximize button (Win+Z still opens it). Accepted.

### 6.3 Size of the change

Additive; the editor, panels and Python back end are untouched. Roughly: frameless window + window API ~20 lines Python; window buttons ~15 HTML / ~30 CSS; drag, double-click and state sync ~25 JS; native drag / resize / DWM / work-area handling ~60–150 lines Python (Win32 via ctypes) + ~30 JS.

## 7. Suggested build order

Progress (implemented in waves):
- **Wave 1 — done:** colour scheme applied app-wide (dark teals moved to the scheme's hue: background / panel `#001E21`, surfaces from Dark mint, accent `#00FFAA`, text `#E6E6E6`); export block C (docked, fixed height, settings chip + encoder popover, progress line on its top edge); islands fold to one row (title or chevron; folded state remembered; reset / X on hover); Curves has a title in its head.
- **Wave 2 — done (tested on Windows):** unified header (section 6). The window keeps a real resizable frame (`WS_THICKFRAME | WS_CAPTION`) whose non-client area is collapsed to zero (`WM_NCCALCSIZE` in a window subclass), so Windows treats it as a normal window: Aero Snap, edge resize, Win+arrows, animations, shadow. Drag / resize start Windows' own loops from the page (`WM_NCLBUTTONDOWN`). Maximized = the monitor's work area, minus 2 px on an auto-hide taskbar's edge. Windows 11 rounded corners. Files: `fastedit/winchrome.py`, `ui/js/winchrome.js`. `--native-frame` keeps the normal Windows frame.
- Wave 3 — chain of instances + reorder (rows get grip and order number) + core list of steps (needs `core\build_core.bat`).
  - **Part 1 — done (tested on Windows):** the core runs the edit as an ordered list of steps (`core/fastedit-core/src/chain.rs`; core 0.12). Python: `fastedit_core.apply_chain(rgb, steps, …)` built by `fastedit/core.py` (`_steps`: `edit["steps"]` in panel order, else the flat edit in `DEFAULT_ORDER`); `apply_edit` removed. WASM: `Params.push_point / push_adjust / push_curves / push_bw / push_film / push_sc` filled by `ui/js/wasm-params.js` (same reading as `_steps`). A tool may appear more than once; Sharpen always runs last. In the old order the chain is bit-identical to the old fixed pipeline (Rust test + Python binding). Two deliberate changes, goldens re-recorded: Curves now run before the filters, and Mid/Detail smooths the image as it arrives at Adjustments (after Point correction). The preview keeps that smoothing while the image arriving at Adjustments is unchanged (parallel fingerprint). Parity on the PC: core 763/763, wasm 763/763; preview speed unchanged (2560×1440: 11–50 ms per frame).
  - Also: the lossy preview encode (JPEG / AVIF / lossy WEBP) runs only while the export block's encoded compare is on; the size chip shows only there. Turning it off kills a running encode.
  - **Part 2 — done:** the chain order is part of the edit: `edit.chain` = `[{ id, t }]` over the re-orderable tools (`CHAIN_TOOLS` in state.js; the first instance of a tool keeps its values in the flat keys, extra instances will carry their own in wave 6). `chainOf()` makes any chain whole (unknown tools dropped, missing ones added at their default place). `computeEdit` adds `E.steps` only when the order differs from the default; `jobFor` sends `edit.steps` with each tool's fields only. Edits live in memory per session, so undo / redo, per-image state, Copy edits / Paste and Reset carry the order with no migration. Parity: 8 reordered-chain cases (819 checks), PC and cloud recordings identical.
  - **Part 3 — done (tested on Windows):** the panel lists tools in run order — Crop, Resize, Color balance pinned at the top, the chain (drag grip + order number 1, 2, 3 … over the open islands; filled when unfolded), Sharpen pinned at the end. Drag by the grip: after a few pixels every re-orderable island folds to its row and the list slides so the held island stays under the pointer (temporary spacers where the list is too short); glowing drop line, edge auto-scroll, Esc cancels; on drop the islands that were open unfold again. The Histogram is now a fixed block docked to the panel's top (twin of the export block); the panel and file list can't be scrolled by `scrollIntoView` any more (`overflow: clip`).
- **Speed pass — done (core 0.13):**
  1. *Per-pixel engine* (`core/fastedit-core/src/px32.rs`): Point, Adjustments, Curves, the Filter and Film on 64-pixel chunks as separate R / G / B arrays, table lookups in loops of their own, no bounds checks (indices are in range by construction), so the compiler vectorizes the arithmetic. f64 kept: bit-identical to the reference (f32 was only ~10 % faster and put ±1 at many stages, which later steps can amplify). Mid/Detail's fade-in weight comes from a 4097-entry table instead of a `pow` per pixel (one pixel off by 1 level in the whole parity suite). Per-pixel runs read the source and write the final RGBA + histograms directly when they can (no separate copy-in / write-out pass); the working buffer is reused between frames.
  2. *Prefix cache* (preview only, `MdCache` in chain.rs): the image after the last Perceptual Saturation / Super Contrast step above the tool being changed is kept, so dragging a later tool doesn't redo those passes. Exact (reused only while the steps' parameters hash the same).
  - The self-test times full renders and prints a per-phase breakdown. On the PC at 7360×4912: Saturation + Contrast 95 → 78 ms, Mid/Detail while dragging 167 → under 60 ms; the rest is WebAssembly's per-pixel cost (~2× native) and the copy back to the page (~15 ms).
- **Step 3 — zoomed-in drafts — first version (to be tuned):** while a panel control is held (or edits keep coming within 200 ms) and less than half the image is on screen, only the visible part (+128 px margin for Sharpen) is rendered (`chain::run_region`). Perceptual Saturation, Super Contrast and Mid/Detail reuse the last full render's measurements (`ps::PsGrid`, `sc::ScStats`, the smoothing), so a draft equals that part of a full render (Sharpen within 3 levels); only that rectangle of the canvas is updated. A full render follows on release / after 200 ms idle. The histogram dims with a mint shimmer and a "live" tag meanwhile (approved).
  - **Middle zone — done (first version, to be checked on the PC).** Between fit and 1:1 the full-resolution stage used to flicker in and make dragging choppy (most of a 32 MP image visible). Now:
    1. zoomed past the small copy, a load of the full-resolution stage brings the small copy of the same stage along (`load_stage(..., lo=True)`, kept as `S.loImg`); while a control is held, a draft is whichever is cheaper: the visible part at full resolution, or a whole frame of the small copy drawn scaled up (`presentLo`, split views put the original / encoded side back from the full-resolution pixels). When the full-resolution draft isn't possible yet (e.g. the first Mid/Detail change, no smoothing kept), the small copy is used too. The sharp render follows on release, as before;
    2. no 50 % limit while the full-resolution stage is on screen (the small copy caps the cost);
    3. no flicker on the swap: the last frame stays on the canvas, scaled, until the new resolution's first render is ready. The preview worker keeps both images (`Preview` per image id, the two newest), so going back and forth costs no upload and each keeps its Mid/Detail smoothing and cached measurements; zooming back out reuses the small copy without a load. (Carrying the smoothing *between* resolutions is not done: a fresh full-resolution stage still computes it once);
    4. the swap happens at 1.35 screen pixels per small-copy pixel and back below 1.15 (`HIRES_IN` / `HIRES_OUT` in view.js; was 1.01 / 0.8).
  - Tested in headless Chrome against a stand-in back end (6000×4000): no blank frames on either swap, small-copy frames in the middle zone, visible-part drafts zoomed in, split views correct, self-test page all OK.
- Wave 4 — per-tool presets with thumbnails. Wave 5 — chain presets. Wave 6 — stacking UI.

1. ~~Shared slider component~~ — standard slider done; alternative (gradient tracks) still to design.
2. Unified window header (section 6) — independent of the panel work, can be done any time; doing it early gives the panel the extra height.
3. Chain-order data model: ordered list of **instances** per image (section 8.2), used by preview (WASM / JS) and Process (Rust core, list of steps), included in undo, copy / paste.
4. Collapsible rows + drag reorder in the panel.
5. Preset storage + per-tool preset dropdown + thumbnails rendered from the current image.
6. Chain presets in the top bar.
7. Stackable filters UI (section 8): instance tags, Duplicate / row menu, tiles that add instances.

## 8. Stackable filters — multiple instances of the same filter (design draft)

Status: **approved** (decisions in 8.7). Mockups: boards "Stacked filters · panel" and "Stacked filters · Filters popover" on the right-panel canvas.

Goal: the same filter can be in the chain more than once, each copy with its own values, preset, switch and position — e.g. Film Effect as a base look early in the chain and a second Film Effect for grain later, or two Sharpen passes.

### 8.1 Which tools can stack

| Stacks | Single only |
|---|---|
| Perceptual Saturation, Film Effect, Super Contrast, Adjustments, Curves | Crop & rotate, Point correction, Resize, Color balance, Sharpen |

Single-only tools are about the image as a whole (geometry, output size, white balance / points) or, for Sharpen, a final output pass.

### 8.2 Data model

- The ordered chain (section 7, step 3) is a list of **instances**: `{ id, type, on, label, preset, values }`. `type` is the tool (`f_film`, `sharpen`, …), `id` is unique per image, `values` holds that instance's slider / select values (no more one global `flt_film_*` set per image).
- Old edits (flat keys), saved settings and pasted edits are **migrated** on load: one instance per tool that has values, in today's fixed order.
- Undo / redo, Copy edits / Paste and per-image storage all carry the whole chain (instances, order, labels).
- Build the chain-order data model with instances **from the start**, even if the stacking UI comes later — it avoids a second migration.

### 8.3 Pipeline

- Preview (WASM) and Process (Rust core) run the chain **in order, instance by instance**. The core API moves from fixed slots (one `film`, one `sc`, one `sharp`, …) to a list of steps `[{ kind, params }]`; every step keeps its own Opacity.
- Each instance is a full pass, so the cost grows with the chain: **max 8 instances per type** (the tile's Add and Duplicate are disabled at 8).
- Parity tests get chain cases (same filter twice, reordered chains).

### 8.4 Panel (rows)

- Rows of the same type get a small **instance tag** after the title (a small pill: `Film Effect [1]`, `Film Effect [2]`) mono, lavender from **Gradient 2** (`#E0BDFF`) so it reads apart from the mint order number. The tag counts the instances of that type **in chain order** (renumbers when moved). A tool with one instance shows no tag.
- Optional **label** after the tag (`· base`, `· grain`), set with Rename; stays with the instance when it moves.
- Expanded header: **Duplicate** icon, island reset, X (remove).
- **Row menu** (right-click a row, or `⋯`): Duplicate `Ctrl+D`, Rename… `F2`, Move up, Move down, Reset, Remove `Del`.
- **Duplicate** inserts the copy right after the original with the same values and preset, expanded, and scrolls to it.
- Each instance has its own switch and preset button; turning one copy off leaves the others running.

### 8.5 Filters popover (tiles)

- A tile **adds a new instance** (end of the chain, opens expanded, panel scrolls to it); it no longer shows / hides an island.
- The tile shows a **count badge** (`×2`) and where the copies are (`#2, #4 in chain`); hover shows an **Add** button. The on / off switch moves off the tile — switches belong to instances.
- **Drag a tile into the panel** to insert the new instance at the drop line.
- Header count becomes `N in chain`. Footer line in the popover (control instructions only): *Click adds to the end of the chain · Drag a tile into the panel to place it*.

### 8.6 Presets with instances

- A per-tool preset applies to **that instance** only.
- Chain presets store every instance (type, values, order, labels) — a preset can contain the same filter twice.
- Preset thumbnails render the chain **up to and including** the instance being previewed.

### 8.7 Decisions

- Stackable: the three filters, Adjustments, Curves. Single: Crop, Point correction, Resize, Color balance, **Sharpen**.
- Cap: **8 per type**.
- Instance tags **follow chain order** (renumber after a move).
- Rust core changes are rebuilt on the Windows PC with `core\build_core.bat` when asked.

### 8.8 Chain order (decided)

- **Pinned, not draggable:** Crop & rotate, Resize, Color balance run first (they change the pixel grid / need the whole image); Sharpen runs last (output pass).
- **Re-orderable:** Point correction, Adjustments, Curves, Perceptual Saturation, Film Effect, Super Contrast (and their extra instances).
- **Default order = the tool pill order:** Point correction → Adjustments → Curves → Perceptual Saturation → Film Effect → Super Contrast. (Mid/Detail stays part of Adjustments.)
- **Applies to old edits too:** migrated images use the new default order (today Curves run after the filters; images that use both may shift slightly). Parity goldens are re-recorded on purpose for this change.
