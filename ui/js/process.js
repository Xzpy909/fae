'use strict';
/* Process, toast / status and keyboard shortcuts. */

/* ======================================================================
   Process
   ====================================================================== */
function jobFor(f, isCur) {
  const st = isCur ? { stage: S.stage, edit: S.edit, geom: S.geom }
    : (f.state || { stage: S.stage, edit: defaultEdit(), geom: defaultGeom() });
  const E = computeEdit(effectiveEdit(st.edit));
  const edit = Object.assign(jobEdit(E), { sharp: E.sharp });
  // a non-default chain (order, or a tool more than once): the steps in order, each with its own
  // instance's tables, that tool's fields only (fastedit/core.py _steps)
  if (E.steps) edit.steps = E.steps.map(s => {
    if (s.t === 'fx') return { t: 'fx', fx: s.fx };          // plain values already (fxParams)
    const o = { t: s.t }, je = s === E ? edit : jobEdit(s);
    for (const k of STEP_FIELDS[s.t]) o[k] = je[k];
    return o;
  });
  return { path: f.path, stage: stageParams(st.stage, st.geom), edit };
}
/* computeEdit's tables as JSON-able values (typed arrays -> plain arrays) */
function jobEdit(E) {
  return { point: E.point, k: E.k, o: E.o, gains: E.gains, luts: E.luts.map(a => Array.from(a)),
    boost: E.boost ? Array.from(E.boost) : null,
    tones: E.tones ? E.tones.map(t => ({ T: Array.from(t.T), ratio: t.ratio })) : null, sat: E.sat, wb: E.wb,
    con: E.con ? Array.from(E.con) : null, md: E.md || 0, bw: E.bw,
    film: E.film ? { n: E.film.n, d: E.film.d ? Array.from(E.film.d) : null, t1: E.film.t1 ? Array.from(E.film.t1) : null, op: E.film.op } : null,
    sc: E.sc ? Object.assign({}, E.sc, { shl: E.sc.shl ? Array.from(E.sc.shl) : null }) : null };
}

/* Process ran on the CPU core (no GPU): the filter kit's filters it left out (r.skipped = ids) */
function skippedNote(res) {
  const ids = [...new Set(res.flatMap(r => (r && r.skipped) || []))];
  if (!ids.length) return '';
  const names = ids.map(id => ((typeof FX_DEFS !== 'undefined' && FX_DEFS.find(d => d.id === id)) || { name: id }).name);
  return `<br><span style="color:var(--red)">${names.join(', ')} skipped: needs the GPU</span>`;
}

function setBusy(on, label) {
  S.busy = on;
  $('#processBtn').disabled = on; if (!batchOn) $('#processAllBtn').disabled = on;
  $('#processBtn').firstChild.textContent = on ? (label || 'Processing…') : 'Process';
}

async function processCurrent() {
  if (S.busy || S.cur < 0 || !S.img) return;
  if (S.cropMode && (typeof Crop !== "undefined")) { Crop.exit(false); }
  commit();
  const idx = S.cur, f = S.files[idx];
  setBusy(true);
  let res;
  try { res = await api().process([jobFor(f, true)], S.prefs.fmt, encOpts()); }
  catch (e) { res = [{ ok: false, error: String(e) }]; }
  setBusy(false);
  const r = res && res[0];
  if (r && r.ok) {
    f.done = true; f.out = r.out;
    renderList();
    toast(`Saved <b>${basename(r.out)}</b>` + skippedNote([r]), false, r.out);
    if (S.prefs.advance && idx === S.cur && idx < S.files.length - 1) switchTo(idx + 1);
  } else toast('Process failed: ' + (r ? r.error : 'unknown'), true);
}

/* Process all: while it runs, its button cancels it (the image being written finishes, or its external
   encoder is stopped; the rest are left as they were) */
let batchOn = false;
function setBatch(on) {
  batchOn = on;
  const b = $('#processAllBtn');
  b.disabled = false;
  b.classList.toggle('cancel', on);
  b.textContent = on ? 'Cancel' : 'Process all';
  b.title = on ? 'Stop Process all: the images not written yet are left as they are'
    : 'Each image with its own edits; untouched images get the current Output / Color settings';
}
function cancelAll() {
  if (!batchOn) return;
  const b = $('#processAllBtn');
  b.disabled = true; b.textContent = 'Cancelling…';
  try { api().process_cancel(); } catch (_) {}
}

async function processAll() {
  if (batchOn) { cancelAll(); return; }
  if (S.busy || !S.files.length) return;
  storeCurrent();
  const jobs = S.files.map((f, i) => jobFor(f, i === S.cur));
  setBusy(true, `Processing 0 / ${jobs.length}`);
  setBatch(true);
  const bar = $('#progress');
  bar.classList.add('show'); bar.firstElementChild.style.width = '0%';
  let res;
  try { res = await api().process(jobs, S.prefs.fmt, encOpts()); }
  catch (e) { res = []; toast(String(e), true); }
  setBatch(false);
  setBusy(false);
  setTimeout(() => bar.classList.remove('show'), 800);
  const ok = res.filter(r => r.ok).length, stopped = res.filter(r => r.cancelled).length, bad = res.length - ok - stopped;
  res.forEach(r => { const f = S.files.find(x => x.path === r.path); if (f && r.ok) { f.done = true; f.out = r.out; } });
  renderList();
  toast((ok && !stopped ? '<img class="emo" src="emoji/party-popper.webp" alt="🎉">' : '') + `Processed ${ok} image(s)`
    + (stopped ? `, cancelled: ${stopped} not processed` : '')
    + (bad ? `, <span style="color:var(--red)">${bad} failed</span>` : '') + skippedNote(res), bad > 0,
    ok ? res.find(r => r.ok).out : null);
}
Bridge.on('processProgress', (i, n) => {
  $('#progress').firstElementChild.style.width = (i / n * 100) + '%';
  $('#processBtn').firstChild.textContent = `Processing ${i} / ${n}`;
});
$('#processBtn').addEventListener('click', processCurrent);
$('#processAllBtn').addEventListener('click', processAll);

/* ======================================================================
   Toast / status
   ====================================================================== */
let toastTimer = null;
function toast(html, err, revealPath) {
  const t = $('#toast');
  t.innerHTML = html + (revealPath ? `<a id="toastReveal">Show in folder</a>` : '');
  t.classList.toggle('err', !!err);
  t.classList.add('show');
  if (revealPath) $('#toastReveal').onclick = () => api().reveal(revealPath);
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove('show'), err ? 6000 : 3500);
}
let statusTimer = null;
function status(msg) {
  $('#stMsg').textContent = msg;
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => { $('#stMsg').textContent = ''; }, 4000);
}

/* ======================================================================
   Keyboard
   ====================================================================== */
window.addEventListener('keydown', e => {
  if (S.cropMode && (typeof Crop !== "undefined") && Crop.onKey(e)) return;
  if (e.key === 'Alt') { e.preventDefault(); setAlt(true); return; }
  const inInput = /^(INPUT|SELECT|TEXTAREA)$/.test(e.target.tagName) && e.target.type !== 'range' && e.target.type !== 'checkbox';
  const ctrl = e.ctrlKey || e.metaKey, k = e.key.toLowerCase();
  if (ctrl && k === 's') { e.preventDefault(); processCurrent(); return; }
  if (ctrl && k === 'o') { e.preventDefault(); pickFiles(); return; }
  if (inInput) return;
  if (ctrl && e.shiftKey && k === 'c') { e.preventDefault(); copyEdits(); return; }
  if (ctrl && e.shiftKey && k === 'v') { e.preventDefault(); pasteEdits(); return; }
  if (ctrl && k === 'v') { e.preventDefault(); pasteClipboard(); return; }
  if (ctrl && (k === 'y' || (k === 'z' && e.shiftKey))) { e.preventDefault(); redo(); return; }
  if (ctrl && k === 'z') { e.preventDefault(); undo(); return; }
  if (ctrl) return;
  if (e.altKey) e.preventDefault();
  switch (k) {
    case 'b': toggleTool('black'); break;
    case 'g': toggleTool('gray'); break;
    case 'w': toggleTool('white'); break;
    case 'escape': if (FilterPop.isOpen()) FilterPop.close(); else if (EncPop.isOpen()) EncPop.close(); else setTool(null); break;
    case 'c': if ((typeof Crop !== "undefined") && S.img) Crop.enter(); break;
    case '0': fitView(); break;
    case '1': zoom100(); break;
    case '+': case '=': zoomTo(S.view.z * 1.25); break;
    case '-': zoomTo(S.view.z / 1.25); break;
    case ' ': e.preventDefault(); if (!S.space) { S.space = true; vp.classList.add('pan-ready'); } break;
    case '\\': if (!S.before) { S.before = true; requestRender(); } break;
    case 'arrowdown': case 'pagedown': e.preventDefault(); switchTo(S.cur + 1); break;
    case 'arrowup': case 'pageup': e.preventDefault(); switchTo(S.cur - 1); break;
    case 'enter': processCurrent(); break;
  }
});
window.addEventListener('keyup', e => {
  if (e.key === 'Alt') { e.preventDefault(); setAlt(false); return; }
  if (e.key === ' ') { S.space = false; vp.classList.remove('pan-ready'); }
  if (e.key === '\\') { S.before = false; requestRender(); }
});
window.addEventListener('blur', () => { setAlt(false); S.space = false; S.before = false; vp.classList.remove('pan-ready'); requestRender(); });
