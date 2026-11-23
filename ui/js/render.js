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

/* Drafts: while a control is being dragged, or edits keep coming (wheel, typing), and the view is
   zoomed in so only part of the image is on screen, just that part is rendered (the core reuses the
   last full render's whole-image measurements, see run_region). A full render follows once the
   control is let go, or DRAFT_IDLE ms after the last change; the histogram waits for it. */
const DRAFT_IDLE = 200;
let draftTimer = 0, panelHold = false;
function setLive(on) { S.live = on; }
// the histogram's "waiting for the full render" look, while drafts are on screen
const histoLive = on => $('#histoBlock').classList.toggle('live', on);
function settle() {
  clearTimeout(draftTimer);
  if (!S.live || panelHold) return;          // still holding a control: the release settles it
  setLive(false);
  requestRender();
}
$('#panel').addEventListener('pointerdown', () => { panelHold = true; }, true);
for (const ev of ['pointerup', 'pointercancel', 'blur']) window.addEventListener(ev, () => {
  if (!panelHold) return;
  panelHold = false;
  settle();
}, true);
/* the visible part of the preview image (preview pixels, a little slack), or null when it is most of it */
function visibleRect() {
  const I = S.img, v = S.view, r = vp.getBoundingClientRect();
  if (!I || !r.width) return null;
  const x0 = clamp(Math.floor(-v.x / v.z) - 8, 0, I.pw), x1 = clamp(Math.ceil((r.width - v.x) / v.z) + 8, 0, I.pw);
  const y0 = clamp(Math.floor(-v.y / v.z) - 8, 0, I.ph), y1 = clamp(Math.ceil((r.height - v.y) / v.z) + 8, 0, I.ph);
  if (x1 <= x0 || y1 <= y0 || (x1 - x0) * (y1 - y0) > I.pw * I.ph * 0.5) return null;
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

function editChanged() {
  S.E = computeEdit(effectiveEdit(S.edit));
  setLive(true);
  clearTimeout(draftTimer);
  draftTimer = setTimeout(settle, DRAFT_IDLE);
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
  const n = I.pw * I.ph, E = S.E;
  // same image and edit as the pixels in S.outData (only the view changed: before, Alt, compare line)
  if (S.rendered && S.rendered.I === I && S.rendered.E === E) { present(n); return; }
  const rect = S.live && !viewMode() && !S.cropMode ? visibleRect() : null;
  if (!WP.ready()) {                // the core is starting (it asks for a render when ready) or missing
    if (WP.failed()) {              // missing: show the unedited image (WP has said why)
      S.outData.data.set(I.base);
      S.hin.fill(0); S.hout.fill(0); S.hmx.fill(0); S.hmn.fill(0);
      present(n);
    }
    return;
  }
  WP.render(I, E, r => {            // the Rust core in its worker (wasm-preview.js); shown when it's done
    if (r.rect) { if (S.img === I && S.outData) presentDraft(r.rect, new Uint8Array(r.out)); return; }
    if (S.img !== I || !S.outData || r.out.byteLength !== S.outData.data.length) return;   // image changed meanwhile
    S.outData.data.set(new Uint8Array(r.out));
    S.hin.set(r.hin); S.hout.set(r.hout); S.hmx.set(r.hmx); S.hmn.set(r.hmn);
    S.rendered = { I, E };
    histoLive(false);
    present(n);
  }, rect);
}

/* a draft: its rectangle into the image, and only that rectangle onto the canvas (the split views
   are composed there too); histograms, HUD and the rest wait for the full render */
function presentDraft(R, px) {
  const I = S.img, o = S.outData.data, W = I.pw * 4, rw = R.w * 4;
  for (let y = 0; y < R.h; y++) o.set(px.subarray(y * rw, (y + 1) * rw), (R.y + y) * W + R.x * 4);
  S.rendered = null;                       // the full render still has to come
  let disp = S.outData;
  const enc = encSplitOn() && cmpReady();
  if (enc || (!encSplitOn() && S.prefs.split)) {
    // the split: original (or encoded) on one side of the line, edited on the other, in the rectangle only
    const d = S.dispData.data, other = enc ? S.cmp.data.data : S.baseData.data;
    const split = clamp(Math.round(S.cmp.x * I.pw), 0, I.pw);
    for (let y = R.y; y < R.y + R.h; y++) {
      const row = y * W;
      for (let x = R.x; x < R.x + R.w; ) {
        const left = x < split, end = left ? Math.min(split, R.x + R.w) : R.x + R.w;
        const src = (left !== enc) ? other : o;       // original | edited, or edited | encoded
        d.set(src.subarray(row + x * 4, row + end * 4), row + x * 4);
        x = end;
      }
    }
    disp = S.dispData;
  }
  ctx.putImageData(disp, 0, 0, R.x, R.y, R.w, R.h);
  S.shown = disp;
  histoLive(true);
  drawLoupe(); updateCmp();
}

// everything after the pixels: view modes, canvas, histogram, curve, HUD, loupe, compare
function present(n) {
  S.n = n;
  const mode = viewMode();
  let disp = S.outData;
  if (mode === 'before') disp = S.baseData;
  else if (mode) { buildModeView(mode); disp = S.dispData; }
  else if (encSplitOn()) { if (cmpReady()) { buildCompare(); disp = S.dispData; } }   // lossless | encoded
  else if (S.prefs.split) { buildSplitOriginal(); disp = S.dispData; }               // original | edited
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
