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
  const prev = chainOf(S.edit);
  S.stage = o.stage; S.edit = o.edit; S.geom = o.geom || defaultGeom(); S.committed = js;
  arrangeIdle(prev);                        // tools that do nothing keep their place in the panel
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
// Copy / paste / reset live in the Edit menu and the search (commands.js)
function resetEdits() { S.edit = defaultEdit(); syncPanel(); editChanged(); commit(); }

/* ======================================================================
   Files
   ====================================================================== */
// the chain order alone (tools that do nothing, in another order) doesn't make an image edited
const noChain = e => JSON.stringify(Object.assign({}, e, { chain: undefined }));
const DEFAULT_NOCHAIN = noChain(defaultEdit());
const isEditedState = (edit, geom) => noChain(edit) !== DEFAULT_NOCHAIN || !isDefaultGeom(geom);

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

/* Ctrl+V: images from the Windows clipboard (copied files, or a copied image the back end saves as PNG
   first, api.py paste_clipboard); the first one is shown */
async function pasteClipboard() {
  let r;
  try { r = await api().paste_clipboard(); } catch (e) { r = { ok: false, error: String(e) }; }
  if (!r || !r.ok) { status((r && r.error) || 'Nothing to paste'); return; }
  addPaths(r.paths);
  const i = S.files.findIndex(f => f.path.toLowerCase() === r.paths[0].toLowerCase());
  if (i >= 0 && i !== S.cur) switchTo(i);
  if (r.saved) toast(`Pasted image saved as <b>${basename(r.saved)}</b>`, false, r.saved);
}

function renderList() {
  const ul = $('#fileList');
  ul.innerHTML = S.files.map((f, i) => {
    const edited = i === S.cur ? isEditedState(S.edit, S.geom) : (f.state && isEditedState(f.state.edit, f.state.geom));
    return `<li data-i="${i}" class="${i === S.cur ? 'cur' : ''} ${edited ? 'edited' : ''} ${f.done ? 'done' : ''}" title="${f.path.replace(/"/g, '&quot;')}">
      <img loading="lazy" src="${Bridge.thumbUrl(f.path)}" alt="">
      <span class="nm">${f.name.replace(/</g, '&lt;')}</span>
      <span class="badges"><span class="b-edit" title="edited"></span><span class="b-done" title="processed">✓</span></span>
      <button class="rm" title="Remove from the list"><svg class="xbtn" viewBox="0 0 20 20"><circle class="c" cx="10" cy="10" r="9.5"/><path class="x" d="M7 7l6 6M13 7l-6 6"/></svg></button></li>`;
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
