# Redesign: right panel, presets, sliders and window header

Status: **design approved, not implemented yet.** This is the plan for the next round of UI work.

## 1. Right panel layout — collapsible tool rows (approved: design C)

The panel keeps two fixed islands and a re-orderable list between them:

| Position | Island | Re-orderable | Preset button |
|---|---|---|---|
| Top (fixed) | Histogram | no | no |
| Middle | every tool / filter the user has open | **yes** | yes, where it makes sense (see 3.3) |
| Bottom (fixed) | Export (format, encoder settings, Process, Process all) | no | no |

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

### 1.4 What stays from the current panel

- The X (delete: reset + close) and reset buttons keep their behaviour; in collapsed rows they move into the expanded header (or a row hover) so the collapsed row stays clean.
- The Filters popover (top-right pill, tile grid) still adds filters; a newly added filter appears at the end of the chain and opens expanded.

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

## 5. Sliders — one standard slider for the whole app (next design task)

Make every slider in the app look the same; the mockups' slider is the starting point:

- **Standard slider**: thin rounded track (4 px), accent fill from the zero point (from the left for 0…max, from the centre for −…+ ranges), **white pill / round thumb** with a soft shadow, a monospace value box on the right, the label on the left in a fixed-width column so all tracks line up.
- **Alternative slider**: same layout, thumb is a **downward-pointing triangle** (for tracks that show a gradient of what the slider does, like the filter gradient tracks, or for target / threshold values). Exact use to be settled in the slider design.
- Double-click resets to default (as today); scroll-wheel and number box behaviour stay.

This is the next design step after the panel; build both sliders as one shared component before the panel rework so the new rows use it from the start.

## 6. Unified window header (custom title bar)

Goal: **one header row** instead of the Windows title bar plus the app's own top bar, to save working space (about 32 px of height) and give the app one clean, branded strip.

### 6.1 Layout (left to right, one 46 px row)

`[FAE logo + name] [file name + size]  ·  [Zoom: Fit  1:1  31%]  ·  [Compare] [Split] | [Undo] [Redo] | [Presets ▾] [Copy edits] [Paste] [Reset]  [— ▢ ✕]`

- Logo, file info, zoom (centred), view and edit controls stay as in today's top bar.
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

1. Shared slider component (standard + alternative) and switch over existing sliders.
2. Unified window header (section 6) — independent of the panel work, can be done any time; doing it early gives the panel the extra height.
3. Chain-order data model: ordered tool list per image, used by preview (WASM / JS) and Process (Rust core), included in undo, copy / paste.
4. Collapsible rows + drag reorder in the panel.
5. Preset storage + per-tool preset dropdown + thumbnails rendered from the current image.
6. Chain presets in the top bar.
