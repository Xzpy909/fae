"""Edit math applied at full resolution (mirrors ui/js/edit-math.js and processPixels)."""
from __future__ import annotations

import cv2
import numpy as np


# ---------------------------------------------------------------------------
# Mid/Detail (matched to reference grades within ~0.55/255 at +-50 / +-100):
#   smooth = guided filter of luma (radius 26 px at 1440p, scaled with image height, eps 0.01)
#   dY     = amount/100 * 0.952 * min(1, (Y / 0.706)^0.662) * (Y - smooth)   (fades out in shadows)
#   rgb   += dY * 255 on all channels, clip, truncate.   Applied first, before point correction.
# The preview gets `smooth` from md_base() at preview size; ui/js/edit-math.js applies the same formula.
# ---------------------------------------------------------------------------
MD_RADIUS_1440, MD_EPS, MD_GAIN, MD_K, MD_GAMMA = 26.0, 0.01, 0.952, 0.706, 0.662


def md_radius(height: int) -> int:
    return max(1, int(round(MD_RADIUS_1440 * height / 1440.0)))


def md_base(y: np.ndarray, radius: int) -> np.ndarray:
    """Guided filter (self-guided) of float32 luma 0..1."""
    k = (2 * radius + 1, 2 * radius + 1)
    m = cv2.boxFilter(y, -1, k)
    v = cv2.boxFilter(y * y, -1, k) - m * m
    a = v / (v + MD_EPS)
    b = m - a * m
    return cv2.boxFilter(a, -1, k) * y + cv2.boxFilter(b, -1, k)


def rgb_luma01(rgb: np.ndarray) -> np.ndarray:
    f = rgb.astype(np.float32)
    return ((0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2]) / 255.0).astype(np.float32)


def apply_mid_detail(rgb: np.ndarray, amount: float, base: np.ndarray | None = None) -> np.ndarray:
    if not amount:
        return rgb
    if base is None:
        base = md_base(rgb_luma01(rgb), md_radius(rgb.shape[0]))
    out = np.empty_like(rgb)
    for r0 in range(0, rgb.shape[0], 512):          # strips keep float64 memory small
        f = rgb[r0:r0 + 512].astype(np.float64)
        y = (0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2]) / 255.0
        w = np.minimum(1.0, np.power(np.maximum(y, 0.0) / MD_K, MD_GAMMA))
        d = (amount * MD_GAIN * w * (y - base[r0:r0 + 512]) * 255.0)[..., None]
        out[r0:r0 + 512] = np.clip(f + d, 0, 255).astype(np.uint8)
    return out


def apply_edit(rgb: np.ndarray, edit: dict | None, strip_rows: int = 512) -> np.ndarray:
    """
    rgb  : HxWx3 uint8
    edit : {"point": bool, "k": float, "o": float, "gains": [3] | None,
            "luts": [[256], [256], [256]]}   (computed by the UI)
    Per pixel:  lum' = clip(lum*k + o)  ->  rgb *= lum'/max(lum,1)  ->  rgb *= gains
                ->  clip, truncate to uint8
                ->  Temp / Tint: rgb = M @ rgb (Bradford white-balance matrix)  ->  clip, truncate
                ->  Contrast / Pivot: rgb = f(rgb) + f(y) - luma(f(rgb))  ->  clip, truncate
                ->  Shadows, then Highlights: y' = table[y]; lift: rgb *= y'/y, darken: rgb += y' - y
                    (each: clip, truncate)
                ->  Color Boost: rgb = y + (rgb - y) * boost[max - min]  ->  clip, truncate
                ->  Saturation: rgb = y + (rgb - y) * sat / 50  ->  clip, truncate
                ->  Filter Perceptual Saturation ("bw": w = Warmth gains, s = 1 + Saturation/100, op = Opacity/100):
                    w = min(255, floor(rgb * gains));  y = (wR + 2wG + wB) / 4;  f = clip(y + (w - y) * s)
                    rgb = round(rgb + (f - rgb) * op)
                    Perceptual Saturation ("ps" > 0, APPROXIMATION): f is rounded, the whole image's f is
                    collected first, then ps_finish() pushes it against its large surround (ps_grid);
                    the opacity mix happens there.
                ->  per-channel LUT (master curve folded in)
    """
    if not edit:
        return rgb
    luts = [np.asarray(l, dtype=np.uint8) for l in edit.get("luts") or []]
    boost = edit.get("boost")
    boost = None if boost is None else np.asarray(boost, dtype=np.float64)
    tones = [(np.asarray(t["T"], dtype=np.float64), bool(t.get("ratio"))) for t in (edit.get("tones") or [])]
    sat = edit.get("sat")
    bw = edit.get("bw")
    film = edit.get("film")
    wb = edit.get("wb")
    wb = None if wb is None else np.asarray(wb, dtype=np.float64)
    con = edit.get("con")
    con = None if con is None else np.asarray(con, dtype=np.float64)
    if len(luts) != 3:
        luts = [np.arange(256, dtype=np.uint8)] * 3
    point = bool(edit.get("point"))
    k = float(edit.get("k", 1.0))
    o = float(edit.get("o", 0.0))
    gains = edit.get("gains")
    gains = None if gains is None else np.asarray(gains, dtype=np.float64).reshape(1, 1, 3)

    out = np.empty_like(rgb)
    ps = bool(bw is not None and bw.get("ps"))
    if ps:
        fin = np.empty_like(rgb)
        fmid = np.empty_like(rgb)
    for y0 in range(0, rgb.shape[0], strip_rows):
        src = rgb[y0:y0 + strip_rows]
        if point:
            f = src.astype(np.float64)
            if k != 1.0 or o != 0.0:
                lum = 0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2]
                bl = np.clip(lum * k + o, 0, 255)
                f *= (bl / np.maximum(lum, 1.0))[..., None]
            if gains is not None:
                f *= gains
            idx = np.clip(f, 0, 255).astype(np.uint8)
        else:
            idx = src
        if wb is not None:
            # Temp / Tint (3x3 white-balance matrix on gamma-encoded RGB, computed in ui/js/edit-math.js wbMatrix)
            f = idx.astype(np.float64)
            f = f[..., 0:1] * wb[:, 0] + f[..., 1:2] * wb[:, 1] + f[..., 2:3] * wb[:, 2]
            idx = np.clip(f, 0, 255).astype(np.uint8)
        if con is not None:
            # Contrast / Pivot (table computed in ui/js/edit-math.js contrastTable): rgb' = f(rgb) + f(Y) - Y(f(rgb))
            o = con[idx.astype(np.intp) * 16]
            f = idx.astype(np.float64)
            y = 0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2]
            d = con[np.minimum(np.floor(y * 16 + 0.5), 4095).astype(np.intp)] \
                - (0.2126 * o[..., 0] + 0.7152 * o[..., 1] + 0.0722 * o[..., 2])
            idx = np.clip(o + d[..., None], 0, 255).astype(np.uint8)
        for tone_t, tone_ratio in tones:
            # Shadows, then Highlights (tone tables on luma, computed in ui/js/edit-math.js toneTable)
            f = idx.astype(np.float64)
            y = 0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2]
            y2 = tone_t[np.minimum(np.floor(y * 16 + 0.5), 4095).astype(np.intp)]
            if tone_ratio:
                f = f * (y2 / np.maximum(y, 0.5))[..., None]
            else:
                f = f + (y2 - y)[..., None]
            idx = np.clip(f, 0, 255).astype(np.uint8)
        if boost is not None:
            # Color Boost (gain table by s = max - min, computed in ui/js/edit-math.js boostGains)
            s = idx.max(axis=2).astype(np.intp) - idx.min(axis=2)
            gn = boost[s][..., None]
            f = idx.astype(np.float64)
            y = (0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2])[..., None]
            idx = np.clip(y + (f - y) * gn, 0, 255).astype(np.uint8)
        if sat is not None:
            # Saturation (luma-preserving gain, Rec.709)
            f = idx.astype(np.float64)
            y = (0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2])[..., None]
            idx = np.clip(y + (f - y) * float(sat), 0, 255).astype(np.uint8)
        if bw is not None:
            # Filter Perceptual Saturation (Saturation matched to reference grades: max 1 level off)
            f = idx.astype(np.float64)
            w = f if not bw.get("w") else np.minimum(255.0, np.floor(f * np.asarray(bw["w"], dtype=np.float64)))
            y = ((w[..., 0] + 2 * w[..., 1] + w[..., 2]) / 4)[..., None]
            fl = y + (w - y) * float(bw["s"])
            if ps:
                fin[y0:y0 + strip_rows] = idx
                fmid[y0:y0 + strip_rows] = np.where(fl <= 0, 0, np.where(fl >= 255, 255, np.floor(fl + 0.5)))
                continue
            fl = np.clip(fl, 0, 255)
            idx = np.clip(np.floor(f + (fl - f) * float(bw["op"]) + 0.5), 0, 255).astype(np.uint8)
        if film:
            idx = film_apply(idx, film)
        dst = out[y0:y0 + strip_rows]
        for c in range(3):
            dst[..., c] = luts[c][idx[..., c]]
    if ps:
        ps_finish(fin, fmid, bw, luts, out, strip_rows, film)
    return out


# ---------------------------------------------------------------------------
# Film Effect (mirrors filmPx in ui/js/edit-math.js): preset = 3D colour table of deltas (trilinear),
# then the Brightness / Contrast tone table "t1", then Opacity mixed with the input.
# ---------------------------------------------------------------------------
def film_apply(idx: np.ndarray, film: dict) -> np.ndarray:
    f = idx.astype(np.float64)
    q = f
    d = film.get("d")
    if d:
        n = int(film["n"])
        D = np.asarray(d, dtype=np.float64).reshape(-1, 3)
        g = f * (n - 1) / 255
        i = np.minimum(np.floor(g), n - 2).astype(np.intp)
        fr = g - i
        acc = np.zeros_like(f)
        for dr in (0, 1):
            for dg in (0, 1):
                for db in (0, 1):
                    w = (fr[..., 0] if dr else 1 - fr[..., 0]) * (fr[..., 1] if dg else 1 - fr[..., 1]) \
                        * (fr[..., 2] if db else 1 - fr[..., 2])
                    k = ((i[..., 0] + dr) * n + (i[..., 1] + dg)) * n + (i[..., 2] + db)
                    acc = acc + w[..., None] * D[k]
        q = np.clip(np.floor(f + acc + 0.5), 0, 255)
    t1 = film.get("t1")
    if t1:
        q = np.asarray(t1, dtype=np.float64)[q.astype(np.intp)]
    op = float(film.get("op", 1))
    if op != 1:
        q = np.clip(np.floor(f + (q - f) * op + 0.5), 0, 255)
    return q.astype(np.uint8)


# ---------------------------------------------------------------------------
# Perceptual Saturation - APPROXIMATION (mirrors psGrid / psPass in ui/js/edit-math.js; the formula
# and the reasons are described there). The Gaussian kernel comes from the UI ("psk").
# ---------------------------------------------------------------------------
PS_W = (0.331, 0.0256, -0.8911, -0.2478, 3.1506, -0.0857, 0.1625, -0.3969, 0.597)
PS_GH = 45


def _ps_reflect(i: np.ndarray, n: int) -> np.ndarray:
    if n == 1:
        return np.zeros_like(i)
    p = 2 * n - 2
    i = np.abs(i) % p
    return np.where(i >= n, p - i, i)


def ps_grid(src: np.ndarray, kernel) -> tuple[np.ndarray, int, int]:
    H, W = src.shape[:2]
    gh = PS_GH
    gw = max(1, int(np.floor(PS_GH * W / H + 0.5)))
    cell = ((np.arange(H) * gh // H)[:, None] * gw + (np.arange(W) * gw // W)[None, :]).ravel()
    cnt = np.bincount(cell, minlength=gh * gw).astype(np.float64)
    g = np.empty((gh * gw, 3))
    for c in range(3):    # sums of integers: exact, so the order does not matter
        g[:, c] = np.bincount(cell, weights=src[..., c].ravel().astype(np.float64), minlength=gh * gw)
    g = (g / np.where(cnt > 1, cnt, 1.0)[:, None]).reshape(gh, gw, 3)
    k = np.asarray(kernel, dtype=np.float64)
    r = (len(k) - 1) // 2
    for axis, n in ((0, gh), (1, gw)):
        o = np.zeros_like(g)
        for t in range(-r, r + 1):
            j = _ps_reflect(np.arange(n) + t, n)
            o = o + k[t + r] * (g[j] if axis == 0 else g[:, j])
        g = o
    return g, gh, gw


def ps_finish(fin: np.ndarray, fmid: np.ndarray, bw: dict, luts, out: np.ndarray, strip_rows: int, film=None):
    H, W = fmid.shape[:2]
    g, gh, gw = ps_grid(fmid, bw["psk"])
    t, op, w = float(bw["ps"]), float(bw["op"]), PS_W
    gx = np.clip((np.arange(W) + 0.5) * gw / W - 0.5, 0, gw - 1)
    x0 = np.minimum(np.floor(gx), gw - 2).astype(np.intp) if gw > 1 else np.zeros(W, np.intp)
    x1 = np.minimum(x0 + 1, gw - 1)
    fx = (gx - x0)[:, None] if gw > 1 else np.zeros((W, 1))
    for r0 in range(0, H, strip_rows):
        ys = np.arange(r0, min(H, r0 + strip_rows))
        gy = np.clip((ys + 0.5) * gh / H - 0.5, 0, gh - 1)
        y0 = np.minimum(np.floor(gy), gh - 2).astype(np.intp) if gh > 1 else np.zeros(len(ys), np.intp)
        y1 = np.minimum(y0 + 1, gh - 1)
        fy = (gy - y0)[:, None, None] if gh > 1 else np.zeros((len(ys), 1, 1))
        rowv = g[y0] * (1 - fy) + g[y1] * fy             # vertical first, as in psPass
        Sm = rowv[:, x0] * (1 - fx) + rowv[:, x1] * fx
        f = fmid[r0:r0 + strip_rows].astype(np.float64)
        Y = (f[..., 0] + 2 * f[..., 1] + f[..., 2]) / 4
        YS = (Sm[..., 0] + 2 * Sm[..., 1] + Sm[..., 2]) / 4
        C = f - Y[..., None]
        D = Sm - YS[..., None]
        m = Y / 255 - 0.5
        nn = np.sqrt(D[..., 0] * D[..., 0] + D[..., 1] * D[..., 1] + D[..., 2] * D[..., 2]) / 128
        nc = np.sqrt(C[..., 0] * C[..., 0] + C[..., 1] * C[..., 1] + C[..., 2] * C[..., 2]) / 128
        ka = (w[0] + w[1] * m + w[6] * nn + w[7] * nc)[..., None]
        kb = (w[2] + w[3] * m + w[4] * m * m + w[8] * nc)[..., None]
        e = (w[5] * (Y - YS))[..., None]
        q = np.clip(np.floor(f + t * (C * ka + D * kb + e) + 0.5), 0, 255)
        a = fin[r0:r0 + strip_rows].astype(np.float64)
        idx = np.clip(np.floor(a + (q - a) * op + 0.5), 0, 255).astype(np.uint8)
        if film:
            idx = film_apply(idx, film)
        dst = out[r0:r0 + strip_rows]
        for c in range(3):
            dst[..., c] = luts[c][idx[..., c]]
