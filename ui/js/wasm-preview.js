'use strict';
/* Live preview through the Rust core (WebAssembly, multithreaded) in preview-worker.js.
   render() and crop mode hand their image and edit here; the result arrives asynchronously, and only
   the newest pending request is rendered next (steps in between are skipped while a slider moves).
   There is no JS fallback: if the core can't start, WP.failed() is true, the app says how to fix it,
   and render() shows the unedited image. */
const WP = (() => {
  let worker = null, ready = false, failed = false, why = '', threads = 0, version = '';
  let sentImg = null, seq = 0, busy = null, pending = null, lastMs = 0;
  const filmIds = new WeakMap(), filmSent = new Set();
  let nextFilm = 1;

  function start() {
    if (worker || failed) return;
    try {
      worker = new Worker('js/preview-worker.js', { type: 'module' });
    } catch (e) { fail('worker: ' + e); return; }
    worker.onmessage = onMessage;
    // a module worker that can't load gives an event without a message: most likely ui/wasm isn't built
    worker.onerror = e => fail(e && e.message ? 'worker error: ' + e.message : 'the WebAssembly core (ui/wasm) did not load');
  }

  function fail(reason) {
    if (failed) return;
    failed = true; ready = false; why = reason;
    console.error('Live preview: ' + reason);
    if (typeof toast === 'function') {
      toast('Live preview unavailable (' + reason + '). Run core\\build_core.bat, then restart.', true);
    }
    if (worker) worker.terminate();
    worker = null; busy = null; pending = null;
    if (typeof requestRender === 'function') requestRender();
  }

  function onMessage({ data: m }) {
    if (m.type === 'ready') {
      ready = true; threads = m.threads; version = m.version;
      if (typeof requestRender === 'function') requestRender();
      if (typeof Crop !== 'undefined' && S.cropMode) Crop.editChanged();
    } else if (m.type === 'error') {
      fail(m.error);
    } else if (m.type === 'done' || m.type === 'fail') {
      const job = busy;
      busy = null;
      if (m.type === 'fail') { fail(m.error); return; }
      lastMs = m.ms;
      if (job && job.seq === m.seq) job.done(m);
      pump();
    }
  }

  // preview pixels per output pixel: the live image (pw of w), or crop mode's uncropped source
  function outScale(img) {
    if (img.w) return img.pw / img.w;
    if (img.W && typeof S !== 'undefined' && S.img) return img.pw / img.W * (S.img.scale || 1);
    return 1;
  }

  // E as the worker gets it: Film preset tables are sent once and then referred to by id; Sharpen gets
  // the preview's scale
  function pack(E, img) {
    if (E.sharp) E = Object.assign({}, E, { sharp: Object.assign({}, E.sharp, { scale: outScale(img) }) });
    const byId = o => {                // o = E or one of E.steps
      if (!o.film || !o.film.d) return o;
      let id = filmIds.get(o.film.d);
      if (!id) { id = nextFilm++; filmIds.set(o.film.d, id); }
      if (!filmSent.has(id)) { worker.postMessage({ type: 'film', id, d: o.film.d }); filmSent.add(id); }
      return Object.assign({}, o, { film: Object.assign({}, o.film, { d: id }) });
    };
    E = byId(E);
    if (E.steps) E = Object.assign({}, E, { steps: E.steps.map(byId) });
    return E;
  }

  function pump() {
    if (busy || !pending || !worker) return;
    const job = pending;
    pending = null;
    if (sentImg !== job.img) {        // an image is uploaded once and kept until another one comes
      const base = job.img.base.slice().buffer;
      worker.postMessage({ type: 'image', w: job.img.pw, h: job.img.ph, base }, [base]);
      sentImg = job.img;
    }
    job.seq = ++seq;
    busy = job;
    worker.postMessage({ type: 'render', seq: job.seq, E: pack(job.E, job.img) });
  }

  start();
  return {
    ready: () => ready,
    failed: () => failed,
    // img = {base (RGBA), pw, ph}; done(result) gets {out, hin, hout, hmx, hmn, w, h, ms} unless a newer
    // request replaced this one first
    render(img, E, done) {
      if (!ready) return;
      pending = { img, E, done };
      pump();
    },
    info: () => ({ on: ready, threads, version, lastMs, failed, why }),
  };
})();
