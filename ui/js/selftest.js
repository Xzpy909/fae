/* Self-test page (selftest.html): cross-origin isolation, WASM features, the WASM build of the
   Rust core and the Python extension. Each check adds one row; the text report can be copied. */

const rows = document.getElementById('rows'), report = document.getElementById('report');
const lines = [];

function row(name, state, text) {
  const tr = document.createElement('tr');
  tr.innerHTML = `<td class="k"></td><td class="${state}"></td>`;
  tr.children[0].textContent = name;
  tr.children[1].textContent = ({ ok: '✓ ', bad: '✗ ', warn: '! ' }[state] || '') + text;
  rows.appendChild(tr);
  lines.push(`${({ ok: 'OK  ', bad: 'FAIL', warn: 'WARN' }[state] || 'INFO')}  ${name}: ${text}`);
  report.value = lines.join('\n');
}

async function check(name, fn) {
  try {
    const r = await fn();
    row(name, r[0], r[1]);
  } catch (e) {
    row(name, 'bad', String(e && e.message || e));
  }
}

const ms = t => `${t.toFixed(1)} ms`;
function bestOf(n, f) {
  let best = Infinity;
  for (let i = 0; i < n; i++) { const t = performance.now(); f(); best = Math.min(best, performance.now() - t); }
  return best;
}

function randomBytes(n, seed) {
  const a = new Uint8Array(n);
  let x = seed >>> 0;
  for (let i = 0; i < n; i++) { x ^= x << 13; x ^= x >>> 17; x ^= x << 5; a[i] = x & 255; }
  return a;
}

function jsLuts(buf, L) {           // reference: the same loop as the end of processPixels
  for (let j = 0; j < buf.length; j += 4) {
    buf[j] = L[buf[j]]; buf[j + 1] = L[256 + buf[j + 1]]; buf[j + 2] = L[512 + buf[j + 2]];
  }
}

async function main() {
  row('User agent', 'info', navigator.userAgent);
  row('CPU threads', 'info', String(navigator.hardwareConcurrency || '?'));

  await check('Cross-origin isolated', () => self.crossOriginIsolated
    ? ['ok', 'yes (SharedArrayBuffer allowed)']
    : ['bad', 'no: headers missing or FASTEDIT_COI=0; the WASM preview will be single-threaded']);

  await check('SharedArrayBuffer', () => typeof SharedArrayBuffer === 'function'
    ? ['ok', 'available'] : ['bad', 'not available']);

  await check('WASM shared memory', () => {
    const m = new WebAssembly.Memory({ initial: 1, maximum: 2, shared: true });
    return m.buffer instanceof SharedArrayBuffer ? ['ok', 'works (needed for WASM threads)'] : ['bad', 'buffer is not shared'];
  });

  await check('WASM SIMD', () => {
    // (module (func (result v128) i32.const 0 i8x16.splat)) - validates only with SIMD support
    const simd = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 8, 1, 6, 0, 65, 0, 253, 15, 11]);
    return WebAssembly.validate(simd) ? ['ok', 'supported'] : ['bad', 'not supported (rebuild without +simd128)'];
  });

  await check('Worker + shared memory', () => new Promise((resolve) => {
    if (typeof SharedArrayBuffer !== 'function') return resolve(['bad', 'skipped: no SharedArrayBuffer']);
    const src = 'onmessage = e => { const a = new Int32Array(e.data); Atomics.store(a, 0, 42); Atomics.notify(a, 0); postMessage(1); }';
    const w = new Worker(URL.createObjectURL(new Blob([src], { type: 'text/javascript' })));
    const sab = new SharedArrayBuffer(4);
    const timer = setTimeout(() => { w.terminate(); resolve(['bad', 'worker did not answer']); }, 3000);
    w.onmessage = () => {
      clearTimeout(timer); w.terminate();
      resolve(new Int32Array(sab)[0] === 42 ? ['ok', 'a worker wrote into shared memory'] : ['bad', 'value not seen']);
    };
    w.onerror = e => { clearTimeout(timer); resolve(['bad', 'worker error: ' + e.message]); };
    w.postMessage(sab);
  }));

  let wasm = null;
  await check('WASM core loads', async () => {
    try { wasm = await import('../wasm/fastedit_wasm.js'); }
    catch (e) { return ['bad', 'ui/wasm/fastedit_wasm.js missing: run core\\build_core.bat']; }
    await wasm.default();      // fetches fastedit_wasm_bg.wasm (application/wasm)
    return ['ok', 'version ' + wasm.version()];
  });

  if (wasm) {
    await check('WASM core = JS', () => {
      const W = 1031, H = 683, L = randomBytes(768, 11);
      const a = randomBytes(W * H * 4, 5), b = a.slice();
      jsLuts(a, L); wasm.apply_luts_rgba(b, W, L);
      let diff = 0;
      for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) diff++;
      return diff ? ['bad', `${diff} bytes differ`] : ['ok', `identical on ${W}x${H}`];
    });
    await check('Curves speed 2560x1440', () => {
      const W = 2560, H = 1440, L = randomBytes(768, 3), a = randomBytes(W * H * 4, 9);
      const tj = bestOf(5, () => jsLuts(a, L)), tw = bestOf(5, () => wasm.apply_luts_rgba(a, W, L));
      return ['info', `JS ${ms(tj)}, WASM ${ms(tw)} (single thread; WASM includes copying the image in and out)`];
    });
  }

  // --- the live preview worker (what the app uses): threads, same result as JS, speed ---
  let pw = null;
  await check('Preview worker', () => new Promise((resolve) => {
    if (typeof computeEdit !== 'function') return resolve(['bad', 'UI edit math did not load']);
    pw = new Worker('js/preview-worker.js', { type: 'module' });
    const timer = setTimeout(() => resolve(['bad', 'no answer from the worker']), 15000);
    pw.addEventListener('message', function onReady({ data: m }) {
      if (m.type !== 'ready' && m.type !== 'error') return;
      clearTimeout(timer); pw.removeEventListener('message', onReady);
      if (m.type === 'error') { pw = null; resolve(['bad', m.error]); }
      else resolve([m.threads > 1 ? 'ok' : 'warn', `core ${m.version}, ${m.threads} threads`]);
    });
    pw.onerror = e => { clearTimeout(timer); pw = null; resolve(['bad', 'worker error: ' + e.message]); };
  }));

  if (pw) {
    let seq = 0, nextFilm = 1;
    const filmIds = new Map();          // table -> id, as wasm-preview.js does
    const ask = (E, W, H, base) => new Promise((resolve, reject) => {
      const s = ++seq, e = Object.assign({}, E);
      if (E.film && E.film.d) {
        let id = filmIds.get(E.film.d);
        if (!id) { id = nextFilm++; filmIds.set(E.film.d, id); pw.postMessage({ type: 'film', id, d: E.film.d }); }
        e.film = Object.assign({}, E.film, { d: id });
      }
      pw.addEventListener('message', function on({ data: m }) {
        if (m.seq !== s) return;
        pw.removeEventListener('message', on);
        m.type === 'done' ? resolve(m) : reject(new Error(m.error));
      });
      if (base) { const b = base.slice().buffer; pw.postMessage({ type: 'image', w: W, h: H, base: b }, [b]); }
      pw.postMessage({ type: 'render', seq: s, E: e });
    });
    const image = (W, H) => {           // smooth gradient + noise
      const a = new Uint8ClampedArray(W * H * 4);
      let x = 12345;
      for (let y = 0, j = 0; y < H; y++) for (let i = 0; i < W; i++, j += 4) {
        x ^= x << 13; x ^= x >>> 17; x ^= x << 5;
        const nz = (x & 15) - 8, v = 1 - y / H;
        a[j] = 255 * v * (i / W) + nz; a[j + 1] = 255 * v * (1 - i / W) + nz; a[j + 2] = 200 * v + nz; a[j + 3] = 255;
      }
      return a;
    };
    const edits = {
      'Saturation + Contrast': e => { e.adj_sat = 75; e.adj_contrast = 1.3; },
      'Film': e => { e.flt_film_preset = 'k_64'; e.flt_film_bright = 30; e.flt_film_opacity = 80; },
      'Perceptual Saturation': e => { e.flt_bw_psat = 45; e.flt_bw_warmth = 20; e.flt_bw_opacity = 70; },
      'Super Contrast (all)': e => { e.flt_sc_cast = 60; e.flt_sc_contrast = 40; e.flt_sc_dynamic = 50; e.flt_sc_opacity = 90; },
      'Mid/Detail': e => { e.adj_middetail = 60; },
      'Everything': e => { e.adj_middetail = -40; e.adj_sat = 60; e.adj_temp = 800; e.adj_shadows = 30; e.adj_boost = 40; e.flt_bw_psat = 30;
        e.flt_film_preset = 'f_velvia_100'; e.flt_sc_dynamic = 40; e.curves.m = [[0, 10], [128, 140], [255, 245]]; },
    };
    const editOf = f => { const e = defaultEdit(); f(e); return computeEdit(e); };
    const hk = ['hin', 'hout', 'hmx', 'hmn'];

    await check('Preview worker = single thread', async () => {
      // the threaded worker against the same core single-threaded here on the page
      if (!wasm) return ['bad', 'skipped: the WASM core did not load on the page'];
      const { toParams } = await import('./wasm-params.js');
      const W = 1280, H = 720, base = image(W, H), bad = [];
      const pv = new wasm.Preview(W, H, new Uint8Array(base.buffer));
      let first = true;
      for (const [name, f] of Object.entries(edits)) {
        const E = editOf(f);
        const m = await ask(E, W, H, first ? base : null); first = false;
        const p = toParams(wasm, E);
        pv.render(p); p.free();
        const ref = pv.output(), o = new Uint8Array(m.out);
        let px = 0;
        for (let i = 0; i < o.length; i++) if (o[i] !== ref[i]) px++;
        const hd = hk.filter(k => { const a = pv[k](); return m[k].some((v, i) => v !== a[i]); });
        if (px || hd.length) bad.push(`${name}: ${px} bytes${hd.length ? ', histograms ' + hd.join('/') : ''}`);
      }
      pv.free();
      return bad.length ? ['bad', bad.join('; ')] : ['ok', `${Object.keys(edits).length} edits identical at ${W}x${H} (pixels and histograms)`];
    });

    for (const [W, H] of [[2560, 1440], [7360, 4912]]) {
      await check(`Preview speed ${W}x${H}`, async () => {
        const base = image(W, H), parts = [];
        let first = true;
        for (const [name, f] of Object.entries(edits)) {
          const E = editOf(f);
          await ask(E, W, H, first ? base : null); first = false;       // warm-up (and image upload)
          let tw = Infinity, tc = Infinity;
          for (let r = 0; r < 3; r++) {
            const t = performance.now(); const m = await ask(E, W, H, null);
            tw = Math.min(tw, performance.now() - t); tc = Math.min(tc, m.ms);
          }
          parts.push(`${name}: ${tw.toFixed(0)} ms (core ${tc.toFixed(0)})`);
        }
        return ['info', parts.join(' | ')];
      });
    }
    pw.terminate();
  }

  await check('Python core', () => new Promise((resolve) => {
    const run = async () => {
      try {
        const r = await window.pywebview.api.core_info();
        if (r && r.ok) resolve(['ok', `fastedit_core ${r.version}, ${r.threads} threads, loads and runs`]);
        else resolve(['bad', (r && r.error) || 'not available: run core\\build_core.bat']);
      } catch (e) { resolve(['bad', String(e)]); }
    };
    if (window.pywebview && window.pywebview.api) run();
    else {
      window.addEventListener('pywebviewready', run, { once: true });
      setTimeout(() => resolve(['warn', 'no pywebview API (page opened outside the app?)']), 5000);
    }
  }));
}

document.getElementById('copy').onclick = () => { report.select(); document.execCommand('copy'); };
main();
