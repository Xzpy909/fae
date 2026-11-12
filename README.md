# Fast Image Editor

A fast, keyboard-friendly desktop editor for batch-preparing images: crop and rotate, colour-correct with eyedroppers and curves, resize to 720p or a target megapixel count, then export to lossless WEBP/PNG or lossy JPEG (jpegli), AVIF (SVT-AV1) or WEBP. Lossy formats have a split-view preview, so you can judge the real encode at the chosen settings before saving.

Built with Python + [pywebview](https://pywebview.flowrl.com/). The UI is HTML/JS; image processing uses NumPy, OpenCV and Pillow.

## Preview

![img](https://github.com/Xzpy909/fast-image-editor/blob/main/preview%201.jpg?raw=true)

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
- Wheel rotates; `Alt`/`Ctrl` + wheel zooms the crop toward the cursor
- The view follows the crop: as the box gets smaller the camera zooms in and pans to keep its edges near the screen edges

**Colour**
- Black / neutral-gray / white point eyedroppers that stay active, so you can keep clicking to refine
- Hold **Alt** for a clipping / threshold view (shadows, highlights or neutral candidates); scroll to move the threshold
- Master + R/G/B curves (monotone cubic) with live input histogram
- Output histogram with clipping percentages
- Automatic colour balance: Percentile White-Patch and Gray-Edge, each with a strength control

**Resize**
- 720p cover-crop (1280×720)
- Target Mpix with *Mpix priority*, *Balanced* or *Ratio error priority*

**Output formats**

| Format | Encoder | Settings |
|---|---|---|
| WEBP | libwebp (Pillow), `method=6` | Lossless, or Lossy + quality |
| PNG | Pillow, `compress_level=9` | — |
| JPEG | **jpegli** (`encoders/cjpegli.exe`), Pillow fallback | Quality *or* Butteraugli distance |
| AVIF | **SVT-AV1** (`encoders/SvtAv1EncApp.exe`), 10-bit 4:4:4, `--tune 3`; Pillow/aom fallback | CRF (0.25 steps), preset 0–8 |

- ICC profiles from the source are carried over
- **Split-view compare** for lossy formats: drag the divider. Left is the lossless edit, right is the actual encode decoded back
- The encoded file size and bits per pixel are shown at the image's bottom-right corner

Pipeline order: `geometry → resize → White-Patch → Gray-Edge → point correction → curves → encode`

## Requirements

- Windows 10/11 (the bundled encoders are Windows builds; the Python code itself is portable)
- Python 3.10+
- Python packages:

```
pip install -r requirements.txt
```

(`pywebview>=5.0`, `pillow>=11.2`, `numpy`, `opencv-python`). Pillow 11.2 or newer is needed for AVIF decoding in the preview and for the aom fallback.

### Encoders

The encoder binaries are **not in the repository**. They come with the [release](../../releases) zip. Put them in the `encoders/` folder:

```
encoders/
├── cjpegli.exe        # jpegli, from libjxl
└── SvtAv1EncApp.exe   # SVT-AV1 (SVT-AV1-HDR fork)
```

Both are optional:
- Without `cjpegli.exe`, JPEG uses Pillow's libjpeg-turbo, which has no distance mode.
- Without `SvtAv1EncApp.exe`, AVIF uses Pillow's libavif/aom.

The app shows which encoder is in use.

## Running

```
run.bat [images or folders...]
```

or

```
python fast_image_editor.py [images or folders...]
python fast_image_editor.py --debug     # opens the web inspector
```

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Ctrl+S` / `Enter` | Process current image |
| `Ctrl+O` | Add files |
| `↑` / `↓`, `PgUp` / `PgDn` | Previous / next image |
| `Ctrl+Z`, `Ctrl+Y` / `Ctrl+Shift+Z` | Undo / redo |
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
| `R` | Reset |
| Mouse wheel | Fine angle ±0.1° (Shift = ±1°) |
| `Alt` / `Ctrl` + wheel | Zoom the crop in / out toward the cursor (Shift = finer) |
| Arrow keys | Nudge crop (Shift = 10 px) |

**Curves and sliders:** click to add a point, drag to move it, double-click it or drag it out of the box to remove it. Double-click a slider to reset it. Scroll over a number box to step its value (Shift = ×10).

## Notes on encoding

- **JPEG (jpegli)**: the image is handed to `cjpegli` as a PNG with its ICC profile. Transparent images are flattened onto white.
- **AVIF (SVT-AV1)**: RGB is converted to 10-bit full-range BT.709 YUV 4:4:4 and encoded as a still picture with `--avif 1 --tune 3`. The AV1 bitstream is then wrapped into an `.avif` container by the app itself, so no `avifenc` is needed. The file is tagged as sRGB / full range, plus ICC if present. Images with alpha use the aom fallback.
- **WEBP lossless** uses `quality=100, method=6, exact=True` (maximum effort).

## Project layout

```
fast_image_editor.py   # back end: image pipeline, encoders, pywebview API, local HTTP server
ui/
├── index.html
├── style.css
├── app.js             # live preview, point correction, curves, split compare
└── crop.js            # crop & rotate mode
encoders/              # cjpegli.exe, SvtAv1EncApp.exe (from the release)
run.bat
requirements.txt
```

Settings are saved to `fast_image_editor_settings.json` next to the script.
