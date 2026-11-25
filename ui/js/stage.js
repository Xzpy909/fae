'use strict';
/* Stage loading from Python. */

/* ======================================================================
   Stage loading (Python)
   ====================================================================== */
let stageTimer = null;
function requestStage(immediate) {
  clearTimeout(stageTimer);
  stageTimer = setTimeout(loadStage, immediate ? 0 : 140);
}

/* The stage at the preview's resolution: full resolution when the GPU draws the preview (up to what
   its device takes, GV.maxPixels()), else PREVIEW_MAX on the long side for the CPU core. When that
   changes (the GPU became ready, or failed) gpu-view.js asks for the stage again. */
async function loadStage() {
  const f = S.files[S.cur];
  if (!f) return;
  const id = ++S.reqId, t0 = S.loadT || performance.now();
  spinner(true);
  await GV.started;                    // the resolution depends on whether the GPU draws (quick)
  const full = GV.maxPixels(), params = stageParams(S.stage, S.geom);
  let r;
  try { r = await api().load_stage(f.path, params, id, full); }
  catch (e) { r = { ok: false, req_id: id, error: String(e) }; }
  if (id !== S.reqId) return;
  if (!r || !r.ok) { spinner(false); toast(r ? r.error : 'Load failed', true); return; }
  let buf;
  try { buf = await (await fetch(Bridge.previewUrl(r.token))).arrayBuffer(); }
  catch (e) { spinner(false); toast('Preview transfer failed: ' + e, true); return; }
  if (id !== S.reqId) return;
  spinner(false);
  const old = S.img;
  const sizeChanged = !old || old.pw !== r.pw || old.ph !== r.ph;
  // same output at another preview resolution (the GPU came or went): keep the view where it is
  const resSwap = sizeChanged && old && !S.newFile && old.path === f.path && old.w === r.w && old.h === r.h;
  S.img = Object.assign({}, r, { base: new Uint8ClampedArray(buf), path: f.path });
  f.src = [r.src_w, r.src_h];                // its source size (Paste edits to…: crop carries over only at the same size)
  if (sizeChanged) {
    canvas.width = r.pw; canvas.height = r.ph;
    S.outData = new ImageData(r.pw, r.ph);
    S.dispData = new ImageData(r.pw, r.ph);
    S.shown = null; S.rendered = null;
  }
  S.baseData = new ImageData(S.img.base, r.pw, r.ph);
  canvas.style.display = 'block';
  canvas.classList.toggle('alpha', !!r.alpha);         // no frame around transparent images (style.css)
  if (resSwap) { S.view.z *= old.pw / r.pw; applyView(); }
  else if (sizeChanged || S.newFile) fitView();
  const wasNew = S.newFile;
  S.newFile = false;
  updateInfo();
  render();
  if (wasNew) status(`Loaded in ${Math.round(performance.now() - t0)} ms` + (S.loadFromDrop ? ' (from drop)' : ''));
  S.loadT = null; S.loadFromDrop = false;
  requestJpeg(true);
}

function spinner(on) { $('#spinner').classList.toggle('show', on); }

function updateInfo() {
  const f = S.files[S.cur];
  $('#fileName').textContent = f ? f.name : 'No image';
  const I = S.img;
  $('#fileDims').textContent = I && f ? `${I.src_w}×${I.src_h}  →  ${I.w}×${I.h}` : '';
  $('#filePos').textContent = f && S.files.length > 1 ? `${S.cur + 1} / ${S.files.length}` : '';
  $('#outInfo').textContent = I && f
    ? `Output ${I.w}×${I.h} · ${(I.w * I.h / 1e6).toFixed(2)} MP${I.alpha ? (isJpeg() ? ' · alpha → white' : ' · alpha') : ''}`
      + (isJpeg() && !S.cmp.jpegli ? ' · cjpegli.exe missing, using Pillow' : '')
      + (S.prefs.fmt === 'AVIF' && !S.cmp.avif ? ' · preview needs Pillow ≥ 11.2 (pip install -U pillow)' : '')
      + (S.prefs.fmt === 'AVIF' && !S.cmp.svt ? ' · SvtAv1EncApp.exe missing, using rav1e' : '')
      + (S.prefs.fmt === 'AVIF' && S.prefs.avif_enc === 'svt' && S.cmp.svt && I.alpha ? ' · alpha → rav1e' : '')
    : '';
  $('#outInfo').title = $('#outInfo').textContent;     // the block shows one line; the full text on hover
  $('#empty').style.display = S.files.length ? 'none' : 'grid';
  syncResize(); syncEncRow();
}
