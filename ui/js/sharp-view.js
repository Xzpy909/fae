'use strict';
/* Screen-resolution view. Zooming and panning scale the preview canvas with the browser's quick
   filter, which softens every size that isn't 1:1 (and how soft depends on which preview is on screen).
   A moment after the view and the pixels stop changing, the visible part is redrawn at exactly the
   screen's pixel size with Lanczos-3 (sharp-worker.js, a few workers in parallel) into #sharp over the
   canvas; any change hides it until the next redraw, like an image viewer settling after a zoom.
   Only below 1:1 (shrinking): at 1:1 and above the canvas already shows every pixel. */
const Sharp = (() => {
  const IDLE = 150;                     // ms without changes before the redraw
  const cv = document.createElement('canvas'), g = cv.getContext('2d');
  cv.id = 'sharp';
  stageEl.after(cv);
  const workers = [];
  let timer = 0, gen = 0, pending = 0, img = null;

  function pool() {
    if (!workers.length) {
      const n = Math.max(1, Math.min(8, (navigator.hardwareConcurrency || 4) - 1));
      for (let i = 0; i < n; i++) {
        const w = new Worker('js/sharp-worker.js');
        w.onmessage = onBand;
        w.onerror = () => {};
        workers.push(w);
      }
    }
    return workers;
  }

  function hide() { gen++; cv.classList.remove('show'); }

  // something changed (view, pixels, layout): hide now, redraw once things are quiet
  function dirty() {
    hide();
    clearTimeout(timer);
    timer = setTimeout(draw, IDLE);
  }

  function draw() {
    const I = S.img, v = S.view, d = dpr(), src = S.shown;
    const s = v.z * d;                  // screen pixels per canvas pixel
    if (!I || !src || S.cropMode || S.live || canvas.style.display !== 'block' || s >= 0.999
        || src.width !== canvas.width || src.height !== canvas.height) return;
    const r = vp.getBoundingClientRect();
    if (!r.width || !r.height) return;
    // the overlay covers the viewport with its pixels on whole screen pixels
    const L = Math.round(r.left * d), T = Math.round(r.top * d);
    const ow = Math.round((r.left + r.width) * d) - L, oh = Math.round((r.top + r.height) * d) - T;
    // screen position of the canvas corner (whole pixels: applyView snaps the translate the same way)
    const ox = Math.round((r.left + v.x) * d) - L, oy = Math.round((r.top + v.y) * d) - T;
    const pw = src.width, ph = src.height;
    const X0 = Math.max(0, ox), X1 = Math.min(ow, Math.round(ox + pw * s));
    const Y0 = Math.max(0, oy), Y1 = Math.min(oh, Math.round(oy + ph * s));
    if (X1 <= X0 || Y1 <= Y0) return;
    let f = 1;
    while (s * f < 0.5) f *= 2;
    const R = (3 / (s * f) + 2) * f;     // filter reach in canvas pixels
    const al = x => Math.floor(x / f) * f;
    const rx = Math.max(0, al((X0 - ox) / s - R)), rx1 = Math.min(pw, Math.ceil((X1 - ox) / s + R));
    const rw = rx1 - rx;

    const my = ++gen;
    if (cv.width !== ow || cv.height !== oh) { cv.width = ow; cv.height = oh; }
    cv.style.width = ow / d + 'px'; cv.style.height = oh / d + 'px';
    cv.style.left = (L / d - r.left) + 'px'; cv.style.top = (T / d - r.top) + 'px';
    img = new ImageData(ow, oh);         // transparent outside the image
    const ws = pool(), n = ws.length, band = Math.ceil((Y1 - Y0) / n), data = src.data;
    pending = 0;
    for (let i = 0; i < n; i++) {
      const b0 = Y0 + i * band, b1 = Math.min(Y1, b0 + band);
      if (b1 <= b0) break;
      const ry = Math.max(0, al((b0 - oy) / s - R)), ry1 = Math.min(ph, Math.ceil((b1 - oy) / s + R));
      const rh = ry1 - ry, buf = new Uint8ClampedArray(rw * rh * 4);
      for (let y = 0; y < rh; y++) {
        const o = ((ry + y) * pw + rx) * 4;
        buf.set(data.subarray(o, o + rw * 4), y * rw * 4);
      }
      pending++;
      ws[i].postMessage({ id: my, buf: buf.buffer, rx, ry, rw, rh, f, s, ox, oy, X0, Y0: b0, dw: X1 - X0, dh: b1 - b0 },
                        [buf.buffer]);
    }
    img.X0 = X0;
  }

  function onBand({ data: m }) {
    if (m.id !== gen || !img) return;            // the view or pixels changed meanwhile
    const out = new Uint8ClampedArray(m.out), W = img.width * 4, x = img.X0 * 4, rowLen = m.dw * 4;
    for (let y = 0; y < m.dh; y++) img.data.set(out.subarray(y * rowLen, (y + 1) * rowLen), (m.Y0 + y) * W + x);
    if (--pending === 0) {
      g.putImageData(img, 0, 0);
      cv.classList.add('show');
      img = null;
    }
  }

  new ResizeObserver(dirty).observe(vp);
  window.addEventListener('resize', dirty);
  return { dirty, hide };
})();
