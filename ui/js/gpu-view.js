'use strict';
/* GPU preview (GPU core plan, waves G1-G2): when every step of the edit runs on the GPU core
   (ui/gpu/, WebGPU), the image is shown by a WebGPU canvas over the viewport, drawn at exactly the
   screen's pixels (Lanczos-3; nearest from 2 screen px per image px, like the pixel view) on every
   edit, pan and zoom - no drafts, no screen-resolution redraw. The original | edited split and the
   before view are drawn there too.
   The GPU also counts the histograms (every frame, so they follow a dragged slider) and, once the
   edit settles, hands back the pixels (S.outData: RGB readout, loupe, the Alt views on the 2D
   canvas). The CPU core isn't used for such an edit at all. An edit with Sharpen (not on the GPU
   yet) uses the CPU path unchanged. Off with ?gpu=0 (fae.py --no-gpu); without WebGPU or ui/gpu/ the app works
   as before. */
const GV = (() => {
  const params = new URLSearchParams(location.search);
  const cv = document.createElement('canvas');
  cv.id = 'gpu';
  ($('#sharp') || stageEl).after(cv);       // over the image and the Lanczos overlay, under markers / split line

  let g = null, ready = false, failed = false, why = '', info = null;
  let img = null, edit = null, on = false, queued = false, checking = false;
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
    if (E.sharp) { out = null; blocker = 'sharp'; }
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
    plans.set(E, out);
    if (blocker) blockers.set(E, blocker);
    return out;
  }

  function fail(reason) {
    if (failed) return;
    failed = true; ready = false; why = reason; g = null; img = null; edit = null;
    hide();
    console.warn('GPU preview off: ' + reason);
    if (typeof requestRender === 'function') requestRender();
  }

  async function start() {
    if (params.get('gpu') === '0') { failed = true; why = 'turned off (--no-gpu)'; return; }
    if (!('gpu' in navigator)) { failed = true; why = 'WebGPU not available'; return; }
    try {
      const mod = await import('../gpu/fae_gpu_wasm.js');
      await mod.default();
      cv.width = cv.height = 1;
      g = await mod.create(cv);
      info = JSON.parse(g.info());
      info.version = mod.version();
      console.info(`GPU preview: fae-gpu ${info.version} on ${info.name || 'WebGPU adapter'} (${info.backend})`);
      ready = true;
      if (typeof requestRender === 'function') requestRender();
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

  start();
  return {
    ready: () => ready,
    on: () => on,
    // can the GPU show this image and edit in the current view?
    wants(I, E, mode) {
      if (!ready || !I || !E || S.cropMode || encSplitOn()) return false;
      if (mode && mode !== 'before') return false;
      if (info && I.pw * I.ph > info.max_pixels) return false;
      return plan(E) !== null;
    },
    // show I with E (upload / new steps as needed) and draw now; false if the GPU failed
    show(I, E) {
      try {
        if (img !== I) { g.upload(I.base, I.pw, I.ph, 4); img = I; }
        if (edit !== E) {
          g.steps_begin();
          for (const st of plan(E)) g['step_' + st.t](...st.args);
          g.steps_end();
          edit = E;
        }
      } catch (e) { fail(String((e && e.message) || e)); return false; }
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
    badge(I, E, mode) {
      const el = $('#stEngine');
      if (!el) return;
      let gpu = false, text, why2 = '';
      if (!ready) { text = 'CPU preview'; why2 = failed ? 'GPU off: ' + why : 'GPU starting…'; }
      else if (S.cropMode) { text = 'CPU preview'; why2 = 'crop mode'; }
      else if (encSplitOn()) { text = 'CPU preview'; why2 = 'encoded compare'; }
      else if (mode && mode !== 'before') { text = 'CPU preview'; why2 = 'Alt view'; }
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
