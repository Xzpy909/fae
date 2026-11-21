"""AVIF output: SVT-AV1 + own container muxer, or Pillow's libavif/aom."""
from __future__ import annotations

import io
import os
import struct
import tempfile
import numpy as np
from PIL import Image, features
from .config import AOM_Q_DEFAULT, AOM_SPEED_DEFAULT, AVIF_CRF_DEFAULT, AVIF_PRESET_DEFAULT, SVTAV1
from .proc import check_cancel, run_encoder


# ---------------------------------------------------------------------------
# AVIF via SvtAv1EncApp: RGB -> 10-bit full-range BT.709 YUV 4:4:4 -> SVT (OBU) -> AVIF container
# ---------------------------------------------------------------------------


class _Bits:
    def __init__(self, data: bytes):
        self.d, self.p = data, 0

    def f(self, n: int) -> int:
        v = 0
        for _ in range(n):
            byte = self.d[self.p >> 3]
            v = (v << 1) | ((byte >> (7 - (self.p & 7))) & 1)
            self.p += 1
        return v

    def uvlc(self) -> int:
        lz = 0
        while not self.f(1):
            lz += 1
            if lz >= 32:
                return (1 << 32) - 1
        return (1 << lz) - 1 + self.f(lz)


def _leb128(d: bytes, p: int) -> tuple[int, int]:
    v = 0
    for i in range(8):
        b = d[p + i]
        v |= (b & 0x7F) << (7 * i)
        if not b & 0x80:
            return v, p + i + 1
    raise ValueError("bad leb128")


def split_obus(data: bytes) -> list[tuple[int, bytes]]:
    """[(obu_type, full obu bytes incl. header)], requires obu_has_size_field."""
    out, p = [], 0
    while p < len(data):
        start = p
        h = data[p]
        typ, ext, has_size = (h >> 3) & 0xF, (h >> 2) & 1, (h >> 1) & 1
        p += 1 + ext
        if not has_size:
            raise ValueError("OBU without size field")
        size, p = _leb128(data, p)
        p += size
        out.append((typ, data[start:p]))
    return out


def parse_seq_header(obu: bytes) -> dict:
    h = obu[0]
    p = 1 + ((h >> 2) & 1)
    _, p = _leb128(obu, p)
    b = _Bits(obu[p:])
    prof = b.f(3)
    b.f(1)  # still_picture
    reduced = b.f(1)
    tier = 0
    if reduced:
        level = b.f(5)
    else:
        timing = b.f(1)
        dmi = 0
        bdl = 0
        if timing:
            b.f(32); b.f(32)
            if b.f(1):
                b.uvlc()
            dmi = b.f(1)
            if dmi:
                bdl = b.f(5) + 1
                b.f(32); b.f(5); b.f(5)
        idd = b.f(1)
        cnt = b.f(5) + 1
        level = None
        for i in range(cnt):
            b.f(12)
            lv = b.f(5)
            t = b.f(1) if lv > 7 else 0
            if i == 0:
                level, tier = lv, t
            if dmi and b.f(1):
                b.f(bdl); b.f(bdl); b.f(1)
            if idd and b.f(1):
                b.f(4)
    wb, hb = b.f(4) + 1, b.f(4) + 1
    b.f(wb); b.f(hb)
    if not reduced and b.f(1):
        b.f(4); b.f(3)
    b.f(1); b.f(1); b.f(1)
    if not reduced:
        b.f(1); b.f(1); b.f(1); b.f(1)
        order_hint = b.f(1)
        if order_hint:
            b.f(1); b.f(1)
        force_sct = 2 if b.f(1) else b.f(1)
        if force_sct > 0:
            if not b.f(1):
                b.f(1)
        if order_hint:
            b.f(3)
    b.f(1); b.f(1); b.f(1)
    # color_config
    hbd = b.f(1)
    twelve = b.f(1) if (prof == 2 and hbd) else 0
    mono = 0 if prof == 1 else b.f(1)
    cp = tc = mc = 2
    if b.f(1):
        cp, tc, mc = b.f(8), b.f(8), b.f(8)
    csp = 0
    if mono:
        sx = sy = 1
    elif cp == 1 and tc == 13 and mc == 0:
        sx = sy = 0
    else:
        b.f(1)
        if prof == 0:
            sx = sy = 1
        elif prof == 1:
            sx = sy = 0
        elif twelve:
            sx = b.f(1)
            sy = b.f(1) if sx else 0
        else:
            sx, sy = 1, 0
        if sx and sy:
            csp = b.f(2)
    return {"profile": prof, "level": level, "tier": tier, "high_bitdepth": hbd, "twelve_bit": twelve,
            "mono": mono, "sx": sx, "sy": sy, "csp": csp,
            "depth": 12 if twelve else (10 if hbd else 8)}


def _box(typ: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", 8 + len(payload)) + typ + payload


def _fbox(typ: bytes, ver: int, flags: int, payload: bytes) -> bytes:
    return _box(typ, struct.pack(">I", (ver << 24) | flags) + payload)


def mux_avif(obu_stream: bytes, width: int, height: int, icc: bytes | None = None,
             nclx=(1, 13, 1, 1)) -> bytes:
    """Wrap a single-frame AV1 OBU stream into a still-image AVIF file."""
    obus = split_obus(obu_stream)
    seq = next((o for t, o in obus if t == 1), None)
    if seq is None:
        raise ValueError("no sequence header in AV1 stream")
    item = b"".join(o for t, o in obus if t not in (2, 15))   # drop temporal delimiters / padding
    s = parse_seq_header(seq)
    av1c = bytes([
        0x81,
        (s["profile"] << 5) | (s["level"] & 0x1F),
        (s["tier"] << 7) | (s["high_bitdepth"] << 6) | (s["twelve_bit"] << 5) | (s["mono"] << 4)
        | (s["sx"] << 3) | (s["sy"] << 2) | (s["csp"] & 3),
        0,
    ]) + seq
    ch = 1 if s["mono"] else 3
    props = [
        _fbox(b"ispe", 0, 0, struct.pack(">II", width, height)),
        _fbox(b"pixi", 0, 0, bytes([ch] + [s["depth"]] * ch)),
        _box(b"av1C", av1c),
        _box(b"colr", b"nclx" + struct.pack(">HHHB", nclx[0], nclx[1], nclx[2], (nclx[3] & 1) << 7)),
    ]
    if icc:
        props.append(_box(b"colr", b"prof" + icc))
    assoc = [(1, False), (2, False), (3, True), (4, False)] + ([(5, False)] if icc else [])
    ipma = _fbox(b"ipma", 0, 0, struct.pack(">IHB", 1, 1, len(assoc))
                 + bytes(((0x80 if ess else 0) | idx) for idx, ess in assoc))
    iprp = _box(b"iprp", _box(b"ipco", b"".join(props)) + ipma)
    ftyp = _box(b"ftyp", b"avif" + struct.pack(">I", 0) + b"avifmif1miaf")
    hdlr = _fbox(b"hdlr", 0, 0, struct.pack(">I", 0) + b"pict" + bytes(12) + b"\0")
    pitm = _fbox(b"pitm", 0, 0, struct.pack(">H", 1))
    iinf = _fbox(b"iinf", 0, 0, struct.pack(">H", 1)
                 + _fbox(b"infe", 2, 0, struct.pack(">HH", 1, 0) + b"av01" + b"\0"))

    def build(offset: int) -> bytes:
        iloc = _fbox(b"iloc", 0, 0, bytes([0x44, 0x00]) + struct.pack(">HHHHII", 1, 1, 0, 1, offset, len(item)))
        return _fbox(b"meta", 0, 0, hdlr + pitm + iloc + iinf + iprp)

    meta = build(0)
    offset = len(ftyp) + len(meta) + 8      # data starts right after the mdat header
    meta = build(offset)
    return ftyp + meta + _box(b"mdat", item)


def rgb_to_yuv444p10(rgb) -> bytes:
    """uint8 HxWx3 sRGB -> planar 10-bit little-endian Y, Cb, Cr (full range, BT.709 matrix)."""
    f = rgb.astype(np.float32) * (1.0 / 255.0)
    r, g, b_ = f[..., 0], f[..., 1], f[..., 2]
    y = 0.2126 * r + 0.7152 * g + 0.0722 * b_
    cb = (b_ - y) / 1.8556
    cr = (r - y) / 1.5748
    planes = [y * 1023.0, cb * 1023.0 + 512.0, cr * 1023.0 + 512.0]
    return b"".join(np.clip(np.rint(pl), 0, 1023).astype("<u2").tobytes() for pl in planes)


def ivf_payload(data: bytes) -> bytes:
    """IVF container -> concatenated OBUs of the first frame (raw OBU input passes through)."""
    if data[:4] != b"DKIF":
        return data
    hdr = struct.unpack("<H", data[6:8])[0]
    size = struct.unpack("<I", data[hdr:hdr + 4])[0]
    return data[hdr + 12:hdr + 12 + size]


# ---------------------------------------------------------------------------
# AVIF encoding: SVT-AV1 (preferred) or Pillow's libavif/aom (fallback, also used for alpha)
# ---------------------------------------------------------------------------
def svt_available() -> bool:
    return os.path.isfile(SVTAV1)


def avif_available() -> bool:
    """Pillow AVIF support: needed to decode the preview and for the aom fallback."""
    try:
        return bool(features.check("avif"))
    except Exception:
        return False


def _clampi(v, lo, hi, default):
    try:
        return int(min(hi, max(lo, round(float(v)))))
    except (TypeError, ValueError):
        return default


def avif_opts(o) -> dict:
    o = o if isinstance(o, dict) else {}
    try:
        crf = float(o.get("crf", AVIF_CRF_DEFAULT))
    except (TypeError, ValueError):
        crf = AVIF_CRF_DEFAULT
    crf = round(min(70.0, max(1.0, crf)) * 4) / 4          # SVT takes 0.25 steps
    return {"enc": "aom" if o.get("enc") == "aom" else "svt",     # chosen in the UI; svt falls back to aom
            "crf": crf, "preset": _clampi(o.get("preset"), 0, 8, AVIF_PRESET_DEFAULT),
            # separate settings for Pillow's libavif/aom (used without SvtAv1EncApp, or for alpha)
            "aom_q": _clampi(o.get("aom_q"), 0, 100, AOM_Q_DEFAULT),
            "aom_speed": _clampi(o.get("aom_speed"), 0, 10, AOM_SPEED_DEFAULT)}


def encode_avif_svt(img: Image.Image, o: dict, icc: bytes | None = None) -> bytes:
    rgb = np.ascontiguousarray(np.asarray(img.convert("RGB")))
    h, w = rgb.shape[:2]
    with tempfile.TemporaryDirectory(prefix="fie_avif_") as td:
        src, dst = os.path.join(td, "in.yuv"), os.path.join(td, "out.ivf")
        with open(src, "wb") as f:
            f.write(rgb_to_yuv444p10(rgb))
        check_cancel()
        cmd = [SVTAV1, "-i", src, "-w", str(w), "-h", str(h),
               "--input-depth", "10", "--color-format", "444", "-n", "1", "--avif", "1",
               "--tune", "3", "--crf", f"{o['crf']:g}", "--preset", str(o["preset"]),
               "--color-primaries", "1", "--transfer-characteristics", "13",
               "--matrix-coefficients", "1", "--color-range", "1",
               "--progress", "0", "-b", dst]
        r = run_encoder(cmd)
        if r.returncode != 0 or not os.path.isfile(dst) or os.path.getsize(dst) == 0:
            msg = (r.stderr or r.stdout or b"").decode(errors="replace").strip()
            raise RuntimeError(f"SvtAv1EncApp failed ({r.returncode}): {msg[-400:]}")
        with open(dst, "rb") as f:
            stream = ivf_payload(f.read())
    return mux_avif(stream, w, h, icc, nclx=(1, 13, 1, 1))


def encode_avif(img: Image.Image, opts=None, icc: bytes | None = None) -> tuple[bytes, str]:
    o = avif_opts(opts)
    if img.mode not in ("RGB", "RGBA"):
        img = img.convert("RGB")
    if o["enc"] == "svt" and svt_available() and img.mode == "RGB":
        return encode_avif_svt(img, o, icc), "svt"
    if not avif_available():
        raise RuntimeError("no AVIF encoder: SvtAv1EncApp.exe missing and Pillow has no AVIF "
                           "(pip install -U pillow, 11.2 or newer)")
    # aom fallback (also for images with alpha): its own quality / speed, passed straight to Pillow
    kw = dict(quality=o["aom_q"], speed=o["aom_speed"], subsampling="4:4:4", range="full")
    if icc:
        kw["icc_profile"] = icc
    buf = io.BytesIO()
    try:
        img.save(buf, "AVIF", codec="aom", **kw)
        enc = "aom"
    except Exception:
        # aom missing from this build: let libavif pick (svt would be 4:2:0 only)
        buf = io.BytesIO()
        img.save(buf, "AVIF", **kw)
        enc = "avif"
    return buf.getvalue(), enc
