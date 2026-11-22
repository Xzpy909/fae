/* computeEdit's result -> Params of the WebAssembly core. ES module, shared by the preview worker
   and the parity tests (tests/parity/run_js.js), so the tests check the same mapping the app uses.
   A missing / null table means the step is off (the same reading as fastedit/core.py's _args). */

const f64 = a => (a == null ? undefined : a instanceof Float64Array ? a : Float64Array.from(a));

// filmD: the Film preset table for E.film.d (the worker keeps the tables by id; see preview-worker.js)
export function toParams(fe, E, filmD) {
  const p = new fe.Params();
  if (E.md) p.set_md(E.md);
  const L = new Uint8Array(768);
  L.set(E.luts[0], 0); L.set(E.luts[1], 256); L.set(E.luts[2], 512);
  p.set_luts(L);
  p.set_point(!!E.point, E.k, E.o, E.gains ? f64(E.gains) : undefined);
  if (E.wb) p.set_wb(Float64Array.from([...E.wb[0], ...E.wb[1], ...E.wb[2]]));
  if (E.con) p.set_con(f64(E.con));
  if (E.tones) for (const t of E.tones) p.add_tone(f64(t.T), !!t.ratio);
  if (E.boost) p.set_boost(f64(E.boost));
  if (E.sat !== null && E.sat !== undefined) p.set_sat(E.sat);
  const BW = E.bw;
  if (BW) p.set_bw(BW.s, BW.op, BW.w ? f64(BW.w) : undefined, BW.ps ? BW.ps : 0, BW.ps ? f64(BW.psk) : undefined);
  const FM = E.film;
  if (FM) p.set_film(FM.n, FM.d ? f64(filmD) : undefined, FM.t1 ? f64(FM.t1) : undefined, FM.op);
  const SC = E.sc;
  if (SC) p.set_sc(SC.cast, SC.cc, SC.dyn, SC.op, f64(SC.dk || []), SC.shl ? f64(SC.shl) : undefined);
  if (E.sharp) p.set_sharp(E.sharp.as || 0, E.sharp.bs || 0, E.sharp.st || 0, E.sharp.lc || 0, E.sharp.fo || 0, E.sharp.scale || 1);
  return p;
}
