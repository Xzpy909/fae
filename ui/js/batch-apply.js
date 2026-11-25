'use strict';
/* Edit › Apply preset to… and Edit › Paste edits to…: one edit preset, or the copied edits, onto many
   images of the list at once. A window like the search and Filters (dark veil with the water caustics,
   the same frosted box): every image of the list as a card (thumbnail, file name), "Search images" top
   right. A card's Apply pill applies to that image alone; a click on the card selects it, and once one is
   selected the head shows Select all / Deselect / Apply to N.
   Each image gets the change as one undo step of its own (its history goes with it, like an edit made by
   hand); the current image goes through ChainPresets.apply / pasteEdits, exactly as if done there.
   Pasted edits: the crop carries over only to images of the same source size, as with Paste edits (an
   image not opened yet has no known size, so its crop stays). */
const BatchApply = (() => {
  const esc = s => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
  const CHECK = '<svg viewBox="0 0 24 24"><path d="m5 12 5 5 9-10"/></svg>';
  const wrap = document.createElement('div');
  wrap.id = 'batchPop';
  wrap.innerHTML = `
    <div class="cpal-veil"></div>
    <div class="fp-box bp-box" role="dialog" aria-label="Apply to images">
      <div class="fp-head">
        <span class="fp-title bp-title"></span><span class="fp-count bp-count"></span>
        <span class="bp-acts">
          <button class="bp-btn" data-a="all">Select all</button>
          <button class="bp-btn" data-a="none">Deselect</button>
          <button class="bp-btn bp-go" data-a="go"></button>
        </span>
        <input class="fp-search" type="search" placeholder="Search images" spellcheck="false" autocomplete="off" aria-label="Search images">
        <button class="isl-x bp-close" title="Close (Esc)"><svg class="xbtn" viewBox="0 0 20 20"><circle class="c" cx="10" cy="10" r="9.5"/><path class="x" d="M7 7l6 6M13 7l-6 6"/></svg></button>
      </div>
      <div class="bp-src"></div>
      <div class="fp-tiles bp-cards"></div>
      <div class="fp-empty bp-empty">No images match</div>
      <div class="fp-foot bp-foot">Click a card to select it · Apply on a card does that image only · Ctrl+A selects all · each image keeps its own undo</div>
    </div>`;
  document.body.appendChild(wrap);
  window.addEventListener('DOMContentLoaded', () => Caustics.attach(wrap.querySelector('.cpal-veil'), { overlay: true }));
  const $$ = s => wrap.querySelector(s);
  const grid = $$('.bp-cards'), search = $$('.fp-search'), src = $$('.bp-src');

  let mode = null, preset = null;          // 'preset' | 'paste'; the preset chosen
  const sel = new Set(), done = new Set();  // file paths: selected, applied while the window is open
  const isOpen = () => wrap.classList.contains('show');

  /* ---------- applying ---------- */
  // another image's state as it would show when opened (untouched: fresh edits, the current output settings)
  const stateOf = f => f.state ? clone(f.state) : { stage: clone(S.stage), edit: defaultEdit(), geom: defaultGeom() };
  const key = st => JSON.stringify({ stage: st.stage, edit: st.edit, geom: st.geom });    // = snapshot()
  // the copied edits on an image's state ({ cropSkipped } when its crop could not carry over)
  function pasted(base, clip, size) {
    const same = !!(size && clip.src && size[0] === clip.src[0] && size[1] === clip.src[1]);
    const geom = same ? clone(clip.geom) : clone(base.geom), edit = clone(clip.edit);
    if (JSON.stringify(geom) !== JSON.stringify(base.geom)) for (const t of ['black', 'gray', 'white']) if (edit[t]) edit[t].pos = null;
    return { st: { stage: clone(clip.stage), edit, geom }, cropSkipped: !same && !isDefaultGeom(clip.geom) };
  }
  // one image: returns { cropSkipped } or null when nothing changed
  function applyOne(i) {
    const f = S.files[i];
    if (i === S.cur) {
      if (mode === 'preset') ChainPresets.apply(preset);
      else pasteEdits();
      const skipped = mode === 'paste' && !(S.img && S.clipboard.src && S.clipboard.src[0] === S.img.src_w
        && S.clipboard.src[1] === S.img.src_h) && !isDefaultGeom(S.clipboard.geom);
      return { cropSkipped: skipped };
    }
    const base = stateOf(f);
    let st, cropSkipped = false;
    if (mode === 'preset') st = ChainPresets.appliedState(base, preset);
    else ({ st, cropSkipped } = pasted(base, S.clipboard, f.src));
    const before = f.committed || key(base), now = key(st);
    if (now === before) return { cropSkipped };
    f.hist = f.hist || [];
    f.hist.push(before);
    if (f.hist.length > 300) f.hist.shift();
    f.fut = [];
    f.state = st; f.committed = now;
    return { cropSkipped };
  }
  function applyTo(paths) {
    const idx = paths.map(p => S.files.findIndex(f => f.path === p)).filter(i => i >= 0);
    if (!idx.length || !ready()) return;
    // the current image last: its apply redraws the panel once the others are stored
    idx.sort((a, b) => (a === S.cur) - (b === S.cur));
    let skipped = 0;
    for (const i of idx) {
      const r = applyOne(i);
      if (r && r.cropSkipped) skipped++;
      done.add(S.files[i].path);
    }
    if (mode === 'preset' && !idx.includes(S.cur)) ChainPresets.openTools(preset);
    if (typeof syncPanel === 'function' && !idx.includes(S.cur)) syncPanel();
    renderList();
    const what = mode === 'preset' ? `Preset <b>${esc(preset.name)}</b> applied` : 'Edits pasted';
    const n = idx.length;
    toast(`${what} to ${n} image${n > 1 ? 's' : ''}` + (skipped ? `<br>Crop skipped on ${skipped}: different or unknown image size` : ''));
  }
  const ready = () => mode === 'preset' ? !!preset : !!S.clipboard;

  /* ---------- the window ---------- */
  function head() {
    const n = sel.size;
    $$('.bp-title').textContent = mode === 'preset' ? 'Apply preset to' : 'Paste edits to';
    const cnt = $$('.bp-count');
    cnt.textContent = n ? `${n} selected` : ''; cnt.classList.toggle('show', n > 0);
    $$('.bp-acts').classList.toggle('show', n > 0);
    $$('.bp-go').textContent = `Apply to ${n}`;
    $$('.bp-go').disabled = !ready();
    $$('[data-a="all"]').disabled = visible().every(c => sel.has(c.dataset.p));
  }
  function source() {
    if (mode === 'preset') {
      const l = ChainPresets.presets();
      src.innerHTML = '<span class="bp-lab">Preset</span>' + l.map((p, i) =>
        `<button class="bp-btn bp-chip${p === preset ? ' on' : ''}" data-i="${i}">${esc(p.name)}</button>`).join('');
    } else {
      const c = S.clipboard;
      src.innerHTML = c ? `<span class="bp-lab">Edits copied from</span><span class="bp-from">${esc(c.name || 'an image')}</span>`
        + (isDefaultGeom(c.geom) ? '' : '<span class="bp-note">the crop goes only to images of the same size</span>') : '';
    }
  }
  function cards() {
    grid.innerHTML = S.files.map((f, i) => `
      <div class="fp-tile bp-card${sel.has(f.path) ? ' sel' : ''}" data-p="${esc(f.path)}" title="${esc(f.path)}">
        <div class="bp-thumb"><img loading="lazy" src="${Bridge.thumbUrl(f.path)}" alt="" draggable="false"></div>
        <div class="bp-row"><span class="bp-check">${CHECK}</span><span class="fp-name bp-name">${esc(f.name)}</span>`
        + `${i === S.cur ? '<span class="bp-tag">Current</span>' : ''}</div>
        <span class="fp-badge"${done.has(f.path) ? '' : ' hidden'}>Applied</span>
        <button class="fp-add bp-apply" title="Apply to this image only">Apply</button>
      </div>`).join('');
    filter();
  }
  const visible = () => [...grid.querySelectorAll('.bp-card:not([hidden])')];
  function filter() {
    const q = search.value.trim().toLowerCase();
    let any = false;
    grid.querySelectorAll('.bp-card').forEach(c => {
      const hit = !q || c.querySelector('.bp-name').textContent.toLowerCase().includes(q);
      c.hidden = !hit; any = any || hit;
    });
    $$('.bp-empty').classList.toggle('show', !any);
    head();
  }
  function mark(c, on) { on ? sel.add(c.dataset.p) : sel.delete(c.dataset.p); c.classList.toggle('sel', on); }
  function markDone() {
    grid.querySelectorAll('.bp-card').forEach(c => { c.querySelector('.fp-badge').hidden = !done.has(c.dataset.p); });
  }

  // mode 'preset' (p = the preset, else the last one used) or 'paste'
  function open(m, p) {
    if (S.cropMode || !S.files.length) return;
    if (m === 'paste' && !S.clipboard) copyEdits();          // nothing copied yet: the current image's edits
    if (typeof Cmd !== 'undefined') { Cmd.closePop(); Cmd.Palette.close(); }
    if (typeof FilterPop !== 'undefined') FilterPop.close();
    if (ChainPresets.isOpen()) ChainPresets.close();
    mode = m;
    if (m === 'preset') {
      const l = ChainPresets.presets();
      preset = p || l.find(x => x.name === S.prefs.batchPreset) || l[0] || null;
      if (!preset) { status('No edit presets yet: save one in Presets first'); return; }
    }
    sel.clear(); done.clear();
    search.value = '';
    source(); cards();
    wrap.classList.add('show');
    grid.scrollTop = 0;
    search.focus({ preventScroll: true });
  }
  function close() { if (!isOpen()) return; wrap.classList.remove('show'); search.blur(); grid.innerHTML = ''; }

  src.addEventListener('click', e => {
    const b = e.target.closest('.bp-chip'); if (!b) return;
    preset = ChainPresets.presets()[+b.dataset.i];
    S.prefs.batchPreset = preset.name; savePrefsSoon();
    done.clear(); markDone();
    source(); head();
  });
  grid.addEventListener('click', e => {
    const c = e.target.closest('.bp-card'); if (!c) return;
    if (e.target.closest('.bp-apply')) { applyTo([c.dataset.p]); markDone(); return; }
    // Shift+click: everything between the last clicked card and this one
    const vis = visible(), at = vis.indexOf(c), on = !sel.has(c.dataset.p);
    if (e.shiftKey && lastClick >= 0 && lastClick < vis.length) {
      const [a, b] = [Math.min(lastClick, at), Math.max(lastClick, at)];
      for (let i = a; i <= b; i++) mark(vis[i], on);
    } else mark(c, on);
    lastClick = at;
    head();
  });
  let lastClick = -1;
  function selectAll(on) { for (const c of visible()) mark(c, on); lastClick = -1; head(); }
  $$('.bp-acts').addEventListener('click', e => {
    const a = e.target.closest('[data-a]'); if (!a) return;
    if (a.dataset.a === 'all') selectAll(true);
    else if (a.dataset.a === 'none') { sel.clear(); grid.querySelectorAll('.bp-card.sel').forEach(c => c.classList.remove('sel')); head(); }
    else if (a.dataset.a === 'go' && sel.size) { applyTo([...sel]); close(); }
  });
  $$('.bp-close').addEventListener('click', close);
  search.addEventListener('input', () => { lastClick = -1; filter(); });
  // the window sits over everything: its clicks, scrolling and keys stay in it
  wrap.addEventListener('pointerdown', e => {
    e.stopPropagation();
    if (e.target.classList.contains('cpal-veil')) close();
  });
  wrap.addEventListener('wheel', e => e.stopPropagation());
  wrap.addEventListener('dblclick', e => e.stopPropagation());
  // loaded before process.js and commands.js: this runs first, and no shortcut of the app goes past it
  window.addEventListener('keydown', e => {
    if (!isOpen()) return;
    e.stopImmediatePropagation();
    const typing = e.target === search;
    if (e.key === 'Escape') { e.preventDefault(); close(); }
    else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a' && !typing) { e.preventDefault(); selectAll(true); }
    else if (e.key === 'Enter' && sel.size && !e.target.closest('button')) { e.preventDefault(); applyTo([...sel]); close(); }
  }, true);

  return { open, close, isOpen };
})();
