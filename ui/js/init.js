'use strict';
/* Start-up: settings, encoders, initial files. */

/* ======================================================================
   Init
   ====================================================================== */
window.app = { addPaths };

async function init() {
  try {
    const s = await api().get_settings();
    if (s && typeof s === 'object') {
      if (s.fmt) S.prefs.fmt = s.fmt;
      if (Array.isArray(s.secs)) S.prefs.secs = s.secs.filter(x => SECS.includes(x));
      if (typeof s.filtersOpen === 'boolean') S.prefs.filtersOpen = s.filtersOpen;
      if (s.jpeg_q != null) S.prefs.jpeg_q = clamp(Math.round(Number(s.jpeg_q)) || 90, 1, 100);
      if (s.jpeg_d != null && isFinite(Number(s.jpeg_d))) S.prefs.jpeg_d = clamp(Number(s.jpeg_d), 0, 25);
      if (s.jpeg_mode === 'q' || s.jpeg_mode === 'd') S.prefs.jpeg_mode = s.jpeg_mode;
      if (typeof s.webp_lossy === 'boolean') S.prefs.webp_lossy = s.webp_lossy;
      if (s.webp_q != null) S.prefs.webp_q = clamp(Math.round(Number(s.webp_q)) || 90, 0, 100);
      if (s.avif_crf != null && isFinite(Number(s.avif_crf))) S.prefs.avif_crf = clamp(Math.round(Number(s.avif_crf) * 4) / 4, 1, 70);
      if (s.avif_preset != null && isFinite(Number(s.avif_preset))) S.prefs.avif_preset = clamp(Math.round(Number(s.avif_preset)), 0, 8);
      if (s.aom_q != null && isFinite(Number(s.aom_q))) S.prefs.aom_q = clamp(Math.round(Number(s.aom_q)), 0, 100);
      if (s.aom_speed != null && isFinite(Number(s.aom_speed))) S.prefs.aom_speed = clamp(Math.round(Number(s.aom_speed)), 0, 10);
      if (!['WEBP', 'PNG', 'JPEG', 'AVIF'].includes(S.prefs.fmt)) S.prefs.fmt = 'WEBP';
      if (typeof s.advance === 'boolean') S.prefs.advance = s.advance;
      if (s.stage) S.stage = Object.assign(clone(DEFAULT_STAGE), s.stage);
      if (S.stage.mode === '720p') Object.assign(S.stage, { mode: 'preset', preset_ar: '16:9', preset_res: 720 });
    }
  } catch (_) {}
  try { const ji = await api().encoder_info(); S.cmp.jpegli = !!(ji && ji.jpegli); S.cmp.avif = !!(ji && ji.avif); S.cmp.svt = !!(ji && ji.svt); } catch (_) {}
  S.committed = snapshot();
  syncPanel(); editChanged(); updateInfo();
  try { const init = await api().initial_files(); if (init && init.length) addPaths(init); } catch (_) {}
}
S.E = computeEdit(S.edit);
syncPanel(); updateInfo();
if (window.pywebview && window.pywebview.api) init();
else window.addEventListener('pywebviewready', init);
