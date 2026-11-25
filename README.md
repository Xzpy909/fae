<p align="center"><img src="ui/logo.svg" width="112" alt="Fae logo"></p>

<h1 align="center">Fae</h1>

<p align="center"><b>Fast Image Editor.</b> Batch-prepare images quickly, mostly from the keyboard.</p>

![Fae](https://github.com/Xzpy909/fae/blob/main/preview%201.webp?raw=true)

<!-- more screenshots go here -->

## What it does

- **Crop, rotate and straighten** with aspect presets and a crop that never leaves the image
- **Colour**: black / gray / white eyedroppers, curves, automatic colour balance, Resolve-style adjustments, and filters (Film Effect, Super Contrast, Perceptual Saturation, Sharpen)
- **Resize** to a preset (1080p, 720p, …), a target megapixel count or a custom size
- **Export** to lossless WEBP / PNG or lossy JPEG (jpegli), AVIF (SVT-AV1) or WEBP, with a split view of the real encode before you save
- **Batch**: every image keeps its own edits; copy edits between images, save whole edits as presets, then Process all
- **Fast**: a Rust back end, the live preview on the GPU (WebGPU), a single `Fae.exe` with no Python at run time

Full list of tools and keyboard shortcuts: [docs/features.md](docs/features.md).

## Getting started

1. Download the zip from [Releases](../../releases) and unpack it anywhere.
2. Run `Fae.exe`, then drag in images or whole folders.
3. Press `Ctrl+S` (or **Process**) to save. Output goes next to the source as `name_WxH.ext` and never overwrites a file.

Needs Windows 10/11 (WebView2 is part of Windows). Settings and presets are saved next to `Fae.exe`.

## Building

```
pip install -r requirements.txt
build.bat                  # -> dist\Fae\Fae.exe
```

Needs Rust from https://rustup.rs (MSVC build tools) and Python 3.10+. Encoders, command-line options,
tests and how the cores work: [docs/development.md](docs/development.md).

## Trademarks

Film stock names used for the Film Effect presets are trademarks of their respective owners. The presets are approximations made for this project and are not affiliated with or endorsed by them.
