'use strict';
/* Panel controls: fields, jog wheels, drag-to-change numbers. */

/* ======================================================================
   Panel controls
   ====================================================================== */
const FIELDS = [];
function makeField(parent, spec) {
  const { obj, key, label, min, max, step, dec } = spec;
  const smin = spec.smin ?? min, smax = spec.smax ?? max;
  const row = document.createElement('div');
  row.className = spec.wheel ? 'field wheel' : spec.grad ? 'field grad' : 'field';
  const defTxt = spec.def == null ? '' : Number(spec.def).toFixed(dec);
  const ctl = spec.wheel ? `<canvas class="jog" title="Drag left/right (Shift fine, Ctrl coarse), double-click to reset"></canvas>`
                         : `<input type="range" min="${smin}" max="${smax}" step="${step}">`;
  const numHtml = `<input type="number" min="${min}" max="${max}" step="${step}">`;
  row.innerHTML = `<label>${label}</label>${spec.wheel ? '' : ctl}`
    + (spec.unit ? `<span class="numw" data-unit="${spec.unit}">${numHtml}</span>` : numHtml)
    + `<button class="rst" title="Reset to ${defTxt}"><svg viewBox="0 0 24 24"><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/></svg></button>`
    + (spec.wheel ? ctl : '');
  const rng = row.querySelector('input[type=range], canvas'), num = row.querySelector('input[type=number]'), rst = row.querySelector('.rst');
  // value change per pixel of mouse travel (wheel 1:1 with the ridges; number scrub a bit faster)
  const perPx = spec.perPx ?? (max - min) / 800;
  if (spec.grad) rng.style.setProperty('--track', spec.grad);
  const jog = spec.wheel ? makeJog(rng, () => (S[obj][key] - (spec.def ?? 0)) / perPx) : null;
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
    num.value = Number(v).toFixed(dec);
    if (jog) jog.draw();
    else { rng.value = v; rng.style.setProperty('--p', ((clamp(v, smin, smax) - smin) / (smax - smin) * 100) + '%'); }
    rst.classList.toggle('at-def', spec.def == null || Number(v) === Number(spec.def));
    if (spec.enabled) row.classList.toggle('disabled', !spec.enabled());
  };
  if (jog) {
    dragScrub(rng, perPx, { get: () => S[obj][key], set, onDrag: on => jog.active(on), lo: min, hi: max });
    rng.addEventListener('wheel', e => {
      e.preventDefault();
      set(S[obj][key] + (e.deltaY < 0 ? 1 : -1) * step * (e.shiftKey ? 0.1 : e.ctrlKey ? 10 : 1), true);
    }, { passive: false });
  } else {
    rng.addEventListener('input', () => set(rng.value, false));
    rng.addEventListener('change', () => commit());
  }
  rng.addEventListener('dblclick', () => { if (spec.def != null) set(spec.def, true); });
  dragScrub(num, perPx * 2, { get: () => S[obj][key], set, field: true, lo: min, hi: max });
  rst.addEventListener('click', () => { if (spec.def != null) set(spec.def, true); });
  num.addEventListener('change', () => set(num.value, true));
  num.addEventListener('keydown', e => { if (e.key === 'Enter') num.blur(); e.stopPropagation(); });
  num.addEventListener('wheel', e => {
    e.preventDefault();
    set(S[obj][key] + (e.deltaY < 0 ? 1 : -1) * step * (e.shiftKey ? 10 : 1), true);
  }, { passive: false });
  parent.appendChild(row);
  FIELDS.push({ sync });
}

/* Horizontal drag that changes a value: Shift = fine (x0.1), Ctrl = coarse (x10).
   On a number box a plain click (no drag) still focuses it for typing. */
function dragScrub(el, perPx, { get, set, onDrag, field, lo = -Infinity, hi = Infinity }) {
  let x0 = null, acc = 0, moved = false;
  el.addEventListener('pointerdown', e => {
    if (e.button !== 0 || (field && document.activeElement === el)) return;
    e.preventDefault();
    x0 = e.clientX; acc = Number(get()); moved = false;
    el.setPointerCapture(e.pointerId);
  });
  el.addEventListener('pointermove', e => {
    if (x0 === null) return;
    const dx = e.clientX - x0;
    if (!moved && Math.abs(dx) < 3) return;
    if (!moved) { moved = true; onDrag?.(true); document.body.classList.add('scrubbing'); }
    x0 = e.clientX;
    acc += dx * perPx * (e.shiftKey ? 0.1 : e.ctrlKey ? 10 : 1);
    acc = clamp(acc, lo, hi);           // no dead zone when coming back from an end stop
    set(acc, false);
  });
  const end = e => {
    if (x0 === null) return;
    x0 = null;
    if (moved) { onDrag?.(false); document.body.classList.remove('scrubbing'); commit(); }
    else if (field) { el.focus(); el.select(); }
  };
  el.addEventListener('pointerup', end);
  el.addEventListener('pointercancel', end);
}

/* Jog wheel: a ridged cylinder seen from the front. pos() = surface travel in px (value / perPx),
   so the ridges under the cursor follow the mouse 1:1. */
function makeJog(cv, pos) {
  const ctx = cv.getContext('2d');
  let on = false;
  const draw = () => {
    const dpr = window.devicePixelRatio || 1, W = cv.clientWidth, H = cv.clientHeight;
    if (!W || !H) return;
    if (cv.width !== Math.round(W * dpr) || cv.height !== Math.round(H * dpr)) {
      cv.width = Math.round(W * dpr); cv.height = Math.round(H * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    const R = W / 2 - 3, cx = W / 2, rr = H / 2;
    ctx.save();
    ctx.beginPath(); ctx.roundRect(0.5, 0.5, W - 1, H - 1, rr); ctx.clip();
    let g = ctx.createLinearGradient(0, 0, 0, H);
    g.addColorStop(0, '#202328'); g.addColorStop(0.5, '#15171a'); g.addColorStop(1, '#0b0c0e');
    ctx.fillStyle = g; ctx.fillRect(0, 0, W, H);
    // ridges every 6 px of arc at the centre
    const dA = 6 / R, phase = (((pos() / R) % dA) + dA) % dA;
    for (let a = -Math.PI / 2 + phase; a < Math.PI / 2; a += dA) {
      const c = Math.cos(a), x = cx + R * Math.sin(a), w = Math.max(0.5, 2.2 * c);
      const lit = 0.10 + 0.55 * c * c;
      ctx.fillStyle = `rgba(0,0,0,${0.55 * c})`;
      ctx.fillRect(x + w * 0.5, 4, Math.max(0.5, 1.6 * c), H - 8);
      ctx.fillStyle = on ? `rgba(240,179,90,${lit})` : `rgba(200,206,214,${lit})`;
      ctx.fillRect(x - w * 0.5, 4, w, H - 8);
    }
    // cylinder shading towards the ends + top sheen
    g = ctx.createLinearGradient(0, 0, W, 0);
    g.addColorStop(0, 'rgba(8,9,10,.95)'); g.addColorStop(0.2, 'rgba(8,9,10,.15)');
    g.addColorStop(0.5, 'rgba(8,9,10,0)'); g.addColorStop(0.8, 'rgba(8,9,10,.15)'); g.addColorStop(1, 'rgba(8,9,10,.95)');
    ctx.fillStyle = g; ctx.fillRect(0, 0, W, H);
    g = ctx.createLinearGradient(0, 0, 0, H);
    g.addColorStop(0, 'rgba(255,255,255,.07)'); g.addColorStop(0.35, 'rgba(255,255,255,0)');
    g.addColorStop(1, 'rgba(0,0,0,.35)');
    ctx.fillStyle = g; ctx.fillRect(0, 0, W, H);
    ctx.restore();
    ctx.strokeStyle = on ? 'rgba(240,179,90,.55)' : 'rgba(255,255,255,.09)';
    ctx.lineWidth = 1; ctx.beginPath(); ctx.roundRect(0.5, 0.5, W - 1, H - 1, rr); ctx.stroke();
  };
  new ResizeObserver(draw).observe(cv);
  return { draw, active(v) { on = v; draw(); } };
}

function valueChanged(obj) {
  if (obj === 'stage') { requestStage(); savePrefsSoon(); syncEnabled(); markCurrentEdited(); }
  else if (obj === 'prefs') { savePrefsSoon(); requestJpeg(); updateCmp(); }
  else editChanged();
}

makeField($('#pointFields'), { obj: 'edit', key: 'tBlack', label: 'Black target', min: 0, max: 255, step: 1, dec: 0, def: 0 });
makeField($('#pointFields'), { obj: 'edit', key: 'tWhite', label: 'White target', min: 0, max: 255, step: 1, dec: 0, def: 255 });
makeField($('#pointFields'), { obj: 'edit', key: 'strength', label: 'Strength', min: 0.05, max: 1, step: 0.05, dec: 2, def: 1 });
for (const [key, label, def, min, max, step, dec] of ADJUST)
  makeField($('#adjFields'), { obj: 'edit', key, label, min, max, step, dec, def, wheel: true });
$('#resetAdjust').addEventListener('click', () => { Object.assign(S.edit, defaultAdjust()); syncPanel(); editChanged(); commit(); });
const FILTER_UI = { f_bw: { fields: '#bwFields', reset: '#resetBW' }, f_film: { fields: '#filmFields', reset: '#resetFilm' } };
function makeSelectField(parent, a) {
  const row = document.createElement('div');
  row.className = 'field fsel';
  row.innerHTML = `<label>${a.label}</label><select>${a.options.map(([v, t]) => `<option value="${v}">${t}</option>`).join('')}</select>`;
  const sel = row.querySelector('select');
  sel.addEventListener('change', () => { S.edit[a.key] = sel.value; valueChanged('edit'); commit(); });
  sel.addEventListener('keydown', e => e.stopPropagation());
  parent.appendChild(row);
  FIELDS.push({ sync: () => { sel.value = S.edit[a.key] ?? a.def; } });
}
for (const f of FILTERS) {
  const ui = FILTER_UI[f.sec];
  for (const a of f.fields) {
    if (a === '---') { $(ui.fields).insertAdjacentHTML('beforeend', '<div class="fsep"></div>'); continue; }
    if (!Array.isArray(a)) { makeSelectField($(ui.fields), a); continue; }
    const [key, label, def, min, max, step, dec, unit] = a;
    // filters use plain sliders with a gradient track that shows what they do
    makeField($(ui.fields), { obj: 'edit', key, label, min, max, step, dec, def, unit, grad: FILTER_GRADS[key] });
  }
  $(ui.reset).addEventListener('click', () => { Object.assign(S.edit, defaultFilter(f)); syncPanel(); editChanged(); commit(); });
}
makeField($('#jpegQ'), { obj: 'prefs', key: 'jpeg_q', label: 'Quality', min: 1, max: 100, smin: 50, smax: 100, step: 1, dec: 0, def: 90 });
makeField($('#jpegD'), { obj: 'prefs', key: 'jpeg_d', label: 'Distance', min: 0, max: 25, smin: 0.3, smax: 4, step: 0.05, dec: 2, def: 1 });
makeField($('#webpQ'), { obj: 'prefs', key: 'webp_q', label: 'Quality', min: 0, max: 100, smin: 50, smax: 100, step: 1, dec: 0, def: 90 });
makeField($('#avifQ'), { obj: 'prefs', key: 'avif_crf', label: 'CRF', min: 1, max: 70, smin: 5, smax: 50, step: 0.25, dec: 2, def: 25 });
makeField($('#avifS'), { obj: 'prefs', key: 'avif_preset', label: 'Preset', min: 0, max: 8, step: 1, dec: 0, def: 4 });
makeField($('#aomQ'), { obj: 'prefs', key: 'aom_q', label: 'Quality', min: 0, max: 100, smin: 30, smax: 100, step: 1, dec: 0, def: 80 });
makeField($('#aomS'), { obj: 'prefs', key: 'aom_speed', label: 'Speed', min: 0, max: 10, step: 1, dec: 0, def: 6 });
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
const syncAEnc = segInit('#aEncSeg', () => avifEnc(), v => { S.prefs.avif_enc = v; savePrefsSoon(); fmtChanged(); });
/* AVIF encoder actually used: SVT-AV1 if chosen and possible (exe found, image without alpha), else aom */
const svtUsable = () => !!S.cmp.svt && !(S.img && S.img.alpha);
const avifEnc = () => S.prefs.avif_enc === 'svt' && svtUsable() ? 'svt' : 'aom';
function syncEncRow() {
  const f = S.prefs.fmt;
  $('#encRow').classList.toggle('show', f !== 'PNG');
  for (const g of ['JPEG', 'WEBP', 'AVIF']) $('#grp' + g).classList.toggle('show', f === g);
  $('#jpegQ').style.display = S.prefs.jpeg_mode === 'd' ? 'none' : '';
  $('#jpegD').style.display = S.prefs.jpeg_mode === 'd' ? '' : 'none';
  $('#webpQ').style.display = S.prefs.webp_lossy ? '' : 'none';
  // AVIF: encoder choice; SVT-AV1 is greyed out when it can't be used. Show the sliders of the encoder in use.
  const svtBtn = $('#aEncSeg button[data-v="svt"]');
  svtBtn.disabled = !svtUsable();
  svtBtn.title = !S.cmp.svt ? 'SvtAv1EncApp.exe not found in encoders/' : (S.img && S.img.alpha) ? 'Not for images with alpha' : '';
  syncAEnc();
  const aom = avifEnc() === 'aom';
  $('#avifSvt').style.display = aom ? 'none' : '';
  $('#avifAom').style.display = aom ? '' : 'none';
  const forced = S.prefs.avif_enc === 'svt' && aom;
  $('#aomNote').textContent = !forced ? '' : !S.cmp.svt ? 'aom via Pillow (SvtAv1EncApp.exe not found)'
    : 'aom via Pillow (this image has alpha)';
  $('#aomNote').style.display = forced ? '' : 'none';
}
$('#prioSel').addEventListener('change', e => { S.stage.prio = e.target.value; valueChanged('stage'); commit(); });
$('#resizeOn').addEventListener('change', e => { S.stage.resize_on = e.target.checked; valueChanged('stage'); commit(); syncPanel(); });
$('#wpOn').addEventListener('change', e => { S.stage.wp_on = e.target.checked; valueChanged('stage'); commit(); syncPanel(); });
$('#geOn').addEventListener('change', e => { S.stage.ge_on = e.target.checked; valueChanged('stage'); commit(); syncPanel(); });
$('#advanceChk').addEventListener('change', e => { S.prefs.advance = e.target.checked; savePrefsSoon(); });

/* ---------- resize: Preset / Target Mpix / Custom ---------- */
const AR_PRESETS = ['16:9', '21:9', '4:3', '3:2', '1:1', '4:5', '9:16'];
const RES_PRESETS = [2160, 1440, 1080, 720, 480, 360];
const arRatio = k => { const [a, b] = String(k).split(':').map(Number); return a > 0 && b > 0 ? a / b : 16 / 9; };
function presetDims(ar, res) {          // must match preset_size() in fast_image_editor.py
  const r = arRatio(ar);
  return r >= 1 ? [Math.max(2, Math.round(res * r / 2) * 2), res] : [res, Math.max(2, Math.round(res / r / 2) * 2)];
}
/* The output aspect the resize will force (used by crop mode), or null. */
function resizeCropAspect() {
  const st = S.stage;
  if (st.resize_on === false) return null;
  if (st.mode === 'preset') {
    const [w, h] = presetDims(st.preset_ar, st.preset_res);
    return { key: st.preset_ar === '1:1' ? 'square' : st.preset_ar, r: arRatio(st.preset_ar), label: `${w}×${h}` };
  }
  if (st.mode === 'custom' && st.cmode !== 'stretch') return { key: null, r: st.cw / st.ch, label: `${st.cw}×${st.ch}` };
  return null;
}
function setStage(k, v) { S.stage[k] = v; valueChanged('stage'); commit(); syncPanel(); }
function buildChips() {
  const box = a => {               // icon rectangle in the real ratio, longest side 22 px
    const r = arRatio(a), M = 22;
    const w = r >= 1 ? M : Math.round(M * r), h = r >= 1 ? Math.round(M / r) : M;
    return `<span class="ic"><i style="width:${w}px;height:${h}px"></i></span>`;
  };
  $('#arChips').innerHTML = AR_PRESETS.map(a => `<button class="rzchip ar" data-v="${a}" title="${a}">${box(a)}${a}</button>`).join('');
  $('#resChips').innerHTML = RES_PRESETS.map(r => `<button class="rzchip" data-v="${r}">${r}p</button>`).join('');
  $('#arChips').addEventListener('click', e => { const b = e.target.closest('.rzchip'); if (b) setStage('preset_ar', b.dataset.v); });
  $('#resChips').addEventListener('click', e => { const b = e.target.closest('.rzchip'); if (b) setStage('preset_res', Number(b.dataset.v)); });
}
buildChips();
for (const [id, key] of [['#cwIn', 'cw'], ['#chIn', 'ch']]) {
  const el = $(id);
  el.addEventListener('change', () => {
    const v = clamp(Math.round(Number(el.value)) || S.stage[key], 1, 32768);
    if (v !== S.stage[key]) setStage(key, v); else el.value = v;
  });
  el.addEventListener('keydown', e => { if (e.key === 'Enter') el.blur(); e.stopPropagation(); });
}
$('#swapWH').addEventListener('click', () => { const w = S.stage.cw; S.stage.cw = S.stage.ch; setStage('ch', w); });
const syncCmode = segInit('#cmodeSeg', () => S.stage.cmode || 'crop', v => setStage('cmode', v));

function syncResize() {
  const st = S.stage, m = st.mode;
  $('#rzPreset').classList.toggle('show', m === 'preset');
  $('#rzMpix').classList.toggle('show', m === 'Mpix');
  $('#rzCustom').classList.toggle('show', m === 'custom');
  document.querySelectorAll('#arChips .rzchip').forEach(b => b.classList.toggle('on', b.dataset.v === st.preset_ar));
  document.querySelectorAll('#resChips .rzchip').forEach(b => b.classList.toggle('on', Number(b.dataset.v) === st.preset_res));
  if (document.activeElement !== $('#cwIn')) $('#cwIn').value = st.cw;
  if (document.activeElement !== $('#chIn')) $('#chIn').value = st.ch;
  syncCmode();
  let html = '';
  if (m === 'preset') { const [w, h] = presetDims(st.preset_ar, st.preset_res); html = `→ ${w}×${h} · crop in`; }
  else if (m === 'custom') html = `→ ${st.cw}×${st.ch} · ${st.cmode === 'stretch' ? 'stretch (aspect not kept)' : 'crop in'}`;
  const I = S.img;
  if (st.resize_on !== false && I && I.scale > 1.001)
    html += `${html ? '<br>' : ''}<span class="warn">⚠ Upscaling ×${I.scale.toFixed(2)} · source ${I.geo_w}×${I.geo_h} is smaller than the target</span>`;
  $('#rzInfo').innerHTML = html;
}

function syncEnabled() {
  $('#prioSel').disabled = S.stage.mode !== 'Mpix';
  $('#prioSel').closest('.field-row').style.opacity = S.stage.mode === 'Mpix' ? 1 : .38;
}
function syncPanel() {
  FIELDS.forEach(f => f.sync());
  syncSections(); updatePills(); syncResize();
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
