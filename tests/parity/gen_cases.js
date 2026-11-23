'use strict';
/* Builds the parity test cases: edit states (what the sliders hold) and the edit payload Python
   gets for them (jobFor, exactly as Process sends it). Deterministic for a given seed.
   usage: node gen_cases.js <repo root> <out.json> [seed] */
const fs = require('fs');
const { loadUi } = require('./ui_math');

const [root, outPath, seedArg] = process.argv.slice(2);
const UI = loadUi(root);

let seed = Number(seedArg || 20261121) >>> 0;
function rnd() {                    // mulberry32
  seed = (seed + 0x6D2B79F5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const pick = a => a[Math.floor(rnd() * a.length)];
const int = (a, b) => a + Math.floor(rnd() * (b - a + 1));

// [key, label, default, min, max, step, decimals]: a value on the slider's grid; ends now and then
function slider(f, mode) {
  const [, , def, min, max, step] = f;
  if (mode === 'min') return min;
  if (mode === 'max') return max;
  let v;
  do {
    const r = rnd();
    v = r < 0.1 ? min : r < 0.2 ? max : min + Math.round((rnd() * (max - min)) / step) * step;
    v = Number(v.toFixed(6));
  } while (v === def);
  return v;
}

function randomCurve() {
  const n = int(1, 4), xs = new Set();
  while (xs.size < n) xs.add(int(8, 247));
  const pts = [...xs].sort((a, b) => a - b).map(x => [x, Math.max(0, Math.min(255, x + int(-70, 70)))]);
  return [[0, int(0, 40)], ...pts, [255, int(215, 255)]];
}
const sample = (lo = 0, hi = 255) => ({ rgb: [int(lo, hi), int(lo, hi), int(lo, hi)] });
// black / white samples on their own side, so a black + white pair actually stretches the range
const sampleFor = k => (k === 'black' ? sample(0, 80) : k === 'white' ? sample(170, 255) : sample());

const adjust = UI.ADJUST;
const groups = UI.FILTERS.map(g => UI.filterFields(g));
const scalarFields = groups.map(fs_ => fs_.filter(f => Array.isArray(f)));
const presets = Object.keys(UI.FILM_PRESETS);

const cases = [];
function add(id, desc, mutate) {
  const e = UI.defaultEdit();
  mutate(e);
  cases.push({ id, desc, state: e });
}

add('identity', 'default edit', () => {});
for (let i = 0; i < 4; i++) {
  add(`curves_${i}`, 'curves only', e => {
    for (const c of i === 0 ? ['m'] : i === 1 ? ['r', 'g', 'b'] : ['m', 'r', 'g', 'b']) e.curves[c] = randomCurve();
  });
}
const pointSets = [['black'], ['white'], ['black', 'white'], ['gray'], ['black', 'gray', 'white']];
pointSets.forEach((set, i) => add(`point_${i}`, 'point correction: ' + set.join('+'), e => {
  for (const k of set) e[k] = sampleFor(k);
  if (set.includes('black')) e.tBlack = int(0, 30);
  if (set.includes('white')) e.tWhite = int(220, 255);
  if (i === 4) e.strength = 0.5;
}));
for (const f of adjust) {
  for (const mode of ['min', 'max', 'rnd']) add(`${f[0]}_${mode}`, `${f[1]} ${mode}`, e => {
    e[f[0]] = slider(f, mode);
    if (f[0] === 'adj_pivot') e.adj_contrast = slider(adjust.find(a => a[0] === 'adj_contrast'), 'rnd');   // no-op alone
  });
}
scalarFields.forEach(fields => {
  const opacity = fields.find(f => /opacity/.test(f[0]));
  for (const f of fields) {
    if (f === opacity) continue;
    for (const mode of ['max', 'rnd']) {
      add(`${f[0]}_${mode}`, `${f[1]} ${mode}`, e => { e[f[0]] = slider(f, mode); });
    }
    // opacity strictly between 0 and 100 (0 would turn the filter off)
    if (opacity) add(`${f[0]}_op`, `${f[1]} with opacity`, e => { e[f[0]] = slider(f, 'rnd'); e[opacity[0]] = int(1, 99); });
  }
});
for (const p of presets) {
  add(`film_${p}`, `film preset ${p}`, e => {
    e.flt_film_preset = p;
    if (rnd() < 0.5) e.flt_film_bright = int(0, 100);
    if (rnd() < 0.5) e.flt_film_contrast = int(0, 100);
    if (rnd() < 0.5) e.flt_film_opacity = int(1, 99);
  });
}
add('sc_all', 'super contrast, every slider', e => {
  for (const f of scalarFields[2]) e[f[0]] = /opacity/.test(f[0]) ? int(1, 100) : slider(f, 'rnd');
});
for (let i = 0; i < 30; i++) {
  add(`combo_${i}`, 'random combination', e => {
    for (const f of adjust) if (rnd() < 0.4) e[f[0]] = slider(f, 'rnd');
    for (const fields of scalarFields) for (const f of fields) if (rnd() < 0.3) e[f[0]] = slider(f, 'rnd');
    if (rnd() < 0.4) e.flt_film_preset = pick(presets);
    if (rnd() < 0.4) for (const c of ['m', 'r', 'g', 'b']) if (rnd() < 0.5) e.curves[c] = randomCurve();
    for (const k of ['black', 'gray', 'white']) if (rnd() < 0.2) e[k] = sampleFor(k);
  });
}

// reordered chains (the panel's drag order): a few tools on, in a shuffled order
const CHAIN = UI.CHAIN_TOOLS;
for (let i = 0; i < 8; i++) {
  add(`chain_${i}`, 'reordered chain', e => {
    e.curves.m = randomCurve();
    if (i % 2 === 0) for (const k of ['black', 'white']) e[k] = sampleFor(k);
    if (rnd() < 0.7) e.adj_sat = slider(adjust.find(a => a[0] === 'adj_sat'), 'rnd');
    if (rnd() < 0.5) e.adj_contrast = slider(adjust.find(a => a[0] === 'adj_contrast'), 'rnd');
    if (i % 4 === 1) e.adj_middetail = slider(adjust.find(a => a[0] === 'adj_middetail'), 'rnd');
    for (const fields of scalarFields) for (const f of fields) if (rnd() < 0.35) e[f[0]] = slider(f, 'rnd');
    if (i % 3 !== 2) e.flt_film_preset = pick(presets);
    const order = CHAIN.slice();
    for (let k = order.length - 1; k > 0; k--) { const j = int(0, k); [order[k], order[j]] = [order[j], order[k]]; }
    if (order.join() === CHAIN.join()) order.reverse();
    e.chain = order.map(t => ({ id: t, t }));
  });
}

const stage = UI.S.stage, geom = UI.defaultGeom();
const out = cases.map(c => ({ ...c, job: UI.jobFor({ path: 'x', state: { stage, edit: c.state, geom } }, false).edit }));
const ids = new Set();
for (const c of out) { if (ids.has(c.id)) throw new Error('duplicate case id ' + c.id); ids.add(c.id); }
fs.writeFileSync(outPath, JSON.stringify(out));
console.log(`${out.length} cases -> ${outPath}`);
