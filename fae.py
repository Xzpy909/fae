"""
Fae (Fast Image Editor) - pywebview front end, Rust back end (core/backend, fae_backend).

What it does (per image, nothing is written until you press Process):
    Output     : Resize - Preset (aspect + 2160p..360p, cover-crop), Target Mpix (with priority),
                 or Custom W x H (crop in or stretch).
                 WEBP (lossless or lossy), PNG, JPEG via encoders/cjpegli.exe (Pillow fallback),
                 AVIF 10-bit 4:4:4 via encoders/SvtAv1EncApp.exe (--tune 3), muxed here;
                 Pillow's libavif/aom as fallback. Lossy formats get a split-view preview.
    Color      : Percentile White-Patch, Gray-Edge color constancy
    Points     : Black / neutral-gray / white eyedroppers (stay active - keep clicking)
    Curves     : Master + R/G/B curves with live histogram
    Alt (hold) : clipping / threshold view that helps you find where to click

Pipeline order:
    resize -> White-Patch -> Gray-Edge -> point correction -> curves

Code layout: fastedit/ (the pywebview shell around the Rust back end), ui/ (front end: index.html, style.css, js/).

Requirements:
    pip install -r requirements.txt, then core\build_core.bat and core\backend\build_backend.bat
    optional input formats: pip install pillow-heif (HEIC/HEIF)  pillow-jxl-plugin (JPEG XL)

Run:
    python fae.py [images or folders...]   (--debug opens dev tools; --no-gpu; --test-filters shows the
                                            filter kit's test filters, docs/filter-kit.md)
"""

from fastedit.app import main

if __name__ == "__main__":
    main()
