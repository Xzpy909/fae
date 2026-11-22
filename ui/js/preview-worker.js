/* Live preview in WebAssembly (module worker, started by wasm-preview.js).
   Runs the Rust core (ui/wasm/) on a thread pool of its own workers: the page's main thread may
   not block, so the work can't run there. Messages:
     in : {type:'image', w, h, base}            base = RGBA ArrayBuffer (transferred)
          {type:'film', id, d}                  a Film preset table, sent once per preset
          {type:'render', seq, E}               E = computeEdit result, film.d replaced by its id
     out: {type:'ready', threads, version} | {type:'error', error}
          {type:'done', seq, w, h, out, hin, hout, hmx, hmn, ms} | {type:'fail', seq, error} */
import init, * as fe from '../wasm/fastedit_wasm.js';
import { toParams } from './wasm-params.js';

let preview = null;
const films = new Map();

const ready = (async () => {
  await init();
  let threads = 1;
  if (self.crossOriginIsolated) {
    try {
      const n = Math.max(1, Math.min(navigator.hardwareConcurrency || 4, 64));
      await fe.initThreadPool(n);
      fe.enable_threads(true);
      threads = fe.threads();
    } catch (e) {
      console.warn('WASM preview: thread pool failed, single-threaded', e);
    }
  }
  postMessage({ type: 'ready', threads, version: fe.version() });
})().catch(e => {
  postMessage({ type: 'error', error: String((e && e.message) || e) });
  throw e;
});

onmessage = async ({ data: m }) => {
  await ready;
  if (m.type === 'image') {
    if (preview) preview.free();
    preview = new fe.Preview(m.w, m.h, new Uint8Array(m.base));
  } else if (m.type === 'film') {
    films.set(m.id, m.d);
  } else if (m.type === 'render') {
    try {
      if (!preview) throw new Error('no image');
      const t0 = performance.now();
      const E = m.E, d = E.film && E.film.d ? films.get(E.film.d) : null;
      if (E.film && E.film.d && !d) throw new Error('film table missing');
      const p = toParams(fe, E, d);
      try { preview.render(p); } finally { p.free(); }
      const out = preview.output();
      const r = { type: 'done', seq: m.seq, w: preview.width(), h: preview.height(), out: out.buffer,
        hin: preview.hin(), hout: preview.hout(), hmx: preview.hmx(), hmn: preview.hmn(), ms: performance.now() - t0 };
      postMessage(r, [out.buffer]);
    } catch (e) {
      postMessage({ type: 'fail', seq: m.seq, error: String((e && e.message) || e) });
    }
  }
};
