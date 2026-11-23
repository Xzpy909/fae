'use strict';
/* Screen-resolution view (see sharp-view.js): one band of the visible image resized to exactly the
   screen's pixels with Lanczos-3. Below half size the source is first averaged in f×f blocks
   (f = power of 2), so the filter itself never shrinks by more than 2× and stays cheap.
   in : {id, buf, rx, ry, rw, rh, f, s, ox, oy, X0, Y0, dw, dh}
        buf = RGBA of canvas pixels [rx, rx+rw) × [ry, ry+rh) (rx, ry multiples of f)
        s = screen pixels per canvas pixel; (ox, oy) = screen position of canvas pixel (0, 0)'s corner
        dest = screen pixels [X0, X0+dw) × [Y0, Y0+dh)
   out: {id, Y0, dw, dh, out} out = RGBA of the dest band */
const sinc = x => { if (x === 0) return 1; const p = Math.PI * x; return Math.sin(p) / p; };
const lanczos = x => (x <= -3 || x >= 3) ? 0 : sinc(x) * sinc(x / 3);

// for each dest pixel along one axis: first source index and normalized weights (Float32)
function weights(n, first, origin, s, f, len) {
  const sr = s * f, scale = Math.min(1, sr), R = 3 / scale;
  const start = new Int32Array(n), cnt = new Int32Array(n), maxTaps = Math.ceil(R * 2) + 2;
  const w = new Float32Array(n * maxTaps);
  for (let j = 0; j < n; j++) {
    // centre of screen pixel (first + j) in local block coordinates (pixel centres at i + 0.5)
    const c = (first + j + 0.5 - origin) / sr;
    let i0 = Math.max(0, Math.floor(c - R)), i1 = Math.min(len - 1, Math.ceil(c + R));
    if (i1 < i0) { i0 = i1 = Math.min(len - 1, Math.max(0, Math.floor(c))); }
    let sum = 0, k = 0;
    for (let i = i0; i <= i1 && k < maxTaps; i++, k++) { const v = lanczos((i + 0.5 - c) * scale); w[j * maxTaps + k] = v; sum += v; }
    if (sum === 0) { w[j * maxTaps] = 1; sum = 1; k = Math.max(k, 1); }
    for (let t = 0; t < k; t++) w[j * maxTaps + t] /= sum;
    start[j] = i0; cnt[j] = k;
  }
  return { start, cnt, w, maxTaps };
}

onmessage = ({ data: m }) => {
  const { rw, rh, f, s, dw, dh } = m, src = new Uint8ClampedArray(m.buf);
  // 1. f×f block averages (partial blocks at the edges average what they have)
  const bw = Math.ceil(rw / f), bh = Math.ceil(rh / f);
  let B;
  if (f === 1) {
    B = new Float32Array(bw * bh * 3);
    for (let i = 0, j = 0; i < bw * bh; i++, j += 4) { B[i * 3] = src[j]; B[i * 3 + 1] = src[j + 1]; B[i * 3 + 2] = src[j + 2]; }
  } else {
    B = new Float32Array(bw * bh * 3);
    const n = new Float32Array(bw * bh);
    for (let y = 0; y < rh; y++) {
      const by = (y / f) | 0, row = y * rw * 4;
      for (let x = 0; x < rw; x++) {
        const b = by * bw + ((x / f) | 0), j = row + x * 4;
        B[b * 3] += src[j]; B[b * 3 + 1] += src[j + 1]; B[b * 3 + 2] += src[j + 2]; n[b]++;
      }
    }
    for (let i = 0; i < bw * bh; i++) { const k = 1 / n[i]; B[i * 3] *= k; B[i * 3 + 1] *= k; B[i * 3 + 2] *= k; }
  }
  // screen position of the block grid's corner: canvas pixel (rx, ry)
  const gx = m.ox + m.rx * s, gy = m.oy + m.ry * s;
  // 2. horizontal pass: bh rows × dw
  const H = weights(dw, m.X0, gx, s, f, bw), T = new Float32Array(bh * dw * 3);
  for (let y = 0; y < bh; y++) {
    const rowB = y * bw * 3, rowT = y * dw * 3;
    for (let j = 0; j < dw; j++) {
      const st = H.start[j], c = H.cnt[j], wo = j * H.maxTaps;
      let r = 0, g = 0, b = 0;
      for (let t = 0, p = rowB + st * 3; t < c; t++, p += 3) { const v = H.w[wo + t]; r += B[p] * v; g += B[p + 1] * v; b += B[p + 2] * v; }
      const q = rowT + j * 3; T[q] = r; T[q + 1] = g; T[q + 2] = b;
    }
  }
  // 3. vertical pass: dh × dw → RGBA
  const V = weights(dh, m.Y0, gy, s, f, bh), out = new Uint8ClampedArray(dw * dh * 4);
  for (let i = 0; i < dh; i++) {
    const st = V.start[i], c = V.cnt[i], wo = i * V.maxTaps, rowO = i * dw * 4;
    for (let j = 0; j < dw; j++) {
      let r = 0, g = 0, b = 0;
      for (let t = 0, p = (st * dw + j) * 3; t < c; t++, p += dw * 3) { const v = V.w[wo + t]; r += T[p] * v; g += T[p + 1] * v; b += T[p + 2] * v; }
      const o = rowO + j * 4;
      out[o] = r + 0.5; out[o + 1] = g + 0.5; out[o + 2] = b + 0.5; out[o + 3] = 255;
    }
  }
  postMessage({ id: m.id, Y0: m.Y0, dw, dh, out: out.buffer }, [out.buffer]);
};
