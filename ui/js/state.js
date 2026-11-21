'use strict';
/* Fast Image Editor - front end.
   Python delivers the "stage" image (resize + White-Patch + Gray-Edge); everything after
   that (point correction, curves, clipping views) runs here, live, on every change.
   Process sends the same parameters back to Python, which applies identical math at full res.
   The scripts in ui/js are plain (non-module) scripts sharing one global scope, loaded in the order
   listed in index.html; code that runs while loading may only use what earlier files defined. */

const $ = s => document.querySelector(s);
const clamp = (v, a, b) => v < a ? a : v > b ? b : v;
const clone = o => JSON.parse(JSON.stringify(o));
const basename = p => String(p).split(/[\\/]/).pop();
const api = () => window.pywebview.api;
const dpr = () => window.devicePixelRatio || 1;

const DEFAULT_STAGE = {
  resize_on: false, mode: 'preset', mpix: 2.0, prio: 'Balanced',
  preset_ar: '16:9', preset_res: 720, cw: 1920, ch: 1080, cmode: 'crop',
  wp_on: false, wp_pct: 99.0, wp_strength: 1.0,
  ge_on: false, ge_p: 1, ge_sigma: 2.0, ge_strength: 1.0,
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
/* Filters (nested under the Filters pill). Stored per image; bwParams() says which values are live.
   sec = card / pill id, fields = [key, label, default, min, max, step, decimals, unit] or '---' (divider).
   Opacity mixes the filtered result with the image before the filter (100% = full effect). */
const FILTERS = [
  { sec: 'f_bw', fields: [
    ['flt_bw_sat', 'Saturation', 0, -100, 100, 1, 1, '%'],
    ['flt_bw_warmth', 'Warmth', 0, -100, 100, 1, 1, '%'],
    ['flt_bw_psat', 'Perceptual Saturation ≈', 0, 0, 60, 1, 1, '%'],
    '---',
    ['flt_bw_opacity', 'Opacity', 100, 0, 100, 1, 1, '%'],
  ] },
];
const filterFields = f => f.fields.filter(a => a !== '---');
const defaultFilter = f => Object.fromEntries(filterFields(f).map(a => [a[0], a[2]]));
const filterActive = (f, e) => filterFields(f).some(([k, , def]) => e[k] !== undefined && e[k] !== def);
const defaultEdit = () => ({
  black: null, gray: null, white: null,
  tBlack: 0, tWhite: 255, strength: 1,
  curves: { m: ident(), r: ident(), g: ident(), b: ident() },
  ...defaultAdjust(),
  ...Object.assign({}, ...FILTERS.map(defaultFilter)),
});
const DEFAULT_EDIT_JSON = JSON.stringify(defaultEdit());
const defaultGeom = () => ({ q: 0, angle: 0, flip: false, crop: null, aspect: null });
const isDefaultGeom = g => !g || (!g.q && !g.angle && !g.flip && !g.crop);
function geomParams(g) {
  g = g || defaultGeom();
  return { rot_q: g.q || 0, angle: g.angle || 0, flip: !!g.flip,
    crop_x: g.crop ? g.crop.x : 0, crop_y: g.crop ? g.crop.y : 0,
    crop_w: g.crop ? g.crop.w : 0, crop_h: g.crop ? g.crop.h : 0 };
}
const stageParams = (stage, geom) => Object.assign(clone(stage), geomParams(geom));

const S = {
  files: [], cur: -1,
  stage: clone(DEFAULT_STAGE),
  edit: defaultEdit(),
  geom: defaultGeom(),
  cropMode: false,
  E: null,
  prefs: { fmt: 'WEBP', advance: true, stage: null, jpeg_q: 90, jpeg_mode: 'q', jpeg_d: 1.0,
    webp_lossy: false, webp_q: 90, avif_crf: 25, avif_preset: 4, aom_q: 80, aom_speed: 6, secs: [] },
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

const canvas = $('#view'), ctx = canvas.getContext('2d');
const vp = $('#viewport'), stageEl = $('#stage');
