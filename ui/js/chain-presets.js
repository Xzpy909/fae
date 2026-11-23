'use strict';
/* Edit presets (redesign section 4): the whole chain in one preset — the values of Adjustments, Curves,
   the filters and Sharpen, which of them are switched off, and the order they run in. Crop, Point
   correction (its picked colours belong to one image), Resize, Color balance and the output settings stay
   with the image. The Presets button in the top bar (before Copy edits) opens a grid of cards: thumbnail
   of the current image through the preset, name, the preset's tools in order; hover previews it on the
   image, click (or Apply) applies it as one undo step; ⋯ = rename / update with current edits / delete.
   Stored with the tool presets in fae_presets.json, under "_chain": [{ name, chain, values, off }]. */
const ChainPresets = (() => {
  const SECS_C = ['adjust', 'curves', 'f_bw', 'f_film', 'f_sc', 'sharpen'];   // what an edit preset holds
  const TW = 352, TH = 216;                   // thumbnail pixels (shown at 176×108)
  const T = Presets.TOOLS;
  const list = () => { const P = Presets.data(); return P._chain || (P._chain = []); };
  const isDefault = (sec, v) => Presets.same(v, Presets.valuesOf(sec, T[sec].def()));

  // the tools' names and icons (from the tool pills and the filter tiles)
  const NAME = {}, ICON = {};
  for (const sec of SECS_C) {
    const f = FILTERS.find(f => f.sec === sec), pill = document.querySelector(`#toolPills .pill[data-sec="${sec}"]`);
    NAME[sec] = f ? f.name : (pill ? pill.title : sec);
    ICON[sec] = f ? f.icon : (pill ? pill.querySelector('svg').innerHTML : '');
  }

  // a preset read from disk, made whole (unknown tools / keys dropped, missing values neutral)
  function clean(p) {
    if (!p || typeof p.name !== 'string') return null;
    const values = {};
    for (const sec of SECS_C) values[sec] = Presets.valuesOf(sec, Object.assign(T[sec].def(), (p.values || {})[sec] || {}));
    const chain = chainOf({ chain: (Array.isArray(p.chain) ? p.chain : []).map(id => ({ id, t: id })) }).map(x => x.id);
    const off = (Array.isArray(p.off) ? p.off : []).filter(s => SECS_C.includes(s));
    return { name: p.name.slice(0, 60), chain, values, off };
  }
  // the current edit as a preset
  function capture(name) {
    const e = S.edit, values = {};
    for (const sec of SECS_C) values[sec] = Presets.valuesOf(sec, e);
    return { name, chain: chainOf(e).map(x => x.id), values,
      off: SECS_C.filter(sec => e.off && e.off[sec] && !isDefault(sec, values[sec])) };
  }
  const toolsOf = p => [...p.chain.filter(s => SECS_C.includes(s)), 'sharpen']
    .filter((s, i, a) => a.indexOf(s) === i && !isDefault(s, p.values[s]));
  // the edit with the preset applied to it
  function applied(base, p) {
    const e = clone(base);
    for (const sec of SECS_C) Object.assign(e, clone(p.values[sec]));
    e.off = Object.assign({}, e.off);
    for (const sec of SECS_C) delete e.off[sec];
    for (const sec of p.off) e.off[sec] = true;
    e.chain = chainOf({ chain: p.chain.map(id => ({ id, t: id })) });
    return e;
  }

  /* ---------- hover preview ---------- */
  let saved = null;
  const chip = $('#presetChip');
  function preview(p) {
    if (!saved) saved = S.edit;
    S.edit = applied(saved, p);
    chip.textContent = 'Previewing ' + p.name;
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
    // open the islands of the tools in the preset
    const open = S.prefs.secs || [];
    S.prefs.secs = [...open, ...toolsOf(p).filter(s => !open.includes(s))];
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
    <div class="cp-foot">Applying replaces the filters and their order; crop, point correction, resize and output settings stay as they are.</div>`;
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
        <div class="cp-strip">${toolsOf(p).map(s => `<span class="cp-tool" title="${esc(NAME[s])}${p.off.includes(s) ? ' (off)' : ''}"`
          + `${p.off.includes(s) ? ' data-off' : ''}><svg viewBox="0 0 24 24">${ICON[s]}</svg></span>`).join('<i>›</i>') || '<span class="cp-none">No tools</span>'}</div>
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
