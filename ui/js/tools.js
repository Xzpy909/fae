'use strict';
/* Tool pills and the Filters popover. */

/* ======================================================================
   Tool pills: each tool card in the right panel is shown only when its pill is on.
   Each pill toggles its own card, so any number can be open at once.
   The dot on a pill means that tool is doing something to the current image.
   ====================================================================== */
const SECS = ['crop', 'point', 'adjust', 'curves', 'resize', 'color', ...PANELS.map(f => f.sec)];
function syncSections() {
  orderPanel();
  const on = new Set(S.prefs.secs || []);
  document.querySelectorAll('#panel .card.sec').forEach(c => c.classList.toggle('show', on.has(c.dataset.sec)));
  document.querySelectorAll('#toolPills .pill[data-sec]').forEach(p => p.classList.toggle('on', on.has(p.dataset.sec)));
  if (typeof FilterPop !== 'undefined') FilterPop.sync();
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
  // the Filters pill shows a dot when any filter is in use
  $('#filtersPill').classList.toggle('active', FILTERS.some(f => st[f.sec] && secOn(f.sec)));
  syncIslands();
  if (typeof FilterPop !== 'undefined') FilterPop.sync();
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
    if (!card.classList.contains('sec')) return;          // (every card is a tool section)
    head.classList.add('isl-head');
    let title = head.firstElementChild;
    if (title && title.tagName === 'SPAN') title.classList.add('isl-title');
    else {                                     // e.g. Curves (tabs in the head): give it a title for the folded row
      title = document.createElement('span');
      title.className = 'isl-title';
      title.textContent = document.querySelector(`#toolPills .pill[data-sec="${sec}"]`)?.title || sec;
      head.prepend(title);
    }
    head.insertAdjacentHTML('afterbegin',
      `<label class="isw" title="On / off: off keeps the values but leaves them out"><input type="checkbox"><span></span></label>`);
    if (CHAIN_TOOLS.includes(sec))           // re-orderable: drag grip + its place in the chain
      head.insertAdjacentHTML('afterbegin', `<span class="isl-grip" title="Drag to reorder (the order the tools run in)">`
        + `<svg viewBox="0 0 10 16"><circle cx="3" cy="3" r="1.3"/><circle cx="7" cy="3" r="1.3"/><circle cx="3" cy="8" r="1.3"/>`
        + `<circle cx="7" cy="8" r="1.3"/><circle cx="3" cy="13" r="1.3"/><circle cx="7" cy="13" r="1.3"/></svg></span><span class="isl-num"></span>`);
    // the head's own buttons (reset all, clear) sit together on the right, before the X
    const tools = document.createElement('span');
    tools.className = 'isl-tools';
    [...head.children].slice([...head.children].indexOf(title) + 1).filter(el => !el.classList.contains('tabs')).forEach(el => tools.appendChild(el));
    head.appendChild(tools);
    head.insertAdjacentHTML('beforeend', `<button class="isl-x" title="Delete: reset and close">${ISL_X}</button>`
      + `<button class="isl-chev" title="Fold / unfold"><svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg></button>`);
    // fold / unfold: click the title or the chevron (remembered per tool)
    const fold = () => {
      const c = S.prefs.collapsed || [];
      S.prefs.collapsed = c.includes(sec) ? c.filter(x => x !== sec) : [...c, sec];
      syncIslands(); savePrefsSoon();
    };
    head.querySelector('.isl-title').addEventListener('click', fold);
    head.querySelector('.isl-chev').addEventListener('click', fold);
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
  const folded = new Set(S.prefs.collapsed || []);
  document.querySelectorAll('#panel .card.sec').forEach(card => {
    const on = secOn(card.dataset.sec), sw = card.querySelector('.isw input');
    card.classList.toggle('collapsed', folded.has(card.dataset.sec));
    card.classList.toggle('off', !on);
    if (sw) sw.checked = on;
  });
  // order numbers: 1, 2, 3 … over the open re-orderable islands, top to bottom
  let n = 0;
  document.querySelectorAll('#panel .card.sec.show .isl-num').forEach(b => { b.textContent = ++n; });
}
buildIslands();

/* ======================================================================
   Panel order (under the Histogram block) = the order the tools run in: Crop, Resize and Color balance first (pinned), then the
   chain (S.edit.chain, re-orderable by dragging the grip), then Sharpen (pinned, runs last).
   Re-ordering is an edit: undo / redo, per image, Copy edits / Paste.
   ====================================================================== */
const PIN_TOP = ['crop', 'resize', 'color'], PIN_END = ['sharpen'];
const panelScroll = $('#panel .scroll');
const cardOf = sec => panelScroll.querySelector(`:scope > .card.sec[data-sec="${sec}"]`);
function orderPanel() {
  const want = [...PIN_TOP, ...chainOf(S.edit).map(c => c.t), ...PIN_END].map(cardOf).filter(Boolean);
  const have = [...panelScroll.querySelectorAll(':scope > .card.sec')];
  if (want.length === have.length && want.every((c, i) => c === have[i])) return;
  let prev = panelScroll.querySelector(':scope > .reorder-space');      // the reorder spacer stays first
  for (const c of want) {
    if (!prev) { if (panelScroll.firstElementChild !== c) panelScroll.prepend(c); }
    else if (prev.nextElementSibling !== c) prev.after(c);
    prev = c;
  }
}
/* new chain with `id` moved before `beforeId` (null = after the last open re-orderable island) */
function moveInChain(id, beforeId) {
  const c = chainOf(S.edit), from = c.findIndex(x => x.id === id);
  if (from < 0 || id === beforeId) return;
  const [it] = c.splice(from, 1);
  let to;
  if (beforeId) to = c.findIndex(x => x.id === beforeId);
  else {
    const open = [...panelScroll.querySelectorAll(':scope > .card.sec.show .isl-grip')].map(g => g.closest('.card').dataset.sec).filter(s => s !== id);
    to = open.length ? c.findIndex(x => x.id === open[open.length - 1]) + 1 : c.length;
  }
  c.splice(to < 0 ? c.length : to, 0, it);
  if (JSON.stringify(c) === JSON.stringify(chainOf(S.edit))) return;
  S.edit.chain = c;
  editChanged(); commit(); syncPanel();
}
(() => {
  const line = document.createElement('div');
  line.className = 'drop-line';
  panelScroll.appendChild(line);
  const spacerTop = document.createElement('div'), spacerEnd = document.createElement('div');
  spacerTop.className = spacerEnd.className = 'reorder-space';
  panelScroll.prepend(spacerTop);
  panelScroll.appendChild(spacerEnd);
  let drag = null;
  const targets = () => [...panelScroll.querySelectorAll(':scope > .card.sec.show')]
    .filter(c => c.querySelector('.isl-grip') && c !== drag.card);
  function place(y) {
    // the gap nearest to the pointer, among the open re-orderable islands
    const ts = targets(), sr = panelScroll.getBoundingClientRect();
    let before = null, lineY;
    for (const c of ts) {
      const r = c.getBoundingClientRect();
      if (y < r.top + r.height / 2) { before = c; lineY = r.top - 5; break; }
    }
    if (!before) { const last = ts[ts.length - 1]; lineY = last ? last.getBoundingClientRect().bottom + 5 : drag.r0.top; }
    drag.before = before ? before.dataset.sec : null;
    line.style.top = (lineY - sr.top + panelScroll.scrollTop - 1) + 'px';
  }
  function move(e) {
    if (!drag) return;
    drag.y = e.clientY;
    const sr = panelScroll.getBoundingClientRect();
    // scroll while near the panel's top / bottom edge
    const edge = 40, v = e.clientY < sr.top + edge ? -(sr.top + edge - e.clientY) : e.clientY > sr.bottom - edge ? e.clientY - (sr.bottom - edge) : 0;
    drag.speed = v * 0.35;
    if (v && drag.started && !drag.raf) drag.raf = requestAnimationFrame(tick);
    update();
  }
  // once the pointer really moves: every re-orderable island folds to its one-line row (the open ones
  // unfold again on drop), and the list is scrolled so the held island stays under the pointer
  function start() {
    drag.started = true;
    const cards = [...panelScroll.querySelectorAll(':scope > .card.sec.show')].filter(c => c.querySelector('.isl-grip'));
    cards.forEach(c => c.classList.add('collapsed'));
    drag.card.classList.add('dragging'); line.classList.add('show'); document.body.classList.add('reordering');
    // scroll by `need` so the held island lands under the pointer; where the list is too short to
    // scroll that far, temporary spacers at the top / bottom make the room
    const need = drag.card.getBoundingClientRect().top - (drag.y0 - drag.off);
    const maxTop = panelScroll.scrollHeight - panelScroll.clientHeight;
    const lack = Math.max(0, -(panelScroll.scrollTop + need)), over = Math.max(0, panelScroll.scrollTop + need - maxTop);
    spacerTop.style.height = lack + 'px'; spacerEnd.style.height = over + 'px';
    panelScroll.scrollTop += need + lack;
    drag.s0 = panelScroll.scrollTop;
    drag.top0 = drag.card.getBoundingClientRect().top;
  }
  function update() {
    if (!drag.started) { if (Math.abs(drag.y - drag.y0) < 4) return; start(); }
    // where the island would be without the lift, and where the pointer holds it
    const layoutTop = drag.top0 - (panelScroll.scrollTop - drag.s0);
    drag.card.style.setProperty('--dy', (drag.y - drag.off - layoutTop) + 'px');
    place(drag.y);
  }
  function tick() {
    if (!drag || !drag.speed) { if (drag) drag.raf = 0; return; }
    panelScroll.scrollTop += drag.speed;
    update();
    drag.raf = requestAnimationFrame(tick);
  }
  function end(e) {
    if (!drag) return;
    const d = drag; drag = null;
    if (d.raf) cancelAnimationFrame(d.raf);
    const y = d.card.getBoundingClientRect().top;     // where it was let go (still lifted)
    d.card.classList.remove('dragging'); d.card.style.removeProperty('--dy');
    line.classList.remove('show'); document.body.classList.remove('reordering');
    if (!d.started) return;
    if (e.type === 'pointerup') moveInChain(d.card.dataset.sec, d.before);
    syncIslands();                               // the islands that were open unfold again
    spacerTop.style.height = spacerEnd.style.height = '0px';
    if (!document.body.contains(d.card)) return;
    panelScroll.scrollTop += d.card.getBoundingClientRect().top - y;   // keep the dropped island where it was let go
    if (S.prefs.secs && S.prefs.secs.includes('curves')) requestAnimationFrame(drawCurve);
  }
  panelScroll.addEventListener('pointerdown', e => {
    const g = e.target.closest('.isl-grip');
    if (!g || e.button !== 0) return;
    e.preventDefault(); e.stopPropagation();
    const card = g.closest('.card');
    panelScroll.setPointerCapture(e.pointerId);          // the lifted island itself ignores the pointer
    const r0 = card.getBoundingClientRect();
    drag = { card, y0: e.clientY, y: e.clientY, off: e.clientY - r0.top, s0: panelScroll.scrollTop, top0: r0.top, r0,
      before: undefined, speed: 0, raf: 0, started: false };
  });
  panelScroll.addEventListener('pointermove', move);
  panelScroll.addEventListener('pointerup', end);
  panelScroll.addEventListener('pointercancel', end);
  panelScroll.addEventListener('lostpointercapture', end);
  window.addEventListener('keydown', e => { if (drag && e.key === 'Escape') { e.stopPropagation(); end(e); } }, true);
})();

/* ======================================================================
   Filters popover: the Filters pill (top-right corner, next to the histogram) opens a centred,
   scrollable grid of tiles, one per entry in FILTERS, with a search box for when the list grows.
   Clicking a tile shows / hides that filter's island in the right panel (like a tool pill);
   the switch on a tile mirrors the island's switch: on while the filter is open (or has values) and
   switched on; turning it on opens the island too.
   It stays open while you pick; Esc, the X, the pill or a click outside the box closes it.
   ====================================================================== */
const FilterPop = (() => {
  const pop = $('#filterPop'), pill = $('#filtersPill'), grid = $('#fpTiles'), search = $('#fpSearch');
  grid.innerHTML = FILTERS.map(f => `
    <div class="fp-tile" data-sec="${f.sec}">
      <button class="fp-tbtn" title="Show / hide its island"><svg viewBox="0 0 24 24">${f.icon || ''}</svg><span class="fp-lab"><span class="fp-name">${f.name || f.sec}</span><span class="fp-state"></span></span></button>
      <label class="isw fp-sw" title="On / off: off keeps the values but leaves them out"><input type="checkbox"><span></span></label>
    </div>`).join('');
  const isOpen = () => pop.classList.contains('show');
  function sync() {
    const st = secState(), shown = new Set(S.prefs.secs || []);
    let n = 0;
    grid.querySelectorAll('.fp-tile').forEach(t => {
      const sec = t.dataset.sec, has = !!st[sec], open = shown.has(sec), on = secOn(sec) && (open || has);
      if (has && on) n++;
      t.classList.toggle('sel', open);
      t.classList.toggle('active', on);
      t.classList.toggle('bypass', has && !on);
      t.querySelector('.fp-state').textContent = has ? (on ? 'On' : 'Bypassed') : (open ? (on ? 'On · no edits yet' : 'Off') : 'No edits');
      t.querySelector('.fp-sw input').checked = on;
    });
    const cnt = $('#fpCount');
    cnt.textContent = n ? `${n} on` : ''; cnt.classList.toggle('show', n > 0);
    pill.classList.toggle('on', isOpen());
  }
  function filter() {
    const q = search.value.trim().toLowerCase();
    let any = false;
    grid.querySelectorAll('.fp-tile').forEach(t => {
      const hit = !q || t.querySelector('.fp-name').textContent.toLowerCase().includes(q);
      t.hidden = !hit; any = any || hit;
    });
    $('#fpEmpty').classList.toggle('show', !any);
  }
  function open() {
    if (S.cropMode) return;
    pop.classList.add('show'); search.value = ''; filter(); grid.scrollTop = 0; sync();
    search.focus({ preventScroll: true });
  }
  function close() { if (!isOpen()) return; pop.classList.remove('show'); search.blur(); sync(); }

  pill.addEventListener('pointerdown', e => e.stopPropagation());
  pill.addEventListener('click', e => { e.stopPropagation(); isOpen() ? close() : open(); });
  // the popover sits over the image: keep its clicks / scrolling away from pan, zoom and the eyedropper
  let downOnBackdrop = false;
  pop.addEventListener('pointerdown', e => { e.stopPropagation(); downOnBackdrop = e.target === pop || e.target.classList.contains('fp-veil'); });
  pop.addEventListener('wheel', e => e.stopPropagation());
  // fast repeated clicks on tiles / the pills must not reach the image's double-click zoom
  for (const el of [pop, pill, $('#toolPills')]) el.addEventListener('dblclick', e => e.stopPropagation());
  pop.addEventListener('click', e => { if ((e.target === pop || e.target.classList.contains('fp-veil')) && downOnBackdrop) close(); });
  $('#fpClose').addEventListener('click', close);
  search.addEventListener('input', filter);
  search.addEventListener('keydown', e => {
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); }
  });
  grid.addEventListener('click', e => {
    const b = e.target.closest('.fp-tbtn'); if (!b) return;
    const sec = b.parentElement.dataset.sec, cur = S.prefs.secs || [], add = !cur.includes(sec);
    S.prefs.secs = add ? [...cur, sec] : cur.filter(x => x !== sec);
    if (add) S.prefs.collapsed = (S.prefs.collapsed || []).filter(x => x !== sec);
    syncSections(); syncIslands(); savePrefsSoon();
    if (add) {
      const card = document.querySelector(`#panel .card.sec[data-sec="${sec}"]`);
      if (card) card.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }
  });
  grid.addEventListener('change', e => {
    const cb = e.target.closest('.fp-sw input'); if (!cb) return;
    const sec = cb.closest('.fp-tile').dataset.sec, cur = S.prefs.secs || [];
    if (cb.checked && !cur.includes(sec)) {           // switching on a closed filter opens its island too
      S.prefs.secs = [...cur, sec]; savePrefsSoon(); syncSections();
    }
    if (secOn(sec) !== cb.checked) setSecOn(sec, cb.checked); else sync();
  });
  return { open, close, isOpen, sync };
})();
document.querySelectorAll('#toolPills .pill[data-sec]').forEach(p => {
  p.addEventListener('pointerdown', e => e.stopPropagation());     // don't start a pan / eyedropper sample
  p.addEventListener('click', e => {
    e.stopPropagation();
    const sec = p.dataset.sec, cur = S.prefs.secs || [];
    S.prefs.secs = cur.includes(sec) ? cur.filter(x => x !== sec) : [...cur, sec];
    if (S.prefs.secs.includes(sec)) S.prefs.collapsed = (S.prefs.collapsed || []).filter(x => x !== sec);
    syncSections(); syncIslands(); savePrefsSoon();
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
// fae.py). The zone is an empty full-window overlay shown only during a file
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
    const on = !(S.prefs.split && !encSplitOn());     // the button shows "on" only while its own split is shown
    S.prefs.split = on; if (on) S.prefs.encSplit = false;
    savePrefsSoon(); requestRender(); updateCmp();
  });
  // export block: lossless | encoded split (only for lossy formats)
  $('#btnEncSplit').addEventListener('click', () => {
    const on = !encSplitOn();
    S.prefs.encSplit = on; if (on) S.prefs.split = false;
    if (on) requestJpeg(true); else cancelJpeg();
    savePrefsSoon(); requestRender(); updateCmp();
  });
})();
