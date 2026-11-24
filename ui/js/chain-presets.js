'use strict';
/* Edit presets (redesign section 4): the whole edit in one preset — the values of Adjustments, Curves,
   the filters and Sharpen, which of them are switched off, the order they run in, and the Resize and
   Color balance settings. Crop and Point correction (both belong to one image) and the output format
   stay with the image. Resize and Color balance are worked out by Python before the live preview, so a
   hover preview and the thumbnails show the rest; they take effect when the preset is applied. The Presets button in the top bar (before Copy edits) opens a grid of cards: thumbnail
   of the current image through the preset, name, the preset's tools in order; hover previews it on the
   image, click (or Apply) applies it as one undo step; ⋯ = rename / update with current edits / delete.
   Stored with the tool presets in fae_presets.json, under "_chain": [{ name, chain, values, off, stage }]
   (stage = { resize: {...}, color: {...} }; presets saved before it existed leave Resize / Color balance alone). */
const ChainPresets = (() => {
  const SECS_C = ['adjust', 'curves', 'f_bw', 'f_film', 'f_sc', ...FX_LIST.map(d => d.sec), 'sharpen'];   // what an edit preset holds
  // + the stage settings of Resize and Color balance (S.stage keys)
  const STAGE = {
    resize: ['resize_on', 'mode', 'mpix', 'prio', 'preset_ar', 'preset_res', 'cw', 'ch', 'cmode'],
    color: ['color_on', 'wp_on', 'wp_pct', 'wp_strength', 'ge_on', 'ge_p', 'ge_sigma', 'ge_strength'],
  };
  const stageOf = st => Object.fromEntries(Object.entries(STAGE).map(([t, keys]) =>
    [t, Object.fromEntries(keys.map(k => [k, st[k] !== undefined ? st[k] : DEFAULT_STAGE[k]]))]));
  const stageUsed = (t, v) => t === 'resize' ? v.resize_on !== false : v.color_on !== false && !!(v.wp_on || v.ge_on);
  const TW = 352, TH = 216;                   // thumbnail pixels (shown at 176×108)
  const T = Presets.TOOLS;
  const list = () => { const P = Presets.data(); return P._chain || (P._chain = []); };
  const isDefault = (sec, v) => Presets.same(v, Presets.valuesOf(sec, T[sec].def()));

  // the tools' names and icons (from the tool pills and the filter tiles)
  const NAME = {}, ICON = {};
  for (const sec of [...SECS_C, 'resize', 'color']) {
    const f = FILTERS.find(f => f.sec === sec), pill = document.querySelector(`#toolPills .pill[data-sec="${sec}"]`);
    NAME[sec] = f ? f.name : (pill ? pill.title : sec);
    ICON[sec] = f ? f.icon : (pill ? pill.querySelector('svg').innerHTML : '');
  }

  // a preset read from disk, made whole (unknown tools / keys dropped, missing values neutral)
  const typeOf = id => id.split('#')[0], entry = id => ({ id, t: typeOf(id) });
  function clean(p) {
    if (!p || typeof p.name !== 'string') return null;
    const values = {};
    for (const sec of SECS_C) values[sec] = Presets.valuesOf(sec, Object.assign(T[sec].def(), (p.values || {})[sec] || {}));
    const chain = chainOf({ chain: (Array.isArray(p.chain) ? p.chain : []).map(entry) }).map(x => x.id);
    // extra instances (stackable tools): their values, switches and labels
    const inst = {}, lbl = {};
    for (const id of chain.filter(id => id.includes('#'))) {
      inst[id] = Presets.valuesOf(typeOf(id), Object.assign(T[typeOf(id)].def(), (p.inst || {})[id] || {}));
      if (p.lbl && typeof p.lbl[id] === 'string') lbl[id] = p.lbl[id].slice(0, 30);
    }
    for (const sec of SECS_C) if (p.lbl && typeof p.lbl[sec] === 'string') lbl[sec] = p.lbl[sec].slice(0, 30);
    const off = (Array.isArray(p.off) ? p.off : []).filter(s => SECS_C.includes(s) || inst[s]);
    const stage = p.stage && typeof p.stage === 'object'
      ? stageOf(Object.assign({}, DEFAULT_STAGE, p.stage.resize || {}, p.stage.color || {})) : null;
    return { name: p.name.slice(0, 60), chain, values, off, stage, inst, lbl };
  }
  // the current edit as a preset
  function capture(name) {
    const e = S.edit, values = {};
    for (const sec of SECS_C) values[sec] = Presets.valuesOf(sec, e);
    const chain = chainOf(e), inst = {}, lbl = {};
    for (const x of chain.filter(isExtra)) inst[x.id] = Presets.valuesOf(x.t, Object.assign(T[x.t].def(), (e.inst || {})[x.id] || {}));
    for (const x of chain) if (e.lbl && e.lbl[x.id]) lbl[x.id] = e.lbl[x.id];
    return { name, chain: chain.map(x => x.id), values, inst, lbl, stage: stageOf(S.stage),
      off: [...SECS_C.filter(sec => e.off && e.off[sec] && !isDefault(sec, values[sec])), ...Object.keys(inst).filter(id => e.off && e.off[id])] };
  }
  // the tools a preset uses, in the order they run: Resize, Color balance, the chain, Sharpen
  // (ids: a tool, or an extra instance tool#n)
  const usedId = (p, id) => id.includes('#') ? !isDefault(typeOf(id), (p.inst || {})[id]) : !isDefault(id, p.values[id]);
  const toolsOf = p => [...(p.stage ? ['resize', 'color'].filter(t => stageUsed(t, p.stage[t])) : []),
    ...[...p.chain.filter(s => SECS_C.includes(typeOf(s))), 'sharpen'].filter((s, i, a) => a.indexOf(s) === i && usedId(p, s))];
  // the edit with the preset applied to it
  function applied(base, p) {
    const e = clone(base);
    for (const sec of SECS_C) Object.assign(e, clone(p.values[sec]));
    e.off = Object.assign({}, e.off);
    for (const sec of SECS_C) delete e.off[sec];
    for (const x of chainOf(base)) if (isExtra(x)) delete e.off[x.id];
    for (const sec of p.off) e.off[sec] = true;
    e.chain = chainOf({ chain: p.chain.map(entry) });
    if (p.inst && Object.keys(p.inst).length) e.inst = clone(p.inst); else delete e.inst;
    if (p.lbl && Object.keys(p.lbl).length) e.lbl = clone(p.lbl); else delete e.lbl;
    return e;
  }

  /* ---------- hover preview ---------- */
  let saved = null;
  const chip = $('#presetChip');
  function preview(p) {
    if (!saved) saved = S.edit;
    S.edit = applied(saved, p);
    const later = p.stage ? ['resize', 'color'].filter(t => stageUsed(t, p.stage[t])
      || stageUsed(t, stageOf(S.stage)[t])).filter(t => JSON.stringify(p.stage[t]) !== JSON.stringify(stageOf(S.stage)[t])) : [];
    chip.textContent = 'Previewing ' + p.name + (later.length ? ' · ' + later.map(t => NAME[t]).join(' and ') + ' on click' : '');
    chip.classList.add('show');
    editChanged();
  }
  function endPreview() {
    chip.classList.remove('show');
    if (!saved) return;
    S.edit = saved; saved = null;
    editChanged();
  }
  function apply(p) {
    const base = saved || S.edit;
    saved = null; chip.classList.remove('show');
    S.edit = applied(base, p);
    if (p.stage) {                              // Resize / Color balance: a new stage from Python
      const before = JSON.stringify(stageOf(S.stage));
      Object.assign(S.stage, p.stage.resize, p.stage.color);
      if (JSON.stringify(stageOf(S.stage)) !== before) valueChanged('stage');
    }
    // open the islands of the tools in the preset
    const open = S.prefs.secs || [];
    S.prefs.secs = [...open, ...toolsOf(p).map(typeOf).filter((s, i, a) => a.indexOf(s) === i && !open.includes(s))];
    savePrefsSoon();
    close();
    syncPanel(); editChanged(); commit();
  }

  /* ---------- the dropdown ---------- */
  const btn = document.createElement('button');
  btn.id = 'chainBtn'; btn.className = 'ghost';
  btn.title = 'Edit presets: the whole chain of tools, in order';
  btn.innerHTML = 'Presets <svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>';
  $('#copyBtn').before(btn);
  const pop = document.createElement('div');
  pop.id = 'chainPop';
  pop.innerHTML = `
    <div class="cp-head">
      <span class="cp-title">Edit presets</span><span class="cp-hint">whole chain, in order</span>
      <input class="cp-search" placeholder="Search" spellcheck="false">
      <button class="cp-save">Save current edits</button>
    </div>
    <div class="cp-new"></div>
    <div class="cp-grid"></div>
    <div class="cp-foot">Applying replaces the tools and their order, Resize and Color balance included; crop, point correction and the output format stay as they are.</div>`;
  document.body.appendChild(pop);
  const grid = pop.querySelector('.cp-grid'), search = pop.querySelector('.cp-search'), newRow = pop.querySelector('.cp-new');
  let isOpen = false;
  const esc = s => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
  const DOTS = '<svg viewBox="0 0 24 24"><circle cx="5" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="19" cy="12" r="1.6"/></svg>';

  function drawThumb(cv, p) {
    const I = S.img, g = cv.getContext('2d');
    if (!I || !I.base) return;
    const t = Presets.smallOf(I, false, TW, TH);
    g.putImageData(new ImageData(new Uint8ClampedArray(t.base), TW, TH), 0, 0);
    WP.thumb(t, computeEdit(effectiveEdit(applied(saved || S.edit, p)))).then(r => {
      if (r && r.w === TW && r.h === TH && cv.isConnected) g.putImageData(new ImageData(new Uint8ClampedArray(r.out), TW, TH), 0, 0);
    });
  }

  function render() {
    WP.clearThumbs();
    const q = search.value.trim().toLowerCase(), l = list();
    const shown = l.map((p, i) => ({ p, i })).filter(({ p }) => !q || p.name.toLowerCase().includes(q));
    if (!l.length) { grid.innerHTML = '<div class="cp-empty">No edit presets yet. Save current edits to make one.</div>'; return; }
    if (!shown.length) { grid.innerHTML = '<div class="cp-empty">No preset matches the search.</div>'; return; }
    grid.innerHTML = shown.map(({ p, i }) => `
      <div class="cp-card" data-i="${i}">
        <div class="cp-thumbw"><canvas class="cp-thumb" width="${TW}" height="${TH}"></canvas>
          <button class="cp-apply">Apply</button><button class="cp-more" title="More">${DOTS}</button></div>
        <div class="cp-row"><span class="cp-name">${esc(p.name)}</span><span class="cp-meta">Yours</span></div>
        <div class="cp-strip">${toolsOf(p).map(s => `<span class="cp-tool" title="${esc(NAME[typeOf(s)])}${(p.lbl || {})[s] ? ' · ' + esc(p.lbl[s]) : ''}${p.off.includes(s) ? ' (off)' : ''}"`
          + `${p.off.includes(s) ? ' data-off' : ''}><svg viewBox="0 0 24 24">${ICON[typeOf(s)]}</svg></span>`).join('<i>›</i>') || '<span class="cp-none">No tools</span>'}</div>
      </div>`).join('');
    grid.querySelectorAll('.cp-card').forEach(card => {
      const p = l[+card.dataset.i];
      drawThumb(card.querySelector('canvas'), p);
      card.addEventListener('mouseenter', () => { if (!card.classList.contains('editing')) preview(p); });
      card.addEventListener('click', e => {
        if (e.target.closest('.cp-more, .cp-menu, input') || card.classList.contains('editing')) return;
        apply(p);
      });
      card.querySelector('.cp-more').addEventListener('click', e => { e.stopPropagation(); menu(card, p); });
    });
  }

  function menu(card, p) {
    grid.querySelectorAll('.cp-menu').forEach(m => m.remove());
    const m = document.createElement('div');
    m.className = 'cp-menu';
    m.innerHTML = '<button data-a="ren">Rename…</button><button data-a="upd">Update with current edits</button><button data-a="del" class="danger">Delete</button>';
    card.querySelector('.cp-thumbw').appendChild(m);
    m.addEventListener('click', e => {
      e.stopPropagation();
      const a = e.target.closest('button') && e.target.closest('button').dataset.a, l = list();
      if (a === 'ren') {
        m.remove(); endPreview();
        card.classList.add('editing');
        nameInput(card.querySelector('.cp-row'), p.name, name => {
          if (name && name !== p.name) {
            const clash = l.findIndex(x => x.name === name && x !== p);
            if (clash >= 0) l.splice(clash, 1);
            p.name = name; Presets.store();
          }
          render();
        });
      } else if (a === 'upd') {
        endPreview();
        Object.assign(p, capture(p.name)); Presets.store(); render();
      } else if (a === 'del') {
        const b = e.target.closest('button');
        if (!b.classList.contains('arm')) { b.classList.add('arm'); b.textContent = 'Click again to delete'; return; }
        endPreview();
        l.splice(l.indexOf(p), 1); Presets.store(); render();
      }
    });
  }

  function nameInput(el, value, done) {
    el.innerHTML = `<input class="pp-input" maxlength="60" placeholder="Preset name" value="${esc(value)}">`;
    const inp = el.querySelector('input');
    let finished = false;
    const finish = v => { if (finished) return; finished = true; done(v); };
    inp.addEventListener('keydown', e => {
      e.stopPropagation();
      if (e.key === 'Enter') finish(inp.value.trim());
      else if (e.key === 'Escape') { e.preventDefault(); finish(null); }
    });
    inp.addEventListener('blur', () => finish(null));
    inp.focus(); inp.select();
  }

  pop.querySelector('.cp-save').addEventListener('click', () => {
    endPreview();
    newRow.classList.add('show');
    nameInput(newRow, '', name => {
      newRow.classList.remove('show'); newRow.innerHTML = '';
      if (!name) return;
      const l = list(), p = capture(name), at = l.findIndex(x => x.name === name);
      if (at >= 0) l[at] = p; else l.push(p);
      Presets.store(); render();
    });
  });
  search.addEventListener('input', render);
  search.addEventListener('keydown', e => { e.stopPropagation(); if (e.key === 'Escape') close(); });
  grid.addEventListener('mouseleave', endPreview);
  pop.addEventListener('pointerdown', e => {
    e.stopPropagation();
    if (!e.target.closest('.cp-menu, .cp-more')) grid.querySelectorAll('.cp-menu').forEach(m => m.remove());
  });

  function place() {
    const b = btn.getBoundingClientRect(), w = pop.offsetWidth;
    pop.style.top = (b.bottom + 6) + 'px';
    pop.style.left = Math.max(8, Math.min(window.innerWidth - w - 8, b.right - w + 40)) + 'px';
  }
  function open() {
    Presets.close();
    // presets saved before this module existed / from disk: made whole once
    const P = Presets.data();
    P._chain = (P._chain || []).map(clean).filter(Boolean);
    isOpen = true;
    btn.classList.add('open');
    search.value = '';
    pop.classList.add('show');
    render(); place();
  }
  function close() {
    if (!isOpen) return;
    isOpen = false;
    endPreview();
    WP.clearThumbs();
    btn.classList.remove('open');
    pop.classList.remove('show');
    grid.innerHTML = ''; newRow.innerHTML = ''; newRow.classList.remove('show');
  }
  btn.addEventListener('click', e => { e.stopPropagation(); isOpen ? close() : open(); });
  btn.addEventListener('pointerdown', e => e.stopPropagation());
  document.addEventListener('pointerdown', () => { if (isOpen) close(); });
  document.addEventListener('keydown', e => {
    if (!isOpen || e.key !== 'Escape' || (e.target.closest && e.target.closest('#chainPop .pp-input'))) return;
    e.stopPropagation(); close();
  }, true);
  window.addEventListener('resize', () => { if (isOpen) place(); });
  // a shortcut (undo, copy edits…) while a preset is previewed works on the real edit
  window.addEventListener('keydown', () => { if (saved) endPreview(); }, true);

  return { open, close };
})();
