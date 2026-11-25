'use strict';
/* Unified header: on Windows the window is frameless and the top bar is the title bar.
   The shell (Fae.exe, fae-app shell.rs) does the window work natively; this file only says when:
   - press on an empty part of the bar = Windows' own move loop (snap, drag down to restore),
     a quick second press there = maximize / restore;
   - press on an edge strip = Windows' own resize loop;
   - the three window buttons; the maximize button swaps its icon (the shell's winState, bridge.js). */
const WinChrome = (() => {
  let on = false, lastDown = 0, lastX = 0, lastY = 0;
  const setMax = m => {
    document.body.classList.toggle('win-max', !!m);
    $('#winMax').title = m ? 'Restore' : 'Maximize';
  };
  function init(info) {
    if (!info || !info.frameless) return;
    on = true;
    document.body.classList.add('frameless');
    setMax(info.maximized);
  }
  // parts of the bar that are controls, not drag area
  const NO_DRAG = 'button, input, select, label, a, .seg, .tabs, .win-btns';
  $('#topbar').addEventListener('pointerdown', e => {
    if (!on || e.button !== 0 || e.target.closest(NO_DRAG)) return;
    const now = performance.now(), near = Math.abs(e.clientX - lastX) < 6 && Math.abs(e.clientY - lastY) < 6;
    if (now - lastDown < 400 && near) { lastDown = 0; api().win_toggle_max(); return; }   // double press
    lastDown = now; lastX = e.clientX; lastY = e.clientY;
    e.preventDefault();
    api().win_begin('caption');
  });
  document.querySelectorAll('.win-edge').forEach(el => el.addEventListener('pointerdown', e => {
    if (!on || e.button !== 0) return;
    e.preventDefault(); e.stopPropagation();
    api().win_begin(el.dataset.edge);
  }));
  $('#winMin').addEventListener('click', () => api().win_minimize());
  $('#winMax').addEventListener('click', () => api().win_toggle_max());
  $('#winClose').addEventListener('click', () => api().win_close());
  return { init, setMax };
})();
