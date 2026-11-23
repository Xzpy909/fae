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
  if (S.newFile) S.hires = false;
  const full = S.hires, params = stageParams(S.stage, S.geom);
  const key = f.path + '|' + JSON.stringify(params);       // the stage these pixels are of
  // zoomed in: the small copy of the same stage comes along (live frames while a control is dragged)
  const haveLo = S.loImg && S.loImg.key === key, needLo = full && !haveLo;
  let r;
  // zoomed back out: the small copy is already here (made with the full-resolution stage)
  if (!full && haveLo && !S.newFile && S.img && S.img.key === key && S.loImg.token) r = S.loImg;
  else {
    try { r = await api().load_stage(f.path, params, id, full, needLo); }
    catch (e) { r = { ok: false, req_id: id, error: String(e) }; }
  }
  if (id !== S.reqId) return;
  if (!r || !r.ok) { spinner(false); toast(r ? r.error : 'Load failed', true); return; }
  let buf, loBuf = null;
  try {
    buf = r === S.loImg ? r.base.buffer : await (await fetch('/preview/' + r.token)).arrayBuffer();
    if (r.lo_token) loBuf = await (await fetch('/preview/' + r.lo_token)).arrayBuffer();
  }
  catch (e) { spinner(false); toast('Preview transfer failed: ' + e, true); return; }
  if (id !== S.reqId) return;
  spinner(false);
  const old = S.img;
  const sizeChanged = !old || old.pw !== r.pw || old.ph !== r.ph;
  // same output, only the preview resolution changed (zoom crossed PREVIEW_MAX): keep the view where it is
  const resSwap = sizeChanged && old && !S.newFile && old.path === f.path && old.w === r.w && old.h === r.h;
  if (r === S.loImg) S.img = S.loImg;
  else {
    const base = new Uint8ClampedArray(buf);
    S.img = Object.assign({}, r, { base, path: f.path, key });
  }
  if (!r.full) S.loImg = S.img;
  // the small copy as a stage of its own (the same fields a small load_stage gives)
  else if (loBuf) S.loImg = Object.assign({}, r, { token: r.lo_token, lo_token: null, pw: r.lo_w, ph: r.lo_h, full: false,
                                                   base: new Uint8ClampedArray(loBuf), path: f.path, key });
  else if (S.loImg && S.loImg.key !== key) S.loImg = null;
  if (sizeChanged) {
    // a resolution swap keeps showing the last frame, scaled, until the new resolution's first render
    // is ready (resizing the canvas clears it)
    let snap = null;
    if (resSwap && canvas.width && canvas.height) {
      snap = document.createElement('canvas');
      snap.width = canvas.width; snap.height = canvas.height;
      snap.getContext('2d').drawImage(canvas, 0, 0);
    }
    canvas.width = r.pw; canvas.height = r.ph;
    S.outData = new ImageData(r.pw, r.ph);
    S.dispData = new ImageData(r.pw, r.ph);
    S.shown = null; S.rendered = null;
    if (snap) {
      ctx.imageSmoothingEnabled = true; ctx.imageSmoothingQuality = 'high';
      ctx.drawImage(snap, 0, 0, r.pw, r.ph);
    }
  }
  S.baseData = new ImageData(S.img.base, r.pw, r.ph);
  canvas.style.display = 'block';
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
  // the side panels' frosted islands sit on a heavily blurred copy of the current image
  $('#app').style.setProperty('--amb', f ? `url("/thumb?p=${encodeURIComponent(f.path)}")` : 'none');
  $('#fileDims').textContent = I && f ? `${I.src_w}×${I.src_h}  →  ${I.w}×${I.h}` : '';
  $('#outInfo').textContent = I && f
    ? `Output ${I.w}×${I.h} · ${(I.w * I.h / 1e6).toFixed(2)} MP${I.alpha ? (isJpeg() ? ' · alpha → white' : ' · alpha') : ''}`
      + (isJpeg() && !S.cmp.jpegli ? ' · cjpegli.exe missing, using Pillow' : '')
      + (S.prefs.fmt === 'AVIF' && !S.cmp.avif ? ' · preview needs Pillow ≥ 11.2 (pip install -U pillow)' : '')
      + (S.prefs.fmt === 'AVIF' && !S.cmp.svt ? ' · SvtAv1EncApp.exe missing, using aom' : '')
      + (S.prefs.fmt === 'AVIF' && S.prefs.avif_enc === 'svt' && S.cmp.svt && I.alpha ? ' · alpha → aom' : '')
    : '';
  $('#outInfo').title = $('#outInfo').textContent;     // the block shows one line; the full text on hover
  $('#empty').style.display = S.files.length ? 'none' : 'grid';
  syncResize(); syncEncRow();
}
