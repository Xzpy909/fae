'use strict';
/* The bridge to the app shell (claude/rust-backend-plan.md): the only file that knows what runs the page.
   Today the shell is pywebview: its js_api (fastedit/api.py, forwarding to the Rust back end) and the
   local HTTP server (fastedit/app.py: /preview/<token>, /thumb). A Tauri shell would replace this file
   only: invoke() for the commands, its custom protocol for the URLs, its events for the shell's calls.
   Loaded first; everything else uses api() (state.js), Bridge.previewUrl / thumbUrl, Bridge.ready, Bridge.on.

   UI -> back end: Bridge.api().<command>(...args) -> Promise (the command table: core/backend fae-backend
     lib.rs COMMANDS).
   Shell -> UI: the shell calls the globals below (pywebview: window.evaluate_js); they hand on to the
     handlers registered with Bridge.on(name, fn):
       window.app.addPaths(paths)            'addPaths'          files dropped on the window
       window.app.winState(maximized)        'winState'          the window was maximized / restored
       window.onProcessProgress(i, n, r)     'processProgress'   a Process batch finished image i of n */
const Bridge = (() => {
  const handlers = {};
  const emit = (name, ...args) => { for (const fn of handlers[name] || []) fn(...args); };
  window.app = { addPaths: p => emit('addPaths', p), winState: m => emit('winState', m) };
  window.onProcessProgress = (i, n, r) => emit('processProgress', i, n, r);
  const has = () => !!(window.pywebview && window.pywebview.api);
  return {
    api: () => window.pywebview.api,
    // is the back end there (false on a page opened outside the app)
    available: has,
    // run fn once the back end can be called
    ready(fn) {
      if (has()) fn();
      else window.addEventListener('pywebviewready', () => fn(), { once: true });
    },
    on(name, fn) { (handlers[name] = handlers[name] || []).push(fn); },
    // pixels a command stored (load_stage, load_source, lossy_preview return the token)
    previewUrl: token => '/preview/' + token,
    // the file list's thumbnail of an image
    thumbUrl: path => '/thumb?p=' + encodeURIComponent(path),
  };
})();
