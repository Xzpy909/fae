'use strict';
/* Per-tool presets (redesign section 3). A preset button in the tool's row shows the current preset
   (Default, a preset name, or Custom once the values match none) and opens a dropdown:
   thumbnails of the current image through each preset, hover = preview on the main image (chip
   "Previewing <name>"), click = apply (one undo step), save the current values as a preset, rename /
   delete your own. A preset holds only that tool's values. Stored in fae_presets.json next to the
   settings: { tool: [{ name, values }] }. */
const Presets = (() => {
  // the tools with presets and the edit keys a preset of each holds
  const TOOLS = {
    adjust: { keys: ADJUST.map(a => a[0]), def: () => defaultAdjust() },   // + Curves, the filters, Sharpen
    curves: { keys: ['curves'], def: () => ({ curves: { m: ident(), r: ident(), g: ident(), b: ident() } }) },
  };
  for (const f of [...FILTERS, SHARPEN]) TOOLS[f.sec] = { keys: filterFields(f).map(a => fieldKD(a)[0]), def: () => defaultFilter(f) };
  const TW = 108, TH = 72;                // thumbnail pixels (shown at 54×36)

  let P = {};                             // the user's presets
  let pop = null, openSec = null, btnOf = {};
  let saved = null, previewName = null;   // hover preview: the real edit while a preset is shown

  const valuesOf = (sec, e) => Object.fromEntries(TOOLS[sec].keys.map(k => [k, clone(e[k] !== undefined ? e[k] : TOOLS[sec].def()[k])]));
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  const list = sec => P[sec] || (P[sec] = []);
  const realEdit = () => saved || S.edit;

  function label(sec) {
    const v = valuesOf(sec, realEdit());
    if (same(v, valuesOf(sec, TOOLS[sec].def()))) return 'Default';
    const p = list(sec).find(p => same(valuesOf(sec, p.values), v));
    return p ? p.name : 'Custom';
  }

  async function load() {
    try {
      const d = await api().get_presets();
      if (d && Array.isArray(d._chain)) P._chain = d._chain;      // edit presets (chain-presets.js)
      if (d && typeof d === 'object') for (const sec in TOOLS) {
        const l = Array.isArray(d[sec]) ? d[sec] : [];
        P[sec] = l.filter(p => p && typeof p.name === 'string' && p.values && typeof p.values === 'object')
          .map(p => ({ name: p.name.slice(0, 60), values: valuesOf(sec, Object.assign(TOOLS[sec].def(), p.values)) }));
      }
    } catch (_) {}
    sync();
  }
  const store = () => { try { api().save_presets(P); } catch (_) {} };

  /* ---------- the button in each tool's row ---------- */
  for (const sec in TOOLS) {
    const card = document.querySelector(`#panel .card.sec[data-sec="${sec}"]`);
    const title = card && card.querySelector('.isl-title');
    if (!title) continue;
    const b = document.createElement('button');
    b.className = 'isl-preset';
    b.title = 'Presets';
    b.innerHTML = '<span class="pp-label">Default</span><svg viewBox="0 0 24 24"><path d="m6 9 6 6 6-6"/></svg>';
    b.addEventListener('click', e => { e.stopPropagation(); openSec === sec ? close() : open(sec); });
    title.after(b);
    btnOf[sec] = b;
  }

  function sync() {
    for (const sec in btnOf) {
      const l = label(sec), el = btnOf[sec].querySelector('.pp-label');
      if (el.textContent !== l) el.textContent = l;
      btnOf[sec].classList.toggle('custom', l !== 'Default');
    }
  }

  /* ---------- thumbnails: the current image, cover-cropped to 3:2, through the edit with the preset.
     Sharpen: a crop from the middle at the preview's own scale instead (shrunk to a thumbnail the
     whole image would hide what sharpening does) ---------- */
  const copies = new WeakMap();           // stage image -> { 'WxH' or 'WxH1': its thumbnail-size copy }
  function smallOf(I, oneToOne, TW = 108, TH = 72) {
    const id = TW + 'x' + TH + (oneToOne ? '1' : '');
    let all = copies.get(I);
    if (!all) copies.set(I, all = {});
    let t = all[id];
    if (t) return t;
    const r = TW / TH;
    let cw = I.pw, ch = Math.round(I.pw / r);
    if (ch > I.ph) { ch = I.ph; cw = Math.round(I.ph * r); }
    if (oneToOne) { cw = Math.min(cw, TW); ch = Math.min(ch, TH); }
    const src = document.createElement('canvas');
    src.width = I.pw; src.height = I.ph;
    src.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(I.base), I.pw, I.ph), 0, 0);
    const c = document.createElement('canvas'); c.width = TW; c.height = TH;
    const g = c.getContext('2d');
    g.imageSmoothingEnabled = true; g.imageSmoothingQuality = 'high';
    g.drawImage(src, (I.pw - cw) >> 1, (I.ph - ch) >> 1, cw, ch, 0, 0, TW, TH);
    // w = output pixels across the crop, so Sharpen works at the thumbnail's scale
    t = { base: g.getImageData(0, 0, TW, TH).data, pw: TW, ph: TH, w: Math.round(cw * I.w / I.pw) };
    all[id] = t;
    return t;
  }
  // the edit with sec's values replaced; tools that run after it are left out (the thumbnail shows the
  // chain up to and including this tool)
  function editWith(sec, values) {
    const e = Object.assign(clone(realEdit()), clone(values));
    e.off = Object.assign({}, e.off);
    delete e.off[sec];
    const c = chainOf(e), at = c.findIndex(x => x.id === sec);
    if (at >= 0) c.forEach((x, i) => { if (i > at) e.off[x.t] = true; });   // (Sharpen runs after the whole chain)
    if (sec !== 'sharpen') e.off.sharpen = true;
    return e;
  }
  function drawThumb(cv, sec, values) {
    const I = S.img, g = cv.getContext('2d');
    if (!I || !I.base) return;
    const t = smallOf(I, sec === 'sharpen');
    g.putImageData(new ImageData(new Uint8ClampedArray(t.base), TW, TH), 0, 0);   // unedited until the render is in
    WP.thumb(t, computeEdit(effectiveEdit(editWith(sec, values)))).then(r => {
      if (r && r.w === TW && r.h === TH && cv.isConnected) g.putImageData(new ImageData(new Uint8ClampedArray(r.out), TW, TH), 0, 0);
    });
  }

  /* ---------- hover preview on the main image ---------- */
  const chip = document.createElement('div');
  chip.id = 'presetChip';
  vp.appendChild(chip);
  function preview(sec, values, name) {
    if (!saved) saved = S.edit;
    S.edit = editWithout(sec, values);
    previewName = name;
    chip.textContent = 'Previewing ' + name;
    chip.classList.add('show');
    editChanged();
  }
  const editWithout = (sec, values) => {        // the real edit with sec's values (island on)
    const e = Object.assign(clone(saved), clone(values));
    if (e.off && e.off[sec]) { e.off = Object.assign({}, e.off); delete e.off[sec]; }
    return e;
  };
  function endPreview() {
    chip.classList.remove('show');
    if (!saved) return;
    S.edit = saved; saved = null; previewName = null;
    editChanged();
  }

  function apply(sec, values) {
    const base = saved || S.edit;
    saved = null; previewName = null; chip.classList.remove('show');
    const e = Object.assign(clone(base), clone(values));
    if (e.off && e.off[sec]) { e.off = Object.assign({}, e.off); delete e.off[sec]; }
    S.edit = e;
    close();
    syncPanel(); editChanged(); commit();
  }

  /* ---------- the dropdown ---------- */
  const CHECK = '<svg class="pp-check" viewBox="0 0 24 24"><path d="M20 6 9 17l-5-5"/></svg>';
  const PEN = '<svg viewBox="0 0 24 24"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>';
  const BIN = '<svg viewBox="0 0 24 24"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/></svg>';
  const esc = s => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);

  function render() {
    if (!openSec) return;                  // closed meanwhile (e.g. Esc while naming)
    const sec = openSec, cur = label(sec), items = [{ name: 'Default', meta: 'No changes', values: valuesOf(sec, TOOLS[sec].def()) },
      ...list(sec).map((p, i) => ({ name: p.name, meta: 'Yours', values: p.values, i }))];
    pop.innerHTML = `<div class="pp-list">${items.map((it, k) => `
      <div class="pp-item${it.name === cur ? ' on' : ''}" data-k="${k}">
        <canvas class="pp-thumb" width="${TW}" height="${TH}"></canvas>
        <div class="pp-text"><div class="pp-name">${esc(it.name)}</div><div class="pp-meta">${it.meta}</div></div>
        ${it.i !== undefined ? `<span class="pp-acts"><button class="pp-ren" title="Rename">${PEN}</button>`
          + `<button class="pp-del" title="Delete">${BIN}</button></span>` : ''}${CHECK}
      </div>`).join('')}</div>
      <div class="pp-sep"></div>
      <div class="pp-save-row"><button class="pp-save">+ Save current as preset…</button></div>`;
    const rows = pop.querySelectorAll('.pp-item');
    rows.forEach(row => {
      const it = items[+row.dataset.k];
      drawThumb(row.querySelector('canvas'), sec, it.values);
      row.addEventListener('mouseenter', () => { if (!row.classList.contains('editing')) preview(sec, it.values, it.name); });
      row.addEventListener('click', e => {
        if (e.target.closest('.pp-acts, input') || row.classList.contains('editing')) return;
        apply(sec, it.values);
      });
      if (it.i === undefined) return;
      row.querySelector('.pp-ren').addEventListener('click', () => nameInput(row.querySelector('.pp-text'), it.name, name => {
        if (!name || name === it.name) return render();
        const l = list(sec), clash = l.findIndex(p => p.name === name);
        if (clash >= 0 && clash !== it.i) l.splice(clash, 1);
        const p = l.find(p => p.name === it.name);
        if (p) p.name = name;
        store(); render(); sync();
      }, row));
      const del = row.querySelector('.pp-del');
      del.addEventListener('click', () => {
        if (!del.classList.contains('arm')) { del.classList.add('arm'); del.title = 'Click again to delete'; return; }
        const l = list(sec), at = l.findIndex(p => p.name === it.name);
        if (at >= 0) l.splice(at, 1);
        endPreview(); store(); render(); sync();
      });
      del.addEventListener('mouseleave', () => { del.classList.remove('arm'); del.title = 'Delete'; });
    });
    pop.querySelector('.pp-list').addEventListener('mouseleave', endPreview);
    pop.querySelector('.pp-save').addEventListener('click', () => {
      endPreview();
      nameInput(pop.querySelector('.pp-save-row'), '', name => {
        if (!name) return render();
        const l = list(sec), v = valuesOf(sec, S.edit), at = l.findIndex(p => p.name === name);
        if (at >= 0) l[at].values = v; else l.push({ name, values: v });
        store(); render(); sync();
      });
    });
  }

  // an inline name field in place of el's content; Enter = done(name), Esc / blur = done(null)
  function nameInput(el, value, done, row) {
    endPreview();
    if (row) row.classList.add('editing');
    el.innerHTML = `<input class="pp-input" maxlength="60" placeholder="Preset name" value="${esc(value)}">`;
    const inp = el.querySelector('input');
    let finished = false;
    const finish = v => { if (finished) return; finished = true; done(v); };
    inp.addEventListener('keydown', e => {
      e.stopPropagation();
      if (e.key === 'Enter') finish(inp.value.trim());
      else if (e.key === 'Escape') finish(null);
    });
    inp.addEventListener('blur', () => finish(null));
    inp.focus(); inp.select();
  }

  function place() {
    const b = btnOf[openSec], panel = $('#panel');
    if (!b || !pop) return;
    const pr = panel.getBoundingClientRect(), br = b.getBoundingClientRect();
    pop.style.top = pop.style.bottom = '';
    const below = pr.bottom - br.bottom - 16, h = pop.offsetHeight;
    if (h > below && br.top - pr.top > below) pop.style.bottom = (pr.bottom - br.top + 6) + 'px';
    else pop.style.top = (br.bottom - pr.top + 6) + 'px';
  }

  function open(sec) {
    close();
    if (!pop) {
      pop = document.createElement('div');
      pop.id = 'presetPop';
      $('#panel').appendChild(pop);
      pop.addEventListener('pointerdown', e => e.stopPropagation());
    }
    openSec = sec;
    btnOf[sec].classList.add('open');
    render();
    pop.classList.add('show');
    place();
  }
  function close() {
    if (!openSec) return;
    endPreview();
    WP.clearThumbs();
    btnOf[openSec].classList.remove('open');
    openSec = null;
    pop.classList.remove('show');
    pop.innerHTML = '';
  }
  document.addEventListener('pointerdown', e => { if (openSec && !e.target.closest('.isl-preset')) close(); });
  document.addEventListener('keydown', e => { if (openSec && e.key === 'Escape') { e.stopPropagation(); close(); } }, true);
  $('#panel .scroll').addEventListener('scroll', () => { if (openSec) place(); });
  // a shortcut (undo, copy edits…) while a preset is previewed works on the real edit
  window.addEventListener('keydown', () => { if (saved) endPreview(); }, true);

  return { load, sync, close, previewing: () => !!saved,
    // shared with the edit presets (chain-presets.js)
    TOOLS, valuesOf, same, smallOf, data: () => P, store };
})();
