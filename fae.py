"""
Fae (Fast Image Editor) - pywebview front end, numpy/OpenCV/Pillow back end.

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

Code layout: fastedit/ (Python back end), ui/ (front end: index.html, style.css, js/).

Requirements:
    pip install pywebview pillow numpy opencv-python
    optional input formats: pip install pillow-heif (HEIC/HEIF)  pillow-jxl-plugin (JPEG XL)

Run:
    python fae.py [images or folders...]   (--debug opens dev tools; --no-gpu; --test-filters shows the
                                            filter kit's test filters, docs/filter-kit.md)
"""

from fastedit.app import main

if __name__ == "__main__":
    main()
