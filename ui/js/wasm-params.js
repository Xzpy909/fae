/* computeEdit's result -> Params of the WebAssembly core. ES module, shared by the preview worker
   and the parity tests (tests/parity/run_js.js), so the tests check the same mapping the app uses.
   The edit is an ordered list of steps, the same reading as fastedit/core.py's _steps: E.steps
   lists them in panel order (each an object with t = the kind and that tool's entries); without
   it the flat E runs in DEFAULT_ORDER. A missing / null table means the step is off. */

export const DEFAULT_ORDER = ['point', 'adjust', 'curves', 'bw', 'film', 'sc'];

const f64 = a => (a == null ? undefined : a instanceof Float64Array ? a : Float64Array.from(a));
const isIdentity = luts => luts.every(l => { for (let i = 0; i < 256; i++) if (l[i] !== i) return false; return true; });

function pushStep(p, s, filmTable) {
  switch (s.t) {
    case 'point':
      if (s.point) p.push_point(s.k, s.o, s.gains ? f64(s.gains) : undefined);
      return;
    case 'adjust': {
      const md = s.md || 0, tones = s.tones || [];
      if (!md && s.wb == null && s.con == null && !tones.length && s.boost == null && s.sat == null) return;
      if (md) p.set_md(md);
      if (s.wb != null) p.set_wb(Float64Array.from([...s.wb[0], ...s.wb[1], ...s.wb[2]]));
      if (s.con != null) p.set_con(f64(s.con));
      for (const t of tones) p.add_tone(f64(t.T), !!t.ratio);
      if (s.boost != null) p.set_boost(f64(s.boost));
      if (s.sat != null) p.set_sat(s.sat);
      p.push_adjust();
      return;
    }
    case 'curves': {
      const L = s.luts;
      if (!L || L.length !== 3 || isIdentity(L)) return;      // identity: nothing to do
      const flat = new Uint8Array(768);
      flat.set(L[0], 0); flat.set(L[1], 256); flat.set(L[2], 512);
      p.push_curves(flat);
      return;
    }
    case 'bw': {
      const BW = s.bw;
      if (BW) p.push_bw(BW.s, BW.op, BW.w ? f64(BW.w) : undefined, BW.ps ? BW.ps : 0, BW.ps ? f64(BW.psk) : undefined);
      return;
    }
    case 'film': {
      const FM = s.film;
      if (FM) p.push_film(FM.n, FM.d ? f64(filmTable(FM.d)) : undefined, FM.t1 ? f64(FM.t1) : undefined, FM.op ?? 1);
      return;
    }
    case 'sc': {
      const SC = s.sc;
      if (SC) p.push_sc(SC.cast, SC.cc, SC.dyn, SC.op, f64(SC.dk || []), SC.shl ? f64(SC.shl) : undefined);
      return;
    }
    default:
      throw new Error('unknown step ' + s.t);
  }
}

// filmTable: E.film.d -> the Film preset table (the worker keeps the tables by id, see
// preview-worker.js; elsewhere d already is the table). Defaults to d itself.
export function toParams(fe, E, filmTable = d => d) {
  const p = new fe.Params();
  try {
    const steps = E.steps || DEFAULT_ORDER.map(t => Object.assign({}, E, { t }));
    for (const s of steps) pushStep(p, s, filmTable);
    if (E.sharp) p.set_sharp(E.sharp.as || 0, E.sharp.bs || 0, E.sharp.st || 0, E.sharp.lc || 0, E.sharp.fo || 0, E.sharp.scale || 1);
  } catch (e) {
    p.free();
    throw e;
  }
  return p;
}
