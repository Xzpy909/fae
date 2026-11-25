# Fae: features and shortcuts

Everything Fae does, tool by tool. Back to the [README](../README.md).

## Features

**Workflow**
- Drag and drop images or whole folders, or use **+ Files** / **+ Folder**
- Every image keeps its own edits and undo history
- Copy / paste edits between images
- **Process** the current image, or **Process all**, where each image uses its own edits
- Optionally jump to the next image after processing
- Nothing is written until you press Process. Output goes next to the source as `name_WxH.ext` and never overwrites existing files

**Geometry**
- Crop & rotate mode with 90° steps, fine angle ruler, horizontal flip and aspect presets (Free, Original, 1:1, 16:9, 9:16, 4:3, 3:2, …)
- Bound-aware crop: the crop box can never leave the rotated image
- Wheel zooms the crop toward the cursor; `Alt`/`Ctrl` + wheel rotates (fine angle)
- **Straighten** (`S` or the ruler button): draw a line along a horizon, wall or edge; the image turns so the line becomes exactly level or upright, whichever is nearer
- The view follows the crop: as the box gets smaller the camera zooms in and pans to keep its edges near the screen edges

**Colour**
- Black / neutral-gray / white point eyedroppers that stay active, so you can keep clicking to refine
- Hold **Alt** for a clipping / threshold view (shadows, highlights or neutral candidates); scroll to move the threshold
- Master + R/G/B curves (monotone cubic) with live input histogram
- Output histogram with clipping percentages
- Automatic colour balance: Percentile White-Patch and Gray-Edge, each with a strength control
- **Adjustments** modelled on DaVinci Resolve's Primaries: Color Boost, Shadows, Highlights, Saturation, Temp, Tint, Contrast + Pivot and Mid/Detail, matched to Resolve's output within about half a level (0–255) on reference grades
- **Filters** (the Filters button at the end of the tool buttons opens a searchable grid of filter tiles; a click adds the filter to the end of the chain, or drag a tile into the panel to put it where the drop line shows; a tile shows how many are in the chain and where, e.g. `×2 · #2, #4 in chain`): Perceptual Saturation filter with Saturation and Warmth matched to the reference filter within 1 level (0–255), Perceptual Saturation (marked ≈) as an **approximation** that pushes colours away from their surroundings but does not match the reference exactly, and Opacity to blend the result. Film Effect: film presets (colour tables fitted on reference renders; the Film field opens a list of the films on your image, hover previews, click picks) plus Brightness and Contrast. Super Contrast: Correct Color Cast and Correct Contrast matched to the reference within 1 level, Dynamic Contrast (marked ≈) as an approximation (local, contrast-limited equalization), and Opacity.
- Jog-wheel controls: drag the wheel left/right (`Shift` fine, `Ctrl` coarse), double-click to reset. Every slider value can also be dragged left/right
- **Title bar:** menu bar (File · Edit · Image · View · Help, every command with its shortcut) and the current file on the left; the view toolbar in the centre (Fit / 100% / zoom levels, compare, split view); search, undo / redo and Settings next to the window buttons
- **Tool dock** floating at the bottom of the image shows or hides each tool's card; it can be turned off in Settings › Preferences (the tools stay in the Image menu and the search)
- **Search** (loupe or `Ctrl+Space`): finds tools, filters, edit presets and every menu command
- **Stack tools**: Adjustments, Curves and the filters can be in the chain more than once (up to 8 each), each copy with its own values, switch, place and optional label — Duplicate icon in the island's head or `Ctrl+D`; the Adjustments / Curves button adds another copy once the tool is open, like a Filters tile does for an open filter (or drag the tile into the panel); right-click an island's head for Duplicate / Rename / Move up / Move down / Reset / Remove (`F2`, `Del`). Copies show `[1]`, `[2]` in chain order. Every Adjustments copy has its own Mid/Detail
- **Presets** on Adjustments, Curves, Perceptual Saturation, Super Contrast and Sharpen: the button in the tool's row shows the current preset (`Default`, a name, or `Custom`) and opens a list with thumbnails of the current image through each preset; hover previews it on the image, click applies it (one undo step); save the current values as a preset, rename or delete your own
- **Edit presets** (Presets in the top bar): the whole edit in one preset — Resize, Color balance, Adjustments, Curves, the filters and Sharpen with their order and on / off state. Cards with a thumbnail of the current image through the preset and the tools it uses; hover previews (Resize and Color balance take effect on click), click applies (one undo step), ⋯ renames, updates with the current edits or deletes. Crop, point correction and the output format stay with the image

*Film stock names used for the Film Effect presets are trademarks of their respective owners. The presets are approximations made for this project and are not affiliated with or endorsed by them.*

**Resize**
- **Preset**: aspect ratio (16:9, 21:9, 4:3, 3:2, 1:1, 4:5, 9:16) × resolution (2160p, 1440p, 1080p, 720p, 480p, 360p; the p-number is the short side), cover-crop
- **Target Mpix** with *Mpix priority*, *Balanced* or *Ratio error priority*
- **Custom**: type in width × height, then *Crop in* (cover-crop) or *Stretch*
- Warns when the resize would upscale

**Input formats**

JPEG, PNG, WEBP, AVIF, BMP, GIF, TIFF, TGA, PPM/PGM, ICO, plus **HEIC/HEIF** and **JPEG XL** (load only).

**Output formats**

| Format | Encoder | Settings |
|---|---|---|
| WEBP | libwebp, `method=6` | Lossless, or Lossy + quality |
| PNG | Pillow's filter choice, zlib level 9 | — |
| JPEG | **jpegli** (`encoders/cjpegli.exe`), libjpeg-turbo fallback | Quality *or* Butteraugli distance |
| AVIF | **SVT-AV1** (`encoders/SvtAv1EncApp.exe`), 10-bit 4:4:4, `--tune 3`; rav1e fallback | CRF (0.25 steps), preset 0–8 |

- ICC profiles from the source are carried over
- **Split-view compare** for lossy formats: drag the divider. Left is the lossless edit, right is the actual encode decoded back
- The encoded file size and bits per pixel are shown at the image's bottom-right corner

Pipeline order: `geometry → resize → White-Patch → Gray-Edge → point correction → curves → encode`

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Ctrl+S` / `Enter` | Process current image |
| `Ctrl+O` | Add files |
| `Ctrl+Space` | Search tools, presets and commands |
| `Ctrl+,` | Settings |
| `↑` / `↓`, `PgUp` / `PgDn` | Previous / next image |
| `Ctrl+Z`, `Ctrl+Y` / `Ctrl+Shift+Z` | Undo / redo |
| `Ctrl+V` | Open the images on the clipboard: copied files, or a copied image (saved as PNG in `Pictures\Fae\Pasted` first) |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste edits |
| `B` / `G` / `W` | Black / gray / white eyedropper |
| `Esc` | Drop the eyedropper |
| `Alt` (hold) | Clipping / threshold view; scroll adjusts the threshold (Shift = ×5) |
| `\` (hold) | Before view (point correction and curves off) |
| `C` | Crop & rotate mode |
| `0` / `1` | Fit / actual pixels |
| `+` / `-`, mouse wheel | Zoom |
| `Space` + drag, middle drag | Pan |
| Double-click image | Toggle fit / 1:1 |

**Crop mode**

| Key | Action |
|---|---|
| `Enter` / `C` | Apply |
| `Esc` | Cancel |
| `[` / `]` | Rotate 90° left / right |
| `,` / `.` | Fine angle ±0.1° (Shift = ±1°) |
| `H` | Flip horizontal |
| `S` | Straighten: draw a line along something that should be level or upright (Shift while drawing = steadier aim, Esc = cancel) |
| `R` | Reset |
| Mouse wheel | Zoom the crop in / out toward the cursor (Shift = finer) |
| `Alt` / `Ctrl` + wheel | Fine angle ±0.1° (Shift = ±1°) |
| Arrow keys | Nudge crop (Shift = 10 px) |

**Curves and sliders:** click to add a point, drag to move it, double-click it or drag it out of the box to remove it. Double-click a slider to reset it. A slider's value sits at the end of its track as plain text: click it to type, drag it or scroll over it to step (Shift = ×10).

## Notes on encoding

- **JPEG (jpegli)**: the image is handed to `cjpegli` as a PNG with its ICC profile. Transparent images are flattened onto white.
- **AVIF (SVT-AV1)**: RGB is converted to 10-bit full-range BT.709 YUV 4:4:4 and encoded as a still picture with `--avif 1 --tune 3`. The AV1 bitstream is then wrapped into an `.avif` container by the app itself, so no `avifenc` is needed. The file is tagged as sRGB / full range, plus ICC if present. Images with alpha are encoded with rav1e (alpha as a second 10-bit image in the file).
- **WEBP lossless** uses `quality=100, method=6, exact=True` (maximum effort).
