/* GPU spike page (gpu-spike.html, wave G0 of claude/gpu-core-plan.md): is WebGPU there in this
   window, which limits does the adapter give, does the WebAssembly build of the GPU core (ui/gpu/)
   compute Curves exactly, and how fast are upload / pass / draw / read back at 24 and 50 MP.
   URL parameters: ?big=0 skips the 24 / 50 MP sizes (software GPUs in tests). */

const rows = document.getElementById('rows'), report = document.getElementById('report');
const canvas = document.getElementById('view');
const lines = [];
const params = new URLSearchParams(location.search);
const result = { rows: [], sizes: [] };          // for automated tests (window.gpuSpike)
window.gpuSpike = result;

function row(name, state, text) {
  const tr = document.createElement('tr');
  tr.innerHTML = `<td class="k"></td><td class="${state}"></td>`;
  tr.children[0].textContent = name;
  tr.children[1].textContent = ({ ok: '✓ ', bad: '✗ ', warn: '! ' }[state] || '') + text;
  rows.appendChild(tr);
  lines.push(`${({ ok: 'OK  ', bad: 'FAIL', warn: 'WARN' }[state] || 'INFO')}  ${name}: ${text}`);
  report.value = lines.join('\n');
  result.rows.push({ name, state, text });
}

const ms = t => `${t.toFixed(2)} ms`;
const mb = b => `${(b / 1048576).toFixed(0)} MiB`;
const next = () => new Promise(r => setTimeout(r, 0));

function xorshift(seed) {
  let x = seed >>> 0 || 1;
  return () => { x ^= x << 13; x ^= x >>> 17; x ^= x << 5; return x >>> 0; };
}

/* A photo-like test image: smooth gradients + noise (random bytes alone compress / cache oddly) */
function testImage(w, h, seed) {
  const a = new Uint8Array(w * h * 3), rnd = xorshift(seed);
  for (let y = 0, j = 0; y < h; y++) {
    const fy = y / h;
    for (let x = 0; x < w; x++, j += 3) {
      const fx = x / w, n = rnd();
      a[j] = (fx * 230 + (n & 15)) | 0;
      a[j + 1] = (fy * 230 + ((n >>> 8) & 15)) | 0;
      a[j + 2] = ((1 - fx * fy) * 230 + ((n >>> 16) & 15)) | 0;
    }
  }
  return a;
}

function randomLuts(seed) {
  const L = new Uint8Array(768), rnd = xorshift(seed);
  for (let c = 0; c < 3; c++) {                  // monotone-ish curves with some jitter
    const g = 0.6 + (rnd() % 1000) / 1000;
    for (let i = 0; i < 256; i++) L[c * 256 + i] = Math.min(255, Math.round(255 * Math.pow(i / 255, g)) + (rnd() & 3));
  }
  return L;
}

function jsCurves(src, L) {                      // the CPU reference: what apply_luts does
  const out = new Uint8Array(src.length);
  for (let j = 0; j < src.length; j += 3) {
    out[j] = L[src[j]]; out[j + 1] = L[256 + src[j + 1]]; out[j + 2] = L[512 + src[j + 2]];
  }
  return out;
}

async function timeIt(n, f) {                    // best of n, f async
  let best = Infinity;
  for (let i = 0; i < n; i++) { const t = performance.now(); await f(i); best = Math.min(best, performance.now() - t); }
  return best;
}

async function probe() {
  if (!('gpu' in navigator)) {
    row('navigator.gpu', 'bad', 'missing: WebGPU is not enabled in this WebView2 (try WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--enable-unsafe-webgpu)');
    return null;
  }
  row('navigator.gpu', 'ok', 'present');
  const ad = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
  if (!ad) { row('WebGPU adapter', 'bad', 'requestAdapter returned null (GPU blocklisted or WebGPU disabled)'); return null; }
  const i = ad.info || {};
  row('WebGPU adapter', 'ok', [i.vendor, i.architecture, i.device, i.description].filter(Boolean).join(' / ') || '(no info)');
  if (ad.isFallbackAdapter || (i.isFallbackAdapter)) row('Fallback adapter', 'warn', 'this is a software adapter: speeds below mean nothing');
  const L = ad.limits;
  const tex = L.maxTextureDimension2D, buf = L.maxBufferSize, sto = L.maxStorageBufferBindingSize;
  row('Max texture size', tex >= 16384 ? 'ok' : 'warn', `${tex} px (default 8192; 50 MP photos need > 8192)`);
  row('Max buffer size', buf >= 1 << 30 ? 'ok' : 'warn', `${mb(buf)} (default 256 MiB)`);
  row('Max storage binding', sto >= 1 << 30 ? 'ok' : 'warn', `${mb(sto)} (default 128 MiB; packed 50 MP = 143 MiB)`);
  row('Features', 'info', [...ad.features].sort().join(', ') || '(none)');
  result.adapter = { info: { vendor: i.vendor, architecture: i.architecture, device: i.device, description: i.description },
    maxTextureDimension2D: tex, maxBufferSize: buf, maxStorageBufferBindingSize: sto };
  return ad;
}

async function main() {
  row('User agent', 'info', navigator.userAgent);
  row('Cross-origin isolated', 'info', String(self.crossOriginIsolated));
  if (!(await probe().catch(e => { row('WebGPU probe', 'bad', String(e && e.message || e)); return null; }))) {
    result.done = true; return;
  }

  let mod, s;
  try {
    const t = performance.now();
    mod = await import('../gpu/fae_gpu_wasm.js');
    await mod.default();
    row('GPU core (WebAssembly)', 'ok', `loaded in ${ms(performance.now() - t)}`);
  } catch (e) {
    row('GPU core (WebAssembly)', 'bad', `ui/gpu/ missing or broken (run core\\gpu\\build_gpu.bat): ${e && e.message || e}`);
    result.done = true; return;
  }
  try {
    canvas.width = Math.round(canvas.clientWidth * devicePixelRatio);
    canvas.height = Math.round(canvas.clientHeight * devicePixelRatio);
    const t = performance.now();
    s = await mod.create(canvas);
    const info = JSON.parse(s.info());
    if (s.errors()) throw new Error(s.errors());
    result.core = info;
    row('wgpu device', 'ok', `${info.name || '(adapter)'} via ${info.backend}, canvas ${info.format}, ` +
      `max ${info.max_texture_2d} px, buffers ${mb(info.max_buffer)}, storage ${mb(info.max_storage_binding)} ` +
      `(${ms(performance.now() - t)})`);
  } catch (e) {
    row('wgpu device', 'bad', String(e && e.message || e)); result.done = true; return;
  }

  const sizes = [[1001, 999], [2560, 1440]];
  if (params.get('big') !== '0') sizes.push([6000, 4000], [8660, 5773]);
  for (const [w, h] of sizes) {
    await next();
    const name = `${w}x${h} (${(w * h / 1e6).toFixed(1)} MP)`;
    const rec = { w, h };
    result.sizes.push(rec);
    try {
      const img = testImage(w, h, w * 31 + h);
      const luts = randomLuts(w + h);

      const setCurves = L => { s.steps_begin(); s.step_curves(L); s.steps_end(); };
      setCurves(luts);
      const tUp = await timeIt(2, async () => { s.upload(img, w, h, 3); await s.done(); });
      const tOne = await timeIt(5, async () => { setCurves(luts); s.run(1); await s.done(); });
      const N = 20;
      const tN = (await timeIt(2, async () => { s.run(N); await s.done(); })) / N;
      const cw = canvas.width, ch = canvas.height, sc = Math.min(cw / w, ch / h);
      const draw = () => s.draw(cw, ch, (cw - w * sc) / 2, (ch - h * sc) / 2, sc, 0, 2);
      const tDraw = await timeIt(5, async () => { draw(); await s.done(); });
      // a slider drag: a new table and a redraw every frame, each waited for
      const F = 30, t0 = performance.now();
      for (let f = 0; f < F; f++) { setCurves(randomLuts(f + 1)); draw(); await s.done(); }
      const tFrame = (performance.now() - t0) / F;
      setCurves(luts);
      let t = performance.now();
      const out = await s.readback();
      const tRead = performance.now() - t;
      t = performance.now();
      const ref = jsCurves(img, luts);
      const tJs = performance.now() - t;
      let bad = 0, maxd = 0;
      for (let j = 0; j < ref.length; j++) {
        const d = Math.abs(out[j] - ref[j]);
        if (d) { bad++; if (d > maxd) maxd = d; }
      }
      Object.assign(rec, { upload: tUp, curves1: tOne, curvesPass: tN, draw: tDraw, frame: tFrame, readback: tRead,
        jsCurves: tJs, mismatches: bad, maxDiff: maxd, length: out.length });
      row(`${name}: Curves vs CPU`, bad ? 'bad' : 'ok',
        bad ? `${bad} values differ (max ${maxd} levels)` : 'identical (bit-exact)');
      row(`${name}: times`, 'info',
        `upload ${ms(tUp)} · Curves pass ${ms(tN)} (one, waited: ${ms(tOne)}) · draw ${ms(tDraw)} · ` +
        `frame (Curves + Lanczos draw) ${ms(tFrame)} = ${(1000 / tFrame).toFixed(0)} fps · read back ${ms(tRead)} · ` +
        `JS CPU Curves ${ms(tJs)} (${(tJs / tN).toFixed(1)}× the pass)`);
    } catch (e) {
      rec.error = String(e && e.message || e);
      row(`${name}`, 'warn', `not run: ${rec.error}`);
    }
  }
  row('Done', 'ok', 'copy the report below into the chat');
  result.done = true;
}

document.getElementById('copy').onclick = () => {
  report.select();
  navigator.clipboard?.writeText(report.value).catch(() => document.execCommand('copy'));
};

main().catch(e => { row('Spike', 'bad', String(e && e.stack || e)); result.done = true; });
