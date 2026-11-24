'use strict';
/* GPU preview (GPU core plan, waves G1-G6): the GPU core (ui/gpu/, WebGPU) renders the edit on the
   full-resolution stage (stage.js asks for it at GV.maxPixels()), and a WebGPU canvas over the
   viewport shows it at exactly the screen's pixels (Lanczos-3; nearest from 2 screen px per image
   px, like the pixel view) on every edit, pan and zoom. The original | edited split and the before
   view are drawn there too; the Alt views, the encoded compare and crop mode are drawn on the 2D
   canvas from the GPU's pixels (show(I, E, false): the GPU computes, its canvas stays hidden).
   The GPU also counts the histograms (every frame, so they follow a dragged slider) and, once the
   edit settles, hands back the pixels (S.outData: RGB readout, loupe, the Alt views on the 2D
   canvas). The CPU core isn't used for the live image at all. Off with ?gpu=0 (fae.py --no-gpu);
   without WebGPU or ui/gpu/ the CPU core draws a PREVIEW_MAX copy (wasm-preview.js). */
const GV = (() => {
  const params = new URLSearchParams(location.search);
  const cv = document.createElement('canvas');
  cv.id = 'gpu';
  stageEl.after(cv);                       // over the image, under markers / split line

  let g = null, ready = false, failed = false, why = '', info = null;
  let img = null, edit = null, editImg = null, on = false, queued = false, checking = false;
  const plans = new WeakMap(), blockers = new WeakMap();
  const TOOL_NAMES = { md: 'Mid/Detail', ps: 'Perceptual Saturation ≈', sc: 'Super Contrast', sharp: 'Sharpen',
    point: 'Point correction', adjust: 'Adjustments', bw: 'Perceptual Saturation', film: 'Film Effect' };

  const ORDER = ['point', 'adjust', 'curves', 'bw', 'film', 'sc'];
  const isIdentity = L => L.every(l => { for (let i = 0; i < 256; i++) if (l[i] !== i) return false; return true; });

  /* the edit as GPU steps ({t, args} for g.step_<t>(...args), in chain order), or null when a step
     needs something the GPU doesn't run yet (blockers says which). The same reading of E as
     wasm-params.js / fastedit/core.py _steps: a missing / null table means the step is off. */
  const f64s = new WeakMap();
  const f64 = a => {
    if (a == null) return undefined;
    if (a instanceof Float64Array) return a;
    let v = f64s.get(a);
    if (!v) { v = Float64Array.from(a); f64s.set(a, v); }      // Film tables: converted once
    return v;
  };
  function plan(E) {
    if (plans.has(E)) return plans.get(E);
    let out = [], blocker = null;
    const list = E.steps || ORDER.map(t => ({ t, e: E }));
    for (const x of out ? list : []) {
      const s = x.e || x;
      switch (x.t) {
        case 'point':
          if (s.point) out.push({ t: 'point', args: [s.k, s.o, s.gains ? Float64Array.from(s.gains) : undefined] });
          break;
        case 'adjust': {
          const tones = s.tones || [];
          if (!s.md && s.wb == null && s.con == null && !tones.length && s.boost == null && s.sat == null) break;
          const T = new Float64Array(tones.length * 4096), R = new Uint8Array(tones.length);
          tones.forEach((t, i) => { T.set(t.T, i * 4096); R[i] = t.ratio ? 1 : 0; });
          out.push({ t: 'adjust', args: [s.md || 0, s.wb != null ? Float64Array.from([...s.wb[0], ...s.wb[1], ...s.wb[2]]) : undefined,
            f64(s.con), T, R, f64(s.boost), s.sat != null ? s.sat : undefined] });
          break;
        }
        case 'curves': {
          const L = s.luts;
          if (L && L.length === 3 && !isIdentity(L)) {
            const f = new Uint8Array(768);
            f.set(L[0], 0); f.set(L[1], 256); f.set(L[2], 512);
            out.push({ t: 'curves', args: [f] });
          }
          break;
        }
        case 'bw': {
          const BW = s.bw;
          if (!BW) break;
          out.push({ t: 'bw', args: [BW.s, BW.op, BW.w ? Float64Array.from(BW.w) : undefined, BW.ps || 0,
            BW.ps ? f64(BW.psk) : undefined] });
          break;
        }
        case 'film': {
          const FM = s.film;
          if (FM) out.push({ t: 'film', args: [FM.n, f64(FM.d), f64(FM.t1), FM.op ?? 1] });
          break;
        }
        case 'sc': {
          const SC = s.sc;
          if (SC) out.push({ t: 'sc', args: [SC.cast, SC.cc, SC.dyn, SC.op, SC.dyn > 0 ? f64(SC.dk) : undefined, f64(SC.shl)] });
          break;
        }
        default: out = null; blocker = x.t;
      }
      if (!out) break;
    }
    // Sharpen last, as the CPU core runs it; show() adds the image's scale
    const SH = E.sharp;
    if (out && SH) out.push({ t: 'sharp', args: [SH.as || 0, SH.bs || 0, SH.st || 0, SH.lc || 0, SH.fo || 0], scaled: true });
    plans.set(E, out);
    if (blocker) blockers.set(E, blocker);
    return out;
  }

  // preview pixels per output pixel (wasm-preview.js outScale for the live image)
  const outScale = I => (I.w ? I.pw / I.w : 1);

  function fail(reason) {
    if (failed) return;
    failed = true; ready = false; why = reason; g = null; img = null; edit = null; editImg = null;
    hide();
    console.warn('GPU preview off: ' + reason);
    // the stage was loaded at full resolution for the GPU: the CPU core gets its small copy instead
    if (S.img && S.img.full) requestStage(true);
    else if (typeof requestRender === 'function') requestRender();
  }

  async function start() {
    if (params.get('gpu') === '0') { failed = true; why = 'turned off (--no-gpu)'; return; }
    if (!('gpu' in navigator)) { failed = true; why = 'WebGPU not available'; return; }
    try {
      const mod = await import('../gpu/fae_gpu_wasm.js');
      await mod.default();
      Boot.mark('gpu wasm');
      cv.width = cv.height = 1;
      g = await mod.create(cv);
      Boot.mark('gpu device');
      // compile every shader now, behind the loading screen, one at a time (the browser's GPU process
      // compiles them and draws nothing meanwhile; waiting for each lets the screen update in between)
      const n = g.warm_count ? g.warm_count() : 0;                  // (an older ui/gpu build: compile on first use)
      const took = [];
      for (let i = 0; i < n; i++) {
        const t0 = performance.now();
        g.warm(i);
        await g.done();
        took.push([Math.round(performance.now() - t0), g.warm_name ? g.warm_name(i) : String(i)]);
        Boot.progress(i + 1, n);
        await new Promise(requestAnimationFrame);
      }
      Boot.mark('gpu shaders');
      if (took.length) Boot.note('slowest shaders: ' + took.sort((a, b) => b[0] - a[0]).slice(0, 4).map(([ms, nm]) => `${nm} ${ms}`).join(', ') + ' ms');
      info = JSON.parse(g.info());
      info.version = mod.version();
      console.info(`GPU preview: fae-gpu ${info.version} on ${info.name || 'WebGPU adapter'} (${info.backend})`);
      ready = true;
      if (typeof requestRender === 'function') requestRender();     // also updates the status bar's engine
    } catch (e) {
      fail(String((e && e.message) || e));
    }
  }

  // the image's screen geometry, in the canvas's device pixels (the same snapping as applyView)
  function draw() {
    queued = false;
    if (!on || !g) return;
    const I = S.img, v = S.view, d = dpr(), r = vp.getBoundingClientRect();
    if (!I || !r.width || !r.height) return;
    const L = Math.round(r.left * d), T = Math.round(r.top * d);
    const cw = Math.round((r.left + r.width) * d) - L, ch = Math.round((r.top + r.height) * d) - T;
    if (cv.width !== cw || cv.height !== ch) { cv.width = cw; cv.height = ch; }
    cv.style.width = cw / d + 'px'; cv.style.height = ch / d + 'px';
    cv.style.left = (L / d - r.left) + 'px'; cv.style.top = (T / d - r.top) + 'px';
    const ox = Math.round((r.left + v.x) * d) - L, oy = Math.round((r.top + v.y) * d) - T;
    const split = viewMode() === 'before' ? 1e9
      : S.prefs.split && !encSplitOn() ? clamp(Math.round(S.cmp.x * I.pw), 0, I.pw) : 0;
    try {
      g.draw(cw, ch, ox, oy, v.z * d, split, 2);
    } catch (e) { fail(String((e && e.message) || e)); return; }
    if (!checking) {                     // WebGPU reports errors asynchronously
      checking = true;
      g.done().then(() => {
        checking = false;
        const e = g && g.errors();
        if (e) fail(e);
      });
    }
  }

  let busy = false, pending = null;
  function pump() {
    const job = pending;
    pending = null;
    if (!job || !g || img !== job.I || edit !== job.E) return;      // not what the GPU holds now
    busy = true;
    let req;
    try { req = Promise.all([g.stats(), job.pixels ? g.readback_rgba() : null]); }
    catch (e) { busy = false; fail(String((e && e.message) || e)); return; }
    req.then(([hist, px]) => {
      busy = false;
      job.done({ I: job.I, E: job.E, hist, px });
      if (pending) pump();
    }, e => { busy = false; fail(String((e && e.message) || e)); });
  }

  function hide() {
    on = false;
    cv.classList.remove('show');
  }

  const started = start();
  return {
    ready: () => ready,
    on: () => on,
    // resolves once the GPU is ready or known to be unavailable (stage.js waits for it)
    started,
    // the stage's pixels for the preview: the GPU's limit when it draws, 0 = the CPU core's small copy
    maxPixels: () => (ready && info ? info.max_pixels : 0),
    // can the GPU render this image and edit?
    wants(I, E) {
      if (!ready || !I || !E) return false;
      if (info && I.pw * I.ph > info.max_pixels) return false;
      return plan(E) !== null;
    },
    /* render I with E on the GPU (upload / new steps as needed); display: draw its canvas now (else
       hidden: the 2D canvas shows the pixels, see results()). False if the GPU failed. */
    show(I, E, display = true) {
      try {
        if (img !== I) { g.upload(I.base, I.pw, I.ph, 4); img = I; }
        const P = plan(E), scaled = P.some(st => st.scaled);
        if (edit !== E || (scaled && editImg !== I)) {      // Sharpen's scale depends on the image too
          g.steps_begin();
          for (const st of P) g['step_' + st.t](...(st.scaled ? [...st.args, outScale(I)] : st.args));
          g.steps_end();
          edit = E; editImg = I;
        }
      } catch (e) { fail(String((e && e.message) || e)); return false; }
      if (!display) { hide(); return !failed; }
      on = true;
      cv.classList.add('show');
      draw();
      return on;
    },
    hide,
    /* the histograms of what show() drew, and with pixels = true also the pixels (RGBA). done(r)
       gets {I, E, hist: Uint32Array(2048), px?: Uint8Array}. One request is in flight at a time;
       a newer one replaces a waiting one (a dragged slider asks every frame). */
    results(I, E, pixels, done) {
      if (!g) return;
      pending = { I, E, pixels, done };
      if (!busy) pump();
    },
    // the view changed (pan, zoom, resize, split line): redraw on the next frame
    viewChanged() {
      if (!on || queued) return;
      queued = true;
      requestAnimationFrame(draw);
    },
    info: () => ({ ready, failed, why, info }),
    /* the status bar's engine badge: which core draws the preview right now, and why not the GPU.
       While the GPU core is being built up this makes it obvious which path an edit takes. */
    badge(I, E) {
      const el = $('#stEngine');
      if (!el) return;
      let gpu = false, text, why2 = '';
      if (!ready) { text = 'CPU preview'; why2 = failed ? 'GPU off: ' + why : 'GPU starting…'; }
      else if (E && plan(E) === null) { text = 'CPU preview'; why2 = (TOOL_NAMES[blockers.get(E)] || blockers.get(E)) + ' not on the GPU yet'; }
      else if (I && info && I.pw * I.ph > info.max_pixels) { text = 'CPU preview'; why2 = 'image too big for this GPU'; }
      else { gpu = true; text = 'GPU preview'; }
      const ver = info ? `fae-gpu ${info.version} · ${info.name || 'WebGPU'} (${info.backend})` : '';
      el.textContent = gpu ? text : `${text} · ${why2}`;
      el.title = [gpu ? 'The GPU core draws the preview (ui/gpu/)' : 'The CPU core draws the preview: ' + why2, ver]
        .filter(Boolean).join('\n');
      el.classList.toggle('gpu', gpu);
    },
    // the GPU chain's result as packed RGB (tests compare it with the CPU core's S.outData)
    readback: () => (g && img ? g.readback() : Promise.resolve(null)),
  };
})();
new ResizeObserver(() => GV.viewChanged()).observe(vp);
