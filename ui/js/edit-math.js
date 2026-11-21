'use strict';
/* Edit math: the tables computed from the sliders (computeEdit). The pixels are done by the Rust
   core (core/fastedit-core): WebAssembly for the live preview (wasm-preview.js), the Python extension
   for Process. Both get exactly these tables (jobFor in process.js / wasm-params.js). */

/* ======================================================================
   Edit math
   ====================================================================== */
const lum = c => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];

function curveLut(pts) {
  const L = new Uint8Array(256);
  const n = pts.length;
  const xs = pts.map(p => p[0]), ys = pts.map(p => p[1]);
  if (n === 2 && xs[0] === 0 && ys[0] === 0 && xs[1] === 255 && ys[1] === 255) {
    for (let i = 0; i < 256; i++) L[i] = i;
    return L;
  }
  // Monotone cubic (Fritsch-Carlson): smooth, never overshoots between points.
  const d = [], m = new Array(n).fill(0);
  for (let i = 0; i < n - 1; i++) d[i] = (ys[i + 1] - ys[i]) / Math.max(1e-6, xs[i + 1] - xs[i]);
  if (n === 2) { m[0] = m[1] = d[0]; }
  else {
    m[0] = d[0]; m[n - 1] = d[n - 2];
    for (let i = 1; i < n - 1; i++) m[i] = d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) / 2;
    for (let i = 0; i < n - 1; i++) {
      if (d[i] === 0) { m[i] = 0; m[i + 1] = 0; continue; }
      const a = m[i] / d[i], b = m[i + 1] / d[i], h = a * a + b * b;
      if (h > 9) { const t = 3 / Math.sqrt(h); m[i] = t * a * d[i]; m[i + 1] = t * b * d[i]; }
    }
  }
  let seg = 0;
  for (let x = 0; x < 256; x++) {
    let y;
    if (x <= xs[0]) y = ys[0];
    else if (x >= xs[n - 1]) y = ys[n - 1];
    else {
      while (seg < n - 2 && x > xs[seg + 1]) seg++;
      const h = xs[seg + 1] - xs[seg], t = (x - xs[seg]) / h, t2 = t * t, t3 = t2 * t;
      y = (2 * t3 - 3 * t2 + 1) * ys[seg] + (t3 - 2 * t2 + t) * h * m[seg]
        + (-2 * t3 + 3 * t2) * ys[seg + 1] + (t3 - t2) * h * m[seg + 1];
    }
    L[x] = clamp(Math.round(y), 0, 255);
  }
  return L;
}

/* Turns the editable state into the numbers both renderers use. */
function computeEdit(e) {
  let scale = 1, offset = 0;
  const tb = e.tBlack, tw = e.tWhite;
  if (e.black && e.white) {
    const lb = lum(e.black.rgb), lw = lum(e.white.rgb);
    if (lw > lb + 1e-3) { scale = (tw - tb) / (lw - lb); offset = tb - scale * lb; }
  } else if (e.black) {            // black only: map sample -> target, keep 255 anchored
    const lb = lum(e.black.rgb);
    if (lb < 254.9) { scale = (255 - tb) / (255 - lb); offset = tb - scale * lb; }
  } else if (e.white) {            // white only: map sample -> target, keep 0 anchored
    const lw = lum(e.white.rgb);
    if (lw > 1e-3) scale = tw / lw;
  }
  const s = e.strength;
  const k = 1 + (scale - 1) * s, o = offset * s;
  let gains = null;
  if (e.gray) {
    const g = e.gray.rgb.map(v => 1 / Math.max(v, 1));
    const mean = (g[0] + g[1] + g[2]) / 3;
    gains = g.map(v => 1 + (v / mean - 1) * s);
  }
  const lm = curveLut(e.curves.m);
  const lc = [curveLut(e.curves.r), curveLut(e.curves.g), curveLut(e.curves.b)];
  const luts = lc.map(cl => { const L = new Uint8Array(256); for (let i = 0; i < 256; i++) L[i] = cl[lm[i]]; return L; });
  const point = k !== 1 || o !== 0 || !!gains;
  const tones = [toneTable('shadows', e.adj_shadows), toneTable('highlights', e.adj_highlights)].filter(Boolean);
  // Saturation: plain luma-preserving gain, Rec.709 weights, 50 = unchanged (measured: exactly sat/50)
  const satv = e.adj_sat == null ? 50 : Number(e.adj_sat);
  return { k, o, gains, luts, point, wb: wbMatrix(e.adj_temp, e.adj_tint), con: contrastTable(e.adj_contrast, e.adj_pivot),
    tones: tones.length ? tones : null,
    boost: boostGains(e.adj_boost), sat: satv === 50 ? null : satv / 50,
    md: (Number(e.adj_middetail) || 0) / 100, bw: bwParams(e), film: filmParams(e), sc: scParams(e) };
}
/* Filter Film Effect - Brightness and Contrast, measured from reference renders (Brightness exact,
   Contrast within 1 level on 97-99% of pixels). Both are one tone curve for all three channels:
   per input level the output at 0 / 50 / 100 % is known (FILM_* tables, film-tables.js) and
   interpolated quadratically in between. Brightness is applied first, then Contrast, then Opacity.
   Film preset first (3D colour table, FILM_PRESETS), then Brightness / Contrast, then Opacity
   (mixes with the filter's input). Applied after the other filters, before Curves.
   Mirrored by film_apply() in fastedit/edit.py. */
function filmParams(e) {
  const op = e.flt_film_opacity == null ? 1 : Number(e.flt_film_opacity) / 100;
  if (!(op > 0)) return null;
  const d = FILM_PRESETS[e.flt_film_preset] || null, t1 = filmLut(e);
  return d || t1 ? { n: FILM_N, d: d ? filmTable(e.flt_film_preset, d) : null, t1, op } : null;
}
const FILM_CACHE = {};
const filmTable = (key, d) => FILM_CACHE[key] || (FILM_CACHE[key] = Float64Array.from(d));
// Brightness / Contrast tone table (no opacity; the core mixes that in), or null
function filmLut(e) {
  const t = (Number(e.flt_film_bright) || 0) / 100, c = (Number(e.flt_film_contrast) || 0) / 100;
  if (!t && !c) return null;
  const q = (x, a, b, s) => x * (1 - s) * (1 - 2 * s) + a * 4 * s * (1 - s) + b * s * (2 * s - 1);
  const P = new Float64Array(256);
  for (let x = 0; x < 256; x++) P[x] = q(x, FILM_C50[x], FILM_C100[x], c);
  const L = new Uint8Array(256);
  for (let x = 0; x < 256; x++) {
    const b = Math.min(255, Math.max(0, q(x, FILM_B50[x], FILM_B100[x], t)));
    const i = Math.min(Math.floor(b), 254), f = b - i;
    const v = Math.floor(P[i] * (1 - f) + P[i + 1] * f + 0.5);
    L[x] = v < 0 ? 0 : v > 255 ? 255 : v;
  }
  return L;
}

/* Filter Perceptual Saturation, matched to reference grades (both max 1 level off):
   Warmth: per-channel gain, w = floor(rgb * g) (t = warmth/100; the warm side gains 1 + 0.17038|t|,
           the cool side 1 - 0.15062|t|, green a small quadratic, see warmGains)
   Saturation: grey y = (R + 2G + B) / 4, f = y + (w - y) * (1 + sat/100)
   Opacity mixes f with the filter's input.
   Perceptual Saturation is an APPROXIMATION (not matched exactly, see core/fastedit-core/src/ps.rs). */
function warmGains(t) {
  if (!t) return null;
  const a = Math.abs(t), hot = 1 + 0.17038 * a, cold = 1 - 0.15062 * a;
  const g = t > 0 ? 1 + 0.05065 * t - 0.00662 * t * t : 1 - 0.0301 * a - 0.00616 * t * t;
  return t > 0 ? [hot, g, cold] : [cold, g, hot];
}
function bwParams(e) {
  const s = 1 + (Number(e.flt_bw_sat) || 0) / 100;
  const w = warmGains((Number(e.flt_bw_warmth) || 0) / 100);
  const ps = (Number(e.flt_bw_psat) || 0) / 60;
  const op = e.flt_bw_opacity == null ? 1 : Number(e.flt_bw_opacity) / 100;
  if ((s === 1 && !w && !ps) || !(op > 0)) return null;
  return ps ? { s, op, w, ps, psk: PS_KERNEL } : { s, op, w };
}

/* Perceptual Saturation - APPROXIMATION of the reference filter (which works on a coarse internal
   grid and is non-linear; it could not be matched exactly). Behaviour reproduced: colours are pushed
   away from the average colour of their large surroundings, colours that match their surroundings get
   flatter, and near black / white the surround colour bleeds in instead. Fitted on test renders at 60.
     surround S : mean of the filter's input on a 45-row grid, Gaussian (sigma 0.21 of the height,
                  mirrored edges) on the grid, bilinear back to pixels   (ps.rs in the core)
     per pixel  : Y = (R + 2G + B) / 4, C = rgb - Y, CS = S - Y(S), m = Y/255 - 0.5,
                  n = |CS| / 128, nc = |C| / 128
                  rgb += t * (C * (w0 + w1 m + w6 n + w7 nc) + CS * (w2 + w3 m + w4 m^2 + w8 nc) + w5 (Y - Y(S)))
                  with t = amount / 60 (weights PS_W in core/fastedit-core/src/ps.rs).
   The Gaussian kernel over the grid is computed here and sent to the core. */
const PS_GH = 45, PS_SIGMA = 0.21;
const PS_KERNEL = (() => {
  const sig = PS_SIGMA * PS_GH, r = Math.ceil(3 * sig), k = [];
  let sum = 0;
  for (let t = -r; t <= r; t++) { const v = Math.exp(-0.5 * (t / sig) * (t / sig)); k.push(v); sum += v; }
  return k.map(v => v / sum);
})();
/* Temp / Tint: white balance as Resolve does it (both matched to reference grades within rounding):
   Bradford chromatic adaptation from the black-body white at 6500 K to a target white,
   turned into a Rec.709 RGB matrix, applied to the gamma-encoded values, and scaled so
   white keeps its luma.
     Temp : target on the black-body curve at (6500 - Temp) K        (positive = warmer)
     Tint : target moved Tint * 0.0001 (Duv) across the curve in CIE 1960 uv (positive = magenta)
   Black-body chromaticity: Kang et al. 2002. */
const M_RGB2XYZ = [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];
const M_BRADFORD = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
const mmul = (A, B) => A.map(r => B[0].map((_, j) => r[0] * B[0][j] + r[1] * B[1][j] + r[2] * B[2][j]));
const mvec = (A, v) => A.map(r => r[0] * v[0] + r[1] * v[1] + r[2] * v[2]);
function minv(m) {
  const [[a, b, c], [d, e, f], [g, h, i]] = m;
  const A = e * i - f * h, B = -(d * i - f * g), C = d * h - e * g, det = a * A + b * B + c * C;
  return [[A / det, -(b * i - c * h) / det, (b * f - c * e) / det],
          [B / det, (a * i - c * g) / det, -(a * f - c * d) / det],
          [C / det, -(a * h - b * g) / det, (a * e - b * d) / det]];
}
const xy2uv = ([x, y]) => { const d = -2 * x + 12 * y + 3; return [4 * x / d, 6 * y / d]; };
const uv2xy = ([u, v]) => { const d = 2 * u - 8 * v + 4; return [3 * u / d, 2 * v / d]; };
const xyToXYZ = ([x, y]) => [x / y, 1, (1 - x - y) / y];
function planckXY(T) {
  T = Math.min(25000, Math.max(1667, T));
  const x = T <= 4000 ? -0.2661239e9 / T ** 3 - 0.2343589e6 / T ** 2 + 0.8776956e3 / T + 0.179910
                      : -3.0258469e9 / T ** 3 + 2.1070379e6 / T ** 2 + 0.2226347e3 / T + 0.240390;
  const y = T <= 2222 ? -1.1063814 * x ** 3 - 1.34811020 * x ** 2 + 2.18555832 * x - 0.20219683
          : T <= 4000 ? -0.9549476 * x ** 3 - 1.37418593 * x ** 2 + 2.09137015 * x - 0.16748867
          : 3.0817580 * x ** 3 - 5.87338670 * x ** 2 + 3.75112997 * x - 0.37001483;
  return [x, y];
}
function wbMatrix(temp, tint) {
  const t = Number(temp) || 0, n = Number(tint) || 0;
  if (!t && !n) return null;
  const T0 = 6500, T = T0 - t;
  let dst = planckXY(T);
  if (n) {                                   // move across the black-body curve (Duv) at T
    const a = xy2uv(planckXY(T + 1)), b = xy2uv(planckXY(T - 1)), uv = xy2uv(dst);
    const tu = a[0] - b[0], tv = a[1] - b[1], len = Math.hypot(tu, tv);
    dst = uv2xy([uv[0] - n * 1e-4 * tv / len, uv[1] + n * 1e-4 * tu / len]);
  }
  const ws = mvec(M_BRADFORD, xyToXYZ(planckXY(T0))), wd = mvec(M_BRADFORD, xyToXYZ(dst));
  const D = [[wd[0] / ws[0], 0, 0], [0, wd[1] / ws[1], 0], [0, 0, wd[2] / ws[2]]];
  const m = mmul(minv(M_RGB2XYZ), mmul(minv(M_BRADFORD), mmul(D, mmul(M_BRADFORD, M_RGB2XYZ))));
  const s = [0.2126, 0.7152, 0.0722].reduce((acc, w, i) => acc + w * (m[i][0] + m[i][1] + m[i][2]), 0);
  return m.map(r => r.map(x => x / s));
}

/* Contrast / Pivot (matched to reference grades within ~0.5/255, Contrast 0.5 .. 1.75):
   a curve f on each channel, and the result's luma replaced by f(luma):
       rgb' = f(rgb) + f(Y) - Y(f(rgb))           (contrast does not change saturation)
   Contrast <= 1 : f is a straight line through the pivot, slope = Contrast.
   Contrast  > 1 : f is an S-curve with slope 1/(2 - Contrast) at the pivot. On each side the
                   distance to the pivot, normalised to 0..1, is pushed along the flow
                   da/dt = a (1 - a)^1.211 (1 + 2.2542 a) for t = ln(1/(2 - Contrast)), capped at 4.2
                   (Resolve's Contrast 2.0 is a very steep curve, not a true step).
   Table: 4096 entries, x in 1/16 steps (0 .. 255.94). Shared with Python. */
const CON_FIT = { b: 1.211, g: 2.2542, tmax: 4.2 };
function contrastTable(cv, pv) {
  const c = cv == null ? 1 : Number(cv), p = pv == null ? 0.435 : Math.min(0.999, Math.max(0.001, Number(pv)));
  if (c === 1 || !isFinite(c)) return null;
  const T = new Float64Array(4096);
  if (c < 1) {
    for (let i = 0; i < 4096; i++) T[i] = (p + (i / 16 / 255 - p) * c) * 255;
    return T;
  }
  const t = Math.min(CON_FIT.tmax, Math.log(1 / Math.max(1e-6, 2 - c))), steps = 48, h = t / steps;
  const V = a => { a = a < 0 ? 0 : a > 1 ? 1 : a; return a * Math.pow(1 - a, CON_FIT.b) * (1 + CON_FIT.g * a); };
  for (let i = 0; i < 4096; i++) {
    const x = Math.min(1, i / 16 / 255), lo = x < p;
    let a = lo ? (p - x) / p : (x - p) / (1 - p);
    for (let s = 0; s < steps; s++) {
      const k1 = V(a), k2 = V(a + h / 2 * k1), k3 = V(a + h / 2 * k2), k4 = V(a + h * k3);
      a += h / 6 * (k1 + 2 * k2 + 2 * k3 + k4);
    }
    a = a < 0 ? 0 : a > 1 ? 1 : a;
    T[i] = (lo ? p - a * p : p + a * (1 - p)) * 255;
  }
  return T;
}

/* Shadows / Highlights: tone moves on luma Y, fitted to reference grades at +-50 / +-100.
   The amounts behave like a flow: +100 = +50 applied twice, -50 ~ undoes +50. So Y' is the
   solution of dY/dt = V(Y) for t = amount/100, with V(y) = k * y^a * (1 - y)^b (y = Y/255),
   one field for lifting and one for darkening.
     Shadows    : peaks around Y 60-100, fades towards black and white
     Highlights : works from the upper midtones up; darkening also pulls white down
     lift   : RGB *= Y'/Y          (colour scales with the brightness)
     darken : RGB += Y' - Y        (colour keeps its absolute offset)
   Applied in order: Shadows, then Highlights.
   Table: 4096 entries, Y in 1/16 steps. Shared with Python so preview and output match. */
const TONE_FIT = {
  shadows: { pos: [0.5822167, 0.7119431, 1.4011680], neg: [0.3470188, 0.4093353, 0.9991983] },
  highlights: { pos: [0.4334345, 1.6821352, 0.1206327], neg: [0.4169995, 1.9857309, 0.1087127] },
};
function toneFlow(field, t, n = 4096, steps = 16) {
  const [k, a, b] = field, T = new Float64Array(n), h = t / steps;
  const V = y => { y = y < 1e-6 ? 1e-6 : y > 1 - 1e-6 ? 1 - 1e-6 : y; return k * Math.pow(y, a) * Math.pow(1 - y, b); };
  for (let i = 0; i < n; i++) {
    let y = i / 16 / 255;
    for (let s = 0; s < steps; s++) {
      const k1 = V(y), k2 = V(y + h / 2 * k1), k3 = V(y + h / 2 * k2), k4 = V(y + h * k3);
      y += h / 6 * (k1 + 2 * k2 + 2 * k3 + k4);
    }
    T[i] = y * 255;
  }
  return T;
}
function toneTable(name, v) {
  const a = (Number(v) || 0) / 100;
  if (!a) return null;
  const F = TONE_FIT[name];
  return { T: toneFlow(a > 0 ? F.pos : F.neg, a), ratio: a > 0 };
}

/* Color Boost: luma-preserving chroma scaling whose gain depends on the pixel's distance from
   grey, s = max(R,G,B) - min(R,G,B) (0..1). Fitted to reference grades (+50, +100, -50):
     + : s' = s + a * c  * s^q                     a = amount / 100, 0..1
     - : s' = max(0, s - |a| * cn * s^qn)          weak colours reach grey first
   Both sides are exactly linear in the amount (checked at +-50 and +-100).
   The gain table (256 entries, by integer s) is shared with Python so preview and output match. */
const BOOST_FIT = { c: 0.4266654, q: 0.4008857, cn: 0.5967640, qn: 0.3847268 };
function boostGains(v) {
  const a = (Number(v) || 0) / 100;
  if (!a) return null;
  const F = BOOST_FIT, G = new Float64Array(256);
  G[0] = 1;
  for (let s = 1; s < 256; s++) {
    const x = s / 255;
    let y;
    if (a > 0) {
      y = x + a * F.c * Math.pow(x, F.q);
    } else {
      y = Math.max(0, x + a * F.cn * Math.pow(x, F.qn));
    }
    G[s] = y / x;
  }
  return G;
}

/* Filter Super Contrast (matched to reference renders). Needs the whole filter input, so the core runs it
   as a second pass (core/fastedit-core/src/sc.rs). Runs after Film Effect, before Curves.
   1. Correct Color Cast (max 1 level off): per channel c a curve keeping black and white,
      out = x + 4 D t (1 - t), t = x / 255, D = cast * (g - m_c); m_c = channel mean, g = mean of the means.
   2. Correct Contrast (max 1 level off at 100%, about 2 at in-between strengths): on the cast result,
      M = (R + G + B) / 3,  out_c = a M + b + k a (c - M)   (a linear stretch of M plus a colour gain).
      Black / white points (lo, hi -> 0, 255) come from the channel percentiles: stage A (to 50%) moves
      towards (min_c P2, max_c P98), stage B (50-100%) towards (min_c P10 + 1, max_c P90 - 1); how far at
      each strength, and k, are tables measured from a 10-step strength series (CC_* in sc.rs).
   3. Dynamic Contrast (APPROXIMATION, same idea, not pixel-matched): contrast-limited local histogram
      equalization of M on a coarse grid (DC_GW cells across, 64-bin histograms per cell, Gaussian-smoothed
      across cells, clipped at DC_CLIP x the mean bin), each pixel bilinear between its 4 cells' curves;
      M' = M + s * DC_BETA * (curve(M) - M), colour gain 1 + s * (DC_SAT - 1). Linear in strength, as measured.
   4. Shadows / Highlights: the shared filter tone curve shLut() (same table on all three channels).
   Opacity mixes the result with the filter input. */
/* Shared Shadows / Highlights curve for filters (matched to reference renders, max 1 level off).
   One table for all three channels. Shadows s (0..1): x + SH_A s (1 - x / (SH_W s))^SH_P for x < SH_W s,
   i.e. black lifts to about 24 at 100% and the lift fades out by level ~116. Highlights is the exact mirror:
   x - SH_A h (1 - (255 - x) / (SH_W h))^SH_P. The two never overlap. Reuse this in any filter that has
   Shadows / Highlights sliders; Python only applies the table it is sent. */
const SH_A = 24.2, SH_W = 116.5, SH_P = 2.6;
function shLut(s, h) {
  if (!(s > 0) && !(h > 0)) return null;
  const L = new Uint8Array(256);
  for (let x = 0; x < 256; x++) {
    let d = 0;
    if (s > 0 && x < SH_W * s) d += SH_A * s * Math.pow(1 - x / (SH_W * s), SH_P);
    if (h > 0 && 255 - x < SH_W * h) d -= SH_A * h * Math.pow(1 - (255 - x) / (SH_W * h), SH_P);
    const v = Math.floor(x + d + 0.5); L[x] = v < 0 ? 0 : v > 255 ? 255 : v;
  }
  return L;
}
const DC_SIGMA = 1.5;              // the other Dynamic Contrast constants (DC_*) are in sc.rs
const DC_KERNEL = (() => {         // Gaussian over grid cells, sent to the core
  const r = Math.ceil(3 * DC_SIGMA), k = [];
  let s = 0;
  for (let t = -r; t <= r; t++) { const v = Math.exp(-0.5 * (t / DC_SIGMA) * (t / DC_SIGMA)); k.push(v); s += v; }
  return k.map(v => v / s);
})();
function scParams(e) {
  const cast = (Number(e.flt_sc_cast) || 0) / 100, cc = Number(e.flt_sc_contrast) || 0;
  const dyn = (Number(e.flt_sc_dynamic) || 0) / 100;
  const shl = shLut((Number(e.flt_sc_shadows) || 0) / 100, (Number(e.flt_sc_highlights) || 0) / 100);
  const op = e.flt_sc_opacity == null ? 1 : Number(e.flt_sc_opacity) / 100;
  return (cast > 0 || cc > 0 || dyn > 0 || shl) && op > 0 ? { cast, cc, dyn, op, dk: DC_KERNEL, shl } : null;
}
