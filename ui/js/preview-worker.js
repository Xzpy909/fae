/* Live preview in WebAssembly (module worker, started by wasm-preview.js).
   Runs the Rust core (ui/wasm/) on a thread pool of its own workers: the page's main thread may
   not block, so the work can't run there. Messages:
     in : {type:'image', w, h, base}            base = RGBA ArrayBuffer (transferred)
          {type:'film', id, d}                  a Film preset table, sent once per preset
          {type:'render', seq, E, fresh, rect}  E = computeEdit result, each film.d replaced by its id;
                                                fresh: forget the kept results first (full render);
                                                rect {x, y, w, h}: a draft of that part only (if it can)
     out: {type:'ready', threads, version} | {type:'error', error}
          {type:'done', seq, w, h, out, hin, hout, hmx, hmn, ms}       a full render
          {type:'done', seq, rect, out, ms}                             a draft: out = the rectangle
          {type:'fail', seq, error} */
import init, * as fe from '../wasm/fastedit_wasm.js';
import { toParams } from './wasm-params.js';

let preview = null, wasm = null;
const films = new Map();

const ready = (async () => {
  wasm = await init();
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
      const p = toParams(fe, m.E, id => {
        const d = films.get(id);
        if (!d) throw new Error('film table missing');
        return d;
      });
      if (m.fresh) preview.forget();
      const R = m.rect;
      if (R) {
        let ok = false;
        try { ok = preview.render_region(p, R.x, R.y, R.w, R.h); } catch (e) { p.free(); throw e; }
        if (ok) {
          p.free();
          const out = new Uint8Array(wasm.memory.buffer, preview.part_ptr(), preview.part_len()).slice();
          postMessage({ type: 'done', seq: m.seq, rect: R, out: out.buffer, ms: performance.now() - t0, phases: preview.timing() },
                      [out.buffer]);
          return;
        }
      }
      try { preview.render(p); } finally { p.free(); }
      // one copy straight out of WebAssembly memory (a view on the shared memory can't be transferred)
      const tc = performance.now();
      const out = new Uint8Array(wasm.memory.buffer, preview.out_ptr(), preview.out_len()).slice();
      const phases = preview.timing() + ` | copy out ${(performance.now() - tc).toFixed(1)}`;
      const r = { type: 'done', seq: m.seq, w: preview.width(), h: preview.height(), out: out.buffer, phases,
        hin: preview.hin(), hout: preview.hout(), hmx: preview.hmx(), hmn: preview.hmn(), ms: performance.now() - t0 };
      postMessage(r, [out.buffer]);
    } catch (e) {
      postMessage({ type: 'fail', seq: m.seq, error: String((e && e.message) || e) });
    }
  }
};
