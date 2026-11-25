'use strict';
/* The bridge to the app shell (docs/rust-backend-plan.md): the only file that knows what runs the page.
   The shell is Tauri (Fae.exe, core/backend/fae-app): one command, call(name, args), and the page on its
   fae protocol, which also serves /preview/<token> and /thumb on the page's own origin.
   Loaded first; everything else uses api() (state.js), Bridge.previewUrl / thumbUrl, Bridge.ready, Bridge.on.

   UI -> back end: Bridge.api().<command>(...args) -> Promise (the command table: core/backend fae-backend
     lib.rs COMMANDS, and the shell's own: window buttons, dialogs, Ctrl+V, reveal).
   Shell -> UI: the shell calls the globals below (eval); they hand on to the handlers registered with
     Bridge.on(name, fn):
       window.app.addPaths(paths)            'addPaths'          files dropped on the window
       window.app.dragState(state)           'dragState'         a file drag 'enter's / 'leave's the window,
                                                                 or is 'drop'ped (its overlay)
       window.app.winState(maximized)        'winState'          the window was maximized / restored
       window.onProcessProgress(i, n, r)     'processProgress'   a Process batch finished image i of n */
const Bridge = (() => {
  const handlers = {};
  const emit = (name, ...args) => { for (const fn of handlers[name] || []) fn(...args); };
  window.app = { addPaths: p => emit('addPaths', p), dragState: s => emit('dragState', s), winState: m => emit('winState', m) };
  window.onProcessProgress = (i, n, r) => emit('processProgress', i, n, r);
  // every api().<name>(...args) is the shell's command call(name, args)
  const tauri = window.__TAURI_INTERNALS__;
  const tauriApi = tauri && new Proxy({}, {
    get: (_, name) => name === 'then' ? undefined : (...args) => tauri.invoke('call', { name, args }),
  });
  return {
    api: () => tauriApi,
    // is the back end there (false on a page opened outside the app)
    available: () => !!tauri,
    // run fn once the back end can be called (Tauri's is there before the page's scripts run)
    ready(fn) { if (tauri) fn(); },
    on(name, fn) { (handlers[name] = handlers[name] || []).push(fn); },
    // pixels a command stored (load_stage, load_source, lossy_preview return the token)
    previewUrl: token => '/preview/' + token,
    // the file list's thumbnail of an image
    thumbUrl: path => '/thumb?p=' + encodeURIComponent(path),
  };
})();
