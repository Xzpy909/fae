'use strict';
/* Fae - front end.
   Python delivers the "stage" image (resize + White-Patch + Gray-Edge); everything after
   that (point correction, curves, clipping views) runs here, live, on every change.
   Process sends the same parameters back to Python, which applies identical math at full res.
   The scripts in ui/js are plain (non-module) scripts sharing one global scope, loaded in the order
   listed in index.html; code that runs while loading may only use what earlier files defined. */

const $ = s => document.querySelector(s);
const clamp = (v, a, b) => v < a ? a : v > b ? b : v;
const clone = o => JSON.parse(JSON.stringify(o));
const basename = p => String(p).split(/[\\/]/).pop();
const api = () => Bridge.api();        // the back end's commands (bridge.js)
const dpr = () => window.devicePixelRatio || 1;

const DEFAULT_STAGE = {
  resize_on: false, mode: 'preset', mpix: 2.0, prio: 'Balanced',
  preset_ar: '16:9', preset_res: 720, cw: 1920, ch: 1080, cmode: 'crop',
  wp_on: false, wp_pct: 99.0, wp_strength: 1.0,
  ge_on: false, ge_p: 1, ge_sigma: 2.0, ge_strength: 1.0,
  color_on: true,       // island switch of Color balance (off = White-Patch / Gray-Edge skipped, settings kept)
};
const ident = () => [[0, 0], [255, 255]];
/* Adjustments (modelled on DaVinci Resolve Primaries, see the formulas below). [key, label, default, min, max, step, decimals] */
const ADJUST = [
  ['adj_boost', 'Color Boost', 0, -100, 100, 1, 2],
  ['adj_shadows', 'Shadows', 0, -100, 100, 1, 2],
  ['adj_highlights', 'Highlights', 0, -100, 100, 1, 2],
  ['adj_sat', 'Saturation', 50, 0, 100, 1, 2],
  ['adj_temp', 'Temp', 0, -4000, 4000, 10, 1],
  ['adj_tint', 'Tint', 0, -100, 100, 1, 2],
  ['adj_contrast', 'Contrast', 1, 0, 2, 0.001, 3],
  ['adj_pivot', 'Pivot', 0.435, 0, 1, 0.001, 3],
  ['adj_middetail', 'Mid/Detail', 0, -100, 100, 1, 2],
];
const defaultAdjust = () => Object.fromEntries(ADJUST.map(a => [a[0], a[2]]));
/* Filters (picked from the Filters popover). Stored per image; bwParams() says which values are live.
   sec = card id, name + icon (inner SVG, 24x24 viewBox) = its tile in the Filters popover, fields = [key, label, default, min, max, step, decimals, unit] or '---' (divider).
   Opacity mixes the filtered result with the image before the filter (100% = full effect). */
const FILTERS = [
  { sec: 'f_bw', name: 'Perceptual Saturation',
    icon: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>',
    fields: [
    ['flt_bw_sat', 'Saturation', 0, -100, 100, 1, 1, '%'],
    ['flt_bw_warmth', 'Warmth', 0, -100, 100, 1, 1, '%'],
    ['flt_bw_psat', 'Perceptual Saturation ≈', 0, 0, 60, 1, 1, '%'],
    '---',
    ['flt_bw_opacity', 'Opacity', 100, 0, 100, 1, 0, '%'],
  ] },
  // Film Effect: preset table, then Brightness / Contrast, then Opacity (filmParams in edit-math.js)
  { sec: 'f_film', name: 'Film Effect',
    icon: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M7 3v18"/><path d="M17 3v18"/><path d="M3 7.5h4"/><path d="M3 12h18"/><path d="M3 16.5h4"/><path d="M17 7.5h4"/><path d="M17 16.5h4"/>',
    fields: [
    { type: 'select', key: 'flt_film_preset', label: 'Film', def: 'none', options: [['none', 'None']] },   // presets add themselves (js/film/*.js)
    ['flt_film_bright', 'Brightness', 0, 0, 100, 1, 0, '%'],
    ['flt_film_contrast', 'Contrast', 0, 0, 100, 1, 0, '%'],
    '---',
    ['flt_film_opacity', 'Opacity', 100, 0, 100, 1, 0, '%'],
  ] },
  // Super Contrast: all live (scParams in edit-math.js); Dynamic Contrast is an approximation (≈)
  { sec: 'f_sc', name: 'Super Contrast',
    icon: '<circle cx="12" cy="12" r="9"/><path class="fill" d="M12 3a9 9 0 0 1 0 18z"/>',
    fields: [
    ['flt_sc_cast', 'Correct Color Cast', 0, 0, 100, 1, 0, '%'],
    ['flt_sc_contrast', 'Correct Contrast', 0, 0, 100, 1, 0, '%'],
    ['flt_sc_dynamic', 'Dynamic Contrast ≈', 0, 0, 100, 1, 0, '%'],
    '---',
    ['flt_sc_opacity', 'Opacity', 100, 0, 100, 1, 0, '%'],
  ] },
];
/* The filter kit's filters (docs/filter-kit.md): fx-filters.js is generated from
   core/gpu/fae-gpu/filters/<id>.json by the GPU core's build; each becomes a FILTERS entry with its
   island made from its fields (controls.js), runs on the GPU only (edit-math.js fxParams, the step
   'fx') and is skipped without it. Hidden test filters only with ?fxtest=1 (fae.py --test-filters). */
const FX_TEST = new URLSearchParams(location.search).get('fxtest') === '1';
const FX_LIST = (typeof FX_DEFS !== 'undefined' ? FX_DEFS : []).filter(d => !d.hidden || FX_TEST);
const FX_BY_SEC = Object.fromEntries(FX_LIST.map(d => [d.sec, d]));
FILTERS.push(...FX_LIST.map(d => ({ sec: d.sec, name: d.name, icon: d.icon, fields: d.fields, fx: d })));
/* Sharpen: a main tool of its own (pill after Resize, not inside the Filters group), same field format as FILTERS
   plus { head } sub-headings. All sliders are live (sharpParams in edit-math.js); Structure is an
   approximation (≈); Balance Sharpening shifts Adaptive Sharpening between areas and edges. */
const SHARPEN = { sec: 'sharpen', fields: [
  { head: 'Sharpening' },
  ['shp_adaptive', 'Adaptive Sharpening', 0, 0, 100, 1, 0, '%'],
  ['shp_balance', 'Balance Sharpening', 0, -100, 100, 1, 0, '%'],      // -100 areas … 100 edges (shifts Adaptive Sharpening)
  { head: 'Sharpening+' },
  ['shp_structure', 'Structure ≈', 0, -100, 100, 1, 0, '%'],
  ['shp_local', 'Local Contrast', 0, -100, 100, 1, 0, '%'],
  ['shp_focus', 'Focus', 0, -100, 100, 1, 0, '%'],
] };
/* every per-image tool built from a field list: the Filters group, then the stand-alone tools */
const PANELS = [...FILTERS, SHARPEN];
/* gradient tracks for the filter sliders (what the slider does, left to right) */
const FILTER_GRADS = {
  flt_bw_sat: 'linear-gradient(90deg, #7c7c7c, #8f7a72 50%, #e8502e)',
  flt_bw_warmth: 'linear-gradient(90deg, #4d86ff, #c9c9c9 50%, #ff9a3c)',
  flt_bw_psat: 'linear-gradient(90deg, #7c7c7c, #c06a8e 55%, #3fc6ff)',
  flt_film_bright: 'linear-gradient(90deg, #101010, #f2f2f2)',
  flt_film_contrast: 'linear-gradient(90deg, rgba(120,120,120,1), rgba(120,120,120,0)), repeating-linear-gradient(90deg, #050505 0 2px, #f5f5f5 2px 4px)',
  flt_bw_opacity: 'linear-gradient(90deg, rgba(235,235,235,0), #ebebeb), repeating-conic-gradient(#5a5a5a 0 25%, #2e2e2e 0 50%) 0 0 / 6px 6px',
  flt_film_opacity: 'linear-gradient(90deg, rgba(235,235,235,0), #ebebeb), repeating-conic-gradient(#5a5a5a 0 25%, #2e2e2e 0 50%) 0 0 / 6px 6px',
  flt_sc_cast: 'linear-gradient(90deg, #4f7fd8, #9a8f86 45%, #b0b0b0)',
  flt_sc_contrast: 'linear-gradient(90deg, #6a6a6a, #8a8a8a 50%, #101010 50%, #f2f2f2)',
  flt_sc_dynamic: 'linear-gradient(90deg, rgba(120,120,120,1), rgba(120,120,120,0)), repeating-linear-gradient(90deg, #303030 0 5px, #c8c8c8 5px 10px)',
  shp_adaptive: 'linear-gradient(90deg, rgba(120,120,120,1), rgba(120,120,120,0)), repeating-linear-gradient(90deg, #202020 0 1px, #d8d8d8 1px 2px)',
  shp_balance: 'linear-gradient(90deg, #8c8c8c, #b8b8b8 50%, #303030 50%, #f2f2f2)',
  shp_structure: 'linear-gradient(90deg, #6f6f6f, #8a8a8a 50%, #3a3a3a 50%, #d0d0d0)',
  shp_local: 'linear-gradient(90deg, #7a7a7a, #8a8a8a 50%, #202020 50%, #e8e8e8)',
  shp_focus: 'linear-gradient(90deg, #5c6f80, #8a8a8a 50%, #8a8a8a 50%, #f2f2f2)',
  flt_sc_opacity: 'linear-gradient(90deg, rgba(235,235,235,0), #ebebeb), repeating-conic-gradient(#5a5a5a 0 25%, #2e2e2e 0 50%) 0 0 / 6px 6px',
};
const fieldKD = a => Array.isArray(a) ? [a[0], a[2]] : [a.key, a.def];
const filterFields = f => f.fields.filter(a => Array.isArray(a) || (a && a.key));      // values only (no dividers / headings)
const defaultFilter = f => Object.fromEntries(filterFields(f).map(fieldKD));
const filterActive = (f, e) => filterFields(f).map(fieldKD).some(([k, def]) => e[k] !== undefined && e[k] !== def);
/* The processing chain: the re-orderable tools in the order they run (after the stage: crop, resize and
   colour balance always run first, Sharpen always last). One entry per instance { id, t }; t = the tool's
   section id. The first instance of a tool keeps its values in the edit's flat keys (adj_*, curves,
   flt_*...). Default = the tool pill order. */
const CHAIN_TOOLS = ['point', 'adjust', 'curves', 'f_bw', 'f_film', 'f_sc', ...FX_LIST.map(d => d.sec)];
const defaultChain = () => CHAIN_TOOLS.map(t => ({ id: t, t }));
/* Stackable tools (redesign section 8): the same tool more than once in the chain. The first instance
   of a tool (id = the tool) keeps its values in the edit's flat keys; every extra instance (id =
   tool#n) has its own in e.inst[id], its switch in e.off[id] and an optional label in e.lbl[id].
   Mid/Detail works on every Adjustments instance (each smooths the image as it arrives there). */
const STACKABLE = ['adjust', 'curves', 'f_bw', 'f_film', 'f_sc', ...FX_LIST.filter(d => !d.single).map(d => d.sec)];
const MAX_INST = 8;                     // instances per tool, the first one included
const isExtra = x => x.id !== x.t;
/* the edit's chain, made whole: known tools only, each first instance once, missing ones added at
   their default place (edits from before the chain existed have none) */
function chainOf(e) {
  const c = Array.isArray(e && e.chain) ? e.chain : [];
  const seen = new Set(), out = [];
  const count = {};
  for (const x of c) {
    if (!x || !CHAIN_TOOLS.includes(x.t) || seen.has(x.id)) continue;
    if (x.id !== x.t && (!STACKABLE.includes(x.t) || (count[x.t] || 0) >= MAX_INST - 1)) continue;
    if (x.id !== x.t) count[x.t] = (count[x.t] || 0) + 1;
    seen.add(x.id); out.push({ id: x.id, t: x.t });
  }
  CHAIN_TOOLS.forEach((t, i) => {
    if (seen.has(t)) return;
    const at = out.findIndex(x => CHAIN_TOOLS.indexOf(x.t) > i);    // before the first tool that comes later by default
    out.splice(at < 0 ? out.length : at, 0, { id: t, t });
  });
  return out;
}
const isDefaultChain = c => c.length === CHAIN_TOOLS.length && c.every((x, i) => x.id === CHAIN_TOOLS[i]);
const defaultEdit = () => ({
  black: null, gray: null, white: null,
  tBlack: 0, tWhite: 255, strength: 1,
  curves: { m: ident(), r: ident(), g: ident(), b: ident() },
  ...defaultAdjust(),
  ...Object.assign({}, ...PANELS.map(defaultFilter)),
  off: {},              // sections switched off on their island: { sec: true }; values are kept, the edit skips them
  chain: defaultChain(),
});
const DEFAULT_EDIT_JSON = JSON.stringify(defaultEdit());
const defaultGeom = () => ({ q: 0, angle: 0, flip: false, crop: null, aspect: null });
const isDefaultGeom = g => !g || (!g.q && !g.angle && !g.flip && !g.crop);
function geomParams(g) {
  g = g || defaultGeom();
  if (g.off) g = defaultGeom();         // Crop island switched off: full image (the crop itself is kept)
  return { rot_q: g.q || 0, angle: g.angle || 0, flip: !!g.flip,
    crop_x: g.crop ? g.crop.x : 0, crop_y: g.crop ? g.crop.y : 0,
    crop_w: g.crop ? g.crop.w : 0, crop_h: g.crop ? g.crop.h : 0, crop_free: !!g.free };
}
function stageParams(stage, geom) {
  const p = Object.assign(clone(stage), geomParams(geom));
  if (p.color_on === false) { p.wp_on = false; p.ge_on = false; }
  delete p.color_on;
  return p;
}
/* What a switched-off island leaves of the edit: its values replaced by neutral ones. */
const SEC_NEUTRAL = {
  point: () => ({ black: null, gray: null, white: null }),
  adjust: defaultAdjust,
  curves: () => ({ curves: { m: ident(), r: ident(), g: ident(), b: ident() } }),
  ...Object.fromEntries(PANELS.map(f => [f.sec, () => defaultFilter(f)])),
};
/* instances: the edit keys of a tool, an instance's values, whether it does anything */
const TOOL_KEYS = t => Object.keys(SEC_NEUTRAL[t]());
function instValues(e, id, t) {
  if (id === t) { const n = SEC_NEUTRAL[t](); return Object.fromEntries(Object.keys(n).map(k => [k, e[k] !== undefined ? clone(e[k]) : n[k]])); }
  const v = e.inst && e.inst[id];
  return Object.assign(SEC_NEUTRAL[t](), v ? clone(v) : {});
}
const instActive = (e, x) => JSON.stringify(instValues(e, x.id, x.t)) !== JSON.stringify(SEC_NEUTRAL[x.t]());
const instOn = (e, id) => !(e.off && e.off[id]);
// the edit an extra instance's tables are computed from: its values in place of the flat ones
function instEdit(e, x) {
  const o = Object.assign({}, e, instValues(e, x.id, x.t));
  if (!instOn(e, x.id)) Object.assign(o, SEC_NEUTRAL[x.t]());
  delete o.chain; delete o.inst;
  return o;
}
function nextInstId(e, t) {
  const ids = new Set(chainOf(e).map(x => x.id));
  for (let n = 2; ; n++) if (!ids.has(t + '#' + n)) return t + '#' + n;
}
// the instances of tool t in chain order (the first instance included)
const instancesOf = (e, t) => chainOf(e).filter(x => x.t === t);

function effectiveEdit(e) {
  let out = e;
  for (const sec in e.off || {}) {
    if (!e.off[sec] || !SEC_NEUTRAL[sec]) continue;
    if (out === e) out = Object.assign({}, e);
    Object.assign(out, SEC_NEUTRAL[sec]());
  }
  return out;
}

const S = {
  files: [], cur: -1,
  stage: clone(DEFAULT_STAGE),
  edit: defaultEdit(),
  geom: defaultGeom(),
  cropMode: false,
  E: null,
  prefs: { fmt: 'WEBP', advance: true, stage: null, jpeg_q: 90, jpeg_mode: 'q', jpeg_d: 1.0,
    webp_lossy: false, webp_q: 90, avif_enc: 'svt', avif_crf: 25, avif_preset: 4, aom_q: 80, aom_speed: 6,
    split: true, secs: [] },
  cmp: { x: 0.5, data: null, path: null, key: null, bytes: 0, enc: '', fmt: '', opts: null, req: Date.now(), busy: false,
    jpegli: true, avif: true, svt: true },
  hist: [], fut: [], committed: null,
  img: null, outData: null, dispData: null, baseData: null, n: 0,
  hin: new Uint32Array(768), hout: new Uint32Array(768),
  hmx: new Uint32Array(256), hmn: new Uint32Array(256),
  tool: null, alt: false, before: false, space: false,
  thr: { shadows: null, highlights: null, neutral: null },
  modeCount: 0,
  view: { z: 1, x: 0, y: 0, fit: true },
  curveCh: 'm',
  reqId: 0, newFile: false, busy: false, clipboard: null,
  hover: null,
};

/* Transparent images: the cores compute RGB only (the alpha decides what shows, never the edit), so the
   stage's alpha (`base`, RGBA) is put back onto their pixels before they are shown. Same size only. */
function withAlpha(dst, base) {
  if (!base || dst.length !== base.length) return dst;
  for (let i = 3; i < dst.length; i += 4) dst[i] = base[i];
  return dst;
}

const canvas = $('#view'), ctx = canvas.getContext('2d');
const vp = $('#viewport'), stageEl = $('#stage');
