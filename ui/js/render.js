'use strict';
/* Rendering and the Alt clipping / threshold views. */

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
  processPixels(I.base, S.outData.data, n, S.E, true, I.mdB, { w: I.pw, h: I.ph });
  S.n = n;
  const mode = viewMode();
  let disp = S.outData;
  if (mode === 'before') disp = S.baseData;
  else if (mode) { buildModeView(mode); disp = S.dispData; }
  else if (S.prefs.split && !isLossy()) { buildSplitOriginal(); disp = S.dispData; }
  else if (S.prefs.split && cmpReady()) { buildCompare(); disp = S.dispData; }
  ctx.putImageData(disp, 0, 0);
  S.shown = disp;
  drawHisto(); drawCurve(); updateHud(mode); drawLoupe(); updateCmp();
}

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
