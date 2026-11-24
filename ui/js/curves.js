'use strict';
/* Curves editor and output histogram. */

/* ======================================================================
   Curves editor. One editor per Curves island: the tool's own (#curve, S.edit.curves) and one per
   extra instance (instances.js: makeCurveEditor with that instance's curves). drawCurve() redraws
   them all. The input histogram is the one before the chain's first Curves step.
   ====================================================================== */
const CPAD = 8;
const CURVE_EDITORS = [];
const CURVE_HINT = 'Click to add · drag · double-click or drag out to remove';

function ensureCanvas(c, g) {
  const r = c.getBoundingClientRect();
  const w = Math.round(r.width * dpr()), h = Math.round(r.height * dpr());
  if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
  g.setTransform(dpr(), 0, 0, dpr(), 0, 0);
  return { w: r.width, h: r.height };
}
function c2p(x, y, w, h) { return [CPAD + x / 255 * (w - 2 * CPAD), CPAD + (1 - y / 255) * (h - 2 * CPAD)]; }
function p2c(px, py, w, h) {
  return [clamp(Math.round((px - CPAD) / (w - 2 * CPAD) * 255), 0, 255),
          clamp(Math.round((1 - (py - CPAD) / (h - 2 * CPAD)) * 255), 0, 255)];
}
const CH_COL = { m: '#f2f2f5', r: '#ff6b6b', g: '#5fd08a', b: '#6aa8ff' };
const CH_FILL = { m: 'rgba(200,205,212,.16)', r: 'rgba(255,107,107,.2)', g: 'rgba(95,208,138,.2)', b: 'rgba(106,168,255,.22)' };
function insertPoint(pts, x, y) {
  if (pts.some(p => p[0] === x)) return -1;
  let i = pts.findIndex(p => p[0] > x);
  if (i < 0) i = pts.length;
  pts.splice(i, 0, [x, y]);
  return i;
}

/* el = { cv, tabs: [buttons with data-ch], info, reset }; curves() = the curves object to edit
   ({ m, r, g, b } point lists), or null while it doesn't exist (instance removed) */
function makeCurveEditor(el, curves) {
  const { cv, tabs, info, reset } = el, g = cv.getContext('2d');
  let ch = 'm', drag = null;
  const pts = () => { const c = curves(); return c && c[ch]; };

  function draw() {
    if (!cv.isConnected || !curves()) return;
    const { w, h } = ensureCanvas(cv, g);
    if (!w) return;
    g.clearRect(0, 0, w, h);
    g.fillStyle = '#252528'; g.fillRect(0, 0, w, h);
    // grid
    g.strokeStyle = '#22262c'; g.lineWidth = 1;
    for (let i = 0; i <= 4; i++) {
      const [x] = c2p(i * 63.75, 0, w, h), [, y] = c2p(0, i * 63.75, w, h);
      g.beginPath(); g.moveTo(Math.round(x) + .5, CPAD); g.lineTo(Math.round(x) + .5, h - CPAD); g.stroke();
      g.beginPath(); g.moveTo(CPAD, Math.round(y) + .5); g.lineTo(w - CPAD, Math.round(y) + .5); g.stroke();
    }
    // histogram of the curve input
    if (S.img) {
      const hv = new Float64Array(256);
      for (let i = 0; i < 256; i++) {
        hv[i] = ch === 'm' ? S.hin[i] + S.hin[256 + i] + S.hin[512 + i]
          : S.hin[{ r: 0, g: 256, b: 512 }[ch] + i];
      }
      let mx = 1; for (let i = 1; i < 255; i++) mx = Math.max(mx, hv[i]);
      g.beginPath();
      const [x0, y0] = c2p(0, 0, w, h); g.moveTo(x0, y0);
      for (let i = 0; i < 256; i++) {
        const v = Math.min(1, Math.sqrt(hv[i] / mx)) * 0.92;
        const [x, y] = c2p(i, v * 255, w, h); g.lineTo(x, y);
      }
      const [x1] = c2p(255, 0, w, h); g.lineTo(x1, y0); g.closePath();
      g.fillStyle = CH_FILL[ch]; g.fill();
    }
    // baseline
    g.setLineDash([3, 4]); g.strokeStyle = '#3a3f46';
    g.beginPath(); let p = c2p(0, 0, w, h); g.moveTo(p[0], p[1]); p = c2p(255, 255, w, h); g.lineTo(p[0], p[1]); g.stroke();
    g.setLineDash([]);
    const drawLine = (ps, col, width) => {
      const L = curveLut(ps);
      g.strokeStyle = col; g.lineWidth = width; g.beginPath();
      for (let x = 0; x < 256; x++) { const [px, py] = c2p(x, L[x], w, h); x ? g.lineTo(px, py) : g.moveTo(px, py); }
      g.stroke();
    };
    const isIdent = ps => JSON.stringify(ps) === '[[0,0],[255,255]]';
    const C = curves();
    for (const c of ['r', 'g', 'b', 'm']) {
      if (c === ch || isIdent(C[c])) continue;
      g.globalAlpha = 0.38; drawLine(C[c], CH_COL[c], 1.2); g.globalAlpha = 1;
    }
    drawLine(C[ch], CH_COL[ch], 2);
    C[ch].forEach((pt, i) => {
      const [px, py] = c2p(pt[0], pt[1], w, h);
      const active = drag && drag.idx === i;
      g.beginPath(); g.arc(px, py, active ? 5.5 : 4.5, 0, Math.PI * 2);
      g.fillStyle = active ? '#d4d4d8' : '#252528'; g.fill();
      g.lineWidth = 1.8; g.strokeStyle = active ? '#d4d4d8' : CH_COL[ch]; g.stroke();
    });
  }

  function nearestPoint(px, py, w, h) {
    let best = -1, bd = 1e9;
    pts().forEach((pt, i) => {
      const [x, y] = c2p(pt[0], pt[1], w, h), d = Math.hypot(x - px, y - py);
      if (d < bd) { bd = d; best = i; }
    });
    return bd <= 9 ? best : -1;
  }

  cv.addEventListener('pointerdown', e => {
    if (e.button !== 0 || !pts()) return;
    const r = cv.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
    let idx = nearestPoint(px, py, r.width, r.height);
    if (idx < 0) {
      const [x, y] = p2c(px, py, r.width, r.height);
      idx = insertPoint(pts(), x, y);
      if (idx < 0) return;
      editChanged();
    }
    drag = { idx };
    cv.setPointerCapture(e.pointerId);
    draw();
  });
  cv.addEventListener('pointermove', e => {
    if (!pts()) return;
    const r = cv.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
    const w = r.width, h = r.height;
    const [x, y] = p2c(px, py, w, h);
    if (!drag) {
      const L = curveLut(pts());
      if (px >= 0 && px <= w && py >= 0 && py <= h) info.innerHTML = `<span class="mono">in ${x} → out ${L[x]}</span>`;
      return;
    }
    const P = pts();
    const outside = (px < -24 || px > w + 24 || py < -24 || py > h + 24);
    if (outside) {
      if (drag.idx >= 0 && P.length > 2) { P.splice(drag.idx, 1); drag.idx = -1; editChanged(); }
      info.innerHTML = '<span style="color:var(--red)">release to remove point</span>';
      return;
    }
    if (drag.idx < 0) {
      const i = insertPoint(P, x, y);
      if (i < 0) return;
      drag.idx = i;
    }
    const i = drag.idx, n = P.length;
    const lo = i > 0 ? P[i - 1][0] + 1 : 0, hi = i < n - 1 ? P[i + 1][0] - 1 : 255;
    P[i] = [clamp(x, lo, hi), y];
    info.innerHTML = `<span class="mono">in ${P[i][0]} → out ${P[i][1]}</span>`;
    editChanged();
  });
  const endDrag = () => { if (drag) { drag = null; commit(); draw(); } };
  cv.addEventListener('pointerup', endDrag);
  cv.addEventListener('pointercancel', endDrag);
  cv.addEventListener('pointerleave', () => { if (!drag) info.textContent = CURVE_HINT; });
  cv.addEventListener('dblclick', e => {
    if (!pts()) return;
    const r = cv.getBoundingClientRect(), P = pts();
    const i = nearestPoint(e.clientX - r.left, e.clientY - r.top, r.width, r.height);
    if (i >= 0 && P.length > 2) { P.splice(i, 1); editChanged(); commit(); }
  });
  tabs.forEach(b => b.addEventListener('click', () => {
    ch = b.dataset.ch;
    tabs.forEach(x => x.classList.toggle('on', x === b));
    draw();
  }));
  reset.addEventListener('click', () => { const c = curves(); if (!c) return; c[ch] = ident(); editChanged(); commit(); });
  new ResizeObserver(() => draw()).observe(cv);
  const ed = { draw, cv };
  CURVE_EDITORS.push(ed);
  return ed;
}
function drawCurve() {
  for (let i = CURVE_EDITORS.length - 1; i >= 0; i--) {
    if (!CURVE_EDITORS[i].cv.isConnected && CURVE_EDITORS[i].cv.dataset.inst) CURVE_EDITORS.splice(i, 1);   // removed instance
    else CURVE_EDITORS[i].draw();
  }
}
// the tool's own Curves island
makeCurveEditor({ cv: $('#curve'), tabs: [...document.querySelectorAll('#curveTabs button')], info: $('#curveInfo'), reset: $('#resetCurve') },
  () => S.edit.curves);

/* ======================================================================
   Histogram (output)
   ====================================================================== */
const hc = $('#histo'), hctx = hc.getContext('2d');
function drawHisto() {
  const { w, h } = ensureCanvas(hc, hctx);
  if (!w) return;
  const g = hctx;
  g.clearRect(0, 0, w, h);
  g.fillStyle = '#252528'; g.fillRect(0, 0, w, h);
  if (!S.img || !S.n) { $('#clipLo').textContent = $('#clipHi').textContent = '0%'; return; }
  let mx = 1;
  for (let c = 0; c < 3; c++) for (let i = 1; i < 255; i++) mx = Math.max(mx, S.hout[c * 256 + i]);
  g.globalCompositeOperation = 'lighter';
  const cols = ['rgba(255,80,80,.62)', 'rgba(80,220,120,.55)', 'rgba(90,150,255,.68)'];
  for (let c = 0; c < 3; c++) {
    g.beginPath(); g.moveTo(0, h);
    for (let i = 0; i < 256; i++) {
      const v = Math.min(1, Math.sqrt(S.hout[c * 256 + i] / mx));
      g.lineTo(i / 255 * w, h - v * (h - 6));
    }
    g.lineTo(w, h); g.closePath(); g.fillStyle = cols[c]; g.fill();
  }
  g.globalCompositeOperation = 'source-over';
  const lo = S.hmn[0] / S.n * 100, hi = S.hmx[255] / S.n * 100;
  const f = v => v === 0 ? '0%' : v < 0.01 ? '<0.01%' : v.toFixed(v < 1 ? 2 : 1) + '%';
  $('#clipLo').textContent = '▼ ' + f(lo); $('#clipHi').textContent = f(hi) + ' ▲';
  $('#clipLo').classList.toggle('warn', lo > 0.5); $('#clipHi').classList.toggle('warn', hi > 0.5);
}
new ResizeObserver(() => drawHisto()).observe(hc);
