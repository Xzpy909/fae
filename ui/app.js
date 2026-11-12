'use strict';
/* Fast Image Editor - front end.
   Python delivers the "stage" image (resize + White-Patch + Gray-Edge); everything after
   that (point correction, curves, clipping views) runs here, live, on every change.
   Process sends the same parameters back to Python, which applies identical math at full res. */

const $ = s => document.querySelector(s);
const clamp = (v, a, b) => v < a ? a : v > b ? b : v;
const clone = o => JSON.parse(JSON.stringify(o));
const basename = p => String(p).split(/[\\/]/).pop();
const api = () => window.pywebview.api;
const dpr = () => window.devicePixelRatio || 1;

const DEFAULT_STAGE = {
  resize_on: false, mode: '720p', mpix: 2.0, prio: 'Balanced',
  wp_on: false, wp_pct: 99.0, wp_strength: 1.0,
  ge_on: false, ge_p: 1, ge_sigma: 2.0, ge_strength: 1.0,
};
const ident = () => [[0, 0], [255, 255]];
const defaultEdit = () => ({
  black: null, gray: null, white: null,
  tBlack: 0, tWhite: 255, strength: 1,
  curves: { m: ident(), r: ident(), g: ident(), b: ident() },
});
const DEFAULT_EDIT_JSON = JSON.stringify(defaultEdit());
const defaultGeom = () => ({ q: 0, angle: 0, flip: false, crop: null, aspect: null });
const isDefaultGeom = g => !g || (!g.q && !g.angle && !g.flip && !g.crop);
function geomParams(g) {
  g = g || defaultGeom();
  return { rot_q: g.q || 0, angle: g.angle || 0, flip: !!g.flip,
    crop_x: g.crop ? g.crop.x : 0, crop_y: g.crop ? g.crop.y : 0,
    crop_w: g.crop ? g.crop.w : 0, crop_h: g.crop ? g.crop.h : 0 };
}
const stageParams = (stage, geom) => Object.assign(clone(stage), geomParams(geom));

const S = {
  files: [], cur: -1,
  stage: clone(DEFAULT_STAGE),
  edit: defaultEdit(),
  geom: defaultGeom(),
  cropMode: false,
  E: null,
  prefs: { fmt: 'WEBP', advance: true, stage: null, jpeg_q: 90, jpeg_mode: 'q', jpeg_d: 1.0,
    webp_lossy: false, webp_q: 90, avif_crf: 25, avif_preset: 4 },
  cmp: { x: 0.5, data: null, path: null, key: null, bytes: 0, enc: '', fmt: '', opts: null, req: 0, busy: false,
    jpegli: true, avif: true, svt: true },
  hist: [], fut: [], committed: null,
  img: null, outData: null, dispData: null, baseData: null, n: 0,
  hin: new Uint32Array(768), hout: new Uint32Array(768),
  hmx: new Uint32Array(256), hmn: new Uint32Array(256),
  tool: null, alt: false, before: false, space: false,
  thr: { shadows: null, highlights: null, neutral: null },
  modeCount: 0,
  view: { z: 1, x: 0, y: 0, fit: true },
  curveCh: 'm',
  reqId: 0, newFile: false, busy: false, clipboard: null,
  hover: null,
};

const canvas = $('#view'), ctx = canvas.getContext('2d');
const vp = $('#viewport'), stageEl = $('#stage');

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
  return { k, o, gains, luts, point };
}

/* ======================================================================
   Rendering
   ====================================================================== */
let rq = false;
function requestRender() {
  if (rq) return;
  rq = true;
  requestAnimationFrame(() => { rq = false; render(); });
}

function editChanged() {
  S.E = computeEdit(S.edit);
  updateToolUI();
  updateMarkers();
  requestRender();
  markCurrentEdited();
  if (S.cropMode && (typeof Crop !== "undefined")) Crop.editChanged();
  requestJpeg();
}

function render() {
  const I = S.img;
  if (!I || !S.E) { drawCurve(); drawHisto(); return; }
  const n = I.pw * I.ph;
  processPixels(I.base, S.outData.data, n, S.E, true);
  S.n = n;
  const mode = viewMode();
  let disp = S.outData;
  if (mode === 'before') disp = S.baseData;
  else if (mode) { buildModeView(mode); disp = S.dispData; }
  else if (cmpReady()) { buildCompare(); disp = S.dispData; }
  ctx.putImageData(disp, 0, 0);
  S.shown = disp;
  drawHisto(); drawCurve(); updateHud(mode); drawLoupe(); updateCmp();
}

/* ======================================================================
   Lossy split compare: left = lossless edit (live), right = real JPEG / AVIF / lossy WEBP round trip
   ====================================================================== */
const isJpeg = () => S.prefs.fmt === 'JPEG';
const isLossy = () => S.prefs.fmt === 'JPEG' || S.prefs.fmt === 'AVIF' || (S.prefs.fmt === 'WEBP' && !!S.prefs.webp_lossy);
function cmpReady() {
  const I = S.img, c = S.cmp;
  return isLossy() && I && c.data && c.fmt === S.prefs.fmt && c.path === I.path && c.data.width === I.pw && c.data.height === I.ph;
}
function buildCompare() {
  const I = S.img, o = S.outData.data, j = S.cmp.data.data, d = S.dispData.data;
  const W = I.pw * 4, split = clamp(Math.round(S.cmp.x * I.pw), 0, I.pw) * 4;
  d.set(o);
  if (split >= W) return;
  for (let y = 0, row = 0; y < I.ph; y++, row += W) d.set(j.subarray(row + split, row + W), row + split);
}

const fmtBytes = b => b >= 1048576 ? (b / 1048576).toFixed(2) + ' MB' : Math.max(1, Math.round(b / 1024)) + ' KB';
function updateSizeChip() {
  const el = $('#sizeChip'), I = S.img, c = S.cmp;
  const on = isLossy() && I && !S.cropMode && (cmpReady() || c.busy);
  el.classList.toggle('show', !!on);
  if (!on) return;
  const ready = cmpReady(), stale = c.busy || !ready || c.key !== jpegKey();
  el.innerHTML = ready
    ? `<b>${fmtBytes(c.bytes)}</b> <span class="muted">&middot; ${I.w}×${I.h} &middot; ${(c.bytes * 8 / (I.w * I.h)).toFixed(2)} bpp</span>`
    : '<span class="muted">encoding…</span>';
  el.classList.toggle('stale', stale);
  // pinned to the image's bottom-right corner, kept inside the viewport
  const v = S.view, r = vp.getBoundingClientRect(), m = 10;
  const x = clamp(v.x + I.pw * v.z - m, 160, r.width - m);
  const y = clamp(v.y + I.ph * v.z - m, 40, r.height - m);
  el.style.left = x + 'px'; el.style.top = y + 'px';
}

function updateCmp() {
  updateSizeChip();
  const el = $('#cmp'), I = S.img;
  const on = isLossy() && I && !S.cropMode && !viewMode();
  el.classList.toggle('show', !!on);
  if (!on) return;
  const v = S.view, r = vp.getBoundingClientRect();
  const left = v.x, top = v.y, w = I.pw * v.z, h = I.ph * v.z;
  const x = left + S.cmp.x * w;
  const y0 = clamp(top, 0, r.height), y1 = clamp(top + h, 0, r.height);
  const line = $('#cmpLine');
  line.style.left = x + 'px'; line.style.top = y0 + 'px'; line.style.height = Math.max(0, y1 - y0) + 'px';
  line.style.display = x < -2 || x > r.width + 2 || y1 <= y0 ? 'none' : 'block';
  const c = S.cmp, ready = cmpReady();
  const stale = c.busy || !ready || c.key !== jpegKey();
  const lbl = $('#cmpR');
  lbl.innerHTML = ready
    ? `${encName(c.fmt, c.enc)} ${optsLabel(c.fmt, c.opts, c.enc)}${stale ? ' <span class="muted">&middot; updating…</span>' : ''}`
    : `${encName(S.prefs.fmt, null)} <span class="muted">&middot; encoding…</span>`;
  lbl.classList.toggle('stale', stale);
  const ly = clamp(top + 10, 10, r.height - 34);
  const L = $('#cmpL');
  L.style.top = lbl.style.top = ly + 'px';
  L.style.left = 'auto'; L.style.right = (r.width - x + 10) + 'px';
  lbl.style.left = (x + 10) + 'px';
  L.style.display = x - 10 > 80 ? 'block' : 'none';
  lbl.style.display = r.width - x - 10 > 120 ? 'block' : 'none';
}

const encOpts = () => ({
  jpeg: { mode: S.prefs.jpeg_mode, q: S.prefs.jpeg_q, d: S.prefs.jpeg_d },
  webp: { lossless: !S.prefs.webp_lossy, q: S.prefs.webp_q },
  avif: { crf: S.prefs.avif_crf, preset: S.prefs.avif_preset },
});
function encName(fmt, enc) {
  if (fmt === 'JPEG') return enc === 'pillow' ? 'JPEG (Pillow)' : 'jpegli';
  if (fmt === 'AVIF') return enc === 'svt' ? 'AVIF SVT 4:4:4' : 'AVIF aom (fallback)';
  return 'WEBP lossy';
}
function optsLabel(fmt, o, enc) {
  if (!o) return '';
  if (fmt === 'JPEG') return o.mode === 'd' && enc !== 'pillow' ? `d<b>${Number(o.d).toFixed(2)}</b>` : `q<b>${o.q}</b>`;
  if (fmt === 'AVIF') return `crf <b>${o.crf}</b> &middot; p${o.preset}`;
  return `q<b>${o.q}</b>`;
}
/* only the active format's settings go into the cache key */
const activeOpts = () => ({ JPEG: encOpts().jpeg, AVIF: encOpts().avif, WEBP: encOpts().webp })[S.prefs.fmt] || null;
function jpegKey() {
  const f = S.files[S.cur];
  if (!f) return null;
  return JSON.stringify([jobFor(f, true), S.prefs.fmt, activeOpts()]);
}
let jpegTimer = null;
function requestJpeg(immediate) {
  if (!isLossy() || S.cur < 0 || !S.img || S.cropMode) return;
  clearTimeout(jpegTimer);
  jpegTimer = setTimeout(loadJpeg, immediate ? 0 : 260);
}
async function loadJpeg() {
  const f = S.files[S.cur];
  if (!isLossy() || !f || !S.img) return;
  const key = jpegKey();
  if (key === S.cmp.key && cmpReady()) return;
  const id = ++S.cmp.req;
  S.cmp.busy = true; updateCmp();
  let r;
  const fmt = S.prefs.fmt;
  try { r = await api().lossy_preview(jobFor(f, true), fmt, encOpts(), id); }
  catch (e) { r = { ok: false, error: String(e) }; }
  if (id !== S.cmp.req) return;
  if (!r || !r.ok) { S.cmp.busy = false; updateCmp(); toast('JPEG preview failed: ' + (r ? r.error : 'unknown'), true); return; }
  let buf;
  try { buf = await (await fetch('/preview/' + r.token)).arrayBuffer(); }
  catch (e) { S.cmp.busy = false; updateCmp(); return; }
  if (id !== S.cmp.req) return;
  S.cmp.busy = false;
  if (S.files[S.cur] !== f) return;
  Object.assign(S.cmp, { data: new ImageData(new Uint8ClampedArray(buf), r.pw, r.ph), path: f.path,
    key, bytes: r.bytes, enc: r.encoder, fmt: r.fmt, opts: r.opts });
  requestRender();
  if (jpegKey() !== key) requestJpeg();
}
function dropJpeg() { S.cmp.req++; S.cmp.data = null; S.cmp.key = null; S.cmp.busy = false; clearTimeout(jpegTimer); }

function fmtChanged() {
  syncEncRow();
  if (isLossy()) requestJpeg(true); else dropJpeg();
  updateInfo();
  requestRender();
}

(() => {
  const line = $('#cmpLine');
  let drag = false;
  const setX = e => {
    const I = S.img; if (!I) return;
    const r = canvas.getBoundingClientRect();
    S.cmp.x = clamp((e.clientX - r.left) / r.width, 0, 1);
    requestRender();
  };
  line.addEventListener('pointerdown', e => {
    if (e.button !== 0) return;
    e.stopPropagation(); e.preventDefault();
    drag = true; line.setPointerCapture(e.pointerId); $('#cmp').classList.add('dragging'); setX(e);
  });
  line.addEventListener('pointermove', e => { if (drag) { e.stopPropagation(); setX(e); } });
  const end = e => { if (!drag) return; drag = false; $('#cmp').classList.remove('dragging');
    try { line.releasePointerCapture(e.pointerId); } catch (_) {} };
  line.addEventListener('pointerup', end);
  line.addEventListener('pointercancel', end);
  line.addEventListener('dblclick', e => { e.stopPropagation(); S.cmp.x = 0.5; requestRender(); });
})();

/* Point correction + curves. Must match apply_edit() in fast_image_editor.py. */
function processPixels(base, out, n, E, withHist) {
  const L0 = E.luts[0], L1 = E.luts[1], L2 = E.luts[2];
  const pt = E.point, lumOn = E.k !== 1 || E.o !== 0, k = E.k, o = E.o;
  const gOn = !!E.gains, g0 = gOn ? E.gains[0] : 1, g1 = gOn ? E.gains[1] : 1, g2 = gOn ? E.gains[2] : 1;
  const H = withHist;
  const hin = H ? S.hin : new Uint32Array(768), hout = H ? S.hout : new Uint32Array(768);
  const hmx = H ? S.hmx : new Uint32Array(256), hmn = H ? S.hmn : new Uint32Array(256);
  hin.fill(0); hout.fill(0); hmx.fill(0); hmn.fill(0);
  for (let j = 0, end = n * 4; j < end; j += 4) {
    let r = base[j], g = base[j + 1], b = base[j + 2];
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
    hin[r]++; hin[256 + g]++; hin[512 + b]++;
    const R = L0[r], G = L1[g], B = L2[b];
    hout[R]++; hout[256 + G]++; hout[512 + B]++;
    hmx[R > G ? (R > B ? R : B) : (G > B ? G : B)]++;
    hmn[R < G ? (R < B ? R : B) : (G < B ? G : B)]++;
    out[j] = R; out[j + 1] = G; out[j + 2] = B; out[j + 3] = 255;
  }
}

/* ---------- Alt views ---------- */
function viewMode() {
  if (S.before) return 'before';
  if (!S.alt) return null;
  return { black: 'shadows', white: 'highlights', gray: 'neutral' }[S.tool] || 'clip';
}

function autoThr(mode) {
  const need = Math.max(1, S.n * 0.0005);
  let c = 0;
  if (mode === 'shadows') { for (let t = 0; t < 256; t++) { c += S.hmx[t]; if (c >= need) return t; } return 255; }
  if (mode === 'highlights') { for (let t = 255; t >= 0; t--) { c += S.hmn[t]; if (c >= need) return t; } return 0; }
  return 6;
}
const thrOf = mode => S.thr[mode] == null ? autoThr(mode) : S.thr[mode];

function buildModeView(mode) {
  const o = S.outData.data, d = S.dispData.data, base = S.img.base, len = S.n * 4;
  let cnt = 0;
  if (mode === 'shadows') {
    const T = thrOf(mode);
    for (let j = 0; j < len; j += 4) {
      const a = o[j] <= T, b = o[j + 1] <= T, c = o[j + 2] <= T;
      d[j] = a ? 0 : 255; d[j + 1] = b ? 0 : 255; d[j + 2] = c ? 0 : 255; d[j + 3] = 255;
      if (a && b && c) cnt++;
    }
  } else if (mode === 'highlights') {
    const T = thrOf(mode);
    for (let j = 0; j < len; j += 4) {
      const a = o[j] >= T, b = o[j + 1] >= T, c = o[j + 2] >= T;
      d[j] = a ? 255 : 0; d[j + 1] = b ? 255 : 0; d[j + 2] = c ? 255 : 0; d[j + 3] = 255;
      if (a && b && c) cnt++;
    }
  } else if (mode === 'neutral') {
    const T = thrOf(mode);
    for (let j = 0; j < len; j += 4) {
      const r = base[j], g = base[j + 1], b = base[j + 2];
      const mx = r > g ? (r > b ? r : b) : (g > b ? g : b), mn = r < g ? (r < b ? r : b) : (g < b ? g : b);
      const l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
      if (mx - mn <= T && l >= 12 && l <= 243) {
        d[j] = d[j + 1] = d[j + 2] = l; cnt++;
      } else {
        const v = l * 0.1; d[j] = v; d[j + 1] = v; d[j + 2] = v * 1.6 + 4;
      }
      d[j + 3] = 255;
    }
  } else { // clip overview
    for (let j = 0; j < len; j += 4) {
      const R = o[j], G = o[j + 1], B = o[j + 2];
      const mx = R > G ? (R > B ? R : B) : (G > B ? G : B), mn = R < G ? (R < B ? R : B) : (G < B ? G : B);
      if (mx === 0) { d[j] = 40; d[j + 1] = 125; d[j + 2] = 255; cnt++; }
      else if (mn === 255) { d[j] = 255; d[j + 1] = 55; d[j + 2] = 65; cnt++; }
      else if (mn === 0) { d[j] = 20; d[j + 1] = 70; d[j + 2] = 140; }
      else if (mx === 255) { d[j] = 150; d[j + 1] = 45; d[j + 2] = 40; }
      else { const v = (0.2126 * R + 0.7152 * G + 0.0722 * B) * 0.32; d[j] = d[j + 1] = d[j + 2] = v; }
      d[j + 3] = 255;
    }
  }
  S.modeCount = cnt;
}

function updateHud(mode) {
  const hud = $('#hud');
  if (!mode) { hud.classList.remove('show'); return; }
  const pct = S.n ? (S.modeCount / S.n * 100) : 0;
  const pctS = pct < 0.01 && pct > 0 ? '<0.01' : pct.toFixed(pct < 1 ? 2 : 1);
  const manual = m => S.thr[m] == null ? 'auto' : 'manual';
  let html;
  if (mode === 'before') html = 'Before &mdash; point correction &amp; curves off';
  else if (mode === 'shadows') html = `Shadows &le; <b>${thrOf(mode)}</b> <span class="muted">(${manual(mode)})</span> &middot; ${pctS}% px &middot; <span class="muted">scroll to adjust</span>`;
  else if (mode === 'highlights') html = `Highlights &ge; <b>${thrOf(mode)}</b> <span class="muted">(${manual(mode)})</span> &middot; ${pctS}% px &middot; <span class="muted">scroll to adjust</span>`;
  else if (mode === 'neutral') html = `Neutral candidates &middot; chroma &le; <b>${thrOf(mode)}</b> &middot; ${pctS}% px &middot; <span class="muted">scroll to adjust</span>`;
  else html = `Clipping &middot; <span style="color:#4d8dff">blue</span> shadows, <span style="color:#ff5a64">red</span> highlights &middot; ${pctS}% px &middot; <span class="muted">pick a tool for its view</span>`;
  hud.innerHTML = html;
  hud.classList.add('show');
}

function setAlt(on) {
  if (S.alt === on) return;
  S.alt = on;
  requestRender();
}

/* ======================================================================
   Stage loading (Python)
   ====================================================================== */
let stageTimer = null;
function requestStage(immediate) {
  clearTimeout(stageTimer);
  stageTimer = setTimeout(loadStage, immediate ? 0 : 140);
}

async function loadStage() {
  const f = S.files[S.cur];
  if (!f) return;
  const id = ++S.reqId;
  spinner(true);
  let r;
  try { r = await api().load_stage(f.path, stageParams(S.stage, S.geom), id); }
  catch (e) { r = { ok: false, req_id: id, error: String(e) }; }
  if (id !== S.reqId) return;
  if (!r || !r.ok) { spinner(false); toast(r ? r.error : 'Load failed', true); return; }
  let buf;
  try { buf = await (await fetch('/preview/' + r.token)).arrayBuffer(); }
  catch (e) { spinner(false); toast('Preview transfer failed: ' + e, true); return; }
  if (id !== S.reqId) return;
  spinner(false);
  const sizeChanged = !S.img || S.img.pw !== r.pw || S.img.ph !== r.ph;
  const base = new Uint8ClampedArray(buf);
  S.img = Object.assign({}, r, { base, path: f.path });
  if (sizeChanged) {
    canvas.width = r.pw; canvas.height = r.ph;
    S.outData = new ImageData(r.pw, r.ph);
    S.dispData = new ImageData(r.pw, r.ph);
  }
  S.baseData = new ImageData(base, r.pw, r.ph);
  canvas.style.display = 'block';
  if (sizeChanged || S.newFile) fitView();
  S.newFile = false;
  updateInfo();
  render();
  requestJpeg(true);
}

function spinner(on) { $('#spinner').classList.toggle('show', on); }

function updateInfo() {
  const f = S.files[S.cur];
  $('#fileName').textContent = f ? f.name : 'No image';
  const I = S.img;
  $('#fileDims').textContent = I && f ? `${I.src_w}×${I.src_h}  →  ${I.w}×${I.h}` : '';
  $('#outInfo').textContent = I && f
    ? `Output ${I.w}×${I.h} · ${(I.w * I.h / 1e6).toFixed(2)} MP${I.alpha ? (isJpeg() ? ' · alpha → white' : ' · alpha') : ''}`
      + (isJpeg() && !S.cmp.jpegli ? ' · cjpegli.exe missing, using Pillow' : '')
      + (S.prefs.fmt === 'AVIF' && !S.cmp.avif ? ' · preview needs Pillow ≥ 11.2 (pip install -U pillow)' : '')
      + (S.prefs.fmt === 'AVIF' && !S.cmp.svt ? ' · SvtAv1EncApp.exe missing, using aom' : '')
      + (S.prefs.fmt === 'AVIF' && S.cmp.svt && I.alpha ? ' · alpha → aom' : '')
    : '';
  $('#empty').style.display = S.files.length ? 'none' : 'grid';
}

/* ======================================================================
   View: zoom / pan / markers
   ====================================================================== */
function applyView() {
  const v = S.view, I = S.img;
  stageEl.style.transform = `translate(${v.x}px, ${v.y}px) scale(${v.z})`;
  canvas.classList.toggle('pixel', v.z * dpr() >= 2);
  $('#zoomLabel').textContent = I ? Math.round(v.z * dpr() * (I.pw / I.w) * 100) + '%' : '–';
  updateMarkers();
  updateCmp();
}

function fitView() {
  const I = S.img; if (!I) return;
  const r = vp.getBoundingClientRect(), pad = 28;
  const z = Math.min((r.width - pad * 2) / I.pw, (r.height - pad * 2) / I.ph);
  S.view = { z, x: (r.width - I.pw * z) / 2, y: (r.height - I.ph * z) / 2, fit: true };
  applyView();
}

function zoomTo(z, cx, cy) {
  const v = S.view, I = S.img; if (!I) return;
  const r = vp.getBoundingClientRect();
  if (cx == null) { cx = r.width / 2; cy = r.height / 2; }
  z = clamp(z, 0.02, 40);
  v.x = cx - (cx - v.x) * (z / v.z);
  v.y = cy - (cy - v.y) * (z / v.z);
  v.z = z; v.fit = false;
  applyView();
}

function zoom100() {
  const I = S.img; if (!I) return;
  zoomTo((I.w / I.pw) / dpr());
}

function updateMarkers() {
  const box = $('#markers');
  const I = S.img;
  if (!I) { box.innerHTML = ''; return; }
  const v = S.view;
  let html = '';
  for (const t of ['black', 'gray', 'white']) {
    const s = S.edit[t];
    if (!s || !s.pos) continue;
    const x = v.x + s.pos[0] * I.pw * v.z, y = v.y + s.pos[1] * I.ph * v.z;
    html += `<div class="mk ${t}" style="left:${x}px;top:${y}px">${t[0].toUpperCase()}</div>`;
  }
  box.innerHTML = html;
}

function toImg(e) {
  const I = S.img; if (!I) return null;
  const r = canvas.getBoundingClientRect();
  const nx = (e.clientX - r.left) / r.width, ny = (e.clientY - r.top) / r.height;
  const inside = nx >= 0 && ny >= 0 && nx < 1 && ny < 1;
  return { nx, ny, inside, px: Math.floor(nx * I.pw), py: Math.floor(ny * I.ph) };
}

let pan = null;
vp.addEventListener('pointerdown', e => {
  if (!S.img) return;
  if (S.cropMode) return;
  if (e.altKey !== S.alt) setAlt(e.altKey);
  const wantPan = e.button === 1 || (e.button === 0 && (S.space || !S.tool));
  if (wantPan) {
    e.preventDefault();
    pan = { x: e.clientX, y: e.clientY, vx: S.view.x, vy: S.view.y };
    vp.setPointerCapture(e.pointerId);
    vp.classList.add('panning');
    return;
  }
  if (e.button === 0 && S.tool) {
    const p = toImg(e);
    if (p && p.inside) sampleAt(S.tool, p.nx, p.ny);
  }
});
vp.addEventListener('pointermove', e => {
  if (S.cropMode) return;
  if (e.altKey !== S.alt) setAlt(e.altKey);
  if (pan) {
    S.view.x = pan.vx + e.clientX - pan.x;
    S.view.y = pan.vy + e.clientY - pan.y;
    S.view.fit = false;
    applyView();
    return;
  }
  const p = toImg(e);
  S.hover = p && p.inside ? p : null;
  updateHover();
});
vp.addEventListener('pointerup', e => {
  if (pan) { pan = null; vp.classList.remove('panning'); try { vp.releasePointerCapture(e.pointerId); } catch (_) {} }
});
vp.addEventListener('pointerleave', () => { if (!pan) { S.hover = null; updateHover(); } });
vp.addEventListener('dblclick', e => { if (!S.cropMode && !S.tool && S.img) { S.view.fit ? zoom100() : fitView(); } });
vp.addEventListener('auxclick', e => { if (e.button === 1) e.preventDefault(); });

vp.addEventListener('wheel', e => {
  if (!S.img) return;
  if (S.cropMode) return;
  e.preventDefault();
  const mode = viewMode();
  if (e.altKey && mode && mode !== 'clip' && mode !== 'before') {
    const step = (e.deltaY < 0 ? 1 : -1) * (e.shiftKey ? 5 : 1);
    const cur = thrOf(mode);
    if (mode === 'shadows') S.thr.shadows = clamp(cur + step, 0, 255);
    else if (mode === 'highlights') S.thr.highlights = clamp(cur - step, 0, 255);
    else S.thr.neutral = clamp(cur + step, 0, 60);
    requestRender();
    return;
  }
  const r = vp.getBoundingClientRect();
  zoomTo(S.view.z * Math.exp(-e.deltaY * 0.0015), e.clientX - r.left, e.clientY - r.top);
}, { passive: false });

$('#zoomFit').addEventListener('click', fitView);
$('#zoom100').addEventListener('click', zoom100);

window.addEventListener('resize', () => { if (S.view.fit) fitView(); else applyView(); drawCurve(); drawHisto(); });

/* ---------- hover readout + loupe ---------- */
function updateHover() {
  const I = S.img, h = S.hover;
  if (!I || !h || !S.outData) {
    $('#stCoords').textContent = '–'; $('#stRGB').innerHTML = '';
    $('#loupe').classList.remove('show');
    return;
  }
  const j = (h.py * I.pw + h.px) * 4, b = I.base, o = S.outData.data;
  const ox = Math.floor(h.nx * I.w), oy = Math.floor(h.ny * I.h);
  $('#stCoords').textContent = `x ${ox}  y ${oy}`;
  const chip = (r, g, bb) => `<span class="chip" style="background:rgb(${r},${g},${bb})"></span>`;
  const fmt = (r, g, bb) => `${String(r).padStart(3)} ${String(g).padStart(3)} ${String(bb).padStart(3)}`;
  $('#stRGB').innerHTML = `in${chip(b[j], b[j + 1], b[j + 2])}${fmt(b[j], b[j + 1], b[j + 2])}   →   out${chip(o[j], o[j + 1], o[j + 2])}${fmt(o[j], o[j + 1], o[j + 2])}`;
  drawLoupe();
}

const loupe = $('#loupe'), lctx = loupe.getContext('2d');
const lsrc = document.createElement('canvas'); lsrc.width = lsrc.height = 11;
const lsctx = lsrc.getContext('2d');
function drawLoupe() {
  const I = S.img, h = S.hover;
  if (!I || !h || !S.tool || !S.shown) { loupe.classList.remove('show'); return; }
  loupe.classList.add('show');
  const N = 11, half = 5, id = lsctx.createImageData(N, N), src = S.shown.data;
  for (let y = 0; y < N; y++) for (let x = 0; x < N; x++) {
    const sx = h.px + x - half, sy = h.py + y - half, d = (y * N + x) * 4;
    if (sx < 0 || sy < 0 || sx >= I.pw || sy >= I.ph) { id.data[d + 3] = 0; continue; }
    const s = (sy * I.pw + sx) * 4;
    id.data[d] = src[s]; id.data[d + 1] = src[s + 1]; id.data[d + 2] = src[s + 2]; id.data[d + 3] = 255;
  }
  lsctx.putImageData(id, 0, 0);
  lctx.imageSmoothingEnabled = false;
  lctx.fillStyle = '#000'; lctx.fillRect(0, 0, 132, 132);
  lctx.drawImage(lsrc, 0, 0, 132, 132);
  // 3x3 sample box (what the eyedropper medians)
  lctx.strokeStyle = 'rgba(0,0,0,.85)'; lctx.lineWidth = 3;
  lctx.strokeRect(12 * (half - 1), 12 * (half - 1), 36, 36);
  lctx.strokeStyle = '#f0b35a'; lctx.lineWidth = 1.5;
  lctx.strokeRect(12 * (half - 1), 12 * (half - 1), 36, 36);
}

/* ======================================================================
   Eyedroppers
   ====================================================================== */
async function sampleAt(tool, nx, ny) {
  let rgb = null;
  try { rgb = await api().sample(nx, ny); } catch (e) { toast(String(e), true); }
  if (!rgb) return;
  S.edit[tool] = { rgb, pos: [nx, ny] };
  editChanged();
  commit();
  const names = { black: 'Black point', gray: 'Neutral gray', white: 'White point' };
  status(`${names[tool]} ← RGB ${rgb.join(', ')}`);
}

function setTool(t) {
  S.tool = t;
  vp.classList.toggle('tool', !!t);
  document.querySelectorAll('.tool').forEach(b => b.classList.toggle('on', b.dataset.tool === t));
  const chip = $('#modeChip');
  if (t) {
    const col = { black: '#000', gray: '#808080', white: '#fff' }[t];
    chip.innerHTML = `<span class="dot" style="background:${col}"></span>${{ black: 'Black point', gray: 'Neutral gray', white: 'White point' }[t]} &middot; click to sample &middot; <span class="muted">Alt = ${t === 'gray' ? 'neutral finder' : 'threshold view'}</span>`;
    chip.classList.add('show');
  } else chip.classList.remove('show');
  drawLoupe();
  requestRender();
}
const toggleTool = t => setTool(S.tool === t ? null : t);

function updateToolUI() {
  for (const t of ['black', 'gray', 'white']) {
    const sw = $('#sw-' + t), s = S.edit[t];
    sw.classList.toggle('set', !!s);
    sw.style.background = s ? `rgb(${s.rgb.join(',')})` : 'transparent';
    sw.title = s ? `RGB ${s.rgb.join(', ')}` : 'not set';
  }
}

/* ======================================================================
   Curves editor
   ====================================================================== */
const cv = $('#curve'), cctx = cv.getContext('2d');
const CPAD = 8;
let cdrag = null;

function ensureCanvas(c, g) {
  const r = c.getBoundingClientRect();
  const w = Math.round(r.width * dpr()), h = Math.round(r.height * dpr());
  if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
  g.setTransform(dpr(), 0, 0, dpr(), 0, 0);
  return { w: r.width, h: r.height };
}
function c2p(x, y, w, h) { return [CPAD + x / 255 * (w - 2 * CPAD), CPAD + (1 - y / 255) * (h - 2 * CPAD)]; }
function p2c(px, py, w, h) {
  return [clamp(Math.round((px - CPAD) / (w - 2 * CPAD) * 255), 0, 255),
          clamp(Math.round((1 - (py - CPAD) / (h - 2 * CPAD)) * 255), 0, 255)];
}
const CH_COL = { m: '#e7e9ec', r: '#ff6b6b', g: '#5fd08a', b: '#6aa8ff' };
const CH_FILL = { m: 'rgba(200,205,212,.16)', r: 'rgba(255,107,107,.2)', g: 'rgba(95,208,138,.2)', b: 'rgba(106,168,255,.22)' };

function drawCurve() {
  const { w, h } = ensureCanvas(cv, cctx);
  if (!w) return;
  const g = cctx;
  g.clearRect(0, 0, w, h);
  g.fillStyle = '#111316'; g.fillRect(0, 0, w, h);
  // grid
  g.strokeStyle = '#22262c'; g.lineWidth = 1;
  for (let i = 0; i <= 4; i++) {
    const [x] = c2p(i * 63.75, 0, w, h), [, y] = c2p(0, i * 63.75, w, h);
    g.beginPath(); g.moveTo(Math.round(x) + .5, CPAD); g.lineTo(Math.round(x) + .5, h - CPAD); g.stroke();
    g.beginPath(); g.moveTo(CPAD, Math.round(y) + .5); g.lineTo(w - CPAD, Math.round(y) + .5); g.stroke();
  }
  // histogram of the curve input
  const ch = S.curveCh;
  if (S.img) {
    const hv = new Float64Array(256);
    for (let i = 0; i < 256; i++) {
      hv[i] = ch === 'm' ? S.hin[i] + S.hin[256 + i] + S.hin[512 + i]
        : S.hin[{ r: 0, g: 256, b: 512 }[ch] + i];
    }
    let mx = 1; for (let i = 1; i < 255; i++) mx = Math.max(mx, hv[i]);
    g.beginPath();
    const [x0, y0] = c2p(0, 0, w, h); g.moveTo(x0, y0);
    for (let i = 0; i < 256; i++) {
      const v = Math.min(1, Math.sqrt(hv[i] / mx)) * 0.92;
      const [x, y] = c2p(i, v * 255, w, h); g.lineTo(x, y);
    }
    const [x1] = c2p(255, 0, w, h); g.lineTo(x1, y0); g.closePath();
    g.fillStyle = CH_FILL[ch]; g.fill();
  }
  // baseline
  g.setLineDash([3, 4]); g.strokeStyle = '#3a3f46';
  g.beginPath(); let p = c2p(0, 0, w, h); g.moveTo(p[0], p[1]); p = c2p(255, 255, w, h); g.lineTo(p[0], p[1]); g.stroke();
  g.setLineDash([]);
  const drawLine = (pts, col, width) => {
    const L = curveLut(pts);
    g.strokeStyle = col; g.lineWidth = width; g.beginPath();
    for (let x = 0; x < 256; x++) { const [px, py] = c2p(x, L[x], w, h); x ? g.lineTo(px, py) : g.moveTo(px, py); }
    g.stroke();
  };
  const isIdent = pts => JSON.stringify(pts) === '[[0,0],[255,255]]';
  for (const c of ['r', 'g', 'b', 'm']) {
    if (c === ch || isIdent(S.edit.curves[c])) continue;
    g.globalAlpha = 0.38; drawLine(S.edit.curves[c], CH_COL[c], 1.2); g.globalAlpha = 1;
  }
  const pts = S.edit.curves[ch];
  drawLine(pts, CH_COL[ch], 2);
  pts.forEach((pt, i) => {
    const [px, py] = c2p(pt[0], pt[1], w, h);
    const active = cdrag && cdrag.idx === i;
    g.beginPath(); g.arc(px, py, active ? 5.5 : 4.5, 0, Math.PI * 2);
    g.fillStyle = active ? '#f0b35a' : '#111316'; g.fill();
    g.lineWidth = 1.8; g.strokeStyle = active ? '#f0b35a' : CH_COL[ch]; g.stroke();
  });
}

function nearestPoint(px, py, w, h) {
  const pts = S.edit.curves[S.curveCh];
  let best = -1, bd = 1e9;
  pts.forEach((pt, i) => {
    const [x, y] = c2p(pt[0], pt[1], w, h), d = Math.hypot(x - px, y - py);
    if (d < bd) { bd = d; best = i; }
  });
  return bd <= 9 ? best : -1;
}
function insertPoint(pts, x, y) {
  if (pts.some(p => p[0] === x)) return -1;
  let i = pts.findIndex(p => p[0] > x);
  if (i < 0) i = pts.length;
  pts.splice(i, 0, [x, y]);
  return i;
}

cv.addEventListener('pointerdown', e => {
  if (e.button !== 0) return;
  const r = cv.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
  const pts = S.edit.curves[S.curveCh];
  let idx = nearestPoint(px, py, r.width, r.height);
  if (idx < 0) {
    const [x, y] = p2c(px, py, r.width, r.height);
    idx = insertPoint(pts, x, y);
    if (idx < 0) return;
    editChanged();
  }
  cdrag = { idx };
  cv.setPointerCapture(e.pointerId);
  drawCurve();
});
cv.addEventListener('pointermove', e => {
  const r = cv.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
  const w = r.width, h = r.height;
  const [x, y] = p2c(px, py, w, h);
  if (!cdrag) {
    const L = curveLut(S.edit.curves[S.curveCh]);
    if (px >= 0 && px <= w && py >= 0 && py <= h) $('#curveInfo').innerHTML = `<span class="mono">in ${x} → out ${L[x]}</span>`;
    return;
  }
  const pts = S.edit.curves[S.curveCh];
  const outside = (px < -24 || px > w + 24 || py < -24 || py > h + 24);
  if (outside) {
    if (cdrag.idx >= 0 && pts.length > 2) { pts.splice(cdrag.idx, 1); cdrag.idx = -1; editChanged(); }
    $('#curveInfo').innerHTML = '<span style="color:var(--red)">release to remove point</span>';
    return;
  }
  if (cdrag.idx < 0) {
    const i = insertPoint(pts, x, y);
    if (i < 0) return;
    cdrag.idx = i;
  }
  const i = cdrag.idx, n = pts.length;
  const lo = i > 0 ? pts[i - 1][0] + 1 : 0, hi = i < n - 1 ? pts[i + 1][0] - 1 : 255;
  pts[i] = [clamp(x, lo, hi), y];
  $('#curveInfo').innerHTML = `<span class="mono">in ${pts[i][0]} → out ${pts[i][1]}</span>`;
  editChanged();
});
const endCurveDrag = () => { if (cdrag) { cdrag = null; commit(); drawCurve(); } };
cv.addEventListener('pointerup', endCurveDrag);
cv.addEventListener('pointercancel', endCurveDrag);
cv.addEventListener('pointerleave', () => { if (!cdrag) $('#curveInfo').textContent = 'Click to add · drag · double-click or drag out to remove'; });
cv.addEventListener('dblclick', e => {
  const r = cv.getBoundingClientRect();
  const pts = S.edit.curves[S.curveCh];
  const i = nearestPoint(e.clientX - r.left, e.clientY - r.top, r.width, r.height);
  if (i >= 0 && pts.length > 2) { pts.splice(i, 1); editChanged(); commit(); }
});

document.querySelectorAll('#curveTabs button').forEach(b => b.addEventListener('click', () => {
  S.curveCh = b.dataset.ch;
  document.querySelectorAll('#curveTabs button').forEach(x => x.classList.toggle('on', x === b));
  drawCurve();
}));
$('#resetCurve').addEventListener('click', () => { S.edit.curves[S.curveCh] = ident(); editChanged(); commit(); });
new ResizeObserver(() => drawCurve()).observe(cv);

/* ======================================================================
   Histogram (output)
   ====================================================================== */
const hc = $('#histo'), hctx = hc.getContext('2d');
function drawHisto() {
  const { w, h } = ensureCanvas(hc, hctx);
  if (!w) return;
  const g = hctx;
  g.clearRect(0, 0, w, h);
  g.fillStyle = '#111316'; g.fillRect(0, 0, w, h);
  if (!S.img || !S.n) { $('#clipLo').textContent = $('#clipHi').textContent = '0%'; return; }
  let mx = 1;
  for (let c = 0; c < 3; c++) for (let i = 1; i < 255; i++) mx = Math.max(mx, S.hout[c * 256 + i]);
  g.globalCompositeOperation = 'lighter';
  const cols = ['rgba(255,80,80,.62)', 'rgba(80,220,120,.55)', 'rgba(90,150,255,.68)'];
  for (let c = 0; c < 3; c++) {
    g.beginPath(); g.moveTo(0, h);
    for (let i = 0; i < 256; i++) {
      const v = Math.min(1, Math.sqrt(S.hout[c * 256 + i] / mx));
      g.lineTo(i / 255 * w, h - v * (h - 6));
    }
    g.lineTo(w, h); g.closePath(); g.fillStyle = cols[c]; g.fill();
  }
  g.globalCompositeOperation = 'source-over';
  const lo = S.hmn[0] / S.n * 100, hi = S.hmx[255] / S.n * 100;
  const f = v => v === 0 ? '0%' : v < 0.01 ? '<0.01%' : v.toFixed(v < 1 ? 2 : 1) + '%';
  $('#clipLo').textContent = '▼ ' + f(lo); $('#clipHi').textContent = f(hi) + ' ▲';
  $('#clipLo').classList.toggle('warn', lo > 0.5); $('#clipHi').classList.toggle('warn', hi > 0.5);
}
new ResizeObserver(() => drawHisto()).observe(hc);

/* ======================================================================
   Panel controls
   ====================================================================== */
const FIELDS = [];
function makeField(parent, spec) {
  const { obj, key, label, min, max, step, dec } = spec;
  const smin = spec.smin ?? min, smax = spec.smax ?? max;
  const row = document.createElement('div');
  row.className = 'field';
  row.innerHTML = `<label>${label}</label><input type="range" min="${smin}" max="${smax}" step="${step}"><input type="number" min="${min}" max="${max}" step="${step}">`;
  const rng = row.children[1], num = row.children[2];
  const set = (v, fin) => {
    v = clamp(Number(v), min, max);
    if (!isFinite(v)) return;
    v = dec === 0 ? Math.round(v) : Number(v.toFixed(dec));
    S[obj][key] = v;
    sync();
    valueChanged(obj);
    if (fin) commit();
  };
  const sync = () => {
    const v = S[obj][key];
    rng.value = v; num.value = Number(v).toFixed(dec);
    rng.style.setProperty('--p', ((clamp(v, smin, smax) - smin) / (smax - smin) * 100) + '%');
    if (spec.enabled) row.classList.toggle('disabled', !spec.enabled());
  };
  rng.addEventListener('input', () => set(rng.value, false));
  rng.addEventListener('change', () => commit());
  rng.addEventListener('dblclick', () => set(spec.def, true));
  num.addEventListener('change', () => set(num.value, true));
  num.addEventListener('keydown', e => { if (e.key === 'Enter') num.blur(); e.stopPropagation(); });
  num.addEventListener('wheel', e => {
    e.preventDefault();
    set(S[obj][key] + (e.deltaY < 0 ? 1 : -1) * step * (e.shiftKey ? 10 : 1), true);
  }, { passive: false });
  parent.appendChild(row);
  FIELDS.push({ sync });
}

function valueChanged(obj) {
  if (obj === 'stage') { requestStage(); savePrefsSoon(); syncEnabled(); markCurrentEdited(); }
  else if (obj === 'prefs') { savePrefsSoon(); requestJpeg(); updateCmp(); }
  else editChanged();
}

makeField($('#pointFields'), { obj: 'edit', key: 'tBlack', label: 'Black target', min: 0, max: 255, step: 1, dec: 0, def: 0 });
makeField($('#pointFields'), { obj: 'edit', key: 'tWhite', label: 'White target', min: 0, max: 255, step: 1, dec: 0, def: 255 });
makeField($('#pointFields'), { obj: 'edit', key: 'strength', label: 'Strength', min: 0.05, max: 1, step: 0.05, dec: 2, def: 1 });
makeField($('#jpegQ'), { obj: 'prefs', key: 'jpeg_q', label: 'Quality', min: 1, max: 100, smin: 50, smax: 100, step: 1, dec: 0, def: 90 });
makeField($('#jpegD'), { obj: 'prefs', key: 'jpeg_d', label: 'Distance', min: 0, max: 25, smin: 0.3, smax: 4, step: 0.05, dec: 2, def: 1 });
makeField($('#webpQ'), { obj: 'prefs', key: 'webp_q', label: 'Quality', min: 0, max: 100, smin: 50, smax: 100, step: 1, dec: 0, def: 90 });
makeField($('#avifQ'), { obj: 'prefs', key: 'avif_crf', label: 'CRF', min: 1, max: 70, smin: 5, smax: 50, step: 0.25, dec: 2, def: 25 });
makeField($('#avifS'), { obj: 'prefs', key: 'avif_preset', label: 'Preset', min: 0, max: 8, step: 1, dec: 0, def: 4 });
makeField($('#mpixFields'), { obj: 'stage', key: 'mpix', label: 'Target Mpix', min: 0.05, max: 100, smin: 0.1, smax: 24, step: 0.05, dec: 2, def: 2,
  enabled: () => S.stage.mode === 'Mpix' });
makeField($('#wpFields'), { obj: 'stage', key: 'wp_pct', label: 'Percentile', min: 50, max: 100, step: 0.1, dec: 1, def: 99, enabled: () => S.stage.wp_on });
makeField($('#wpFields'), { obj: 'stage', key: 'wp_strength', label: 'Strength', min: 0, max: 1, step: 0.05, dec: 2, def: 1, enabled: () => S.stage.wp_on });
makeField($('#geFields'), { obj: 'stage', key: 'ge_p', label: 'Minkowski p', min: 1, max: 12, step: 1, dec: 0, def: 1, enabled: () => S.stage.ge_on });
makeField($('#geFields'), { obj: 'stage', key: 'ge_sigma', label: 'Blur sigma', min: 0, max: 10, step: 0.5, dec: 1, def: 2, enabled: () => S.stage.ge_on });
makeField($('#geFields'), { obj: 'stage', key: 'ge_strength', label: 'Strength', min: 0, max: 1, step: 0.05, dec: 2, def: 1, enabled: () => S.stage.ge_on });

function segInit(sel, get, set) {
  const el = $(sel);
  el.querySelectorAll('button').forEach(b => b.addEventListener('click', () => { set(b.dataset.v); syncPanel(); }));
  return () => el.querySelectorAll('button').forEach(b => b.classList.toggle('on', b.dataset.v === get()));
}
const syncMode = segInit('#modeSeg', () => S.stage.mode, v => { S.stage.mode = v; valueChanged('stage'); commit(); });
const syncFmt = segInit('#fmtSeg', () => S.prefs.fmt, v => { S.prefs.fmt = v; savePrefsSoon(); fmtChanged(); });
const syncJMode = segInit('#jModeSeg', () => S.prefs.jpeg_mode, v => { S.prefs.jpeg_mode = v; valueChanged('prefs'); });
const syncWMode = segInit('#wModeSeg', () => S.prefs.webp_lossy ? 'lossy' : 'lossless',
  v => { S.prefs.webp_lossy = v === 'lossy'; savePrefsSoon(); fmtChanged(); });
function syncEncRow() {
  const f = S.prefs.fmt;
  $('#encRow').classList.toggle('show', f !== 'PNG');
  for (const g of ['JPEG', 'WEBP', 'AVIF']) $('#grp' + g).classList.toggle('show', f === g);
  $('#jpegQ').style.display = S.prefs.jpeg_mode === 'd' ? 'none' : '';
  $('#jpegD').style.display = S.prefs.jpeg_mode === 'd' ? '' : 'none';
  $('#webpQ').style.display = S.prefs.webp_lossy ? '' : 'none';
}
$('#prioSel').addEventListener('change', e => { S.stage.prio = e.target.value; valueChanged('stage'); commit(); });
$('#resizeOn').addEventListener('change', e => { S.stage.resize_on = e.target.checked; valueChanged('stage'); commit(); syncPanel(); });
$('#wpOn').addEventListener('change', e => { S.stage.wp_on = e.target.checked; valueChanged('stage'); commit(); syncPanel(); });
$('#geOn').addEventListener('change', e => { S.stage.ge_on = e.target.checked; valueChanged('stage'); commit(); syncPanel(); });
$('#advanceChk').addEventListener('change', e => { S.prefs.advance = e.target.checked; savePrefsSoon(); });

function syncEnabled() {
  $('#prioSel').disabled = S.stage.mode !== 'Mpix';
  $('#prioSel').closest('.field-row').style.opacity = S.stage.mode === 'Mpix' ? 1 : .38;
}
function syncPanel() {
  FIELDS.forEach(f => f.sync());
  syncMode(); syncFmt(); syncJMode(); syncWMode();
  syncEncRow();
  $('#prioSel').value = S.stage.prio;
  $('#resizeOn').checked = S.stage.resize_on !== false;
  $('#resizeBody').classList.toggle('off', S.stage.resize_on === false);
  $('#wpOn').checked = S.stage.wp_on;
  $('#geOn').checked = S.stage.ge_on;
  $('#advanceChk').checked = !!S.prefs.advance;
  syncEnabled();
  updateToolUI();
  updateUndoUI();
}

document.querySelectorAll('.tool').forEach(b => b.addEventListener('click', () => toggleTool(b.dataset.tool)));
$('#clearPoints').addEventListener('click', () => { S.edit.black = S.edit.gray = S.edit.white = null; editChanged(); commit(); });

/* ---------- prefs ---------- */
let prefsTimer = null;
function savePrefsSoon() {
  clearTimeout(prefsTimer);
  prefsTimer = setTimeout(() => {
    S.prefs.stage = clone(S.stage);
    try { api().save_settings(S.prefs); } catch (_) {}
  }, 400);
}

/* ======================================================================
   History (per image)
   ====================================================================== */
const snapshot = () => JSON.stringify({ stage: S.stage, edit: S.edit, geom: S.geom });
function commit() {
  const now = snapshot();
  if (now === S.committed) return;
  if (S.committed !== null) {
    S.hist.push(S.committed);
    if (S.hist.length > 300) S.hist.shift();
    S.fut = [];
  }
  S.committed = now;
  updateUndoUI();
  markCurrentEdited();
}
function restore(js) {
  const o = JSON.parse(js);
  const stageChanged = JSON.stringify(o.stage) !== JSON.stringify(S.stage);
  const geomChanged = JSON.stringify(o.geom) !== JSON.stringify(S.geom);
  S.stage = o.stage; S.edit = o.edit; S.geom = o.geom || defaultGeom(); S.committed = js;
  syncPanel(); editChanged();
  if (geomChanged) { S.newFile = true; requestStage(true); }
  else if (stageChanged) { requestStage(); savePrefsSoon(); }
}
function undo() { commit(); if (!S.hist.length) return; S.fut.push(S.committed); restore(S.hist.pop()); updateUndoUI(); }
function redo() { if (!S.fut.length) return; S.hist.push(S.committed); restore(S.fut.pop()); updateUndoUI(); }
function updateUndoUI() { $('#undoBtn').disabled = !S.hist.length; $('#redoBtn').disabled = !S.fut.length; }
$('#undoBtn').addEventListener('click', undo);
$('#redoBtn').addEventListener('click', redo);

function copyEdits() {
  S.clipboard = clone({ stage: S.stage, edit: S.edit, geom: S.geom, src: S.img ? [S.img.src_w, S.img.src_h] : null });
  status('Edits copied');
}
function pasteEdits() {
  if (!S.clipboard || S.cur < 0) return;
  const stageChanged = JSON.stringify(S.clipboard.stage) !== JSON.stringify(S.stage);
  S.stage = clone(S.clipboard.stage); S.edit = clone(S.clipboard.edit);
  // crop/rotation only carries over between images with the same source size
  const sameSize = S.img && S.clipboard.src && S.clipboard.src[0] === S.img.src_w && S.clipboard.src[1] === S.img.src_h;
  const newGeom = sameSize ? clone(S.clipboard.geom) : S.geom;
  const geomChanged = JSON.stringify(newGeom) !== JSON.stringify(S.geom);
  S.geom = newGeom;
  if (geomChanged) for (const t of ['black', 'gray', 'white']) if (S.edit[t]) S.edit[t].pos = null;
  syncPanel(); editChanged(); commit();
  if (geomChanged) { S.newFile = true; requestStage(true); }
  else if (stageChanged) requestStage(true);
  status(sameSize || isDefaultGeom(S.clipboard.geom) ? 'Edits pasted' : 'Edits pasted (crop skipped: different image size)');
}
$('#copyBtn').addEventListener('click', copyEdits);
$('#pasteBtn').addEventListener('click', pasteEdits);
$('#resetBtn').addEventListener('click', () => { S.edit = defaultEdit(); syncPanel(); editChanged(); commit(); });

/* ======================================================================
   Files
   ====================================================================== */
const isEditedState = (edit, geom) => JSON.stringify(edit) !== DEFAULT_EDIT_JSON || !isDefaultGeom(geom);

function addPaths(paths) {
  const known = new Set(S.files.map(f => f.path.toLowerCase()));
  let first = -1;
  for (const p of paths || []) {
    if (known.has(p.toLowerCase())) continue;
    known.add(p.toLowerCase());
    S.files.push({ path: p, name: basename(p), state: null, done: false });
    if (first < 0) first = S.files.length - 1;
  }
  renderList();
  updateInfo();
  if (first >= 0 && S.cur < 0) switchTo(first);
  else if (first >= 0) status(`${paths.length} image(s) added`);
}

function renderList() {
  const ul = $('#fileList');
  ul.innerHTML = S.files.map((f, i) => {
    const edited = i === S.cur ? isEditedState(S.edit, S.geom) : (f.state && isEditedState(f.state.edit, f.state.geom));
    return `<li data-i="${i}" class="${i === S.cur ? 'cur' : ''} ${edited ? 'edited' : ''} ${f.done ? 'done' : ''}" title="${f.path.replace(/"/g, '&quot;')}">
      <img loading="lazy" src="/thumb?p=${encodeURIComponent(f.path)}" alt="">
      <span class="nm">${f.name.replace(/</g, '&lt;')}</span>
      <span class="badges"><span class="b-edit" title="edited"></span><span class="b-done" title="processed">✓</span></span></li>`;
  }).join('');
  $('#count').textContent = S.files.length;
  const cur = ul.querySelector('li.cur');
  if (cur) cur.scrollIntoView({ block: 'nearest' });
}
function markCurrentEdited() {
  const li = document.querySelector('#fileList li.cur');
  if (li) li.classList.toggle('edited', isEditedState(S.edit, S.geom));
  if ((typeof Crop !== "undefined")) Crop.updateSummary();
}
$('#fileList').addEventListener('click', e => {
  const li = e.target.closest('li'); if (!li) return;
  switchTo(Number(li.dataset.i));
});

function storeCurrent() {
  const f = S.files[S.cur]; if (!f) return;
  commit();
  f.state = { stage: clone(S.stage), edit: clone(S.edit), geom: clone(S.geom) };
  f.hist = S.hist; f.fut = S.fut; f.committed = S.committed;
}

function switchTo(i) {
  if (i < 0 || i >= S.files.length || (i === S.cur && S.img)) return;
  if (S.cropMode && (typeof Crop !== "undefined")) Crop.exit(true);
  storeCurrent();
  S.cur = i;
  const f = S.files[i];
  if (f.state) {
    S.stage = clone(f.state.stage); S.edit = clone(f.state.edit); S.geom = clone(f.state.geom || defaultGeom());
    S.hist = f.hist || []; S.fut = f.fut || []; S.committed = f.committed || snapshot();
  } else {
    S.edit = defaultEdit();               // output/color settings carry over, points/curves/crop start fresh
    S.geom = defaultGeom();
    S.hist = []; S.fut = []; S.committed = snapshot();
  }
  S.thr = { shadows: null, highlights: null, neutral: null };
  dropJpeg();
  S.newFile = true;
  syncPanel(); renderList(); updateInfo(); editChanged();
  requestStage(true);
}

function removeCurrent() {
  if (S.cur < 0) return;
  const i = S.cur;
  S.files.splice(i, 1);
  S.cur = -1;
  if (!S.files.length) { clearView(); renderList(); updateInfo(); return; }
  switchTo(Math.min(i, S.files.length - 1));
}
function clearView() {
  if (S.cropMode && (typeof Crop !== "undefined")) Crop.exit(true);
  S.img = null; S.reqId++; canvas.style.display = 'none'; dropJpeg(); updateCmp();
  S.edit = defaultEdit(); S.geom = defaultGeom(); S.hist = []; S.fut = []; S.committed = snapshot();
  syncPanel(); editChanged(); updateMarkers(); $('#zoomLabel').textContent = '–';
}

async function pickFiles() { const p = await api().pick_files(); if (p && p.length) addPaths(p); }
async function pickFolder() { const p = await api().pick_folder(); if (p && p.length) addPaths(p); else if (p) status('No images in that folder'); }
$('#addFiles').addEventListener('click', pickFiles);
$('#addFolder').addEventListener('click', pickFolder);
$('#removeBtn').addEventListener('click', removeCurrent);
$('#clearBtn').addEventListener('click', () => { S.files = []; S.cur = -1; clearView(); renderList(); updateInfo(); });

// visual feedback only - Python receives the real file paths from the native drop
let dragDepth = 0;
window.addEventListener('dragenter', e => { e.preventDefault(); dragDepth++; vp.classList.add('dragover'); });
window.addEventListener('dragover', e => e.preventDefault());
window.addEventListener('dragleave', () => { if (--dragDepth <= 0) { dragDepth = 0; vp.classList.remove('dragover'); } });
window.addEventListener('drop', e => { e.preventDefault(); dragDepth = 0; vp.classList.remove('dragover'); });

/* ======================================================================
   Process
   ====================================================================== */
function jobFor(f, isCur) {
  const st = isCur ? { stage: S.stage, edit: S.edit, geom: S.geom }
    : (f.state || { stage: S.stage, edit: defaultEdit(), geom: defaultGeom() });
  const E = computeEdit(st.edit);
  return {
    path: f.path, stage: stageParams(st.stage, st.geom),
    edit: { point: E.point, k: E.k, o: E.o, gains: E.gains, luts: E.luts.map(a => Array.from(a)) },
  };
}

function setBusy(on, label) {
  S.busy = on;
  $('#processBtn').disabled = on; $('#processAllBtn').disabled = on;
  $('#processBtn').firstChild.textContent = on ? (label || 'Processing…') : 'Process';
}

async function processCurrent() {
  if (S.busy || S.cur < 0 || !S.img) return;
  if (S.cropMode && (typeof Crop !== "undefined")) { Crop.exit(false); }
  commit();
  const idx = S.cur, f = S.files[idx];
  setBusy(true);
  let res;
  try { res = await api().process([jobFor(f, true)], S.prefs.fmt, encOpts()); }
  catch (e) { res = [{ ok: false, error: String(e) }]; }
  setBusy(false);
  const r = res && res[0];
  if (r && r.ok) {
    f.done = true; f.out = r.out;
    renderList();
    toast(`Saved <b>${basename(r.out)}</b>`, false, r.out);
    if (S.prefs.advance && idx === S.cur && idx < S.files.length - 1) switchTo(idx + 1);
  } else toast('Process failed: ' + (r ? r.error : 'unknown'), true);
}

async function processAll() {
  if (S.busy || !S.files.length) return;
  storeCurrent();
  const jobs = S.files.map((f, i) => jobFor(f, i === S.cur));
  setBusy(true, `Processing 0 / ${jobs.length}`);
  const bar = $('#progress');
  bar.classList.add('show'); bar.firstElementChild.style.width = '0%';
  let res;
  try { res = await api().process(jobs, S.prefs.fmt, encOpts()); }
  catch (e) { res = []; toast(String(e), true); }
  setBusy(false);
  setTimeout(() => bar.classList.remove('show'), 800);
  const ok = res.filter(r => r.ok).length, bad = res.length - ok;
  res.forEach(r => { const f = S.files.find(x => x.path === r.path); if (f && r.ok) { f.done = true; f.out = r.out; } });
  renderList();
  toast(`Processed ${ok} image(s)` + (bad ? `, <span style="color:var(--red)">${bad} failed</span>` : ''), bad > 0,
    ok ? res.find(r => r.ok).out : null);
}
window.onProcessProgress = (i, n) => {
  $('#progress').firstElementChild.style.width = (i / n * 100) + '%';
  $('#processBtn').firstChild.textContent = `Processing ${i} / ${n}`;
};
$('#processBtn').addEventListener('click', processCurrent);
$('#processAllBtn').addEventListener('click', processAll);

/* ======================================================================
   Toast / status
   ====================================================================== */
let toastTimer = null;
function toast(html, err, revealPath) {
  const t = $('#toast');
  t.innerHTML = html + (revealPath ? `<a id="toastReveal">Show in folder</a>` : '');
  t.classList.toggle('err', !!err);
  t.classList.add('show');
  if (revealPath) $('#toastReveal').onclick = () => api().reveal(revealPath);
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove('show'), err ? 6000 : 3500);
}
let statusTimer = null;
function status(msg) {
  $('#stMsg').textContent = msg;
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => { $('#stMsg').textContent = ''; }, 4000);
}

/* ======================================================================
   Keyboard
   ====================================================================== */
window.addEventListener('keydown', e => {
  if (S.cropMode && (typeof Crop !== "undefined") && Crop.onKey(e)) return;
  if (e.key === 'Alt') { e.preventDefault(); setAlt(true); return; }
  const inInput = /^(INPUT|SELECT|TEXTAREA)$/.test(e.target.tagName) && e.target.type !== 'range' && e.target.type !== 'checkbox';
  const ctrl = e.ctrlKey || e.metaKey, k = e.key.toLowerCase();
  if (ctrl && k === 's') { e.preventDefault(); processCurrent(); return; }
  if (ctrl && k === 'o') { e.preventDefault(); pickFiles(); return; }
  if (inInput) return;
  if (ctrl && e.shiftKey && k === 'c') { e.preventDefault(); copyEdits(); return; }
  if (ctrl && e.shiftKey && k === 'v') { e.preventDefault(); pasteEdits(); return; }
  if (ctrl && (k === 'y' || (k === 'z' && e.shiftKey))) { e.preventDefault(); redo(); return; }
  if (ctrl && k === 'z') { e.preventDefault(); undo(); return; }
  if (ctrl) return;
  if (e.altKey) e.preventDefault();
  switch (k) {
    case 'b': toggleTool('black'); break;
    case 'g': toggleTool('gray'); break;
    case 'w': toggleTool('white'); break;
    case 'escape': setTool(null); break;
    case 'c': if ((typeof Crop !== "undefined") && S.img) Crop.enter(); break;
    case '0': fitView(); break;
    case '1': zoom100(); break;
    case '+': case '=': zoomTo(S.view.z * 1.25); break;
    case '-': zoomTo(S.view.z / 1.25); break;
    case ' ': e.preventDefault(); if (!S.space) { S.space = true; vp.classList.add('pan-ready'); } break;
    case '\\': if (!S.before) { S.before = true; requestRender(); } break;
    case 'arrowdown': case 'pagedown': e.preventDefault(); switchTo(S.cur + 1); break;
    case 'arrowup': case 'pageup': e.preventDefault(); switchTo(S.cur - 1); break;
    case 'enter': processCurrent(); break;
  }
});
window.addEventListener('keyup', e => {
  if (e.key === 'Alt') { e.preventDefault(); setAlt(false); return; }
  if (e.key === ' ') { S.space = false; vp.classList.remove('pan-ready'); }
  if (e.key === '\\') { S.before = false; requestRender(); }
});
window.addEventListener('blur', () => { setAlt(false); S.space = false; S.before = false; vp.classList.remove('pan-ready'); requestRender(); });

/* ======================================================================
   Init
   ====================================================================== */
window.app = { addPaths };

async function init() {
  try {
    const s = await api().get_settings();
    if (s && typeof s === 'object') {
      if (s.fmt) S.prefs.fmt = s.fmt;
      if (s.jpeg_q != null) S.prefs.jpeg_q = clamp(Math.round(Number(s.jpeg_q)) || 90, 1, 100);
      if (s.jpeg_d != null && isFinite(Number(s.jpeg_d))) S.prefs.jpeg_d = clamp(Number(s.jpeg_d), 0, 25);
      if (s.jpeg_mode === 'q' || s.jpeg_mode === 'd') S.prefs.jpeg_mode = s.jpeg_mode;
      if (typeof s.webp_lossy === 'boolean') S.prefs.webp_lossy = s.webp_lossy;
      if (s.webp_q != null) S.prefs.webp_q = clamp(Math.round(Number(s.webp_q)) || 90, 0, 100);
      if (s.avif_crf != null && isFinite(Number(s.avif_crf))) S.prefs.avif_crf = clamp(Math.round(Number(s.avif_crf) * 4) / 4, 1, 70);
      if (s.avif_preset != null && isFinite(Number(s.avif_preset))) S.prefs.avif_preset = clamp(Math.round(Number(s.avif_preset)), 0, 8);
      if (!['WEBP', 'PNG', 'JPEG', 'AVIF'].includes(S.prefs.fmt)) S.prefs.fmt = 'WEBP';
      if (typeof s.advance === 'boolean') S.prefs.advance = s.advance;
      if (s.stage) S.stage = Object.assign(clone(DEFAULT_STAGE), s.stage);
    }
  } catch (_) {}
  try { const ji = await api().encoder_info(); S.cmp.jpegli = !!(ji && ji.jpegli); S.cmp.avif = !!(ji && ji.avif); S.cmp.svt = !!(ji && ji.svt); } catch (_) {}
  S.committed = snapshot();
  syncPanel(); editChanged(); updateInfo();
  try { const init = await api().initial_files(); if (init && init.length) addPaths(init); } catch (_) {}
}
S.E = computeEdit(S.edit);
syncPanel(); updateInfo();
if (window.pywebview && window.pywebview.api) init();
else window.addEventListener('pywebviewready', init);
