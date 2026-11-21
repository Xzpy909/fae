'use strict';
/* Edit math: tables computed from the sliders and processPixels (mirrored by fastedit/edit.py). */

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
    md: (Number(e.adj_middetail) || 0) / 100, bw: bwParams(e) };
}
/* Filter Brilliance / Warmth, matched to reference grades (both max 1 level off):
   Warmth: per-channel gain, w = floor(rgb * g) (t = warmth/100; the warm side gains 1 + 0.17038|t|,
           the cool side 1 - 0.15062|t|, green a small quadratic, see warmGains)
   Saturation: grey y = (R + 2G + B) / 4, f = y + (w - y) * (1 + sat/100)
   Opacity mixes f with the filter's input.
   Perceptual Saturation is an APPROXIMATION (not matched exactly, see psGrid / PS_W). */
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
                  mirrored edges) on the grid, bilinear back to pixels   (psGrid / psSurround)
     per pixel  : Y = (R + 2G + B) / 4, C = rgb - Y, CS = S - Y(S), m = Y/255 - 0.5,
                  n = |CS| / 128, nc = |C| / 128
                  rgb += t * (C * (w0 + w1 m + w6 n + w7 nc) + CS * (w2 + w3 m + w4 m^2 + w8 nc) + w5 (Y - Y(S)))
                  with t = amount / 60. Kernel weights are computed here and sent to Python. */
const PS_W = [0.331, 0.0256, -0.8911, -0.2478, 3.1506, -0.0857, 0.1625, -0.3969, 0.597];
const PS_GH = 45, PS_SIGMA = 0.21;
const PS_KERNEL = (() => {
  const sig = PS_SIGMA * PS_GH, r = Math.ceil(3 * sig), k = [];
  let sum = 0;
  for (let t = -r; t <= r; t++) { const v = Math.exp(-0.5 * (t / sig) * (t / sig)); k.push(v); sum += v; }
  return k.map(v => v / sum);
})();
function psReflect(i, n) {
  if (n === 1) return 0;
  const p = 2 * n - 2;
  i = Math.abs(i) % p;
  return i >= n ? p - i : i;
}
// src: Uint8Array of n*3 (filter input after Warmth/Saturation); returns blurred grid (Float64Array gh*gw*3)
function psGrid(src, W, H, k) {
  const gh = PS_GH, gw = Math.max(1, Math.floor(PS_GH * W / H + 0.5));
  const acc = new Float64Array(gh * gw * 3), cnt = new Float64Array(gh * gw);
  const cx = new Int32Array(W);
  for (let x = 0; x < W; x++) cx[x] = Math.floor(x * gw / W);
  for (let y = 0, p = 0; y < H; y++) {
    const row = Math.floor(y * gh / H) * gw;
    for (let x = 0; x < W; x++, p += 3) {
      const c = row + cx[x], c3 = c * 3;
      acc[c3] += src[p]; acc[c3 + 1] += src[p + 1]; acc[c3 + 2] += src[p + 2]; cnt[c] += 1;
    }
  }
  let g = new Float64Array(gh * gw * 3);
  for (let c = 0; c < gh * gw; c++) {
    const d = cnt[c] > 1 ? cnt[c] : 1;
    g[c * 3] = acc[c * 3] / d; g[c * 3 + 1] = acc[c * 3 + 1] / d; g[c * 3 + 2] = acc[c * 3 + 2] / d;
  }
  const r = (k.length - 1) / 2;
  for (const axis of [0, 1]) {          // vertical, then horizontal
    const o = new Float64Array(gh * gw * 3);
    for (let yy = 0; yy < gh; yy++) for (let xx = 0; xx < gw; xx++) {
      let s0 = 0, s1 = 0, s2 = 0;
      for (let t = -r; t <= r; t++) {
        const q = axis === 0 ? (psReflect(yy + t, gh) * gw + xx) * 3 : (yy * gw + psReflect(xx + t, gw)) * 3;
        const kw = k[t + r];
        s0 += kw * g[q]; s1 += kw * g[q + 1]; s2 += kw * g[q + 2];
      }
      const c3 = (yy * gw + xx) * 3;
      o[c3] = s0; o[c3 + 1] = s1; o[c3 + 2] = s2;
    }
    g = o;
  }
  return { g, gh, gw };
}
// Second pass when Perceptual Saturation is on: fin = filter input, fmid = after Warmth/Saturation
function psPass(fin, fmid, out, W, H, BW, L0, L1, L2, hin, hout, hmx, hmn) {
  const { g, gh, gw } = psGrid(fmid, W, H, BW.psk);
  const t = BW.ps, op = BW.op, w = PS_W, mix = op !== 1;   // op = 1: mix gives q exactly
  const x0s = new Int32Array(W), x1s = new Int32Array(W), fxs = new Float64Array(W), rowv = new Float64Array(gw * 3);
  for (let x = 0; x < W; x++) {
    let gx = (x + 0.5) * gw / W - 0.5; gx = gx < 0 ? 0 : gx > gw - 1 ? gw - 1 : gx;
    const a = gw > 1 ? Math.min(Math.floor(gx), gw - 2) : 0;
    x0s[x] = a; x1s[x] = Math.min(a + 1, gw - 1); fxs[x] = gw > 1 ? gx - a : 0;
  }
  for (let y = 0, p = 0, j = 0; y < H; y++) {
    let gy = (y + 0.5) * gh / H - 0.5; gy = gy < 0 ? 0 : gy > gh - 1 ? gh - 1 : gy;
    const y0 = gh > 1 ? Math.min(Math.floor(gy), gh - 2) : 0, y1 = Math.min(y0 + 1, gh - 1), fy = gh > 1 ? gy - y0 : 0;
    for (let c = 0; c < gw * 3; c++) rowv[c] = g[y0 * gw * 3 + c] * (1 - fy) + g[y1 * gw * 3 + c] * fy;   // vertical first
    for (let x = 0; x < W; x++, p += 3, j += 4) {
      const fx = fxs[x], a = x0s[x] * 3, bq = x1s[x] * 3;
      const S0 = rowv[a] * (1 - fx) + rowv[bq] * fx;
      const S1 = rowv[a + 1] * (1 - fx) + rowv[bq + 1] * fx;
      const S2 = rowv[a + 2] * (1 - fx) + rowv[bq + 2] * fx;
      const f0 = fmid[p], f1 = fmid[p + 1], f2 = fmid[p + 2];
      const Y = (f0 + 2 * f1 + f2) / 4, YS = (S0 + 2 * S1 + S2) / 4;
      const C0 = f0 - Y, C1 = f1 - Y, C2 = f2 - Y, D0 = S0 - YS, D1 = S1 - YS, D2 = S2 - YS;
      const m = Y / 255 - 0.5;
      const nn = Math.sqrt(D0 * D0 + D1 * D1 + D2 * D2) / 128, nc = Math.sqrt(C0 * C0 + C1 * C1 + C2 * C2) / 128;
      const ka = w[0] + w[1] * m + w[6] * nn + w[7] * nc, kb = w[2] + w[3] * m + w[4] * m * m + w[8] * nc, e = w[5] * (Y - YS);
      // x | 0 instead of Math.floor: they differ only below 0, which is clipped to 0 anyway
      let q = (f0 + t * (C0 * ka + D0 * kb + e) + 0.5) | 0; q = q <= 0 ? 0 : q >= 255 ? 255 : q;
      let v = mix ? (fin[p] + (q - fin[p]) * op + 0.5) | 0 : q; const r = v <= 0 ? 0 : v >= 255 ? 255 : v;
      q = (f1 + t * (C1 * ka + D1 * kb + e) + 0.5) | 0; q = q <= 0 ? 0 : q >= 255 ? 255 : q;
      v = mix ? (fin[p + 1] + (q - fin[p + 1]) * op + 0.5) | 0 : q; const gg = v <= 0 ? 0 : v >= 255 ? 255 : v;
      q = (f2 + t * (C2 * ka + D2 * kb + e) + 0.5) | 0; q = q <= 0 ? 0 : q >= 255 ? 255 : q;
      v = mix ? (fin[p + 2] + (q - fin[p + 2]) * op + 0.5) | 0 : q; const b = v <= 0 ? 0 : v >= 255 ? 255 : v;
      hin[r]++; hin[256 + gg]++; hin[512 + b]++;
      const R = L0[r], G = L1[gg], B = L2[b];
      hout[R]++; hout[256 + G]++; hout[512 + B]++;
      hmx[R > G ? (R > B ? R : B) : (G > B ? G : B)]++;
      hmn[R < G ? (R < B ? R : B) : (G < B ? G : B)]++;
      out[j] = R; out[j + 1] = G; out[j + 2] = B; out[j + 3] = 255;
    }
  }
}
let psBufs = null;                     // reused between renders

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

function processPixels(base, out, n, E, withHist, mdB, dims) {
  // Mid/Detail needs the smoothed luma of this exact image (not available e.g. in crop mode)
  const MDa = E.md && mdB && mdB.length === n ? E.md : 0;
  const L0 = E.luts[0], L1 = E.luts[1], L2 = E.luts[2];
  const pt = E.point, lumOn = E.k !== 1 || E.o !== 0, k = E.k, o = E.o;
  const gOn = !!E.gains, g0 = gOn ? E.gains[0] : 1, g1 = gOn ? E.gains[1] : 1, g2 = gOn ? E.gains[2] : 1;
  const BG = E.boost, TN = E.tones, SG = E.sat, WB = E.wb, CT = E.con;
  const BW = E.bw, bwS = BW ? BW.s : 1, bwO = BW ? BW.op : 1, bwW = BW ? BW.w : null;
  const wR = bwW ? bwW[0] : 1, wG = bwW ? bwW[1] : 1, wB = bwW ? bwW[2] : 1;
  // Perceptual Saturation needs the whole filter input first (two passes); needs the image size
  const PS = !!(BW && BW.ps && dims && dims.w * dims.h === n);
  if (PS && (!psBufs || psBufs.n !== n)) psBufs = { n, fin: new Uint8Array(n * 3), fmid: new Uint8Array(n * 3) };
  const fin = PS ? psBufs.fin : null, fmid = PS ? psBufs.fmid : null;
  const w00 = WB ? WB[0][0] : 1, w01 = WB ? WB[0][1] : 0, w02 = WB ? WB[0][2] : 0,
        w10 = WB ? WB[1][0] : 0, w11 = WB ? WB[1][1] : 1, w12 = WB ? WB[1][2] : 0,
        w20 = WB ? WB[2][0] : 0, w21 = WB ? WB[2][1] : 0, w22 = WB ? WB[2][2] : 1;
  const H = withHist;
  const hin = H ? S.hin : new Uint32Array(768), hout = H ? S.hout : new Uint32Array(768);
  const hmx = H ? S.hmx : new Uint32Array(256), hmn = H ? S.hmn : new Uint32Array(256);
  hin.fill(0); hout.fill(0); hmx.fill(0); hmn.fill(0);
  for (let j = 0, end = n * 4; j < end; j += 4) {
    let r = base[j], g = base[j + 1], b = base[j + 2];
    if (MDa) {                      // Mid/Detail - must match apply_mid_detail() in fast_image_editor.py
      const y = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
      const w = Math.min(1, Math.pow((y > 0 ? y : 0) / 0.706, 0.662));
      const d = MDa * 0.952 * w * (y - mdB[j >> 2]) * 255;
      let v = r + d; r = v <= 0 ? 0 : v >= 255 ? 255 : v | 0;
      v = g + d; g = v <= 0 ? 0 : v >= 255 ? 255 : v | 0;
      v = b + d; b = v <= 0 ? 0 : v >= 255 ? 255 : v | 0;
    }
    if (pt) {
      let fr = r, fg = g, fb = b;
      if (lumOn) {
        const l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let bl = l * k + o;
        bl = bl < 0 ? 0 : bl > 255 ? 255 : bl;
        const ra = bl / (l > 1 ? l : 1);
        fr *= ra; fg *= ra; fb *= ra;
      }
      if (gOn) { fr *= g0; fg *= g1; fb *= g2; }
      r = fr <= 0 ? 0 : fr >= 255 ? 255 : fr | 0;
      g = fg <= 0 ? 0 : fg >= 255 ? 255 : fg | 0;
      b = fb <= 0 ? 0 : fb >= 255 ? 255 : fb | 0;
    }
    if (WB) {                       // Temp - must match apply_edit() in fast_image_editor.py
      const fr = w00 * r + w01 * g + w02 * b, fg = w10 * r + w11 * g + w12 * b, fb = w20 * r + w21 * g + w22 * b;
      r = fr <= 0 ? 0 : fr >= 255 ? 255 : fr | 0;
      g = fg <= 0 ? 0 : fg >= 255 ? 255 : fg | 0;
      b = fb <= 0 ? 0 : fb >= 255 ? 255 : fb | 0;
    }
    if (CT) {                       // Contrast / Pivot - must match apply_edit() in fast_image_editor.py
      const o0 = CT[r * 16], o1 = CT[g * 16], o2 = CT[b * 16];
      const y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
      let yi = Math.floor(y * 16 + 0.5); yi = yi > 4095 ? 4095 : yi;
      const d = CT[yi] - (0.2126 * o0 + 0.7152 * o1 + 0.0722 * o2);
      const fr = o0 + d, fg = o1 + d, fb = o2 + d;
      r = fr <= 0 ? 0 : fr >= 255 ? 255 : fr | 0;
      g = fg <= 0 ? 0 : fg >= 255 ? 255 : fg | 0;
      b = fb <= 0 ? 0 : fb >= 255 ? 255 : fb | 0;
    }
    if (TN) {                       // Shadows, Highlights - must match apply_edit() in fast_image_editor.py
      for (let t = 0; t < TN.length; t++) {
        const y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let yi = Math.floor(y * 16 + 0.5); yi = yi > 4095 ? 4095 : yi;
        const y2 = TN[t].T[yi];
        let fr, fg, fb;
        if (TN[t].ratio) { const q = y2 / (y > 0.5 ? y : 0.5); fr = r * q; fg = g * q; fb = b * q; }
        else { const d = y2 - y; fr = r + d; fg = g + d; fb = b + d; }
        r = fr <= 0 ? 0 : fr >= 255 ? 255 : fr | 0;
        g = fg <= 0 ? 0 : fg >= 255 ? 255 : fg | 0;
        b = fb <= 0 ? 0 : fb >= 255 ? 255 : fb | 0;
      }
    }
    if (BG) {                       // Color Boost - must match apply_edit() in fast_image_editor.py
      const mx = r > g ? (r > b ? r : b) : (g > b ? g : b), mn = r < g ? (r < b ? r : b) : (g < b ? g : b);
      const gn = BG[mx - mn];
      if (gn !== 1) {
        const y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let v = y + (r - y) * gn; r = v <= 0 ? 0 : v >= 255 ? 255 : v | 0;
        v = y + (g - y) * gn; g = v <= 0 ? 0 : v >= 255 ? 255 : v | 0;
        v = y + (b - y) * gn; b = v <= 0 ? 0 : v >= 255 ? 255 : v | 0;
      }
    }
    if (SG !== null && SG !== undefined) {   // Saturation - must match apply_edit()
      const y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
      let v = y + (r - y) * SG; r = v <= 0 ? 0 : v >= 255 ? 255 : v | 0;
      v = y + (g - y) * SG; g = v <= 0 ? 0 : v >= 255 ? 255 : v | 0;
      v = y + (b - y) * SG; b = v <= 0 ? 0 : v >= 255 ? 255 : v | 0;
    }
    if (BW) {                       // Filter Brilliance / Warmth - must match apply_edit()
      let wr = r, wg = g, wb = b;
      if (bwW) {
        wr = Math.floor(r * wR); wr = wr >= 255 ? 255 : wr;
        wg = Math.floor(g * wG); wg = wg >= 255 ? 255 : wg;
        wb = Math.floor(b * wB); wb = wb >= 255 ? 255 : wb;
      }
      const y = (wr + 2 * wg + wb) / 4;
      if (PS) {                     // first pass: keep input and Warmth/Saturation result, finish in psPass
        const p = (j >> 2) * 3;
        fin[p] = r; fin[p + 1] = g; fin[p + 2] = b;
        let f = y + (wr - y) * bwS; fmid[p] = f <= 0 ? 0 : f >= 255 ? 255 : Math.floor(f + 0.5);
        f = y + (wg - y) * bwS; fmid[p + 1] = f <= 0 ? 0 : f >= 255 ? 255 : Math.floor(f + 0.5);
        f = y + (wb - y) * bwS; fmid[p + 2] = f <= 0 ? 0 : f >= 255 ? 255 : Math.floor(f + 0.5);
        continue;
      }
      let f = y + (wr - y) * bwS; f = f <= 0 ? 0 : f >= 255 ? 255 : f;
      let v = Math.floor(r + (f - r) * bwO + 0.5); r = v <= 0 ? 0 : v >= 255 ? 255 : v;
      f = y + (wg - y) * bwS; f = f <= 0 ? 0 : f >= 255 ? 255 : f;
      v = Math.floor(g + (f - g) * bwO + 0.5); g = v <= 0 ? 0 : v >= 255 ? 255 : v;
      f = y + (wb - y) * bwS; f = f <= 0 ? 0 : f >= 255 ? 255 : f;
      v = Math.floor(b + (f - b) * bwO + 0.5); b = v <= 0 ? 0 : v >= 255 ? 255 : v;
    }
    hin[r]++; hin[256 + g]++; hin[512 + b]++;
    const R = L0[r], G = L1[g], B = L2[b];
    hout[R]++; hout[256 + G]++; hout[512 + B]++;
    hmx[R > G ? (R > B ? R : B) : (G > B ? G : B)]++;
    hmn[R < G ? (R < B ? R : B) : (G < B ? G : B)]++;
    out[j] = R; out[j + 1] = G; out[j + 2] = B; out[j + 3] = 255;
  }
  if (PS) psPass(fin, fmid, out, dims.w, dims.h, BW, L0, L1, L2, hin, hout, hmx, hmn);
}

/* ---------- Alt views ---------- */
