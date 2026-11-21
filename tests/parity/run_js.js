'use strict';
/* Runs the WebAssembly build of the Rust core (ui/wasm/) on one image for many cases, through
   ui/js/wasm-params.js exactly as the preview worker does (single-threaded here: node has no
   worker pool). Called by parity.py check --impl wasm.
   usage: node run_js.js <repo root> <cases.json.gz> <ids.json> <image.rgb> <w> <h> <out dir>
   Writes <out dir>/<case id>.rgb (RGB, 8 bit) + <case id>.h32 (hin, hout, hmx, hmn as uint32 LE),
   or <case id>.err. */
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const { loadUi } = require('./ui_math');

// computeEdit's shape (what the worker gets) from the stored job (what Python got)
function editFromJob(j, UI) {
  const f64 = a => (a == null ? null : Float64Array.from(a)), u8 = a => (a == null ? null : Uint8Array.from(a));
  let film = null;
  if (j.film) {
    const d = j.film.d && !Array.isArray(j.film.d) ? UI.FILM_PRESETS[j.film.d.preset] : j.film.d;
    film = { n: j.film.n, d: f64(d), t1: u8(j.film.t1), op: j.film.op };
  }
  return {
    k: j.k, o: j.o, gains: j.gains, luts: j.luts.map(u8), point: j.point, wb: j.wb, con: f64(j.con),
    tones: j.tones ? j.tones.map(t => ({ T: f64(t.T), ratio: t.ratio })) : null,
    boost: f64(j.boost), sat: j.sat, md: j.md, bw: j.bw, film,
    sc: j.sc ? Object.assign({}, j.sc, { shl: u8(j.sc.shl) }) : null,
  };
}

async function main() {
  const [root, casesPath, idsPath, imgPath, ws, hs, outDir] = process.argv.slice(2);
  const W = Number(ws), H = Number(hs), n = W * H;
  const ids = new Set(JSON.parse(fs.readFileSync(idsPath, 'utf8')));
  const raw = fs.readFileSync(casesPath);
  const cases = JSON.parse((casesPath.endsWith('.gz') ? zlib.gunzipSync(raw) : raw).toString('utf8')).filter(c => ids.has(c.id));
  const rgb = fs.readFileSync(imgPath);
  const base = new Uint8Array(n * 4);                  // RGBA, as the preview gets the image
  for (let i = 0, j = 0; i < n; i++, j += 4) { base[j] = rgb[i * 3]; base[j + 1] = rgb[i * 3 + 1]; base[j + 2] = rgb[i * 3 + 2]; base[j + 3] = 255; }

  globalThis.self ??= { addEventListener() {}, removeEventListener() {} };   // the pool helper expects a worker global
  const dir = path.join(root, 'ui', 'wasm');
  const fe = await import(path.join(dir, 'fastedit_wasm.js'));
  fe.initSync({ module: fs.readFileSync(path.join(dir, 'fastedit_wasm_bg.wasm')) });
  const { toParams } = await import(path.join(root, 'ui', 'js', 'wasm-params.js'));
  const UI = loadUi(root);
  const pv = new fe.Preview(W, H, base);

  for (const c of cases) {
    try {
      const E = editFromJob(c.job, UI);
      const p = toParams(fe, E, E.film && E.film.d);
      try { pv.render(p); } finally { p.free(); }
      const out = pv.output(), o = Buffer.alloc(n * 3);
      for (let i = 0, j = 0; i < n; i++, j += 4) { o[i * 3] = out[j]; o[i * 3 + 1] = out[j + 1]; o[i * 3 + 2] = out[j + 2]; }
      fs.writeFileSync(path.join(outDir, c.id + '.rgb'), o);
      const h = [pv.hin(), pv.hout(), pv.hmx(), pv.hmn()].map(a => Buffer.from(Uint32Array.from(a).buffer));
      fs.writeFileSync(path.join(outDir, c.id + '.h32'), Buffer.concat(h));
    } catch (e) {
      fs.writeFileSync(path.join(outDir, c.id + '.err'), String((e && e.stack) || e));
    }
  }
}
main().catch(e => { console.error(e); process.exit(1); });
