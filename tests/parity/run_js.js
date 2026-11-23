'use strict';
/* Runs the WebAssembly build of the Rust core (ui/wasm/) on one image for many cases, through
   ui/js/wasm-params.js exactly as the preview worker does (single-threaded here: node has no
   worker pool). Called by parity.py check --impl wasm.
   usage: node run_js.js <repo root> <cases.json.gz> <ids.json> <image.rgb> <w> <h> <out dir>
   Writes <out dir>/<case id>.rgb (RGB, 8 bit) + <case id>.h32 (hin, hout, hmx, hmn as uint32 LE),
   or <case id>.err. */
const fs = require('fs');
const path = require('path');
const { pathToFileURL } = require('url');   // import() wants a URL: a drive-letter path fails on Windows
const zlib = require('zlib');
const { loadUi } = require('./ui_math');

// computeEdit's shape (what the worker gets) from the stored job (what Python got)
function editFromJob(j, UI) {
  const f64 = a => (a == null ? null : Float64Array.from(a)), u8 = a => (a == null ? null : Uint8Array.from(a));
  const film = fm => {
    if (!fm) return null;
    const d = fm.d && !Array.isArray(fm.d) ? UI.FILM_PRESETS[fm.d.preset] : fm.d;
    return { n: fm.n, d: f64(d), t1: u8(fm.t1), op: fm.op };
  };
  const tables = x => ({                 // the fields a step / the flat edit has, as computeEdit gives them
    k: x.k, o: x.o, gains: x.gains, luts: x.luts ? x.luts.map(u8) : undefined, point: x.point, wb: x.wb, con: f64(x.con),
    tones: x.tones ? x.tones.map(t => ({ T: f64(t.T), ratio: t.ratio })) : null,
    boost: f64(x.boost), sat: x.sat, md: x.md, bw: x.bw, film: film(x.film),
    sc: x.sc ? Object.assign({}, x.sc, { shl: u8(x.sc.shl) }) : null,
  });
  const E = tables(j);
  if (j.steps) E.steps = j.steps.map(s => Object.assign(tables(s), { t: s.t }));
  return E;
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
  const fe = await import(pathToFileURL(path.join(dir, 'fastedit_wasm.js')).href);
  fe.initSync({ module: fs.readFileSync(path.join(dir, 'fastedit_wasm_bg.wasm')) });
  const { toParams } = await import(pathToFileURL(path.join(root, 'ui', 'js', 'wasm-params.js')).href);
  const UI = loadUi(root);
  const pv = new fe.Preview(W, H, base);

  for (const c of cases) {
    try {
      const E = editFromJob(c.job, UI);
      const p = toParams(fe, E);
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
