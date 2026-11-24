/* Live preview in WebAssembly (module worker, started by wasm-preview.js).
   Runs the Rust core (ui/wasm/) on a thread pool of its own workers: the page's main thread may
   not block, so the work can't run there. Messages:
     in : {type:'image', id, drop, w, h, base}  base = RGBA ArrayBuffer (transferred); drop = id to free
          {type:'film', id, d}                  a Film preset table, sent once per preset
          {type:'render', seq, img, E}         img = image id; E = computeEdit result, each film.d replaced by its id
     out: {type:'ready', threads, version} | {type:'error', error}
          {type:'done', seq, w, h, out, hin, hout, hmx, hmn, ms} | {type:'fail', seq, error} */
import init, * as fe from '../wasm/fastedit_wasm.js';
import { toParams } from './wasm-params.js';

let wasm = null;
const previews = new Map();          // image id -> fe.Preview
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
    if (m.drop != null && previews.has(m.drop)) { previews.get(m.drop).free(); previews.delete(m.drop); }
    if (previews.has(m.id)) previews.get(m.id).free();
    previews.set(m.id, new fe.Preview(m.w, m.h, new Uint8Array(m.base)));
  } else if (m.type === 'film') {
    films.set(m.id, m.d);
  } else if (m.type === 'render') {
    try {
      const preview = previews.get(m.img);
      if (!preview) throw new Error('no image');
      const t0 = performance.now();
      const p = toParams(fe, m.E, id => {
        const d = films.get(id);
        if (!d) throw new Error('film table missing');
        return d;
      });
      try { preview.render(p); } finally { p.free(); }
      // one copy straight out of WebAssembly memory (a view on the shared memory can't be transferred)
      const out = new Uint8Array(wasm.memory.buffer, preview.out_ptr(), preview.out_len()).slice();
      const r = { type: 'done', seq: m.seq, w: preview.width(), h: preview.height(), out: out.buffer,
        hin: preview.hin(), hout: preview.hout(), hmx: preview.hmx(), hmn: preview.hmn(), ms: performance.now() - t0 };
      postMessage(r, [out.buffer]);
    } catch (e) {
      postMessage({ type: 'fail', seq: m.seq, error: String((e && e.message) || e) });
    }
  }
};
