'use strict';
/* Water caustics: light shimmering as if through moving water, splitting into faint rainbows where it
   bends (each colour's caustic lands a little apart). One WebGL fragment shader on a canvas.

   Caustics.attach(host, { overlay }) puts a canvas into `host` and animates it while `host` is shown:
   - the loading screen (#splash): drawn on the splash's own colour; this file loads right after #splash,
     before the other scripts, and the effect stops when startup.js removes the splash;
   - the dark veils behind the search, the Filters popover and Apply preset to… (overlay: light only, the
     veil and the image show through); attached once the page has loaded, running only while they are open.
   The waves ease in over 1 s once a window opens and out over .2 s when it closes (show / hide below).

   Nothing without a hardware WebGL context (failIfMajorPerformanceCaveat: software rendering would slow
   the app down) or when the shader doesn't compile. Reduced motion: one still frame. Tuned in a
   standalone preview: strength .35 (overlays x1.4), size 1.2, speed .6, sharpness 8, rainbow .6, vignette .6, grey. */
const Caustics = (() => {
  const P = { strength: 0.35, scale: 1.2, speed: 0.6, sharp: 8.0, disp: 0.6, vig: 0.6, tint: [0.92, 0.92, 0.95] };
  const VS = 'attribute vec2 a; void main() { gl_Position = vec4(a, 0.0, 1.0); }';
  const FS = `
precision highp float;
uniform vec2 res; uniform float t, strength, scale, sharp, vig, disp, overlay; uniform vec3 tint, bg;
// light focused by a moving water surface: an iterated sine warp of the plane, bright where it bunches up
float caustic(vec2 p, float time) {
  vec2 i = p; float c = 1.0; const float inten = 0.005;
  for (int n = 0; n < 5; n++) {
    float tt = time * (1.0 - 3.5 / float(n + 1));
    i = p + vec2(cos(tt - i.x) + sin(tt + i.y), sin(tt - i.y) + cos(tt + i.x));
    c += 1.0 / length(vec2(p.x / (sin(i.x + tt) / inten), p.y / (cos(i.y + tt) / inten)));
  }
  c /= 5.0;
  c = 1.17 - pow(c, 1.4);
  return pow(abs(c), sharp);
}
// two layers at different scales and speeds: less regular, more like real water
float light(vec2 p, float time) {
  return clamp(caustic(p, time) * 0.65 + caustic(p * 0.67 + 40.0, time * 0.8 + 11.0) * 0.35, 0.0, 1.0);
}
void main() {
  vec2 p = gl_FragCoord.xy / res.y * 6.2831 * scale - 250.0;
  // dispersion: red bent least, blue most; white where they overlap, rainbow fringes at the edges
  vec2 c = vec2(0.5 * res.x / res.y, 0.5) * 6.2831 * scale - 250.0;
  float d = disp * 0.035;
  vec3 l = vec3(light(c + (p - c) * (1.0 - d), t - disp * 0.04),
                light(p, t),
                light(c + (p - c) * (1.0 + d), t + disp * 0.04));
  vec2 q = gl_FragCoord.xy / res - 0.5;
  float v = mix(1.0, smoothstep(0.85, 0.15, length(q * vec2(res.x / res.y, 1.0))), vig);
  vec3 add = tint * l * strength * v;
  // overlay: premultiplied light over what is underneath; else on the background colour
  gl_FragColor = overlay > 0.5 ? vec4(add, max(add.r, max(add.g, add.b))) : vec4(bg + add, 1.0);
}`;
  const still = matchMedia('(prefers-reduced-motion: reduce)').matches;

  function attach(host, opts = {}) {
    if (!host || host.querySelector(':scope > canvas.fx')) return;
    const overlay = !!opts.overlay;
    const cv = document.createElement('canvas');
    cv.className = 'fx';
    host.prepend(cv);
    let gl = null;
    try {
      gl = cv.getContext('webgl', { antialias: false, alpha: overlay, premultipliedAlpha: true, depth: false,
        stencil: false, powerPreference: 'low-power', failIfMajorPerformanceCaveat: true });
    } catch (_) {}
    if (!gl) { cv.remove(); return; }
    const U = {};
    try {
      const sh = (type, src) => {
        const s = gl.createShader(type);
        gl.shaderSource(s, src);
        gl.compileShader(s);
        if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
        return s;
      };
      const prog = gl.createProgram();
      gl.attachShader(prog, sh(gl.VERTEX_SHADER, VS));
      gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, FS));
      gl.linkProgram(prog);
      if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog));
      gl.useProgram(prog);
      gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
      const loc = gl.getAttribLocation(prog, 'a');
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
      for (const n of ['res', 't', 'strength', 'scale', 'sharp', 'vig', 'disp', 'overlay', 'tint', 'bg']) U[n] = gl.getUniformLocation(prog, n);
    } catch (e) {
      console.warn('Caustics off:', e);
      cv.remove();
      return;
    }
    // the host's own background colour (opaque mode), so the effect sits on it
    const bg = (getComputedStyle(host).backgroundColor.match(/\d+/g) || [35, 35, 38]).slice(0, 3).map(v => v / 255);
    gl.uniform3f(U.bg, bg[0], bg[1], bg[2]);
    gl.uniform1f(U.overlay, overlay ? 1 : 0);
    // over the veils a little stronger: they sit on a busier, lighter app than the plain splash
    gl.uniform1f(U.strength, overlay ? P.strength * 1.4 : P.strength); gl.uniform1f(U.scale, P.scale); gl.uniform1f(U.sharp, P.sharp);
    gl.uniform1f(U.vig, P.vig); gl.uniform1f(U.disp, P.disp); gl.uniform3fv(U.tint, P.tint);

    let T = 23.0, last = 0, running = false;
    const visible = () => cv.isConnected && cv.getClientRects().length > 0 && !host.closest('.gone');
    function frame(now) {
      if (!visible()) { running = false; cv.classList.remove('on'); return; }
      T += Math.min(0.1, last ? (now - last) / 1000 : 0) * P.speed;
      last = now;
      const dpr = Math.min(window.devicePixelRatio || 1, 1.5);      // a soft effect: no need for full resolution
      const w = Math.max(1, Math.round(cv.clientWidth * dpr)), h = Math.max(1, Math.round(cv.clientHeight * dpr));
      if (cv.width !== w || cv.height !== h) { cv.width = w; cv.height = h; gl.viewport(0, 0, w, h); }
      gl.uniform2f(U.res, w, h);
      gl.uniform1f(U.t, T);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      cv.classList.add('on');
      if (still) { running = false; return; }
      requestAnimationFrame(frame);
    }
    const start = () => {
      if (running || !visible()) return;
      running = true; last = 0;
      requestAnimationFrame(frame);
    };
    // overlays open and close by a class on the host or its parent (.show): watch both
    if (overlay) {
      const mo = new MutationObserver(start);
      for (let el = host; el && el !== document.body; el = el.parentElement) mo.observe(el, { attributes: true, attributeFilter: ['class', 'style'] });
    }
    start();
  }

  // the windows with the waves (search, Filters, Apply preset to…): show at once, and hide after a .2 s
  // fade of the veil and its waves (style.css .closing); showing again during the fade cancels it
  function show(el) { clearTimeout(el._fxHide); el.classList.remove('closing'); el.classList.add('show'); }
  function hide(el) {
    if (!el.classList.contains('show') || el.classList.contains('closing')) return;
    el.classList.add('closing');
    el._fxHide = setTimeout(() => el.classList.remove('show', 'closing'), 200);
  }
  const shown = el => el.classList.contains('show') && !el.classList.contains('closing');

  // the dark veils behind the search and the Filters popover, once every script has built its part
  window.addEventListener('DOMContentLoaded', () => {
    attach(document.querySelector('#cmdPal .cpal-veil'), { overlay: true });
    attach(document.querySelector('#filterPop .fp-veil'), { overlay: true });
  });
  attach(document.getElementById('splash'));
  return { attach, show, hide, shown };
})();
