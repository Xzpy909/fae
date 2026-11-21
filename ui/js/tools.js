'use strict';
/* Tool pills and the nested Filters group. */

/* ======================================================================
   Tool pills: each tool card in the right panel is shown only when its pill is on.
   Each pill toggles its own card, so any number can be open at once.
   The dot on a pill means that tool is doing something to the current image.
   ====================================================================== */
const SECS = ['crop', 'point', 'adjust', 'curves', 'resize', 'color', ...FILTERS.map(f => f.sec)];
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
function updatePills() {
  const ident = pts => pts.length === 2 && pts[0][0] === 0 && pts[0][1] === 0 && pts[1][0] === 255 && pts[1][1] === 255;
  const st = {
    crop: !isDefaultGeom(S.geom),
    point: !!(S.edit.black || S.edit.gray || S.edit.white),
    curves: !['m', 'r', 'g', 'b'].every(c => ident(S.edit.curves[c])),
    adjust: ADJUST.some(([k, , def]) => S.edit[k] !== undefined && S.edit[k] !== def),
    resize: S.stage.resize_on !== false,
    color: !!(S.stage.wp_on || S.stage.ge_on),
  };
  for (const f of FILTERS) st[f.sec] = filterActive(f, S.edit);
  document.querySelectorAll('#toolPills .pill[data-sec]').forEach(p => p.classList.toggle('active', !!st[p.dataset.sec]));
  // the Filters pill shows a dot when any filter inside it is in use (handy while it is rolled up)
  $('#pgFilters .gp').classList.toggle('active', FILTERS.some(f => st[f.sec]));
}
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
$('#removeBtn').addEventListener('click', removeCurrent);
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
