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

/* S.live: a control is being dragged, or edits keep coming (wheel, typing). The GPU then hands back
   only the histograms each frame and the pixels once it settles (on release, or SETTLE_IDLE ms after
   the last change); the CPU core renders the whole preview every time. */
const SETTLE_IDLE = 200;
let settleTimer = 0, panelHold = false;
function setLive(on) { S.live = on; }
function settle() {
  clearTimeout(settleTimer);
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

function editChanged() {
  S.E = computeEdit(effectiveEdit(S.edit));
  setLive(true);
  clearTimeout(settleTimer);
  settleTimer = setTimeout(settle, SETTLE_IDLE);
  updateToolUI();
  updateMarkers();
  requestRender();
  markCurrentEdited();
  if (typeof Presets !== 'undefined') Presets.sync();
  if (S.cropMode && (typeof Crop !== "undefined")) Crop.editChanged();
  requestJpeg();
}

function render() {
  const I = S.img;
  if (!I || !S.E) { GV.hide(); GV.badge(I, S.E); drawCurve(); drawHisto(); return; }
  const n = I.pw * I.ph, E = S.E, mode = viewMode();
  // views built from the pixels on the 2D canvas; the GPU canvas shows the rest itself
  const flat = !!(mode && mode !== 'before') || encSplitOn() || S.cropMode;
  S.gpuFrame = GV.wants(I, E) && GV.show(I, E, !flat);
  GV.badge(I, E);
  // same image and edit as the pixels in S.outData (only the view changed: before, Alt, compare line)
  if (S.rendered && S.rendered.I === I && S.rendered.E === E) { present(n); return; }
  if (S.gpuFrame) {                  // histograms from the GPU every frame, its pixels when needed
    GV.results(I, E, !S.live || flat, r => {
      if (S.img !== I) return;
      S.hin.set(r.hist.subarray(0, 768)); S.hout.set(r.hist.subarray(768, 1536));
      S.hmx.set(r.hist.subarray(1536, 1792)); S.hmn.set(r.hist.subarray(1792, 2048));
      if (r.px && r.E === S.E && S.outData && r.px.length === S.outData.data.length) {
        S.outData.data.set(r.px);
        S.rendered = { I, E: r.E };
        present(n);
      } else if (S.gpuFrame) {
        drawHisto(); drawCurve();
      }
    });
    if (GV.on()) { drawCurve(); drawLoupe(); updateCmp(); }
    return;
  }
  if (!WP.ready()) {                // the core is starting (it asks for a render when ready) or missing
    if (WP.failed()) {              // missing: show the unedited image (WP has said why)
      S.outData.data.set(I.base);
      S.hin.fill(0); S.hout.fill(0); S.hmx.fill(0); S.hmn.fill(0);
      present(n);
    }
    return;
  }
  WP.render(I, E, r => {            // the CPU core in its worker (wasm-preview.js); shown when it's done
    if (S.img !== I || !S.outData || r.out.byteLength !== S.outData.data.length) return;   // image changed meanwhile
    S.outData.data.set(new Uint8Array(r.out));
    S.hin.set(r.hin); S.hout.set(r.hout); S.hmx.set(r.hmx); S.hmn.set(r.hmn);
    S.rendered = { I, E };
    present(n);
  });
}

// everything after the pixels: view modes, canvas, histogram, curve, HUD, loupe, compare
function present(n) {
  S.n = n;
  const mode = viewMode();
  if (GV.on()) {                    // the GPU canvas shows the image; S.outData feeds the readouts
    S.shown = mode === 'before' ? S.baseData : S.outData;
    drawHisto(); drawCurve(); updateHud(mode); drawLoupe(); updateCmp();
    return;
  }
  GV.hide();
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
