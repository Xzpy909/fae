'use strict';
/* Start-up: the loading screen (#splash) and a timing line.

   The splash covers the window from the first paint until the app can be used: init.js has asked
   Python for everything it needs (Boot.mark('init')), the GPU core has started or is known to be
   unavailable (GV.started), the CPU core's worker is ready or failed (WP), and the page isn't blocked
   any more (two frames in a row on time). Its bar follows the GPU core compiling its shaders
   (gpu-view.js: all of them now, so none compiles later while the app is in use).

   The timing line (console, `Fae.exe --debug`): when each part was done, in ms since the page
   started, and the longest blocks of the page (long tasks), e.g.
     Start-up: window 440 · settings 450 · gpu device 500 · gpu shaders 2900 · ... · shown 3000 ms; blocked: 120 at 330, ...
*/
const Boot = (() => {
  const marks = [], long = [], notes = [];
  const now = () => Math.round(performance.now());
  try {
    new PerformanceObserver(l => { for (const e of l.getEntries()) long.push([Math.round(e.duration), Math.round(e.startTime)]); })
      .observe({ type: 'longtask', buffered: true });
  } catch (_) {}
  const mark = name => marks.push([name, now()]);
  const note = text => notes.push(text);           // extra detail for the timing line
  // the bar: done of total (the GPU core's shaders)
  function progress(done, total) {
    const el = document.querySelector('#splash');
    if (!el) return;
    el.querySelector('.bar i').style.width = (100 * done / total).toFixed(1) + '%';
    el.querySelector('span').textContent = `Preparing the GPU · ${done} / ${total}`;
  }

  // two frames in a row within 50 ms of each other (or give up after 15 s)
  function settled() {
    return new Promise(res => {
      const t0 = performance.now();
      let last = t0, good = 0;
      const step = t => {
        good = t - last < 50 ? good + 1 : 0;
        last = t;
        if (good >= 2 || t - t0 > 15000) res(); else requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    });
  }
  const until = test => new Promise(res => {
    const tick = () => (test() ? res() : setTimeout(tick, 30));
    tick();
  });

  let inited;
  const initDone = new Promise(res => { inited = res; });

  async function run() {
    await Promise.all([
      initDone,
      typeof GV !== 'undefined' ? GV.started.then(() => mark(GV.ready() ? 'gpu ready' : 'gpu off')) : null,
      typeof WP !== 'undefined' ? until(() => WP.ready() || WP.failed()).then(() => mark('cpu worker')) : null,
    ]);
    await settled();
    mark('shown');
    const el = document.getElementById('splash');
    if (el) {
      el.classList.add('gone');
      setTimeout(() => el.remove(), 400);
    }
    const blocks = long.sort((a, b) => b[0] - a[0]).slice(0, 5).map(([d, t]) => `${d} at ${t}`);
    console.info('Start-up: ' + marks.map(([n, t]) => `${n} ${t}`).join(' · ') + ' ms'
                 + (blocks.length ? '; blocked: ' + blocks.join(', ') + ' ms' : '')
                 + notes.map(t => '; ' + t).join(''));
  }
  // after every script has loaded (GV, WP exist)
  window.addEventListener('DOMContentLoaded', () => { run(); });

  return { mark, note, progress, init: () => { mark('init'); inited(); } };
})();
