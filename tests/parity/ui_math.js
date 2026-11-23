'use strict';
/* Loads the UI's edit math (the same files, in index.html order) into node, with a stub DOM,
   so tests can call computeEdit / jobFor exactly as the app does. */
const fs = require('fs');
const path = require('path');
const vm = require('vm');

// Anything the UI touches while loading (document.querySelector(...).getContext(...), styles, ...)
function stub() {
  const fn = function () { return stub(); };
  return new Proxy(fn, {
    get: (t, k) => (k === Symbol.toPrimitive ? () => 0 : k === 'then' ? undefined : stub()),
    set: () => true,
    apply: () => stub(),
    construct: () => stub(),
  });
}

let loaded = null;

// Runs the scripts in this process's own global scope, like <script> tags share one scope in the
// page. (A separate vm context would be cleaner but makes every global lookup slow, which
// distorts the benchmarks by a factor of ~10.) So: one UI per process.
function loadUi(root) {
  if (loaded) return loaded;
  const ui = path.join(root, 'ui');
  const html = fs.readFileSync(path.join(ui, 'index.html'), 'utf8');
  const scripts = [...html.matchAll(/<script src="([^"]+)"><\/script>/g)].map(m => m[1]);
  const wanted = s => /^js\/(state|film-tables|edit-math|process)\.js$/.test(s) || s.startsWith('js/film/');
  const globals = {
    document: stub(), window: stub(), navigator: stub(), localStorage: stub(),
    requestAnimationFrame: () => 0, ResizeObserver: function () { return stub(); },
    ImageData: function (w, h) { this.width = w; this.height = h; this.data = new Uint8ClampedArray(w * h * 4); },
  };
  for (const [k, v] of Object.entries(globals)) {
    Object.defineProperty(globalThis, k, { value: v, writable: true, configurable: true });
  }
  for (const s of scripts.filter(wanted)) {
    vm.runInThisContext(fs.readFileSync(path.join(ui, s), 'utf8'), { filename: s });
  }
  // const / function declarations live in the shared script scope, not on globalThis
  loaded = vm.runInThisContext(`({ computeEdit, jobFor, defaultEdit, S, ADJUST, FILTERS,
    FILM_PRESETS, filterFields, defaultGeom, CHAIN_TOOLS })`);
  return loaded;
}

module.exports = { loadUi };
