'use strict';
/* View (zoom / pan / markers) and eyedroppers. */

/* ======================================================================
   View: zoom / pan / markers
   ====================================================================== */
function applyView() {
  const v = S.view, I = S.img;
  // translate snapped to whole device pixels, so 1:1 maps each image pixel onto exactly one screen pixel
  // (snapped in absolute screen coordinates: the viewport itself may start at a fractional device pixel)
  const d = dpr(), r = vp.getBoundingClientRect();
  const sx = Math.round((r.left + v.x) * d) / d - r.left, sy = Math.round((r.top + v.y) * d) / d - r.top;
  stageEl.style.transform = `translate(${sx}px, ${sy}px) scale(${v.z})`;
  canvas.classList.toggle('pixel', v.z * d >= 2);
  checkHires();
  if (typeof Sharp !== 'undefined') Sharp.dirty();
  $('#zoomLabel').textContent = I ? Math.round(v.z * dpr() * (I.pw / I.w) * 100) + '%' : '–';
  updateMarkers();
  updateCmp();
}

/* The live preview is at most PREVIEW_MAX px on the long side. Zoomed in past that (output pixels
   bigger than preview pixels on screen) it is swapped for the full-resolution stage, and back when zoomed
   out below HIRES_OUT. The small copy is never shown scaled up when idle: that would make the sharpness
   jump at the swap. (Dragging in that zone uses the small copy anyway, see render.js.) */
const HIRES_IN = 1.01, HIRES_OUT = 0.8;
function checkHires() {
  const I = S.img, v = S.view;
  if (!I || S.cropMode || !I.lo_pw || I.lo_pw >= I.w) return;
  const shown = v.z * dpr() * I.pw / I.w;        // screen pixels per output pixel
  const lo = I.lo_pw / I.w;                       // output scale the small preview covers
  const want = S.hires ? shown > lo * HIRES_OUT : shown > lo * HIRES_IN;
  if (want === S.hires) return;
  S.hires = want;
  requestStage();
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
$('#zoomLabel').addEventListener('click', zoom100);      // the zoom readout doubles as the 100% button

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
  lctx.strokeStyle = '#00ffaa'; lctx.lineWidth = 1.5;
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
  if (S.edit.off && S.edit.off.point) { S.edit.off = Object.assign({}, S.edit.off); delete S.edit.off.point; }   // picking turns the island on
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
    sw.closest('.tool-cell')?.classList.toggle('set', !!s);
    sw.style.background = s ? `rgb(${s.rgb.join(',')})` : 'transparent';
    sw.title = s ? `RGB ${s.rgb.join(', ')}` : 'not set';
  }
}
