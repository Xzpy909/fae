'use strict';
/* Tool pills and the nested Filters group. */

/* ======================================================================
   Tool pills: each tool card in the right panel is shown only when its pill is on.
   Each pill toggles its own card, so any number can be open at once.
   The dot on a pill means that tool is doing something to the current image.
   ====================================================================== */
const SECS = ['crop', 'point', 'adjust', 'curves', 'resize', 'color', ...PANELS.map(f => f.sec)];
function syncSections() {
  const on = new Set(S.prefs.secs || []);
  document.querySelectorAll('#panel .card.sec').forEach(c => c.classList.toggle('show', on.has(c.dataset.sec)));
  document.querySelectorAll('#toolPills .pill[data-sec]').forEach(p => p.classList.toggle('on', on.has(p.dataset.sec)));
  const fg = $('#pgFilters'), open = !!S.prefs.filtersOpen;
  fg.classList.toggle('open', open);
  fg.querySelector('.gp').classList.toggle('on', open);
  fg.querySelectorAll('.ptray .pill').forEach(p => { p.tabIndex = open ? 0 : -1; });
  if (on.has('curves')) requestAnimationFrame(drawCurve);
}
function secState() {
  const ident = pts => pts.length === 2 && pts[0][0] === 0 && pts[0][1] === 0 && pts[1][0] === 255 && pts[1][1] === 255;
  const st = {
    crop: !isDefaultGeom(S.geom),
    point: !!(S.edit.black || S.edit.gray || S.edit.white),
    curves: !['m', 'r', 'g', 'b'].every(c => ident(S.edit.curves[c])),
    adjust: ADJUST.some(([k, , def]) => S.edit[k] !== undefined && S.edit[k] !== def),
    resize: S.stage.resize_on !== false,
    color: !!(S.stage.wp_on || S.stage.ge_on),
  };
  for (const f of PANELS) st[f.sec] = filterActive(f, S.edit);
  return st;
}
function updatePills() {
  const st = secState();
  document.querySelectorAll('#toolPills .pill[data-sec]').forEach(p => {
    const sec = p.dataset.sec, on = secOn(sec);
    p.classList.toggle('active', !!st[sec] && on);
    p.classList.toggle('bypass', !!st[sec] && !on);     // has values but its island is switched off
  });
  // the Filters pill shows a dot when any filter inside it is in use (handy while it is rolled up)
  $('#pgFilters .gp').classList.toggle('active', FILTERS.some(f => st[f.sec] && secOn(f.sec)));
  syncIslands();
}

/* ======================================================================
   Islands: every tool card is a frosted island with an on / off switch (left) and a delete X (right).
   Off keeps the values but leaves the section out of the preview and Process; X resets the section
   and closes its island (Ctrl+Z brings the values back). Touching a control on an off island turns it on.
   ====================================================================== */
function secOn(sec) {
  if (sec === 'crop') return !(S.geom && S.geom.off);
  if (sec === 'resize') return S.stage.resize_on !== false;
  if (sec === 'color') return S.stage.color_on !== false;
  return !(S.edit.off && S.edit.off[sec]);
}
function geomFrameChanged() {           // the preview's frame changes: picked points lose their marker position
  for (const t of ['black', 'gray', 'white']) if (S.edit[t]) S.edit[t].pos = null;
  updateMarkers();
  S.newFile = true; requestStage(true);
}
function setSecOn(sec, on, noCommit) {
  if (secOn(sec) === on) return;
  if (sec === 'crop') {
    if (on) delete S.geom.off; else S.geom.off = true;
    if (!isDefaultGeom(S.geom)) geomFrameChanged();
  } else if (sec === 'resize') { S.stage.resize_on = on; valueChanged('stage'); }
  else if (sec === 'color') { S.stage.color_on = on; valueChanged('stage'); }
  else {
    S.edit.off = Object.assign({}, S.edit.off);
    if (on) delete S.edit.off[sec]; else S.edit.off[sec] = true;
    editChanged();
  }
  if (!noCommit) commit();
  syncPanel();
}
function deleteSec(sec) {
  if (sec === 'crop') {
    if (S.cropMode && (typeof Crop !== 'undefined')) Crop.exit(true);
    const had = !isDefaultGeom(S.geom) && !S.geom.off;
    S.geom = defaultGeom();
    if (had) geomFrameChanged();
    if (typeof Crop !== 'undefined') Crop.updateSummary();
  } else if (sec === 'resize') { S.stage.resize_on = false; valueChanged('stage'); }
  else if (sec === 'color') { Object.assign(S.stage, { wp_on: false, ge_on: false, color_on: true }); valueChanged('stage'); }
  else {
    if (sec === 'point') {
      Object.assign(S.edit, { black: null, gray: null, white: null, tBlack: 0, tWhite: 255, strength: 1 });
      if (S.tool) setTool(null);
    } else Object.assign(S.edit, SEC_NEUTRAL[sec]());
    if (S.edit.off && S.edit.off[sec]) { S.edit.off = Object.assign({}, S.edit.off); delete S.edit.off[sec]; }
    editChanged();
  }
  S.prefs.secs = (S.prefs.secs || []).filter(x => x !== sec);
  savePrefsSoon(); commit(); syncPanel();
}
const ISL_X = '<svg class="xbtn" viewBox="0 0 20 20"><circle class="c" cx="10" cy="10" r="9.5"/><path class="x" d="M7 7l6 6M13 7l-6 6"/></svg>';
function buildIslands() {
  document.querySelectorAll('#panel .card').forEach(card => {
    card.classList.add('island');
    const head = card.querySelector('.card-head'), sec = card.dataset.sec;
    // everything below the head goes into a body that dims while the island is off
    const body = document.createElement('div');
    body.className = 'isl-body';
    while (head.nextSibling) body.appendChild(head.nextSibling);
    card.appendChild(body);
    if (!card.classList.contains('sec')) return;          // the Histogram island has no switch / X
    head.classList.add('isl-head');
    const title = head.firstElementChild;
    if (title && title.tagName === 'SPAN') title.classList.add('isl-title');
    head.insertAdjacentHTML('afterbegin',
      `<label class="isw" title="On / off: off keeps the values but leaves them out"><input type="checkbox"><span></span></label>`);
    // the head's own buttons (reset all, clear) sit together on the right, before the X
    const tools = document.createElement('span');
    tools.className = 'isl-tools';
    [...head.children].slice(2).forEach(el => tools.appendChild(el));
    head.appendChild(tools);
    head.insertAdjacentHTML('beforeend', `<button class="isl-x" title="Delete: reset and close">${ISL_X}</button>`);
    const sw = head.querySelector('.isw input');
    sw.addEventListener('change', () => setSecOn(sec, sw.checked));
    head.querySelector('.isl-x').addEventListener('click', () => deleteSec(sec));
    // touching a control on a switched-off island turns it back on (so the change shows in the preview)
    body.addEventListener('pointerdown', e => {
      if (!secOn(sec) && e.target.closest('input, select, button, canvas, .toggle')) setSecOn(sec, true, true);
    }, true);
    body.addEventListener('focusin', e => {
      if (!secOn(sec) && e.target.matches('input, select')) setSecOn(sec, true, true);
    });
  });
}
function syncIslands() {
  document.querySelectorAll('#panel .card.sec').forEach(card => {
    const on = secOn(card.dataset.sec), sw = card.querySelector('.isw input');
    card.classList.toggle('off', !on);
    if (sw) sw.checked = on;
  });
}
buildIslands();
$('#pgFilters .gp').addEventListener('pointerdown', e => e.stopPropagation());
$('#pgFilters .gp').addEventListener('click', e => {
  e.stopPropagation();
  S.prefs.filtersOpen = !S.prefs.filtersOpen;
  syncSections(); savePrefsSoon();
});
document.querySelectorAll('#toolPills .pill[data-sec]').forEach(p => {
  p.addEventListener('pointerdown', e => e.stopPropagation());     // don't start a pan / eyedropper sample
  p.addEventListener('click', e => {
    e.stopPropagation();
    const sec = p.dataset.sec, cur = S.prefs.secs || [];
    S.prefs.secs = cur.includes(sec) ? cur.filter(x => x !== sec) : [...cur, sec];
    syncSections(); savePrefsSoon();
  });
});
$('#fileList').addEventListener('click', e => {
  const li = e.target.closest('li'); if (!li) return;
  if (e.target.closest('.rm')) { removeAt(Number(li.dataset.i)); return; }
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
  const fromDrop = S.dropT && performance.now() - S.dropT < 10000;
  S.loadT = fromDrop ? S.dropT : performance.now(); S.loadFromDrop = !!fromDrop; S.dropT = null;
  S.newFile = true;
  syncPanel(); updateListCur(); updateInfo(); editChanged();
  requestStage(true);
}

function removeAt(i) {
  if (i < 0 || i >= S.files.length) return;
  if (i === S.cur) { removeCurrent(); return; }
  S.files.splice(i, 1);
  if (i < S.cur) S.cur--;
  renderList(); updateInfo();
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
$('#clearBtn').addEventListener('click', () => { S.files = []; S.cur = -1; clearView(); renderList(); updateInfo(); });

// Python receives the real file paths from the native drop on #dropZone (see bind_drop in
// fast_image_editor.py). The zone is an empty full-window overlay shown only during a file
// drag, so pywebview's event serialisation stays tiny.
let dragDepth = 0;
const dropZone = $('#dropZone');
const isFileDrag = e => !!(e.dataTransfer && Array.from(e.dataTransfer.types || []).includes('Files'));
const dragEnd = () => { dragDepth = 0; vp.classList.remove('dragover'); dropZone.classList.remove('show'); };
window.addEventListener('dragenter', e => {
  if (!isFileDrag(e)) return;
  e.preventDefault(); dragDepth++;
  vp.classList.add('dragover'); dropZone.classList.add('show');
});
window.addEventListener('dragover', e => e.preventDefault());
window.addEventListener('dragleave', () => { if (--dragDepth <= 0) dragEnd(); });
window.addEventListener('drop', e => { e.preventDefault(); S.dropT = performance.now(); dragEnd(); });

/* View buttons: Compare (hold = original, like holding \) and the split-view toggle */
(() => {
  const cmpBtn = $('#btnCompare'), splitBtn = $('#btnSplit');
  const hold = on => { S.before = on; cmpBtn.classList.toggle('on', on); requestRender(); };
  cmpBtn.addEventListener('pointerdown', e => {
    if (e.button !== 0) return;
    e.stopPropagation(); e.preventDefault(); cmpBtn.setPointerCapture(e.pointerId); hold(true);
  });
  for (const ev of ['pointerup', 'pointercancel', 'lostpointercapture']) cmpBtn.addEventListener(ev, () => { if (S.before) hold(false); });
  splitBtn.addEventListener('pointerdown', e => e.stopPropagation());
  splitBtn.addEventListener('click', e => {
    e.stopPropagation();
    S.prefs.split = !S.prefs.split; savePrefsSoon(); requestRender(); updateCmp();
  });
})();
