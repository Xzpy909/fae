'use strict';
/* Title bar (direction E): the menu bar, the search (loupe / Ctrl+Space), the zoom levels, Settings and the
   tool dock's on / off switch.
   Every command is defined once in CMDS and shows up in its menu, in the search and in Help › Keyboard
   shortcuts, so the three never disagree. A command: { id, menu, label, key?, sub?, run, en?, chk? }
   en() = can run now (greyed out otherwise), chk() = shown with a tick (on / off commands). */
const Cmd = (() => {
  const esc = s => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
  const hasImg = () => !!S.img;
  const hasFile = () => S.cur >= 0 && !!S.files[S.cur];
  const secOpen = sec => (S.prefs.secs || []).includes(sec);
  const pill = sec => document.querySelector(`#toolPills .pill[data-sec="${sec}"]`);
  const ICON_TICK = '<svg viewBox="0 0 24 24"><path d="m5 12 5 5 9-10"/></svg>';

  /* ---------- the commands ---------- */
  const SEP = { sep: true };
  const tool = (sec, label, key) => ({ id: 'tool_' + sec, menu: 'Image', label, key, sub: 'Tool', tool: true,
    run: () => pill(sec).click(), chk: () => secOpen(sec) });
  const MENUS = {
    File: [
      { id: 'addFiles', label: 'Add files…', key: 'Ctrl+O', run: () => pickFiles() },
      { id: 'addFolder', label: 'Add folder…', run: () => pickFolder() },
      { id: 'pasteImages', label: 'Paste images', key: 'Ctrl+V', run: () => pasteClipboard() },
      SEP,
      { id: 'process', label: 'Process', key: 'Ctrl+S', run: () => processCurrent(), en: hasFile },
      { id: 'processAll', label: 'Process all', run: () => processAll(), en: () => S.files.length > 0 },
      { id: 'advance', label: 'Next image after process', run: () => setAdvance(!S.prefs.advance), chk: () => !!S.prefs.advance },
      SEP,
      { id: 'reveal', label: 'Show in folder', run: () => api().reveal(S.files[S.cur].path), en: hasFile },
      { id: 'remove', label: 'Remove from list', run: () => removeCurrent(), en: hasFile },
      { id: 'clear', label: 'Clear list', run: () => $('#clearBtn').click(), en: () => S.files.length > 0 },
    ],
    Edit: [
      { id: 'undo', label: 'Undo', key: 'Ctrl+Z', run: () => undo(), en: () => S.hist.length > 0 },
      { id: 'redo', label: 'Redo', key: 'Ctrl+Y', run: () => redo(), en: () => S.fut.length > 0 },
      SEP,
      { id: 'copy', label: 'Copy edits', key: 'Ctrl+Shift+C', run: () => copyEdits(), en: hasFile },
      { id: 'paste', label: 'Paste edits', key: 'Ctrl+Shift+V', run: () => pasteEdits(), en: () => hasFile() && !!S.clipboard },
      { id: 'pasteTo', label: 'Paste edits to…', run: () => setTimeout(() => BatchApply.open('paste')),
        en: () => S.files.length > 0 && (!!S.clipboard || hasFile()) },
      { id: 'reset', label: 'Reset edits', sub: 'Point correction, curves, adjustments, filters', run: () => resetEdits(), en: hasFile },
      SEP,
      { id: 'presets', label: 'Edit presets…', run: () => setTimeout(() => ChainPresets.open()) },
      { id: 'presetTo', label: 'Apply preset to…', run: () => setTimeout(() => BatchApply.open('preset')),
        en: () => S.files.length > 0 && ChainPresets.list().length > 0 },
      { id: 'settings', label: 'Settings…', key: 'Ctrl+,', run: () => setTimeout(() => Settings.open()) },
    ],
    Image: [
      { id: 'crop', label: 'Crop & rotate…', key: 'C', run: () => Crop.enter(), en: hasImg },
      SEP,
      tool('resize', 'Resize'), tool('color', 'Color balance'), tool('sharpen', 'Sharpen'),
      tool('point', 'Point correction'), tool('adjust', 'Adjustments'), tool('curves', 'Curves'),
      { id: 'filters', label: 'Filters…', sub: 'Tool', tool: true, run: () => setTimeout(() => FilterPop.open()) },
      SEP,
      { id: 'prev', label: 'Previous image', key: '↑', run: () => switchTo(S.cur - 1), en: () => S.cur > 0 },
      { id: 'next', label: 'Next image', key: '↓', run: () => switchTo(S.cur + 1), en: () => S.cur >= 0 && S.cur < S.files.length - 1 },
    ],
    View: [
      { id: 'fit', label: 'Fit', key: '0', run: () => fitView(), en: hasImg },
      { id: 'zoom100', label: 'Actual pixels', key: '1', run: () => zoom100(), en: hasImg },
      { id: 'zoomIn', label: 'Zoom in', key: '+', run: () => zoomTo(S.view.z * 1.25), en: hasImg },
      { id: 'zoomOut', label: 'Zoom out', key: '−', run: () => zoomTo(S.view.z / 1.25), en: hasImg },
      SEP,
      { id: 'split', label: 'Split view', run: () => $('#btnSplit').click(), chk: () => !!S.prefs.split && !encSplitOn() },
      { id: 'dock', label: 'Show tool dock', sub: 'Settings', run: () => setDock(S.prefs.dock === false), chk: () => S.prefs.dock !== false },
    ],
    Help: [
      { id: 'search', label: 'Search…', key: 'Ctrl+Space', run: () => setTimeout(() => Palette.open()) },
      { id: 'keys', label: 'Keyboard shortcuts', run: () => setTimeout(showKeys) },
    ],
  };
  const CMDS = [];
  for (const [menu, items] of Object.entries(MENUS)) for (const c of items) if (!c.sep) { c.menu = c.menu || menu; CMDS.push(c); }
  const can = c => !c.en || c.en();
  function run(c) {
    if (!c || !can(c)) return;
    try { c.run(); } catch (err) { console.error(err); }
    if (typeof syncPanel === 'function') requestAnimationFrame(() => Menu.refresh());
  }

  /* ---------- prefs behind some commands ---------- */
  function setAdvance(on) { S.prefs.advance = !!on; $('#advanceChk').checked = !!on; savePrefsSoon(); Settings.sync(); }
  function syncDock() { document.body.classList.toggle('no-dock', S.prefs.dock === false); }
  function setDock(on) { S.prefs.dock = !!on; syncDock(); savePrefsSoon(); Settings.sync(); }
  $('#advanceChk').addEventListener('change', () => Settings.sync());

  /* ---------- one dropdown, shared by the menus, the zoom levels and Settings ---------- */
  const Pop = (() => {
    const el = document.createElement('div');
    el.className = 'tb-pop'; el.setAttribute('role', 'menu');
    document.body.appendChild(el);
    let owner = null, onClose = null;
    el.addEventListener('pointerdown', e => e.stopPropagation());
    document.addEventListener('pointerdown', () => close());
    window.addEventListener('resize', () => close());
    window.addEventListener('blur', () => close());
    function open(anchor, html, { align = 'left', closed } = {}) {
      close();
      owner = anchor; onClose = closed || null;
      el.innerHTML = html;
      el.classList.add('show');
      const b = anchor.getBoundingClientRect(), w = el.offsetWidth;
      const left = align === 'right' ? b.right - w : b.left;
      el.style.left = Math.max(6, Math.min(window.innerWidth - w - 6, left)) + 'px';
      el.style.top = (b.bottom + 4) + 'px';
      anchor.classList.add('open');
      return el;
    }
    function close() {
      if (!owner) return;
      owner.classList.remove('open');
      const f = onClose; owner = null; onClose = null;
      el.classList.remove('show'); el.innerHTML = '';
      if (f) f();
    }
    return { el, open, close, owner: () => owner };
  })();

  const itemHtml = (c, i) => c.sep ? '<div class="tb-sep" role="separator"></div>'
    : `<button class="tb-item${c.chk ? ' chk' : ''}${c.chk && c.chk() ? ' on' : ''}" role="${c.chk ? 'menuitemcheckbox' : 'menuitem'}"`
      + `${c.chk ? ` aria-checked="${!!c.chk()}"` : ''} data-i="${i}"${can(c) ? '' : ' disabled'}>`
      + `<span class="tick">${c.chk ? ICON_TICK : ''}</span><span class="lab">${esc(c.label)}</span><kbd class="k">${esc(c.key || '')}</kbd></button>`;

  /* ---------- the menu bar ---------- */
  const Menu = (() => {
    const bar = $('#menuBar');
    bar.innerHTML = Object.keys(MENUS).map(m => `<button class="mb" data-m="${m}" aria-haspopup="menu">${m}</button>`).join('');
    let cur = null;
    function show(name, focusFirst) {
      const btn = bar.querySelector(`[data-m="${name}"]`), items = MENUS[name];
      const el = Pop.open(btn, items.map(itemHtml).join(''), { closed: () => { cur = null; } });
      cur = name;
      el.setAttribute('aria-label', name);
      el.onclick = e => {
        const b = e.target.closest('.tb-item'); if (!b || b.disabled) return;
        const c = items[+b.dataset.i];
        Pop.close(); run(c);
      };
      if (focusFirst) el.querySelector('.tb-item:not([disabled])')?.focus();
    }
    function refresh() { if (cur) { const f = document.activeElement && document.activeElement.closest('.tb-pop'); show(cur, !!f); } }
    bar.addEventListener('pointerdown', e => {
      const b = e.target.closest('.mb'); if (!b) return;
      e.stopPropagation(); e.preventDefault();
      if (cur === b.dataset.m) Pop.close(); else show(b.dataset.m);
    });
    bar.addEventListener('keydown', e => {
      const b = e.target.closest('.mb'); if (!b) return;
      if (e.key === 'Enter' || e.key === ' ' || e.key === 'ArrowDown') { e.preventDefault(); e.stopPropagation(); show(b.dataset.m, true); }
    });
    // while a menu is open, moving to another title opens that one
    bar.addEventListener('pointerover', e => {
      const b = e.target.closest('.mb');
      if (b && cur && cur !== b.dataset.m) show(b.dataset.m);
    });
    // keyboard inside an open menu: up / down between items, left / right between menus
    Pop.el.addEventListener('keydown', e => {
      if (!cur) return;
      const items = [...Pop.el.querySelectorAll('.tb-item:not([disabled])')];
      const i = items.indexOf(document.activeElement);
      const names = Object.keys(MENUS), m = names.indexOf(cur);
      if (e.key === 'ArrowDown') items[(i + 1) % items.length]?.focus();
      else if (e.key === 'ArrowUp') items[(i - 1 + items.length) % items.length]?.focus();
      else if (e.key === 'ArrowRight') show(names[(m + 1) % names.length], true);
      else if (e.key === 'ArrowLeft') show(names[(m - 1 + names.length) % names.length], true);
      else if (e.key === 'Escape') { const n = cur; Pop.close(); bar.querySelector(`[data-m="${n}"]`).focus(); }
      else return;
      e.preventDefault(); e.stopPropagation();
    });
    return { refresh };
  })();

  /* ---------- zoom levels (the % button in the view toolbar) ---------- */
  (() => {
    const btn = $('#zoomLabel'), LEVELS = [25, 50, 100, 200, 400];
    const zoomPct = p => { const I = S.img; if (I) zoomTo(p / 100 * (I.w / I.pw) / dpr()); };
    btn.addEventListener('pointerdown', e => {
      e.stopPropagation(); e.preventDefault();
      if (Pop.owner() === btn) { Pop.close(); return; }
      if (!S.img) return;
      const el = Pop.open(btn, '<button class="tb-item" data-p="fit"><span class="tick"></span><span class="lab">Fit</span><kbd class="k">0</kbd></button><div class="tb-sep"></div>'
        + LEVELS.map(p => `<button class="tb-item" data-p="${p}"><span class="tick"></span><span class="lab">${p}%</span><kbd class="k">${p === 100 ? '1' : ''}</kbd></button>`).join(''));
      el.classList.add('narrow');
      el.onclick = e => {
        const b = e.target.closest('.tb-item'); if (!b) return;
        Pop.close();
        if (b.dataset.p === 'fit') fitView(); else zoomPct(+b.dataset.p);
      };
    });
  })();

  /* ---------- Settings (gear): Preferences ---------- */
  const Settings = (() => {
    const btn = $('#settingsBtn');
    function open() {
      const el = Pop.open(btn, `
        <div class="set-head">Preferences</div>
        <label class="toggle"><input type="checkbox" data-k="dock"><span class="tg"></span>Show tool dock</label>
        <div class="set-note">Tools stay in the Image menu and the search when the dock is off.</div>
        <label class="toggle"><input type="checkbox" data-k="advance"><span class="tg"></span>Next image after process</label>
        <label class="toggle"><input type="checkbox" data-k="remember"><span class="tg"></span>Remember last session</label>
        <div class="set-note">Off: the next start is blank, with no tools or filters open.</div>`, { align: 'right' });
      el.classList.add('settings');
      el.setAttribute('role', 'dialog'); el.setAttribute('aria-label', 'Settings');
      sync();
      el.onchange = e => {
        const k = e.target.dataset.k;
        if (k === 'dock') setDock(e.target.checked);
        if (k === 'advance') setAdvance(e.target.checked);
        if (k === 'remember') { S.prefs.remember = e.target.checked; savePrefsSoon(); }
      };
    }
    function sync() {
      if (Pop.owner() !== btn) return;
      const d = Pop.el.querySelector('[data-k="dock"]'), a = Pop.el.querySelector('[data-k="advance"]');
      if (d) d.checked = S.prefs.dock !== false;
      if (a) a.checked = !!S.prefs.advance;
      const r = Pop.el.querySelector('[data-k="remember"]');
      if (r) r.checked = S.prefs.remember !== false;
    }
    btn.addEventListener('pointerdown', e => {
      e.stopPropagation(); e.preventDefault();
      if (Pop.owner() === btn) Pop.close(); else open();
    });
    return { open, sync };
  })();

  /* ---------- the search (loupe, Ctrl+Space): tools, edit presets, filters and every menu command ---------- */
  const Palette = (() => {
    const wrap = document.createElement('div');
    wrap.id = 'cmdPal';
    wrap.innerHTML = `
      <div class="cpal-veil"></div>
      <div class="cpal-box" role="dialog" aria-label="Search">
        <div class="cpal-head">
          <svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/></svg>
          <input class="cpal-in" placeholder="Search tools, presets, commands" spellcheck="false" autocomplete="off" aria-label="Search">
          <kbd>Esc</kbd>
        </div>
        <div class="cpal-list" role="listbox"></div>
        <div class="cpal-foot"><span>↑↓ choose</span><span>Enter run</span><span class="grow"></span><span class="mono">Ctrl+Space</span></div>
      </div>`;
    document.body.appendChild(wrap);
    const inp = wrap.querySelector('.cpal-in'), list = wrap.querySelector('.cpal-list'), btn = $('#searchBtn');
    let rows = [], sel = 0;
    const isOpen = () => Caustics.shown(wrap);

    function entries() {
      const out = [];
      for (const c of CMDS) if (c.id !== 'search') out.push({ group: c.tool ? 'Tools' : 'Commands', label: c.label, sub: c.tool ? '' : c.menu, key: c.key || '', c });
      for (const f of FILTERS) out.push({ group: 'Filters', label: f.name || f.sec, sub: 'Filter', key: '',
        c: { run: () => FilterPop.add(f.sec), en: () => !S.cropMode } });
      try {
        for (const p of ChainPresets.list()) out.push({ group: 'Edit presets', label: p.name, sub: 'Edit preset', key: '',
          c: { run: () => ChainPresets.apply(p), en: hasFile } });
      } catch (_) {}
      return out;
    }
    function render() {
      const q = inp.value.trim().toLowerCase();
      const all = entries();
      let hits = all;
      if (q) {
        hits = all.map(e => {
          const t = (e.label + ' ' + e.sub).toLowerCase(), l = e.label.toLowerCase();
          const score = l.startsWith(q) ? 0 : l.split(/[\s&/-]+/).some(w => w.startsWith(q)) ? 1 : t.includes(q) ? 2 : -1;
          return { e, score };
        }).filter(x => x.score >= 0).sort((a, b) => a.score - b.score).map(x => x.e);
      }
      const order = ['Tools', 'Commands', 'Filters', 'Edit presets'];
      rows = [];
      let html = '';
      for (const g of order) {
        const gs = hits.filter(e => e.group === g);
        if (!gs.length) continue;
        html += `<div class="cpal-group">${g}</div>`;
        for (const e of gs) {
          const i = rows.length; rows.push(e);
          html += `<button class="cpal-row" role="option" data-i="${i}"${can(e.c) ? '' : ' disabled'}>`
            + `<span class="lab">${esc(e.label)}${e.sub ? ` <span class="sub">${esc(e.sub)}</span>` : ''}</span><kbd class="k">${esc(e.key)}</kbd></button>`;
        }
      }
      list.innerHTML = html || '<div class="cpal-empty">Nothing matches</div>';
      sel = rows.findIndex(e => can(e.c));
      mark();
    }
    function mark() {
      list.querySelectorAll('.cpal-row').forEach(r => r.classList.toggle('sel', +r.dataset.i === sel));
      const r = list.querySelector('.cpal-row.sel');
      if (r) r.scrollIntoView({ block: 'nearest' });
    }
    function move(d) {
      if (!rows.length) return;
      for (let n = 0, i = sel; n < rows.length; n++) {
        i = (i + d + rows.length) % rows.length;
        if (can(rows[i].c)) { sel = i; break; }
      }
      mark();
    }
    function open() {
      if (S.cropMode) return;
      Pop.close();
      if (typeof FilterPop !== 'undefined' && FilterPop.isOpen()) FilterPop.close();
      BatchApply.close();
      Caustics.show(wrap); btn.classList.add('on');
      inp.value = ''; render();
      inp.focus();
    }
    function close() { if (!isOpen()) return; Caustics.hide(wrap); btn.classList.remove('on'); inp.blur(); }
    function choose(i) { const e = rows[i]; if (!e || !can(e.c)) return; close(); run(e.c); }
    inp.addEventListener('input', render);
    inp.addEventListener('keydown', e => {
      e.stopPropagation();
      if (e.key === 'ArrowDown') { e.preventDefault(); move(1); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); move(-1); }
      else if (e.key === 'Enter') { e.preventDefault(); choose(sel); }
      else if (e.key === 'Escape' || (e.code === 'Space' && (e.ctrlKey || e.metaKey))) { e.preventDefault(); close(); }
    });
    list.addEventListener('pointermove', e => {
      const r = e.target.closest('.cpal-row'); if (!r || r.disabled || +r.dataset.i === sel) return;
      sel = +r.dataset.i; mark();
    });
    list.addEventListener('click', e => { const r = e.target.closest('.cpal-row'); if (r) choose(+r.dataset.i); });
    wrap.addEventListener('pointerdown', e => {
      e.stopPropagation();
      if (e.target.classList.contains('cpal-veil')) close();
      else if (e.target !== inp) e.preventDefault();          // keep typing in the field
    });
    btn.addEventListener('pointerdown', e => { e.stopPropagation(); e.preventDefault(); });
    btn.addEventListener('click', () => (isOpen() ? close() : open()));
    return { open, close, isOpen };
  })();

  /* ---------- Help › Keyboard shortcuts: the commands' keys plus the ones without a command ---------- */
  const EXTRA_KEYS = [
    ['B / G / W', 'Black / gray / white eyedropper'], ['Esc', 'Drop the eyedropper, close a popover'],
    ['Alt (hold)', 'Clipping / threshold view'], ['\\ (hold)', 'Before view'],
    ['Space + drag', 'Pan'], ['Enter', 'Process current image'], ['PgUp / PgDn', 'Previous / next image'],
  ];
  function showKeys() {
    const rowsHtml = CMDS.filter(c => c.key).map(c => [c.key, c.label.replace('…', '')]).concat(EXTRA_KEYS)
      .map(([k, l]) => `<div class="keys-row"><span>${esc(l)}</span><kbd>${esc(k)}</kbd></div>`).join('');
    const el = Pop.open($('#menuBar').querySelector('[data-m="Help"]'), `<div class="set-head">Keyboard shortcuts</div><div class="keys">${rowsHtml}</div>`);
    el.classList.add('keys-pop');
    el.setAttribute('role', 'dialog'); el.setAttribute('aria-label', 'Keyboard shortcuts');
  }

  /* ---------- global keys: Ctrl+Space search, Ctrl+, settings; Esc closes a dropdown ---------- */
  window.addEventListener('keydown', e => {
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && e.code === 'Space') {
      e.preventDefault(); e.stopPropagation();
      if (!S.cropMode) Palette.isOpen() ? Palette.close() : Palette.open();
      return;
    }
    if (ctrl && e.key === ',') { e.preventDefault(); e.stopPropagation(); if (!S.cropMode) Settings.open(); return; }
    if (e.key === 'Escape' && Pop.owner()) { e.preventDefault(); e.stopPropagation(); Pop.close(); }
  }, true);

  return { CMDS, run, syncDock, Palette, Settings, closePop: () => Pop.close() };
})();
