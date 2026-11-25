'use strict';
/* Lossy split compare (JPEG / AVIF / lossy WEBP round trip) and format switching. */

/* ======================================================================
   Lossy split compare: left = lossless edit (live), right = real JPEG / AVIF / lossy WEBP round trip
   ====================================================================== */
const isJpeg = () => S.prefs.fmt === 'JPEG';
const isLossy = () => S.prefs.fmt === 'JPEG' || S.prefs.fmt === 'AVIF' || (S.prefs.fmt === 'WEBP' && !!S.prefs.webp_lossy);
function cmpReady() {
  const I = S.img, c = S.cmp;
  return isLossy() && I && c.data && c.fmt === S.prefs.fmt && c.path === I.path && c.data.width === I.pw && c.data.height === I.ph;
}
/* split view for lossless formats: left = original (before edits), right = edited */
function buildSplitOriginal() {
  const I = S.img, o = S.outData.data, b = S.baseData.data, d = S.dispData.data;
  const W = I.pw * 4, split = clamp(Math.round(S.cmp.x * I.pw), 0, I.pw) * 4;
  d.set(o);
  if (split <= 0) return;
  for (let y = 0, row = 0; y < I.ph; y++, row += W) d.set(b.subarray(row, row + split), row);
}
function buildCompare() {
  const I = S.img, o = S.outData.data, j = S.cmp.data.data, d = S.dispData.data;
  const W = I.pw * 4, split = clamp(Math.round(S.cmp.x * I.pw), 0, I.pw) * 4;
  d.set(o);
  if (split >= W) return;
  for (let y = 0, row = 0; y < I.ph; y++, row += W) d.set(j.subarray(row + split, row + W), row + split);
}

const fmtTime = ms => ms < 1000 ? Math.max(1, Math.round(ms)) + ' ms' : (ms / 1000).toFixed(1) + ' s';
const fmtBytes = b => b >= 1048576 ? (b / 1048576).toFixed(2) + ' MB' : Math.max(1, Math.round(b / 1024)) + ' KB';
function updateSizeChip() {
  const el = $('#sizeChip'), I = S.img, c = S.cmp;
  const on = encSplitOn() && I && !S.cropMode && (cmpReady() || c.busy);   // only with the encoded compare on
  el.classList.toggle('show', !!on);
  if (!on) return;
  const ready = cmpReady(), stale = c.busy || !ready || c.key !== jpegKey();
  el.innerHTML = ready && !stale     // a change since the shown result: say so until the new encode is in
    ? `<b>${fmtBytes(c.bytes)}</b> <span class="muted">&middot; ${I.w}×${I.h}<br>${(c.bytes * 8 / (I.w * I.h)).toFixed(2)} bpp${c.ms != null ? ` &middot; ${fmtTime(c.ms)}` : ''}</span>`
    : '<span class="muted">encoding…</span>';
  el.classList.toggle('stale', stale);
  // pinned to the image's bottom-right corner, kept inside the viewport
  const v = S.view, r = vp.getBoundingClientRect(), m = 10;
  const x = clamp(v.x + I.pw * v.z - m, 160, r.width - m);
  const y = clamp(v.y + I.ph * v.z - m, 40, r.height - m);
  el.style.left = x + 'px'; el.style.top = y + 'px';
}

/* Two split views: the top bar's (original | edited, any format) and the export block's
   (lossless | encoded, lossy formats only). At most one is on. */
const encSplitOn = () => !!S.prefs.encSplit && isLossy();
function updateCmp() {
  updateSizeChip();
  const el = $('#cmp'), I = S.img, enc = encSplitOn();
  const on = (S.prefs.split || enc) && I && !S.cropMode && !viewMode();
  el.classList.toggle('show', !!on);
  $('#btnSplit').classList.toggle('on', !!S.prefs.split && !enc);
  const eb = $('#btnEncSplit');
  eb.disabled = !isLossy();
  eb.classList.toggle('on', enc);
  if (!on) return;
  const v = S.view, r = vp.getBoundingClientRect();
  const left = v.x, top = v.y, w = I.pw * v.z, h = I.ph * v.z;
  const x = left + S.cmp.x * w;
  const y0 = clamp(top, 0, r.height), y1 = clamp(top + h, 0, r.height);
  const line = $('#cmpLine');
  line.style.left = x + 'px'; line.style.top = y0 + 'px'; line.style.height = Math.max(0, y1 - y0) + 'px';
  line.style.display = x < -2 || x > r.width + 2 || y1 <= y0 ? 'none' : 'block';
  const c = S.cmp, ready = cmpReady();
  const stale = enc && (c.busy || !ready || c.key !== jpegKey());
  const lbl = $('#cmpR');
  $('#cmpL').textContent = enc ? 'Lossless' : 'Original';
  lbl.innerHTML = !enc ? 'Edited' : ready
    ? `${encName(c.fmt, c.enc)} ${optsLabel(c.fmt, c.opts, c.enc)}${stale ? ' <span class="muted">&middot; updating…</span>' : ''}`
    : `${encName(S.prefs.fmt, null)} <span class="muted">&middot; encoding…</span>`;
  lbl.classList.toggle('stale', stale);
  const ly = clamp(top + 10, 52, r.height - 34);     // stay below the tool pills
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
  avif: { enc: avifEnc(), crf: S.prefs.avif_crf, preset: S.prefs.avif_preset, aom_q: S.prefs.aom_q, aom_speed: S.prefs.aom_speed },
});
function encName(fmt, enc) {
  if (fmt === 'JPEG') return enc === 'pillow' ? 'JPEG (Pillow)' : 'jpegli';
  if (fmt === 'AVIF') return enc === 'svt' ? 'AVIF SVT 4:4:4' : 'AVIF rav1e';
  return 'WEBP lossy';
}
function optsLabel(fmt, o, enc) {
  if (!o) return '';
  if (fmt === 'JPEG') return o.mode === 'd' && enc !== 'pillow' ? `d<b>${Number(o.d).toFixed(2)}</b>` : `q<b>${o.q}</b>`;
  if (fmt === 'AVIF') return enc === 'svt' ? `crf <b>${o.crf}</b> &middot; p${o.preset}` : `q<b>${o.aom_q}</b> &middot; speed ${o.aom_speed}`;
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
const stageReady = () => S.cur >= 0 && S.img && S.files[S.cur] && S.img.path === S.files[S.cur].path;
/* The real encode runs only while the export block's encoded compare is on (it also gives the size
   chip); otherwise editing would re-encode after every change. */
function requestJpeg(immediate) {
  if (!encSplitOn() || !stageReady() || S.cropMode) return;   // never encode before the image itself is shown
  clearTimeout(jpegTimer);
  jpegTimer = setTimeout(loadJpeg, immediate ? 0 : 260);
}
async function loadJpeg() {
  const f = S.files[S.cur];
  if (!encSplitOn() || !f || !stageReady()) return;
  const key = jpegKey();
  if (key === S.cmp.key && cmpReady()) return;
  const id = ++S.cmp.req;
  S.cmp.busy = true; updateCmp();
  let r;
  const fmt = S.prefs.fmt;
  try { r = await api().lossy_preview(jobFor(f, true), fmt, encOpts(), id, S.img.full || 0); }
  catch (e) { r = { ok: false, error: String(e) }; }
  if (id !== S.cmp.req || (r && r.cancelled)) return;
  if (!r || !r.ok) { S.cmp.busy = false; updateCmp(); toast('JPEG preview failed: ' + (r ? r.error : 'unknown'), true); return; }
  let buf;
  try { buf = await (await fetch(Bridge.previewUrl(r.token))).arrayBuffer(); }
  catch (e) { S.cmp.busy = false; updateCmp(); return; }
  if (id !== S.cmp.req) return;
  S.cmp.busy = false;
  if (S.files[S.cur] !== f) return;
  Object.assign(S.cmp, { data: new ImageData(new Uint8ClampedArray(buf), r.pw, r.ph), path: f.path,
    key, bytes: r.bytes, ms: r.ms, enc: r.encoder, fmt: r.fmt, opts: r.opts });
  requestRender();
  if (jpegKey() !== key) requestJpeg();
}
/* stop a pending or running encode (the encoder process is killed) and keep the last result */
function cancelJpeg() {
  clearTimeout(jpegTimer);
  if (!S.cmp.busy) return;
  const id = ++S.cmp.req;
  S.cmp.busy = false;
  try { api().lossy_cancel(id); } catch (_) {}
}
function dropJpeg() { cancelJpeg(); S.cmp.req++; S.cmp.data = null; S.cmp.key = null; }

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

/* Point correction + curves. Must match the CPU core (core/fastedit-core). */
