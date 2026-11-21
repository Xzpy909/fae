'use strict';
/* Per-image history and the file list. */

/* ======================================================================
   History (per image)
   ====================================================================== */
const snapshot = () => JSON.stringify({ stage: S.stage, edit: S.edit, geom: S.geom });
function commit() {
  const now = snapshot();
  if (now === S.committed) return;
  if (S.committed !== null) {
    S.hist.push(S.committed);
    if (S.hist.length > 300) S.hist.shift();
    S.fut = [];
  }
  S.committed = now;
  updateUndoUI();
  markCurrentEdited();
}
function restore(js) {
  const o = JSON.parse(js);
  const stageChanged = JSON.stringify(o.stage) !== JSON.stringify(S.stage);
  const geomChanged = JSON.stringify(o.geom) !== JSON.stringify(S.geom);
  S.stage = o.stage; S.edit = o.edit; S.geom = o.geom || defaultGeom(); S.committed = js;
  syncPanel(); editChanged();
  if (geomChanged) { S.newFile = true; requestStage(true); }
  else if (stageChanged) { requestStage(); savePrefsSoon(); }
}
function undo() { commit(); if (!S.hist.length) return; S.fut.push(S.committed); restore(S.hist.pop()); updateUndoUI(); }
function redo() { if (!S.fut.length) return; S.hist.push(S.committed); restore(S.fut.pop()); updateUndoUI(); }
function updateUndoUI() { $('#undoBtn').disabled = !S.hist.length; $('#redoBtn').disabled = !S.fut.length; }
$('#undoBtn').addEventListener('click', undo);
$('#redoBtn').addEventListener('click', redo);

function copyEdits() {
  S.clipboard = clone({ stage: S.stage, edit: S.edit, geom: S.geom, src: S.img ? [S.img.src_w, S.img.src_h] : null });
  status('Edits copied');
}
function pasteEdits() {
  if (!S.clipboard || S.cur < 0) return;
  const stageChanged = JSON.stringify(S.clipboard.stage) !== JSON.stringify(S.stage);
  S.stage = clone(S.clipboard.stage); S.edit = clone(S.clipboard.edit);
  // crop/rotation only carries over between images with the same source size
  const sameSize = S.img && S.clipboard.src && S.clipboard.src[0] === S.img.src_w && S.clipboard.src[1] === S.img.src_h;
  const newGeom = sameSize ? clone(S.clipboard.geom) : S.geom;
  const geomChanged = JSON.stringify(newGeom) !== JSON.stringify(S.geom);
  S.geom = newGeom;
  if (geomChanged) for (const t of ['black', 'gray', 'white']) if (S.edit[t]) S.edit[t].pos = null;
  syncPanel(); editChanged(); commit();
  if (geomChanged) { S.newFile = true; requestStage(true); }
  else if (stageChanged) requestStage(true);
  status(sameSize || isDefaultGeom(S.clipboard.geom) ? 'Edits pasted' : 'Edits pasted (crop skipped: different image size)');
}
$('#copyBtn').addEventListener('click', copyEdits);
$('#pasteBtn').addEventListener('click', pasteEdits);
$('#resetBtn').addEventListener('click', () => { S.edit = defaultEdit(); syncPanel(); editChanged(); commit(); });

/* ======================================================================
   Files
   ====================================================================== */
const isEditedState = (edit, geom) => JSON.stringify(edit) !== DEFAULT_EDIT_JSON || !isDefaultGeom(geom);

function addPaths(paths) {
  const known = new Set(S.files.map(f => f.path.toLowerCase()));
  let first = -1;
  for (const p of paths || []) {
    if (known.has(p.toLowerCase())) continue;
    known.add(p.toLowerCase());
    S.files.push({ path: p, name: basename(p), state: null, done: false });
    if (first < 0) first = S.files.length - 1;
  }
  renderList();
  updateInfo();
  if (first >= 0 && S.cur < 0) switchTo(first);
  else if (first >= 0) status(`${paths.length} image(s) added`);
}

function renderList() {
  const ul = $('#fileList');
  ul.innerHTML = S.files.map((f, i) => {
    const edited = i === S.cur ? isEditedState(S.edit, S.geom) : (f.state && isEditedState(f.state.edit, f.state.geom));
    return `<li data-i="${i}" class="${i === S.cur ? 'cur' : ''} ${edited ? 'edited' : ''} ${f.done ? 'done' : ''}" title="${f.path.replace(/"/g, '&quot;')}">
      <img loading="lazy" src="/thumb?p=${encodeURIComponent(f.path)}" alt="">
      <span class="nm">${f.name.replace(/</g, '&lt;')}</span>
      <span class="badges"><span class="b-edit" title="edited"></span><span class="b-done" title="processed">✓</span></span></li>`;
  }).join('');
  $('#count').textContent = S.files.length;
  const cur = ul.querySelector('li.cur');
  if (cur) cur.scrollIntoView({ block: 'nearest' });
}
function updateListCur() {
  const lis = document.querySelectorAll('#fileList li');
  if (lis.length !== S.files.length) { renderList(); return; }
  lis.forEach((li, i) => li.classList.toggle('cur', i === S.cur));
  markCurrentEdited();
  const cur = lis[S.cur];
  if (cur) cur.scrollIntoView({ block: 'nearest' });
}
function markCurrentEdited() {
  const li = document.querySelector('#fileList li.cur');
  if (li) li.classList.toggle('edited', isEditedState(S.edit, S.geom));
  if ((typeof Crop !== "undefined")) Crop.updateSummary();
  updatePills();
}
