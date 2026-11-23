'use strict';
/* Live preview through the Rust core (WebAssembly, multithreaded) in preview-worker.js.
   render() and crop mode hand their image and edit here; the result arrives asynchronously, and only
   the newest pending request is rendered next (steps in between are skipped while a slider moves).
   There is no JS fallback: if the core can't start, WP.failed() is true, the app says how to fix it,
   and render() shows the unedited image. */
const WP = (() => {
  let worker = null, ready = false, failed = false, why = '', threads = 0, version = '';
  let seq = 0, busy = null, pending = null, lastMs = 0;
  // images kept in the worker (newest last): the full-resolution stage and its small copy both stay,
  // so switching between them costs no upload and keeps each one's cached measurements
  const KEEP = 2, imgIds = new WeakMap(), sent = [];
  let nextImg = 1;
  // thumbnails (presets): queued, never skipped, rendered only while no live render is waiting; their
  // small image has a slot of its own, so it never pushes the live images out
  const thumbs = [];
  let thumbSent = null;
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
    worker = null; busy = null; pending = null; sent.length = 0;
    thumbs.splice(0).forEach(t => t.done(null));
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

  // the worker's id of img, uploading it first if it isn't there (the oldest kept one makes room)
  function upload(img) {
    let id = imgIds.get(img);
    const at = id ? sent.indexOf(id) : -1;
    if (at >= 0) { sent.splice(at, 1); sent.push(id); return id; }
    if (!id) { id = nextImg++; imgIds.set(img, id); }
    let drop = null;
    if (sent.length >= KEEP) drop = sent.shift();
    const base = img.base.slice().buffer;
    worker.postMessage({ type: 'image', id, drop, w: img.pw, h: img.ph, base }, [base]);
    sent.push(id);
    return id;
  }

  function pumpThumb() {
    const job = thumbs.shift();
    let id = imgIds.get(job.img);
    if (!id || id !== thumbSent) {
      if (!id) { id = nextImg++; imgIds.set(job.img, id); }
      const base = job.img.base.slice().buffer;
      worker.postMessage({ type: 'image', id, drop: thumbSent, w: job.img.pw, h: job.img.ph, base }, [base]);
      thumbSent = id;
    }
    job.seq = ++seq;
    busy = job;
    worker.postMessage({ type: 'render', seq: job.seq, img: id, E: pack(job.E, job.img), rect: null });
  }

  function pump() {
    if (busy || !worker) return;
    if (!pending) { if (thumbs.length) pumpThumb(); return; }
    const job = pending;
    pending = null;
    const id = upload(job.img);
    job.seq = ++seq;
    busy = job;
    worker.postMessage({ type: 'render', seq: job.seq, img: id, E: pack(job.E, job.img), rect: job.rect || null });
  }

  start();
  return {
    ready: () => ready,
    failed: () => failed,
    // img = {base (RGBA), pw, ph}; done(result) gets {out, hin, hout, hmx, hmn, w, h, ms} unless a newer
    // request replaced this one first. rect {x, y, w, h}: a draft of that part only; the result then
    // has {rect, out} (no histograms), or is a full render when the core can't draft yet.
    render(img, E, done, rect) {
      if (!ready) return;
      pending = { img, E, done, rect };
      pump();
    },
    // a thumbnail: img = {base, pw, ph, w} (w = the output width, for Sharpen's scale); resolves with
    // the full render result, or null if the core is unavailable or the queue was cleared
    thumb(img, E) {
      if (!ready) return Promise.resolve(null);
      return new Promise(done => { thumbs.push({ img, E, done }); pump(); });
    },
    clearThumbs() { thumbs.splice(0).forEach(t => t.done(null)); },
    info: () => ({ on: ready, threads, version, lastMs, failed, why }),
  };
})();
