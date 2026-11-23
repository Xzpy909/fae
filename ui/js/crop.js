'use strict';
/* Crop & rotate mode.
   Coordinates: "frame" space = source pixels, origin at the image centre, after the
   rotation. The crop rectangle is axis-aligned in frame space ({x, y} = centre, w, h).
   A frame point p maps back to the source with R(-theta): that is what keeps the crop
   "bound aware" (every corner must land inside the source), and it is exactly what
   apply_geometry() in fast_image_editor.py does with cv2.warpAffine. */

const Crop = (() => {
  const layer = $('#cropLayer'), cc = $('#cropCanvas'), g = cc.getContext('2d');
  const ruler = $('#ruler'), rg = ruler.getContext('2d');
  const MIN = 16, PPD = 9;   // min crop size (source px), ruler px per degree

  const ASPECTS = [
    ['free', 'Free', null], ['original', 'Original', null], ['square', 'Square', 1],
    ['9:16', '9 : 16', 9 / 16], ['16:9', '16 : 9', 16 / 9], ['21:9', '21 : 9', 21 / 9], ['4:5', '4 : 5', 4 / 5], ['5:4', '5 : 4', 5 / 4],
    ['3:4', '3 : 4', 3 / 4], ['4:3', '4 : 3', 4 / 3], ['2:3', '2 : 3', 2 / 3], ['3:2', '3 : 2', 3 / 2],
    ['5:7', '5 : 7', 5 / 7], ['7:5', '7 : 5', 7 / 5], ['1:2', '1 : 2', 1 / 2], ['2:1', '2 : 1', 2],
  ];
  const AMAP = Object.fromEntries(ASPECTS.map(a => [a[0], a[2]]));

  let C = null;
  const view = { z: 1, ox: 0, oy: 0, w: 0, h: 0 };
  let drag = null, rotating = null, hover = null, raf = false;

  /* ---------------- geometry ---------------- */
  const theta = () => (C.q * 90 + C.angle) * Math.PI / 180;
  const frameWH = () => (C.q % 2 ? [C.H, C.W] : [C.W, C.H]);
  function ratio() {
    if (C.aspect === 'free') return null;
    if (C.aspect === 'original') { const [w, h] = frameWH(); return w / h; }
    return AMAP[C.aspect] || null;
  }
  function contains(r) {
    const t = theta(), c = Math.cos(t), s = Math.sin(t);
    const hw = C.W / 2 + 0.02, hh = C.H / 2 + 0.02;
    for (const [dx, dy] of [[-1, -1], [1, -1], [1, 1], [-1, 1]]) {
      const px = r.x + dx * r.w / 2, py = r.y + dy * r.h / 2;
      const sx = c * px + s * py, sy = -s * px + c * py;
      if (Math.abs(sx) > hw || Math.abs(sy) > hh) return false;
    }
    return true;
  }
  function maxRect(r, cx = 0, cy = 0) {
    if (!r) { const [w, h] = frameWH(); r = w / h; }
    let lo = 0, hi = C.W + C.H;
    for (let i = 0; i < 48; i++) {
      const m = (lo + hi) / 2;
      if (contains({ x: cx, y: cy, w: m * r, h: m })) lo = m; else hi = m;
    }
    return { x: cx, y: cy, w: lo * r, h: lo };
  }
  function scaleFit(d) {
    let lo = 0, hi = 1;
    for (let i = 0; i < 36; i++) {
      const m = (lo + hi) / 2;
      if (contains({ x: d.x, y: d.y, w: d.w * m, h: d.h * m })) lo = m; else hi = m;
    }
    return { x: d.x, y: d.y, w: d.w * lo, h: d.h * lo };
  }
  /* largest version of `d` (same centre, same aspect) that fits */
  function shrink(d) {
    if (contains(d)) return { ...d };
    let r = scaleFit(d);
    if (r.w < MIN || r.h < MIN) r = scaleFit({ ...d, x: 0, y: 0 });
    return r;
  }
  const lerp = (a, b, t) => ({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, w: a.w + (b.w - a.w) * t, h: a.h + (b.h - a.h) * t });
  function searchT(a, b) {
    let lo = 0, hi = 1;
    for (let i = 0; i < 30; i++) { const m = (lo + hi) / 2; if (contains(lerp(a, b, m))) lo = m; else hi = m; }
    return lerp(a, b, lo);
  }
  /* move from valid rect `a` toward proposal `b` as far as the image bounds allow */
  function fitTowards(a, b, locked) {
    if (contains(b)) return b;
    let r = searchT(a, b);
    if (!locked) {
      const bx = { ...r, x: b.x, w: b.w };
      r = contains(bx) ? bx : searchT(r, bx);
      const by = { ...r, y: b.y, h: b.h };
      r = contains(by) ? by : searchT(r, by);
    }
    return r;
  }
  function fullFrame() { const [w, h] = frameWH(); return { x: 0, y: 0, w, h }; }
  function defaultCrop() { const r = ratio(); return C.angle === 0 && (!r || C.aspect === 'original') ? fullFrame() : maxRect(r); }

  /* ---------------- enter / exit ---------------- */
  async function enter() {
    if (typeof FilterPop !== 'undefined') FilterPop.close();
    if (S.cropMode || !S.img || S.cur < 0) return;
    const f = S.files[S.cur];
    setTool(null); setAlt(false);
    if (!f.src) {
      spinner(true);
      let r;
      try { r = await api().load_source(f.path); } catch (e) { r = { ok: false, error: String(e) }; }
      if (!r.ok) { spinner(false); toast(r.error, true); return; }
      const buf = await (await fetch('/preview/' + r.token)).arrayBuffer();
      spinner(false);
      f.src = { W: r.W, H: r.H, pw: r.pw, ph: r.ph, base: new Uint8ClampedArray(buf) };
    }
    S.files.forEach(x => { if (x !== f) delete x.src; });   // keep memory in check
    if (S.files[S.cur] !== f) return;
    if (S.geom && S.geom.off) setSecOn('crop', true);     // editing the crop turns its island back on
    const G = S.geom || defaultGeom();
    C = {
      W: f.src.W, H: f.src.H, info: f.src,
      q: G.q || 0, angle: G.angle || 0, flip: !!G.flip,
      aspect: G.aspect || (() => { const ra = resizeCropAspect(); return ra && ra.key && AMAP[ra.key] ? ra.key : 'original'; })(),
      img: document.createElement('canvas'),
    };
    C.img.width = f.src.pw; C.img.height = f.src.ph;
    refreshImage();
    C.crop = G.crop ? shrink({ ...G.crop }) : defaultCrop();
    C.desired = { ...C.crop };
    S.cropMode = true;
    layer.classList.add('show');
    buildAspects();
    updateHint();
    layout(); draw(); drawRuler();
  }

  function exit(cancel) {
    if (!S.cropMode) return;
    S.cropMode = false;
    layer.classList.remove('show');
    if (!cancel && C) {
      const [fw, fh] = frameWH(), c = C.crop;
      const angle = Math.round(C.angle * 10) / 10;
      const full = angle === 0 && Math.abs(c.w - fw) < 0.5 && Math.abs(c.h - fh) < 0.5 && Math.abs(c.x) < 0.5 && Math.abs(c.y) < 0.5;
      const r2 = v => Math.round(v * 100) / 100;
      const ng = {
        q: C.q, angle, flip: C.flip, aspect: C.aspect,
        crop: full ? null : { x: r2(c.x), y: r2(c.y), w: r2(c.w), h: r2(c.h) },
      };
      const key = o => JSON.stringify([o.q || 0, o.angle || 0, !!o.flip, o.crop || null]);
      const changed = key(ng) !== key(S.geom || defaultGeom());
      S.geom = ng;
      if (changed) {
        for (const t of ['black', 'gray', 'white']) if (S.edit[t]) S.edit[t].pos = null;
        updateMarkers();
        commit();
        S.newFile = true;
        requestStage(true);
      }
    }
    C = null; drag = null; rotating = null;
    updateSummary(); markCurrentEdited();
  }

  // the whole uncropped image with the current edit, through the preview core (wasm-preview.js)
  function refreshImage() {
    if (!C) return;
    const cur = C, s = C.info;
    if (!WP.ready()) {                 // core still starting (or missing): the unedited image for now
      C.img.getContext('2d').putImageData(new ImageData(s.base.slice(), s.pw, s.ph), 0, 0);
      return;
    }
    WP.render(s, S.E, r => {
      if (C !== cur || r.out.byteLength !== s.pw * s.ph * 4) return;
      C.img.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(r.out), s.pw, s.ph), 0, 0);
      draw();
    });
  }

  /* ---------------- actions ---------------- */
  function setAngle(v) {
    v = clamp(Math.round(v * 10) / 10, -45, 45);
    if (Math.abs(v) < 0.05) v = 0;
    if (v === C.angle) return;
    C.angle = v;
    C.crop = (v === 0 && C.aspect === 'original' && sameRect(C.desired, fullFrame())) ? fullFrame() : shrink(C.desired);
    draw(); drawRuler(); follow();
  }
  const sameRect = (a, b) => a && b && Math.abs(a.x - b.x) < .5 && Math.abs(a.y - b.y) < .5 && Math.abs(a.w - b.w) < .5 && Math.abs(a.h - b.h) < .5;
  function setAspect(key) {
    C.aspect = key;
    const r = ratio();
    if (r) {
      const centred = maxRect(r, 0, 0), here = maxRect(r, C.crop.x, C.crop.y);
      C.crop = here.w * here.h > centred.w * centred.h * 0.98 ? here : centred;
    }
    C.desired = { ...C.crop };
    buildAspects(); updateHint(); draw(); follow();
  }
  function rotate90(dir) {
    C.q = (C.q + (dir > 0 ? 1 : 3)) % 4;
    C.crop = defaultCrop(); C.desired = { ...C.crop };
    layout(); draw();
  }
  function flip() {
    // mirror the whole view: crop and angle mirror with it
    C.flip = !C.flip;
    C.q = (4 - C.q) % 4;
    C.angle = -C.angle;
    C.crop.x = -C.crop.x; C.desired.x = -C.desired.x;
    C.crop = shrink(C.crop);
    layout(); draw(); drawRuler();
  }
  function reset() {
    C.q = 0; C.angle = 0; C.flip = false;
    C.crop = defaultCrop(); C.desired = { ...C.crop };
    layout(true); draw(); drawRuler();
  }
  /* Alt/Ctrl + wheel: scale the crop box. Zooms toward the cursor; when growing hits the
     image edge the box slides inward, and at the limit it becomes the largest box of that
     shape. The aspect ratio of the current box is kept. */
  function zoomCrop(f, px, py) {
    const c = C.crop;
    const ax = px == null ? c.x : clamp((px - view.ox) / view.z, c.x - c.w / 2, c.x + c.w / 2);
    const ay = py == null ? c.y : clamp((py - view.oy) / view.z, c.y - c.h / 2, c.y + c.h / 2);
    const s = Math.max(f, MIN / c.w, MIN / c.h);
    let b = { x: ax + (c.x - ax) * s, y: ay + (c.y - ay) * s, w: c.w * s, h: c.h * s };
    if (!contains(b)) {
      const toward = t => ({ ...b, x: b.x * (1 - t), y: b.y * (1 - t) });
      if (contains(toward(1))) {
        let lo = 0, hi = 1;
        for (let i = 0; i < 30; i++) { const m = (lo + hi) / 2; if (contains(toward(m))) hi = m; else lo = m; }
        b = toward(hi);
      } else {
        b = maxRect(c.w / c.h, 0, 0);
        if (b.w <= c.w) b = { ...c };
      }
    }
    C.crop = b; C.desired = { ...b };
    draw(); follow();
  }
  function nudge(dx, dy) {
    const b = { ...C.crop, x: C.crop.x + dx, y: C.crop.y + dy };
    C.crop = fitTowards(C.crop, b, false); C.desired = { ...C.crop };
    draw(); follow();
  }

  /* ---------------- camera ----------------
     The view follows the crop box: once the box is smaller than ~CAM_FILL of the screen
     area, the camera zooms in so the box edges sit near the screen edges, and pans to keep
     it centred (never panning past the image frame). It eases toward its target and is
     frozen while a handle/box drag is in progress so the box does not move under the mouse. */
  const CAM_FILL = 0.8, CAM_MAX_Z = 4, CAM_EASE = 0.22;
  const cam = { z: 1, cx: 0, cy: 0, tz: 1, tcx: 0, tcy: 0, A: null, anim: false };

  function camTarget() {
    const A = cam.A, c = C.crop, [fw, fh] = frameWH();
    let z = A.z0;
    if (c) z = Math.max(A.z0, Math.min(Math.min(A.aw / c.w, A.ah / c.h) * CAM_FILL, Math.max(A.z0, CAM_MAX_Z)));
    const hx = Math.max(0, fw / 2 - A.aw / (2 * z)), hy = Math.max(0, fh / 2 - A.ah / (2 * z));
    cam.tz = z;
    cam.tcx = c ? clamp(c.x, -hx, hx) : 0;
    cam.tcy = c ? clamp(c.y, -hy, hy) : 0;
  }
  function camApply() {
    const A = cam.A;
    view.z = cam.z;
    view.ox = A.sx0 - cam.cx * cam.z;
    view.oy = A.sy0 - cam.cy * cam.z;
  }
  function camStep() {
    const dz = cam.tz - cam.z, dx = cam.tcx - cam.cx, dy = cam.tcy - cam.cy;
    const done = Math.abs(dz) / cam.tz < 0.002 && Math.abs(dx * cam.z) < 0.5 && Math.abs(dy * cam.z) < 0.5;
    if (done) { cam.z = cam.tz; cam.cx = cam.tcx; cam.cy = cam.tcy; }
    else {
      // ease zoom in log space so in/out feel the same
      cam.z = Math.exp(Math.log(cam.z) + (Math.log(cam.tz) - Math.log(cam.z)) * CAM_EASE);
      cam.cx += dx * CAM_EASE; cam.cy += dy * CAM_EASE;
    }
    camApply(); draw();
    if (!done && C && !drag) requestAnimationFrame(camStep); else cam.anim = false;
  }
  function follow() {
    if (!C || !cam.A || drag) return;
    camTarget();
    if (!cam.anim) { cam.anim = true; requestAnimationFrame(camStep); }
  }

  /* ---------------- drawing ---------------- */
  function layout(animate) {
    if (!C) return;
    const r = cc.getBoundingClientRect(), d = dpr();
    if (cc.width !== Math.round(r.width * d) || cc.height !== Math.round(r.height * d)) {
      cc.width = Math.round(r.width * d); cc.height = Math.round(r.height * d);
    }
    const top = 58, bot = $('#cropBar').offsetHeight + 18, side = 40;
    const [fw, fh] = frameWH();
    const aw = r.width - 2 * side, ah = r.height - top - bot;
    cam.A = { aw, ah, sx0: side + aw / 2, sy0: top + ah / 2, z0: Math.max(0.01, Math.min(aw / fw, ah / fh)) };
    view.w = r.width; view.h = r.height;
    camTarget();
    if (animate) { follow(); return; }
    cam.z = cam.tz; cam.cx = cam.tcx; cam.cy = cam.tcy;
    camApply();
  }
  const toS = (x, y) => [view.ox + x * view.z, view.oy + y * view.z];
  function requestDraw() { if (!raf) { raf = true; requestAnimationFrame(() => { raf = false; draw(); }); } }

  function draw() {
    if (!C) return;
    const d = dpr(), W = view.w, H = view.h;
    g.setTransform(d, 0, 0, d, 0, 0);
    g.fillStyle = '#001517'; g.fillRect(0, 0, W, H);
    // image
    g.save();
    g.translate(view.ox, view.oy); g.scale(view.z, view.z); g.rotate(theta()); g.scale(C.flip ? -1 : 1, 1);
    g.imageSmoothingEnabled = true; g.imageSmoothingQuality = 'high';
    g.drawImage(C.img, -C.W / 2, -C.H / 2, C.W, C.H);
    g.restore();
    // mask outside crop
    const c = C.crop, [x0, y0] = toS(c.x - c.w / 2, c.y - c.h / 2), sw = c.w * view.z, sh = c.h * view.z;
    g.fillStyle = 'rgba(0, 13, 14,.66)';
    g.beginPath(); g.rect(0, 0, W, H); g.rect(x0, y0, sw, sh); g.fill('evenodd');
    // grid
    const lines = rotating ? 10 : 3;
    g.strokeStyle = rotating ? 'rgba(255,255,255,.28)' : (drag ? 'rgba(255,255,255,.4)' : 'rgba(255,255,255,.16)');
    g.lineWidth = 1;
    if (rotating) g.setLineDash([4, 3]);
    g.beginPath();
    for (let i = 1; i < lines; i++) {
      const gx = Math.round(x0 + sw * i / lines) + .5, gy = Math.round(y0 + sh * i / lines) + .5;
      g.moveTo(gx, y0); g.lineTo(gx, y0 + sh); g.moveTo(x0, gy); g.lineTo(x0 + sw, gy);
    }
    g.stroke(); g.setLineDash([]);
    // border
    g.strokeStyle = 'rgba(255,255,255,.92)'; g.lineWidth = 1;
    g.strokeRect(Math.round(x0) + .5, Math.round(y0) + .5, Math.round(sw) - 1, Math.round(sh) - 1);
    // handles
    g.fillStyle = '#fff';
    const L = Math.min(20, sw / 3, sh / 3), T = 3;
    const corner = (cx, cy, sx, sy) => {
      g.fillRect(sx > 0 ? cx - T : cx - L + T, cy - T + (sy > 0 ? 0 : T), L, T);
      g.fillRect(cx - T + (sx > 0 ? 0 : T), sy > 0 ? cy - T : cy - L + T, T, L);
    };
    corner(x0, y0, 1, 1); corner(x0 + sw, y0, -1, 1); corner(x0, y0 + sh, 1, -1); corner(x0 + sw, y0 + sh, -1, -1);
    const E = Math.min(24, sw / 4, sh / 4);
    g.fillRect(x0 + sw / 2 - E / 2, y0 - T, E, T); g.fillRect(x0 + sw / 2 - E / 2, y0 + sh, E, T);
    g.fillRect(x0 - T, y0 + sh / 2 - E / 2, T, E); g.fillRect(x0 + sw, y0 + sh / 2 - E / 2, T, E);
    // labels
    g.font = '500 11px Inter, "Segoe UI", system-ui, sans-serif'; g.textAlign = 'center'; g.textBaseline = 'middle';
    const pill = (txt, cx, cy) => {
      const tw = g.measureText(txt).width + 14;
      g.fillStyle = 'rgba(20,22,25,.82)'; g.beginPath();
      g.roundRect ? g.roundRect(cx - tw / 2, cy - 10, tw, 20, 4) : g.rect(cx - tw / 2, cy - 10, tw, 20);
      g.fill(); g.fillStyle = '#e4f5f3'; g.fillText(txt, cx, cy + .5);
    };
    if (C.aspect !== 'free' && sh > 60) pill(ASPECTS.find(a => a[0] === C.aspect)[1].replace(/ /g, ''), x0 + sw / 2, y0 + 22);
    if (sh > 40) pill(`${Math.round(c.w)} × ${Math.round(c.h)}`, x0 + sw / 2, y0 + sh - 22);
  }

  function drawRuler() {
    if (!C) return;
    const r = ruler.getBoundingClientRect(), d = dpr();
    if (ruler.width !== Math.round(r.width * d)) { ruler.width = Math.round(r.width * d); ruler.height = Math.round(r.height * d); }
    rg.setTransform(d, 0, 0, d, 0, 0);
    const W = r.width, H = r.height, cx = W / 2;
    rg.clearRect(0, 0, W, H);
    for (let deg = -45; deg <= 45; deg++) {
      const x = cx + (deg - C.angle) * PPD;
      if (x < 2 || x > W - 2) continue;
      const fade = Math.min(1, Math.min(x, W - x) / 70);
      const major = deg % 5 === 0;
      rg.fillStyle = `rgba(200,205,212,${(major ? .85 : .45) * fade})`;
      if (deg === 0) { rg.fillStyle = `rgba(255,255,255,${fade})`; rg.fillRect(Math.round(x) - 1, H / 2 - 10, 2, 20); }
      else if (major) rg.fillRect(Math.round(x) - .75, H / 2 - 7, 1.5, 14);
      else { rg.beginPath(); rg.arc(x, H / 2, 1.3, 0, Math.PI * 2); rg.fill(); }
    }
    rg.fillStyle = '#00ffaa';
    rg.fillRect(cx - 1.5, H / 2 - 11, 3, 22);
    $('#angleLabel').textContent = (C.angle > 0 ? '+' : '') + C.angle.toFixed(1) + '°';
  }

  function buildAspects() {
    $('#aspects').innerHTML = ASPECTS.map(([k, label, r]) => {
      let icon;
      if (k === 'free') icon = '<i style="width:15px;height:12px;border-style:dashed"></i>';
      else {
        let rr = r;
        if (k === 'original') { const [w, h] = frameWH(); rr = w / h; }
        const w = rr >= 1 ? 16 : Math.max(6, 16 * rr), h = rr >= 1 ? Math.max(6, 16 / rr) : 16;
        icon = `<i style="width:${w}px;height:${h}px"></i>`;
      }
      return `<button class="asp ${C.aspect === k ? 'on' : ''}" data-k="${k}"><span class="ic">${icon}</span>${label}</button>`;
    }).join('');
  }
  $('#aspects').addEventListener('click', e => { const b = e.target.closest('.asp'); if (b && C) setAspect(b.dataset.k); });

  function updateHint() {
    const top = $('#cropTop .muted');
    if (!C) return;
    const ra = resizeCropAspect(), cr = ratio();
    const warn = ra && (!cr || Math.abs(cr / ra.r - 1) > 0.005);
    top.innerHTML = 'Crop &amp; rotate' + (warn ? ` &nbsp;·&nbsp; <span style="color:#00ffaa">${ra.label} output will still center-crop to that shape</span>` : '');
  }

  function updateSummary() {
    const G = S.geom, el = $('#geomInfo');
    if (!el) return;
    if (isDefaultGeom(G)) { el.textContent = 'Full image'; return; }
    const parts = [];
    if (G.crop) parts.push(`Crop ${Math.round(G.crop.w)}×${Math.round(G.crop.h)}`);
    if (G.angle) parts.push(`${G.angle > 0 ? '+' : ''}${G.angle.toFixed(1)}°`);
    if (G.q) parts.push(`${G.q * 90}° turn`);
    if (G.flip) parts.push('flipped');
    el.textContent = parts.join(' · ');
  }

  /* ---------------- input ---------------- */
  function hit(px, py) {
    const c = C.crop, [x0, y0] = toS(c.x - c.w / 2, c.y - c.h / 2), x1 = x0 + c.w * view.z, y1 = y0 + c.h * view.z;
    const R = 14, near = (a, b) => Math.abs(a - b) <= R;
    const inX = px > x0 - R && px < x1 + R, inY = py > y0 - R && py < y1 + R;
    if (near(px, x0) && near(py, y0)) return 'nw';
    if (near(px, x1) && near(py, y0)) return 'ne';
    if (near(px, x0) && near(py, y1)) return 'sw';
    if (near(px, x1) && near(py, y1)) return 'se';
    if (near(py, y0) && inX) return 'n';
    if (near(py, y1) && inX) return 's';
    if (near(px, x0) && inY) return 'w';
    if (near(px, x1) && inY) return 'e';
    if (px > x0 && px < x1 && py > y0 && py < y1) return 'move';
    return null;
  }
  const CURSOR = { nw: 'nwse-resize', se: 'nwse-resize', ne: 'nesw-resize', sw: 'nesw-resize', n: 'ns-resize', s: 'ns-resize', e: 'ew-resize', w: 'ew-resize', move: 'move' };

  cc.addEventListener('pointerdown', e => {
    if (!C || e.button !== 0) return;
    const r = cc.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
    const h = hit(px, py);
    if (!h) return;
    drag = { h, px, py, orig: { ...C.crop } };
    cc.setPointerCapture(e.pointerId);
    draw();
  });
  cc.addEventListener('pointermove', e => {
    if (!C) return;
    const r = cc.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
    if (!drag) { cc.style.cursor = CURSOR[hit(px, py)] || 'default'; return; }
    const o = drag.orig;
    if (drag.h === 'move') {
      const b = { ...o, x: o.x + (px - drag.px) / view.z, y: o.y + (py - drag.py) / view.z };
      C.crop = fitTowards(C.crop, b, false);
      requestDraw();
      return;
    }
    const mx = (px - view.ox) / view.z, my = (py - view.oy) / view.z;
    let Lx = o.x - o.w / 2, Rx = o.x + o.w / 2, Ty = o.y - o.h / 2, By = o.y + o.h / 2;
    const h = drag.h;
    if (h.includes('w')) Lx = Math.min(mx, Rx - MIN);
    if (h.includes('e')) Rx = Math.max(mx, Lx + MIN);
    if (h.includes('n')) Ty = Math.min(my, By - MIN);
    if (h.includes('s')) By = Math.max(my, Ty + MIN);
    const ar = ratio();
    if (ar) {
      if (h.length === 2) {
        let w = Rx - Lx, hh = By - Ty;
        if (w / hh > ar) hh = w / ar; else w = hh * ar;
        if (h.includes('w')) Lx = Rx - w; else Rx = Lx + w;
        if (h.includes('n')) Ty = By - hh; else By = Ty + hh;
      } else if (h === 'e' || h === 'w') {
        const hh = (Rx - Lx) / ar; Ty = o.y - hh / 2; By = o.y + hh / 2;
      } else {
        const w = (By - Ty) * ar; Lx = o.x - w / 2; Rx = o.x + w / 2;
      }
    }
    const b = { x: (Lx + Rx) / 2, y: (Ty + By) / 2, w: Rx - Lx, h: By - Ty };
    C.crop = fitTowards(C.crop, b, !!ar);
    requestDraw();
  });
  const endDrag = () => { if (drag) { drag = null; C.desired = { ...C.crop }; draw(); follow(); } };
  cc.addEventListener('pointerup', endDrag);
  cc.addEventListener('pointercancel', endDrag);
  cc.addEventListener('wheel', e => {
    e.preventDefault();
    if (!C || !e.deltaY) return;
    if (e.altKey || e.ctrlKey) {            // zoom the crop (wheel up = zoom in = smaller box)
      const r = cc.getBoundingClientRect(), step = e.shiftKey ? 1.02 : 1.08;
      zoomCrop(e.deltaY < 0 ? 1 / step : step, e.clientX - r.left, e.clientY - r.top);
      return;
    }
    setAngle(C.angle + (e.deltaY < 0 ? 1 : -1) * (e.shiftKey ? 1 : 0.1));
  }, { passive: false });
  cc.addEventListener('dblclick', e => { if (C && !hit(e.offsetX, e.offsetY)) { C.crop = defaultCrop(); C.desired = { ...C.crop }; draw(); follow(); } });

  ruler.addEventListener('pointerdown', e => {
    if (!C) return;
    rotating = { x: e.clientX, a: C.angle };
    ruler.setPointerCapture(e.pointerId);
    draw();
  });
  ruler.addEventListener('pointermove', e => {
    if (!rotating) return;
    let v = rotating.a - (e.clientX - rotating.x) / PPD;
    if (Math.abs(v) < 0.35) v = 0;               // gentle snap to level
    setAngle(v);
  });
  const endRot = () => { if (rotating) { rotating = null; draw(); } };
  ruler.addEventListener('pointerup', endRot);
  ruler.addEventListener('pointercancel', endRot);
  ruler.addEventListener('wheel', e => { e.preventDefault(); if (C) setAngle(C.angle + (e.deltaY < 0 ? 1 : -1) * (e.shiftKey ? 1 : 0.1)); }, { passive: false });
  ruler.addEventListener('dblclick', () => C && setAngle(0));
  $('#angleLabel').addEventListener('dblclick', () => C && setAngle(0));

  $('#rotL').addEventListener('click', () => C && rotate90(-1));
  $('#rotR').addEventListener('click', () => C && rotate90(1));
  $('#flipH').addEventListener('click', () => C && flip());
  $('#cropReset').addEventListener('click', () => C && reset());
  $('#cropDone').addEventListener('click', () => exit(false));
  $('#cropCancel').addEventListener('click', () => exit(true));
  $('#cropOpen').addEventListener('click', () => enter());
  $('#geomClear').addEventListener('click', () => {
    if (S.cropMode) exit(true);
    if (isDefaultGeom(S.geom)) return;
    S.geom = defaultGeom();
    for (const t of ['black', 'gray', 'white']) if (S.edit[t]) S.edit[t].pos = null;
    updateMarkers(); commit(); updateSummary(); markCurrentEdited();
    S.newFile = true; requestStage(true);
  });
  window.addEventListener('resize', () => { if (C) { layout(); draw(); drawRuler(); } });

  function onKey(e) {
    const k = e.key.toLowerCase(), ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && (k === 's' || k === 'o')) return false;     // let Process / Open through
    if (/^(INPUT|SELECT)$/.test(e.target.tagName) && e.target.type === 'number') return false;
    const step = e.shiftKey ? 10 : 1;
    switch (k) {
      case 'escape': exit(true); break;
      case 'enter': case 'c': exit(false); break;
      case '[': rotate90(-1); break;
      case ']': rotate90(1); break;
      case 'h': flip(); break;
      case 'r': reset(); break;
      case 'arrowleft': nudge(-step, 0); break;
      case 'arrowright': nudge(step, 0); break;
      case 'arrowup': nudge(0, -step); break;
      case 'arrowdown': nudge(0, step); break;
      case ',': setAngle(C.angle - (e.shiftKey ? 1 : 0.1)); break;
      case '.': setAngle(C.angle + (e.shiftKey ? 1 : 0.1)); break;
      default: if (!ctrl && k !== 'alt') return true;
    }
    e.preventDefault();
    return true;
  }

  return {
    enter, exit, onKey, updateSummary,
    editChanged: () => { if (C) { refreshImage(); draw(); updateHint(); } },
    _state: () => C, _view: () => view,
  };
})();
Crop.updateSummary();
