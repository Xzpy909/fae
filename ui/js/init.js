'use strict';
/* Start-up: settings, encoders, initial files. */

/* ======================================================================
   Init
   ====================================================================== */
Bridge.on('addPaths', addPaths);                       // files dropped on the window
Bridge.on('winState', m => WinChrome.setMax(m));       // the shell reports maximize / restore

async function init() {
  try { await initSteps(); } finally { Boot.init(); }               // the loading screen waits for this
}
async function initSteps() {
  try { WinChrome.init(await api().win_info()); } catch (_) {}      // unified header (frameless window on Windows)
  Boot.mark('window');
  try {
    const s = await api().get_settings();
    if (s && typeof s === 'object') {
      if (s.fmt) S.prefs.fmt = s.fmt;
      // Settings › Remember last session off: start blank (no tools open, the stage at its defaults)
      if (typeof s.remember === 'boolean') S.prefs.remember = s.remember;
      const session = S.prefs.remember !== false;
      if (session && Array.isArray(s.secs)) S.prefs.secs = s.secs.filter(x => SECS.includes(x));
      if (session && Array.isArray(s.collapsed)) S.prefs.collapsed = s.collapsed.filter(x => SECS.includes(x));
      if (s.jpeg_q != null) S.prefs.jpeg_q = clamp(Math.round(Number(s.jpeg_q)) || 90, 1, 100);
      if (s.jpeg_d != null && isFinite(Number(s.jpeg_d))) S.prefs.jpeg_d = clamp(Number(s.jpeg_d), 0, 25);
      if (s.jpeg_mode === 'q' || s.jpeg_mode === 'd') S.prefs.jpeg_mode = s.jpeg_mode;
      if (typeof s.webp_lossy === 'boolean') S.prefs.webp_lossy = s.webp_lossy;
      if (s.webp_q != null) S.prefs.webp_q = clamp(Math.round(Number(s.webp_q)) || 90, 0, 100);
      if (s.avif_crf != null && isFinite(Number(s.avif_crf))) S.prefs.avif_crf = clamp(Math.round(Number(s.avif_crf) * 4) / 4, 1, 70);
      if (s.avif_preset != null && isFinite(Number(s.avif_preset))) S.prefs.avif_preset = clamp(Math.round(Number(s.avif_preset)), 0, 8);
      if (s.aom_q != null && isFinite(Number(s.aom_q))) S.prefs.aom_q = clamp(Math.round(Number(s.aom_q)), 0, 100);
      if (s.avif_enc === 'svt' || s.avif_enc === 'aom') S.prefs.avif_enc = s.avif_enc;
      if (typeof s.split === 'boolean') S.prefs.split = s.split;
      if (typeof s.encSplit === 'boolean') S.prefs.encSplit = s.encSplit;
      if (s.aom_speed != null && isFinite(Number(s.aom_speed))) S.prefs.aom_speed = clamp(Math.round(Number(s.aom_speed)), 0, 10);
      if (!['WEBP', 'PNG', 'JPEG', 'AVIF'].includes(S.prefs.fmt)) S.prefs.fmt = 'WEBP';
      if (typeof s.advance === 'boolean') S.prefs.advance = s.advance;
      if (typeof s.dock === 'boolean') S.prefs.dock = s.dock;
      if (session && s.stage) S.stage = Object.assign(clone(DEFAULT_STAGE), s.stage);
      if (S.stage.mode === '720p') Object.assign(S.stage, { mode: 'preset', preset_ar: '16:9', preset_res: 720 });
    }
  } catch (_) {}
  Cmd.syncDock();
  Boot.mark('settings');
  try { const ji = await api().encoder_info(); S.cmp.jpegli = !!(ji && ji.jpegli); S.cmp.avif = !!(ji && ji.avif); S.cmp.svt = !!(ji && ji.svt); } catch (_) {}
  Boot.mark('encoders');
  S.committed = snapshot();
  syncPanel(); editChanged(); updateInfo();
  Presets.load();
  try { const init = await api().initial_files(); if (init && init.length) addPaths(init); } catch (_) {}
  Boot.mark('files');
}
S.E = computeEdit(effectiveEdit(S.edit));
syncPanel(); updateInfo();
Bridge.ready(init);
