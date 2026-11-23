'use strict';
/* Stackable tools (redesign section 8): extra instances of Adjustments, Curves and the filters, each with its
   own island, values, switch, place in the chain and optional label. Data model in state.js (e.inst,
   e.off[id], e.lbl, chain entries tool#n); render steps in edit-math.js (computeEdit) and process.js.
   - Duplicate: the icon in an island's head, the row menu or Ctrl+D; the copy goes right after the
     original, same values, switch and label.
   - Row menu (right-click an island's head): Duplicate, Rename…, Move up, Move down, Reset, Remove.
   - A Filters tile adds a new instance when the filter is already open (Inst.add).
   - Up to MAX_INST per tool. Mid/Detail is on the first Adjustments only (the core keeps one smoothing). */
const Inst = (() => {
  const RESET = '<svg viewBox="0 0 24 24"><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/></svg>';
  const DUP = '<svg viewBox="0 0 24 24"><rect x="8" y="8" width="13" height="13" rx="2"/><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3"/></svg>';
  const NAME = { point: 'Point Correction', adjust: 'Adjustments', curves: 'Curves', ...Object.fromEntries(FILTERS.map(f => [f.sec, f.name])) };
  const target = id => () => (S.edit.inst || {})[id];

  /* ---------- islands of extra instances ---------- */
  function card(id) {
    let c = panelScroll.querySelector(`:scope > .card.inst[data-id="${id}"]`);
    if (c) return c;
    const t = id.split('#')[0];
    if (!STACKABLE.includes(t)) return null;
    c = document.createElement('section');
    c.className = 'card sec inst'; c.dataset.sec = t; c.dataset.id = id;
    const tg = target(id);
    if (t === 'curves') {                           // its own curve editor (curves.js)
      c.innerHTML = `<div class="card-head"><span>${NAME[t]}</span><button class="rst all ccreset" title="Reset this curve channel">${RESET}</button></div>`
        + `<div class="tabs ctabs"><button data-ch="m" class="on">RGB</button><button data-ch="r" class="r">R</button>`
        + `<button data-ch="g" class="g">G</button><button data-ch="b" class="b">B</button></div>`
        + `<canvas class="curvecv" data-inst="1"></canvas><div class="curve-foot"><span class="muted">${CURVE_HINT}</span></div>`;
      panelScroll.appendChild(c);
      makeCurveEditor({ cv: c.querySelector('canvas'), tabs: [...c.querySelectorAll('.tabs button')], info: c.querySelector('.curve-foot span'),
        reset: c.querySelector('.ccreset') }, () => { const o = tg(); return o && o.curves; });
      buildIsland(c);
      if (typeof Presets !== 'undefined') Presets.attach(c);
      addDupButton(c);
      return c;
    }
    c.innerHTML = `<div class="card-head"><span>${NAME[t]}</span>`
      + `<button class="rst all" title="Reset this ${NAME[t]}">${RESET}</button></div><div class="ifields"></div>`;
    panelScroll.appendChild(c);                     // orderPanel moves it to its place
    const box = c.querySelector('.ifields');
    if (t === 'adjust') {
      for (const [key, label, def, min, max, step, dec] of ADJUST) {
        if (key === 'adj_middetail') continue;      // first Adjustments only
        makeField(box, { obj: 'edit', key, label, min, max, step, dec, def, target: tg,
          center: key === 'adj_sat' || key === 'adj_contrast' ? def : undefined });
      }
    } else {
      for (const a of PANELS.find(f => f.sec === t).fields) {
        if (a.head) { box.insertAdjacentHTML('beforeend', `<div class="fhead">${a.head}</div>`); continue; }
        if (a === '---') { box.insertAdjacentHTML('beforeend', '<div class="fsep"></div>'); continue; }
        if (!Array.isArray(a)) { makeSelectField(box, a, tg); continue; }
        const [key, label, def, min, max, step, dec, unit] = a;
        makeField(box, { obj: 'edit', key, label, min, max, step, dec, def, unit, target: tg });
      }
    }
    c.querySelector('.rst').addEventListener('click', () => {
      const o = tg(); if (!o) return;
      Object.assign(o, SEC_NEUTRAL[t]()); syncPanel(); editChanged(); commit();
    });
    buildIsland(c);
    if (typeof Presets !== 'undefined') Presets.attach(c);
    addDupButton(c);
    FIELDS.forEach(f => { if (f.row && c.contains(f.row)) f.sync(); });   // its fields show the values right away
    return c;
  }
  // islands whose instance left the chain (removed, undo, another image) go
  function prune(ids) {
    panelScroll.querySelectorAll(':scope > .card.inst').forEach(c => {
      if (ids.has(c.dataset.id)) return;
      c.querySelectorAll('.field').forEach(r => { r.dead = true; });
      c.remove();
    });
  }
  // the Duplicate icon in the head of every stackable island (before the reset icon)
  function addDupButton(c) {
    if (!STACKABLE.includes(c.dataset.sec) || c.querySelector('.isl-dup')) return;
    const tools = c.querySelector('.isl-tools');
    if (!tools) return;
    const b = document.createElement('button');
    b.className = 'rst isl-dup'; b.title = 'Duplicate (Ctrl+D)'; b.innerHTML = DUP;
    b.addEventListener('click', e => { e.stopPropagation(); duplicate(keyOf(c)); });
    tools.prepend(b);
  }

  /* ---------- actions ---------- */
  const full = t => instancesOf(S.edit, t).length >= MAX_INST;
  function reveal(id) {
    S.prefs.collapsed = (S.prefs.collapsed || []).filter(x => x !== id);
    syncPanel();
    const c = cardOfId(id);
    if (c) requestAnimationFrame(() => c.scrollIntoView({ block: 'nearest', behavior: 'smooth' }));
  }
  function duplicate(key) {
    const t = key.split('#')[0], e = S.edit;
    if (!STACKABLE.includes(t)) return;
    if (full(t)) { toast(`At most ${MAX_INST} ${NAME[t]} in the chain`, true); return; }
    const id = nextInstId(e, t), v = instValues(e, key, t);
    if (t === 'adjust') v.adj_middetail = 0;
    e.inst = Object.assign({}, e.inst, { [id]: v });
    if (!instOn(e, key)) e.off = Object.assign({}, e.off, { [id]: true });
    if (e.lbl && e.lbl[key]) e.lbl = Object.assign({}, e.lbl, { [id]: e.lbl[key] });
    const c = chainOf(e);
    c.splice(c.findIndex(x => x.id === key) + 1, 0, { id, t });
    e.chain = c;
    editChanged(); commit(); reveal(id);
  }
  // a new, neutral instance of t (the Filters tiles): before the island `before`, or (undefined / null)
  // after the open islands
  function add(t, before) {
    const e = S.edit;
    if (!STACKABLE.includes(t)) return;
    if (full(t)) { toast(`At most ${MAX_INST} ${NAME[t]} in the chain`, true); return; }
    const id = nextInstId(e, t), v = SEC_NEUTRAL[t]();
    e.inst = Object.assign({}, e.inst, { [id]: v });
    const c = chainOf(e), open = [...panelScroll.querySelectorAll(':scope > .card.sec.show .isl-grip')].map(g => keyOf(g.closest('.card')));
    let at = before ? c.findIndex(x => x.id === before) : -1;
    if (at < 0) at = open.length ? c.findIndex(x => x.id === open[open.length - 1]) + 1 : c.length;
    c.splice(at, 0, { id, t });
    e.chain = c;
    editChanged(); commit(); reveal(id);
  }
  function rename(key) {
    const c = cardOfId(key), lbl = c && c.querySelector('.isl-lbl');
    if (!lbl) return;
    const old = (S.edit.lbl || {})[key] || '';
    lbl.innerHTML = '<input class="isl-name" maxlength="30" placeholder="Label">';
    const inp = lbl.querySelector('input');
    inp.value = old;
    let done = false;
    const finish = v => {
      if (done) return; done = true;
      if (v !== null && v !== old) {
        S.edit.lbl = Object.assign({}, S.edit.lbl);
        if (v) S.edit.lbl[key] = v; else delete S.edit.lbl[key];
        if (!Object.keys(S.edit.lbl).length) delete S.edit.lbl;
        commit();
      }
      syncIslands();
    };
    inp.addEventListener('keydown', e => {
      e.stopPropagation();
      if (e.key === 'Enter') finish(inp.value.trim());
      else if (e.key === 'Escape') finish(null);
    });
    inp.addEventListener('blur', () => finish(inp.value.trim()));
    inp.addEventListener('pointerdown', e => e.stopPropagation());
    inp.addEventListener('click', e => e.stopPropagation());
    inp.focus(); inp.select();
  }
  // move one place up / down among the open re-orderable islands
  function move(key, dir) {
    const open = [...panelScroll.querySelectorAll(':scope > .card.sec.show .isl-grip')].map(g => keyOf(g.closest('.card')));
    const i = open.indexOf(key);
    if (i < 0) return;
    if (dir < 0 && i > 0) moveInChain(key, open[i - 1]);
    else if (dir > 0 && i < open.length - 1) moveInChain(key, open[i + 2] || null);
  }
  function reset(key) {
    const t = key.split('#')[0];
    if (key.includes('#')) { const o = target(key)(); if (o) Object.assign(o, SEC_NEUTRAL[t]()); }
    else Object.assign(S.edit, SEC_NEUTRAL[t]());
    syncPanel(); editChanged(); commit();
  }

  /* ---------- row menu (right-click an island's head) and shortcuts ---------- */
  const menu = document.createElement('div');
  menu.id = 'islMenu';
  document.body.appendChild(menu);
  let menuKey = null, lastKey = null;
  function openMenu(key, x, y) {
    const t = key.split('#')[0], stack = STACKABLE.includes(t);
    const open = [...panelScroll.querySelectorAll(':scope > .card.sec.show .isl-grip')].map(g => keyOf(g.closest('.card')));
    const i = open.indexOf(key);
    const item = (a, label, kbd, off) => `<button data-a="${a}"${off ? ' disabled' : ''}>${label}${kbd ? `<span class="kbd-hint">${kbd}</span>` : ''}</button>`;
    menu.innerHTML = item('dup', 'Duplicate', 'Ctrl+D', !stack || full(t)) + item('ren', 'Rename…', 'F2', !stack)
      + '<div class="im-sep"></div>' + item('up', 'Move up', '', i <= 0) + item('down', 'Move down', '', i < 0 || i >= open.length - 1)
      + '<div class="im-sep"></div>' + item('reset', 'Reset', '') + item('del', 'Remove', 'Del');
    menuKey = key;
    menu.classList.add('show');
    const w = menu.offsetWidth, h = menu.offsetHeight;
    menu.style.left = Math.min(x, window.innerWidth - w - 8) + 'px';
    menu.style.top = Math.min(y, window.innerHeight - h - 8) + 'px';
  }
  const closeMenu = () => { menu.classList.remove('show'); menuKey = null; };
  function act(a, key) {
    if (a === 'dup') duplicate(key);
    else if (a === 'ren') rename(key);
    else if (a === 'up') move(key, -1);
    else if (a === 'down') move(key, 1);
    else if (a === 'reset') reset(key);
    else if (a === 'del') deleteIsland(key);
  }
  menu.addEventListener('pointerdown', e => e.stopPropagation());
  menu.addEventListener('click', e => {
    const b = e.target.closest('button');
    if (!b || b.disabled) return;
    const key = menuKey; closeMenu(); act(b.dataset.a, key);
  });
  document.addEventListener('pointerdown', () => { if (menuKey) closeMenu(); });
  panelScroll.addEventListener('contextmenu', e => {
    const c = e.target.closest('.card.sec');
    if (!c || !c.querySelector('.isl-grip') || !e.target.closest('.card-head')) return;
    e.preventDefault();
    lastKey = keyOf(c);
    openMenu(lastKey, e.clientX, e.clientY);
  });
  // the island the shortcuts work on: the one under the mouse (only while the mouse is over the panel)
  let overPanel = false;
  panelScroll.addEventListener('pointerenter', () => { overPanel = true; });
  panelScroll.addEventListener('pointerleave', () => { overPanel = false; });
  panelScroll.addEventListener('pointerover', e => { const c = e.target.closest('.card.sec'); lastKey = c && c.querySelector('.isl-grip') ? keyOf(c) : null; });
  window.addEventListener('keydown', e => {
    if (menuKey && e.key === 'Escape') { e.stopPropagation(); closeMenu(); return; }
    if (/^(INPUT|SELECT|TEXTAREA)$/.test(e.target.tagName) && e.target.type !== 'range' && e.target.type !== 'checkbox') return;
    if (!overPanel || !lastKey || !cardOfId(lastKey) || !cardOfId(lastKey).classList.contains('show')) return;
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && !e.shiftKey && e.key.toLowerCase() === 'd') { e.preventDefault(); e.stopPropagation(); duplicate(lastKey); }
    else if (!ctrl && e.key === 'F2') { e.preventDefault(); rename(lastKey); }
    else if (!ctrl && e.key === 'Delete') { e.preventDefault(); deleteIsland(lastKey); }
  }, true);

  // Duplicate icons on the tools' own islands too
  STACKABLE.forEach(t => { const c = cardOf(t); if (c) addDupButton(c); });

  return { card, prune, duplicate, add, rename, NAME };
})();
