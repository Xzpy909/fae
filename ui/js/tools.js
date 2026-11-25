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
  for (const x of chainOf(S.edit)) if (isExtra(x) && instActive(S.edit, x)) st[x.t] = true;
  return st;
}
// a dock button's count badge: shown from 2 (one open copy is just the button's "on" look)
function setCount(p, n) {
  const b = p.querySelector('.cnt'); if (!b) return;
  b.hidden = n < 2; b.textContent = n;
  p.classList.toggle('has-cnt', n >= 2);
}
function updatePills() {
  const st = secState();
  document.querySelectorAll('#toolPills .pill[data-sec]').forEach(p => {
    const sec = p.dataset.sec, on = secOn(sec);
    if (STACKABLE.includes(sec)) {
      p.dataset.name = p.dataset.name || p.title;
      const n = (S.prefs.secs || []).includes(sec) ? instancesOf(S.edit, sec).length : 0;
      p.title = !n ? p.dataset.name : n >= MAX_INST ? `${p.dataset.name}: at most ${MAX_INST} in the chain` : `${p.dataset.name}: add another one`;
      setCount(p, n);
    } else {
      p.dataset.name = p.dataset.name || p.title;
      p.title = (S.prefs.secs || []).includes(sec) ? `${p.dataset.name}: click again to reset and close` : p.dataset.name;
    }
    p.classList.toggle('active', !!st[sec] && on);
    p.classList.toggle('bypass', !!st[sec] && !on);     // has values but its island is switched off
  });
  // the Filters pill shows a dot when any filter is in use
  $('#filtersPill').classList.toggle('active', FILTERS.some(f => st[f.sec] && secOn(f.sec)));
  const open = new Set(S.prefs.secs || []);
  setCount($('#filtersPill'), FILTERS.reduce((n, f) => n + (open.has(f.sec) ? instancesOf(S.edit, f.sec).length : 0), 0));
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
/* islands by key: a tool (its first instance) or an extra instance (tool#n) */
const keyOf = card => card.dataset.id || card.dataset.sec;
const islandOn = key => key.includes('#') ? instOn(S.edit, key) : secOn(key);
function setIslandOn(key, on, noCommit) {
  if (!key.includes('#')) return setSecOn(key, on, noCommit);
  if (islandOn(key) === on) return;
  S.edit.off = Object.assign({}, S.edit.off);
  if (on) delete S.edit.off[key]; else S.edit.off[key] = true;
  editChanged();
  if (!noCommit) commit();
  syncPanel();
}
// X on an island: an extra instance is removed; a first instance with extra ones hands over to the next
function deleteIsland(key) {
  if (key.includes('#')) return removeInst(key);
  const next = STACKABLE.includes(key) && instancesOf(S.edit, key).find(isExtra);
  if (!next) return deleteSec(key);
  const e = S.edit;
  Object.assign(e, instValues(e, next.id, key));
  e.off = Object.assign({}, e.off);
  if (e.off[next.id]) e.off[key] = true; else delete e.off[key];
  e.lbl = Object.assign({}, e.lbl);
  if (e.lbl[next.id]) e.lbl[key] = e.lbl[next.id]; else delete e.lbl[key];
  e.chain = chainOf(e).filter(x => x.id !== key).map(x => x.id === next.id ? { id: key, t: key } : x);
  dropInst(e, next.id);
  editChanged(); commit(); syncPanel();
}
function dropInst(e, id) {
  if (e.inst) { e.inst = Object.assign({}, e.inst); delete e.inst[id]; if (!Object.keys(e.inst).length) delete e.inst; }
  if (e.off && e.off[id]) { e.off = Object.assign({}, e.off); delete e.off[id]; }
  if (e.lbl && e.lbl[id]) { e.lbl = Object.assign({}, e.lbl); delete e.lbl[id]; if (!Object.keys(e.lbl).length) delete e.lbl; }
}
function removeInst(id) {
  S.edit.chain = chainOf(S.edit).filter(x => x.id !== id);
  dropInst(S.edit, id);
  editChanged(); commit(); syncPanel();
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
function buildIslands() { document.querySelectorAll('#panel .card').forEach(buildIsland); }
function buildIsland(card) {
  {
    card.classList.add('island');
    const head = card.querySelector('.card-head'), sec = card.dataset.sec, key = keyOf(card);
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
    // instance tag ([1], [2] … when the tool is in the chain more than once) and its label
    if (STACKABLE.includes(sec)) title.insertAdjacentHTML('afterend', '<span class="isl-tag mono" hidden></span><span class="isl-lbl"></span>');
    head.insertAdjacentHTML('afterbegin',
      `<label class="isw" title="On / off: off keeps the values but leaves them out"><input type="checkbox"><span></span></label>`);
    if (CHAIN_TOOLS.includes(sec))           // re-orderable: drag grip + its place in the chain
      head.insertAdjacentHTML('afterbegin', `<span class="isl-grip" title="Drag to reorder (the order the tools run in)">`
        + `<svg viewBox="0 0 10 16"><circle cx="3" cy="3" r="1.3"/><circle cx="7" cy="3" r="1.3"/><circle cx="3" cy="8" r="1.3"/>`
        + `<circle cx="7" cy="8" r="1.3"/><circle cx="3" cy="13" r="1.3"/><circle cx="7" cy="13" r="1.3"/></svg></span><span class="isl-num"></span>`);
    // the head's own buttons (reset all, clear) sit together on the right, before the X
    const tools = document.createElement('span');
    tools.className = 'isl-tools';
    [...head.children].slice([...head.children].indexOf(title) + 1)
      .filter(el => !el.classList.contains('tabs') && !el.classList.contains('isl-tag') && !el.classList.contains('isl-lbl')).forEach(el => tools.appendChild(el));
    head.appendChild(tools);
    head.insertAdjacentHTML('beforeend', `<button class="isl-x" title="Delete: reset and close">${ISL_X}</button>`
      + `<button class="isl-chev" title="Fold / unfold"><svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg></button>`);
    // fold / unfold: click the title or the chevron (remembered per tool)
    const fold = () => {
      const c = S.prefs.collapsed || [];
      S.prefs.collapsed = c.includes(key) ? c.filter(x => x !== key) : [...c, key];
      syncIslands(); savePrefsSoon();
    };
    head.querySelector('.isl-title').addEventListener('click', fold);
    head.querySelector('.isl-chev').addEventListener('click', fold);
    const sw = head.querySelector('.isw input');
    sw.addEventListener('change', () => setIslandOn(key, sw.checked));
    head.querySelector('.isl-x').addEventListener('click', () => deleteIsland(key));
    // touching a control on a switched-off island turns it back on (so the change shows in the preview)
    body.addEventListener('pointerdown', e => {
      if (!islandOn(key) && e.target.closest('input, select, button, canvas, .toggle')) setIslandOn(key, true, true);
    }, true);
    body.addEventListener('focusin', e => {
      if (!islandOn(key) && e.target.matches('input, select')) setIslandOn(key, true, true);
    });
  }
}
function syncIslands() {
  const folded = new Set(S.prefs.collapsed || []);
  // instance tags: the place among the tool's instances (chain order), only when there are several
  const byT = {};
  for (const x of chainOf(S.edit)) (byT[x.t] = byT[x.t] || []).push(x.id);
  document.querySelectorAll('#panel .card.sec').forEach(card => {
    const key = keyOf(card), on = islandOn(key), sw = card.querySelector('.isw input');
    card.classList.toggle('collapsed', folded.has(key));
    card.classList.toggle('off', !on);
    if (sw) sw.checked = on;
    const tag = card.querySelector('.isl-tag');
    if (tag) {
      const l = byT[card.dataset.sec] || [], n = l.length > 1 ? l.indexOf(key) + 1 : 0;
      tag.hidden = !n; tag.textContent = n || '';
      const lbl = (S.edit.lbl || {})[key];
      card.querySelector('.isl-lbl').textContent = lbl ? '· ' + lbl : '';
    }
  });
  // order numbers: 1, 2, 3 … over the open re-orderable islands, top to bottom
  let n = 0;
  document.querySelectorAll('#panel .card.sec.show .isl-num').forEach(b => { b.textContent = ++n; });
  if (typeof Presets !== 'undefined') Presets.sync();
}
buildIslands();

/* ======================================================================
   Panel order (under the Histogram block) = the order the tools run in: Crop, Resize and Color balance first (pinned), then the
   chain (S.edit.chain, re-orderable by dragging the grip), then Sharpen (pinned, runs last).
   Re-ordering is an edit: undo / redo, per image, Copy edits / Paste.
   ====================================================================== */
const PIN_TOP = ['crop', 'resize', 'color'], PIN_END = ['sharpen'];
const panelScroll = $('#panel .scroll');
const cardOf = sec => panelScroll.querySelector(`:scope > .card.sec[data-sec="${sec}"]:not(.inst)`);
// a chain entry's island: the tool's own, or an extra instance's (made on first use, instances.js)
const cardOfId = id => id.includes('#') ? (typeof Inst !== 'undefined' ? Inst.card(id) : null) : cardOf(id);
function orderPanel() {
  const chain = chainOf(S.edit);
  if (typeof Inst !== 'undefined') Inst.prune(new Set(chain.filter(isExtra).map(x => x.id)));
  const want = [...PIN_TOP, ...chain.map(c => c.id), ...PIN_END].map(cardOfId).filter(Boolean);
  const have = [...panelScroll.querySelectorAll(':scope > .card.sec')];
  if (want.length === have.length && want.every((c, i) => c === have[i])) return;
  let prev = panelScroll.querySelector(':scope > .reorder-space');      // the reorder spacer stays first
  for (const c of want) {
    if (!prev) { if (panelScroll.firstElementChild !== c) panelScroll.prepend(c); }
    else if (prev.nextElementSibling !== c) prev.after(c);
    prev = c;
  }
}
/* Tools join the chain in the order they are opened: an opened chain tool that does nothing yet moves to
   the end of the chain (after the open ones). A tool that already has values keeps its place, so
   opening it never changes the image. Moving idle tools doesn't change the output either, so it is no
   undo step of its own. On switching images the idle open tools line up in the order they were opened. */
function placeIdle(secs) {
  const c = chainOf(S.edit);
  let moved = false;
  for (const sec of secs) {
    const i = c.findIndex(x => x.id === sec);
    if (i < 0 || instActive(S.edit, c[i]) || (sec === 'point' && secState().point)) continue;
    c.push(c.splice(i, 1)[0]);
    moved = true;
  }
  if (!moved || JSON.stringify(c) === JSON.stringify(chainOf(S.edit))) return;
  const clean = snapshot() === S.committed;
  S.edit.chain = c;
  if (clean) S.committed = snapshot();
  editChanged(); orderPanel(); syncIslands();
}
/* After undo / redo or switching images: tools that do nothing stay where they were in the panel (prev =
   the chain before), the tools that do something take the remaining places in their restored order.
   The output is the restored edit's either way. */
function arrangeIdle(prev) {
  const st = secState(), cur = chainOf(S.edit);
  const act = x => x.t === 'point' ? st.point : instActive(S.edit, x), active = cur.filter(act);
  // an instance that does something but had no place before: keep the restored order as it is
  if (active.some(x => !prev.some(p => p.id === x.id))) return;
  let ai = 0;
  const out = [];
  for (const p of prev) {
    const x = cur.find(y => y.id === p.id);
    if (x) out.push(act(x) ? active[ai++] : x);
  }
  for (const x of cur) if (!out.includes(x)) out.push(x);
  if (out.some(x => !x) || JSON.stringify(out) === JSON.stringify(cur)) return;
  const clean = snapshot() === S.committed;
  S.edit.chain = out;
  if (clean) S.committed = snapshot();
}
/* new chain with `id` moved before `beforeId` (null = after the last open re-orderable island) */
function moveInChain(id, beforeId) {
  const c = chainOf(S.edit), from = c.findIndex(x => x.id === id);
  if (from < 0 || id === beforeId) return;
  const [it] = c.splice(from, 1);
  let to;
  if (beforeId) to = c.findIndex(x => x.id === beforeId);
  else {
    const open = [...panelScroll.querySelectorAll(':scope > .card.sec.show .isl-grip')].map(g => keyOf(g.closest('.card'))).filter(s => s !== id);
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
    drag.before = before ? keyOf(before) : null;
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
    if (e.type === 'pointerup') moveInChain(keyOf(d.card), d.before);
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
   Filters popover: the Filters pill opens a centred, scrollable grid of tiles, one per entry in
   FILTERS, with a search box for when the list grows. A tile adds the filter to the chain: the first
   time it opens the filter's island (at the end of the chain), after that it adds another instance
   (stackable filters, instances.js). Drag a tile into the panel to put it where the drop line shows.
   A tile shows how many there are (×2) and where they sit in the chain (#2, #4). The switches belong
   to the islands. It stays open while you pick; Esc, the X, the pill or a click outside closes it.
   ====================================================================== */
const FilterPop = (() => {
  const pop = $('#filterPop'), pill = $('#filtersPill'), grid = $('#fpTiles'), search = $('#fpSearch');
  grid.innerHTML = FILTERS.map(f => `
    <div class="fp-tile" data-sec="${f.sec}">
      <button class="fp-tbtn"><svg viewBox="0 0 24 24">${f.icon || ''}</svg><span class="fp-lab"><span class="fp-name">${f.name || f.sec}</span><span class="fp-state"></span></span></button>
      <span class="fp-badge mono" hidden></span><span class="fp-add">+ Add</span>
    </div>`).join('');
  $('#fpTiles').insertAdjacentHTML('afterend', '<div class="fp-foot">Click adds to the end of the chain · Drag a tile into the panel to place it</div>');
  const isOpen = () => Caustics.shown(pop);
  // the order numbers of a filter's open islands (the numbers on their rows)
  const places = sec => [...panelScroll.querySelectorAll(`:scope > .card.sec.show[data-sec="${sec}"] .isl-num`)].map(n => n.textContent).filter(Boolean);
  function sync() {
    const shown = new Set(S.prefs.secs || []);
    let total = 0;
    grid.querySelectorAll('.fp-tile').forEach(t => {
      const sec = t.dataset.sec, open = shown.has(sec);
      const inst = open ? instancesOf(S.edit, sec) : [], n = inst.length, at = open ? places(sec) : [];
      const offAll = n && inst.every(x => !islandOn(x.id));
      total += n;
      t.classList.toggle('sel', open);
      t.classList.toggle('active', open && !offAll);
      t.classList.toggle('full', n >= MAX_INST);
      t.querySelector('.fp-state').textContent = !open ? 'Not in the chain'
        : (at.length ? '#' + at.join(', #') + ' in chain' : 'In the chain') + (offAll ? ' · off' : '');
      const badge = t.querySelector('.fp-badge');
      badge.hidden = n < 2; badge.textContent = '×' + n;
      t.querySelector('.fp-add').textContent = n >= MAX_INST ? `At most ${MAX_INST}` : '+ Add';
      t.querySelector('.fp-tbtn').title = n >= MAX_INST ? `At most ${MAX_INST} in the chain` : 'Click: add to the end of the chain · drag into the panel: place it';
    });
    const cnt = $('#fpCount');
    cnt.textContent = total ? `${total} in chain` : ''; cnt.classList.toggle('show', total > 0);
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
    Caustics.show(pop); search.value = ''; filter(); grid.scrollTop = 0; sync();
    search.focus({ preventScroll: true });
  }
  function close() { if (!isOpen()) return; Caustics.hide(pop); search.blur(); sync(); }

  // add filter sec: open its island the first time, else another instance. before = the island to put it
  // before (null = after the open islands, undefined = the end, as a click does)
  function addFilter(sec, before) {
    const cur = S.prefs.secs || [];
    if (!cur.includes(sec)) {
      S.prefs.secs = [...cur, sec];
      S.prefs.collapsed = (S.prefs.collapsed || []).filter(x => x !== sec);
      placeIdle([sec]);
      syncSections(); syncIslands(); savePrefsSoon();
      if (before !== undefined) moveInChain(sec, before);
      const card = cardOf(sec);
      if (card) requestAnimationFrame(() => card.scrollIntoView({ block: 'nearest', behavior: 'smooth' }));
    } else if (typeof Inst !== 'undefined') Inst.add(sec, before === undefined ? null : before);
    sync();
  }

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

  /* click adds; drag into the panel places (a ghost tile follows the pointer, the panel shows a drop line) */
  let tdrag = null;
  const line = panelScroll.querySelector('.drop-line');
  function dropPlace(x, y) {
    const pr = $('#panel').getBoundingClientRect();
    if (x < pr.left || x > pr.right || y < pr.top || y > pr.bottom) { line.classList.remove('show'); return undefined; }
    const ts = [...panelScroll.querySelectorAll(':scope > .card.sec.show')].filter(c => c.querySelector('.isl-grip'));
    const sr = panelScroll.getBoundingClientRect();
    let before = null, lineY;
    for (const c of ts) {
      const r = c.getBoundingClientRect();
      if (y < r.top + r.height / 2) { before = keyOf(c); lineY = r.top - 5; break; }
    }
    if (before === null) {
      const last = ts[ts.length - 1] || [...panelScroll.querySelectorAll(':scope > .card.sec.show')].filter(c => PIN_TOP.includes(c.dataset.sec)).pop();
      lineY = last ? last.getBoundingClientRect().bottom + 5 : sr.top + 8;
    }
    line.style.top = (lineY - sr.top + panelScroll.scrollTop - 1) + 'px';
    line.classList.add('show');
    return before;
  }
  grid.addEventListener('pointerdown', e => {
    const b = e.target.closest('.fp-tbtn'); if (!b || e.button !== 0) return;
    tdrag = { sec: b.parentElement.dataset.sec, tile: b.parentElement, x0: e.clientX, y0: e.clientY, started: false, before: undefined, id: e.pointerId };
  });
  window.addEventListener('pointermove', e => {
    if (!tdrag) return;
    if (!tdrag.started) {
      if (Math.hypot(e.clientX - tdrag.x0, e.clientY - tdrag.y0) < 6) return;
      tdrag.started = true;
      const r = tdrag.tile.getBoundingClientRect();
      tdrag.ghost = tdrag.tile.cloneNode(true);
      tdrag.ghost.classList.add('fp-ghost');
      Object.assign(tdrag.ghost.style, { width: r.width + 'px', height: r.height + 'px' });
      tdrag.dx = e.clientX - r.left; tdrag.dy = e.clientY - r.top;
      document.body.appendChild(tdrag.ghost);
      document.body.classList.add('tile-dragging');
    }
    tdrag.ghost.style.transform = `translate(${e.clientX - tdrag.dx}px, ${e.clientY - tdrag.dy}px)`;
    tdrag.x = e.clientX; tdrag.y = e.clientY;
    tdrag.before = dropPlace(e.clientX, e.clientY);
    // near the panel's top / bottom edge the list scrolls (places out of view can be reached)
    const sr = panelScroll.getBoundingClientRect(), edge = 40, over = tdrag.before !== undefined;
    tdrag.speed = !over ? 0 : e.clientY < sr.top + edge ? -(sr.top + edge - e.clientY) * 0.35 : e.clientY > sr.bottom - edge ? (e.clientY - (sr.bottom - edge)) * 0.35 : 0;
    if (tdrag.speed && !tdrag.raf) {
      const tick = () => {
        if (!tdrag || !tdrag.speed) { if (tdrag) tdrag.raf = 0; return; }
        panelScroll.scrollTop += tdrag.speed;
        tdrag.before = dropPlace(tdrag.x, tdrag.y);
        tdrag.raf = requestAnimationFrame(tick);
      };
      tdrag.raf = requestAnimationFrame(tick);
    }
  });
  const endTileDrag = e => {
    if (!tdrag) return;
    const d = tdrag; tdrag = null;
    if (d.raf) cancelAnimationFrame(d.raf);
    if (!d.started) return;                      // a plain click: the click handler adds it
    d.ghost.remove(); document.body.classList.remove('tile-dragging'); line.classList.remove('show');
    grid.dataset.justDragged = '1'; setTimeout(() => { delete grid.dataset.justDragged; }, 0);
    if (e.type === 'pointerup' && d.before !== undefined) addFilter(d.sec, d.before);
  };
  window.addEventListener('pointerup', endTileDrag);
  window.addEventListener('pointercancel', endTileDrag);
  grid.addEventListener('click', e => {
    const b = e.target.closest('.fp-tbtn'); if (!b || grid.dataset.justDragged) return;
    addFilter(b.parentElement.dataset.sec);
  });
  return { open, close, isOpen, sync, add: addFilter };
})();
/* Opening a tool's island: it joins the chain at the end (if it does nothing yet) and comes up ready to use:
   Resize switched on, Crop straight into crop mode (Crop.enter opens the island too, e.g. from Image › Crop). */
function openSec(sec) {
  const cur = S.prefs.secs || [];
  if (cur.includes(sec)) return;
  S.prefs.secs = [...cur, sec];
  S.prefs.collapsed = (S.prefs.collapsed || []).filter(x => x !== sec);
  placeIdle([sec]);
  syncSections(); syncIslands(); updatePills(); savePrefsSoon();
  if (sec === 'resize' && !secOn('resize')) setSecOn('resize', true);
}
/* Dock buttons. A stackable tool (Adjustments, Curves) adds another copy once it is open, like the Filters
   tiles; its button shows how many copies there are. The single tools (Crop, Resize, Color balance,
   Sharpen, Point correction) toggle: the second click is the island's X, reset and close (Ctrl+Z brings
   the values back). */
document.querySelectorAll('#toolPills .pill[data-sec]').forEach(p => {
  p.addEventListener('pointerdown', e => e.stopPropagation());     // don't start a pan / eyedropper sample
  p.addEventListener('click', e => {
    e.stopPropagation();
    const sec = p.dataset.sec, open = (S.prefs.secs || []).includes(sec);
    if (open && STACKABLE.includes(sec)) { if (typeof Inst !== 'undefined') Inst.add(sec); return; }
    if (open) { deleteIsland(sec); return; }
    openSec(sec);
    if (sec === 'crop' && S.img && !S.cropMode && typeof Crop !== 'undefined') Crop.enter();
  });
});
// count badges on the dock buttons of the tools that stack (and on Filters: the filters in the chain)
document.querySelectorAll('#toolPills .pill[data-sec], #filtersPill').forEach(p => {
  if (p.id === 'filtersPill' || STACKABLE.includes(p.dataset.sec)) p.insertAdjacentHTML('beforeend', '<b class="cnt mono" hidden></b>');
});
$('#fileList').addEventListener('click', e => {
  const li = e.target.closest('li'); if (!li) return;
  if (e.target.closest('.rm')) { removeAt(Number(li.dataset.i)); return; }
  switchTo(Number(li.dataset.i));
});

/* Right-click an image (in the file list, or the one in the view): Show in folder (File menu too) */
const FileMenu = (() => {
  const menu = document.createElement('div');
  menu.id = 'fileMenu';
  menu.innerHTML = '<button data-a="reveal">Show in folder</button>';
  document.body.appendChild(menu);
  let path = null;
  const close = () => { menu.classList.remove('show'); path = null; };
  function open(p, x, y) {
    path = p;
    menu.classList.add('show');
    menu.style.left = Math.min(x, window.innerWidth - menu.offsetWidth - 8) + 'px';
    menu.style.top = Math.min(y, window.innerHeight - menu.offsetHeight - 8) + 'px';
  }
  menu.addEventListener('pointerdown', e => e.stopPropagation());
  menu.addEventListener('click', e => {
    const b = e.target.closest('button'), p = path;
    if (!b || !p) return;
    close();
    if (b.dataset.a === 'reveal') api().reveal(p);
  });
  document.addEventListener('pointerdown', () => { if (path) close(); });
  window.addEventListener('keydown', e => { if (path && e.key === 'Escape') { e.stopPropagation(); close(); } }, true);
  $('#fileList').addEventListener('contextmenu', e => {
    const li = e.target.closest('li'); if (!li) return;
    const f = S.files[Number(li.dataset.i)]; if (!f) return;
    e.preventDefault(); open(f.path, e.clientX, e.clientY);
  });
  vp.addEventListener('contextmenu', e => {
    if (S.cur < 0 || !S.files[S.cur] || S.cropMode) return;
    e.preventDefault(); open(S.files[S.cur].path, e.clientX, e.clientY);
  });
  return { close };
})();

function storeCurrent() {
  const f = S.files[S.cur]; if (!f) return;
  commit();
  f.state = { stage: clone(S.stage), edit: clone(S.edit), geom: clone(S.geom) };
  f.hist = S.hist; f.fut = S.fut; f.committed = S.committed;
}

function switchTo(i) {
  if (i < 0 || i >= S.files.length || (i === S.cur && S.img)) return;
  if (S.cropMode && (typeof Crop !== "undefined")) Crop.exit(true);
  const prev = S.files[S.cur] ? chainOf(S.edit) : null;
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
  if (prev) arrangeIdle(prev); else placeIdle(S.prefs.secs || []);
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
  S.img = null; S.reqId++; canvas.style.display = 'none'; GV.hide(); dropJpeg(); updateCmp();
  S.edit = defaultEdit(); S.geom = defaultGeom(); S.hist = []; S.fut = []; S.committed = snapshot();
  syncPanel(); editChanged(); updateMarkers(); $('#zoomLabel').textContent = '–';
  $('#zoomFit').classList.remove('on'); $('#zoom100').classList.remove('on');
}

async function pickFiles() { const p = await api().pick_files(); if (p && p.length) addPaths(p); }
async function pickFolder() { const p = await api().pick_folder(); if (p && p.length) addPaths(p); else if (p) status('No images in that folder'); }
// adding files / folders: File menu, Ctrl+O, drag and drop (commands.js)
$('#clearBtn').addEventListener('click', () => { S.files = []; S.cur = -1; clearView(); renderList(); updateInfo(); });

// The shell receives the real file paths and hands them on (bridge.js 'addPaths').
// pywebview: from the native drop on #dropZone (bind_drop in fastedit/app.py), an empty full-window
// overlay shown only during a file drag, so pywebview's event serialisation stays tiny.
// Tauri: the window takes file drags itself (the page sees no drag events); it says when one enters,
// leaves or drops (bridge.js 'dragState').
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
Bridge.on('dragState', st => {
  if (st === 'enter') { vp.classList.add('dragover'); dropZone.classList.add('show'); return; }
  if (st === 'drop') S.dropT = performance.now();
  dragEnd();
});

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
