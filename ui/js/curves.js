'use strict';
/* Curves editor and output histogram. */

/* ======================================================================
   Curves editor
   ====================================================================== */
const cv = $('#curve'), cctx = cv.getContext('2d');
const CPAD = 8;
let cdrag = null;

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
const CH_COL = { m: '#e4f5f3', r: '#ff6b6b', g: '#5fd08a', b: '#6aa8ff' };
const CH_FILL = { m: 'rgba(200,205,212,.16)', r: 'rgba(255,107,107,.2)', g: 'rgba(95,208,138,.2)', b: 'rgba(106,168,255,.22)' };

function drawCurve() {
  const { w, h } = ensureCanvas(cv, cctx);
  if (!w) return;
  const g = cctx;
  g.clearRect(0, 0, w, h);
  g.fillStyle = '#001e21'; g.fillRect(0, 0, w, h);
  // grid
  g.strokeStyle = '#22262c'; g.lineWidth = 1;
  for (let i = 0; i <= 4; i++) {
    const [x] = c2p(i * 63.75, 0, w, h), [, y] = c2p(0, i * 63.75, w, h);
    g.beginPath(); g.moveTo(Math.round(x) + .5, CPAD); g.lineTo(Math.round(x) + .5, h - CPAD); g.stroke();
    g.beginPath(); g.moveTo(CPAD, Math.round(y) + .5); g.lineTo(w - CPAD, Math.round(y) + .5); g.stroke();
  }
  // histogram of the curve input
  const ch = S.curveCh;
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
  const drawLine = (pts, col, width) => {
    const L = curveLut(pts);
    g.strokeStyle = col; g.lineWidth = width; g.beginPath();
    for (let x = 0; x < 256; x++) { const [px, py] = c2p(x, L[x], w, h); x ? g.lineTo(px, py) : g.moveTo(px, py); }
    g.stroke();
  };
  const isIdent = pts => JSON.stringify(pts) === '[[0,0],[255,255]]';
  for (const c of ['r', 'g', 'b', 'm']) {
    if (c === ch || isIdent(S.edit.curves[c])) continue;
    g.globalAlpha = 0.38; drawLine(S.edit.curves[c], CH_COL[c], 1.2); g.globalAlpha = 1;
  }
  const pts = S.edit.curves[ch];
  drawLine(pts, CH_COL[ch], 2);
  pts.forEach((pt, i) => {
    const [px, py] = c2p(pt[0], pt[1], w, h);
    const active = cdrag && cdrag.idx === i;
    g.beginPath(); g.arc(px, py, active ? 5.5 : 4.5, 0, Math.PI * 2);
    g.fillStyle = active ? '#00ffaa' : '#001e21'; g.fill();
    g.lineWidth = 1.8; g.strokeStyle = active ? '#00ffaa' : CH_COL[ch]; g.stroke();
  });
}

function nearestPoint(px, py, w, h) {
  const pts = S.edit.curves[S.curveCh];
  let best = -1, bd = 1e9;
  pts.forEach((pt, i) => {
    const [x, y] = c2p(pt[0], pt[1], w, h), d = Math.hypot(x - px, y - py);
    if (d < bd) { bd = d; best = i; }
  });
  return bd <= 9 ? best : -1;
}
function insertPoint(pts, x, y) {
  if (pts.some(p => p[0] === x)) return -1;
  let i = pts.findIndex(p => p[0] > x);
  if (i < 0) i = pts.length;
  pts.splice(i, 0, [x, y]);
  return i;
}

cv.addEventListener('pointerdown', e => {
  if (e.button !== 0) return;
  const r = cv.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
  const pts = S.edit.curves[S.curveCh];
  let idx = nearestPoint(px, py, r.width, r.height);
  if (idx < 0) {
    const [x, y] = p2c(px, py, r.width, r.height);
    idx = insertPoint(pts, x, y);
    if (idx < 0) return;
    editChanged();
  }
  cdrag = { idx };
  cv.setPointerCapture(e.pointerId);
  drawCurve();
});
cv.addEventListener('pointermove', e => {
  const r = cv.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
  const w = r.width, h = r.height;
  const [x, y] = p2c(px, py, w, h);
  if (!cdrag) {
    const L = curveLut(S.edit.curves[S.curveCh]);
    if (px >= 0 && px <= w && py >= 0 && py <= h) $('#curveInfo').innerHTML = `<span class="mono">in ${x} → out ${L[x]}</span>`;
    return;
  }
  const pts = S.edit.curves[S.curveCh];
  const outside = (px < -24 || px > w + 24 || py < -24 || py > h + 24);
  if (outside) {
    if (cdrag.idx >= 0 && pts.length > 2) { pts.splice(cdrag.idx, 1); cdrag.idx = -1; editChanged(); }
    $('#curveInfo').innerHTML = '<span style="color:var(--red)">release to remove point</span>';
    return;
  }
  if (cdrag.idx < 0) {
    const i = insertPoint(pts, x, y);
    if (i < 0) return;
    cdrag.idx = i;
  }
  const i = cdrag.idx, n = pts.length;
  const lo = i > 0 ? pts[i - 1][0] + 1 : 0, hi = i < n - 1 ? pts[i + 1][0] - 1 : 255;
  pts[i] = [clamp(x, lo, hi), y];
  $('#curveInfo').innerHTML = `<span class="mono">in ${pts[i][0]} → out ${pts[i][1]}</span>`;
  editChanged();
});
const endCurveDrag = () => { if (cdrag) { cdrag = null; commit(); drawCurve(); } };
cv.addEventListener('pointerup', endCurveDrag);
cv.addEventListener('pointercancel', endCurveDrag);
cv.addEventListener('pointerleave', () => { if (!cdrag) $('#curveInfo').textContent = 'Click to add · drag · double-click or drag out to remove'; });
cv.addEventListener('dblclick', e => {
  const r = cv.getBoundingClientRect();
  const pts = S.edit.curves[S.curveCh];
  const i = nearestPoint(e.clientX - r.left, e.clientY - r.top, r.width, r.height);
  if (i >= 0 && pts.length > 2) { pts.splice(i, 1); editChanged(); commit(); }
});

document.querySelectorAll('#curveTabs button').forEach(b => b.addEventListener('click', () => {
  S.curveCh = b.dataset.ch;
  document.querySelectorAll('#curveTabs button').forEach(x => x.classList.toggle('on', x === b));
  drawCurve();
}));
$('#resetCurve').addEventListener('click', () => { S.edit.curves[S.curveCh] = ident(); editChanged(); commit(); });
new ResizeObserver(() => drawCurve()).observe(cv);

/* ======================================================================
   Histogram (output)
   ====================================================================== */
const hc = $('#histo'), hctx = hc.getContext('2d');
function drawHisto() {
  const { w, h } = ensureCanvas(hc, hctx);
  if (!w) return;
  const g = hctx;
  g.clearRect(0, 0, w, h);
  g.fillStyle = '#001e21'; g.fillRect(0, 0, w, h);
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
