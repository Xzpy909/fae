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

async function loadStage() {
  const f = S.files[S.cur];
  if (!f) return;
  const id = ++S.reqId, t0 = S.loadT || performance.now();
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
  let mdB = null;                   // Mid/Detail smoothed luma (float32, preview size)
  try { if (r.md_token) mdB = new Float32Array(await (await fetch('/preview/' + r.md_token)).arrayBuffer()); } catch (_) {}
  if (id !== S.reqId) return;
  S.img = Object.assign({}, r, { base, path: f.path, mdB });
  if (sizeChanged) {
    canvas.width = r.pw; canvas.height = r.ph;
    S.outData = new ImageData(r.pw, r.ph);
    S.dispData = new ImageData(r.pw, r.ph);
  }
  S.baseData = new ImageData(base, r.pw, r.ph);
  canvas.style.display = 'block';
  if (sizeChanged || S.newFile) fitView();
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
  $('#outInfo').textContent = I && f
    ? `Output ${I.w}×${I.h} · ${(I.w * I.h / 1e6).toFixed(2)} MP${I.alpha ? (isJpeg() ? ' · alpha → white' : ' · alpha') : ''}`
      + (isJpeg() && !S.cmp.jpegli ? ' · cjpegli.exe missing, using Pillow' : '')
      + (S.prefs.fmt === 'AVIF' && !S.cmp.avif ? ' · preview needs Pillow ≥ 11.2 (pip install -U pillow)' : '')
      + (S.prefs.fmt === 'AVIF' && !S.cmp.svt ? ' · SvtAv1EncApp.exe missing, using aom' : '')
      + (S.prefs.fmt === 'AVIF' && S.cmp.svt && I.alpha ? ' · alpha → aom' : '')
    : '';
  $('#empty').style.display = S.files.length ? 'none' : 'grid';
  syncResize(); syncEncRow();
}
