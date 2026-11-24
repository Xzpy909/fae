// The filter kit (docs/filter-kit.md), shared by every kit filter. A filter's module is
//   common.wgsl (src, dst, load_levels, rnd, luma, ...) + this file + its kind's template (pixel.wgsl,
//   neighbourhood.wgsl or statistics.wgsl) + `const P_<FIELD>: u32` per parameter + filters/<id>.wgsl.
// The filter writes `fn fx(c: vec3<f32>, at: vec2<u32>) -> vec3<f32>`: c = the pixel's levels (whole,
// 0..255), at = its position; it returns levels (any float: the kit rounds them, clamps to 0..255 and
// mixes in the opacity).
//
// P, the step's parameters (lib.rs fx_params): [0..12) the blur's header in sharp_blur.wgsl's layout
// (2 k, 3 taps, 4 radius, 6 w2, 7 h2, 8 w, 9 h), 12 = offset of the filter's parameters, 13 = their
// count, 14 = offset of the opacity table (0: opacity 100 %); [16..272) the blur's level map (identity),
// [272..) its kernel, then the parameters, then the opacity table.
@group(0) @binding(2) var<storage, read> P: array<f32>;

// the filter's parameter i (use the generated constants: par(P_AMOUNT)), the slider's value as shown
fn par(i: u32) -> f32 {
    return P[u32(P[12]) + i];
}

// the image's size in pixels (the stage: preview and Process differ in size, so anything measured in
// pixels should scale with it, e.g. by size().y / 1440)
fn size() -> vec2<u32> {
    return vec2<u32>(u32(P[8]), u32(P[9]));
}

// x / n correctly rounded for a whole n in 1 .. 4096 (the same as sharp_blur.wgsl's: WGSL's division
// may be off by 2.5 ulp, and an exact quotient like 255 * 37 / 37 must stay exact)
fn qdiv(x: f32, n: f32) -> f32 {
    let ax = abs(x);
    var q = ax / n;
    for (var it = 0; it < 2; it++) {
        let qh = bitcast<f32>(bitcast<u32>(q) & 0xfffff000u);
        let ql = q - qh;
        let r = (ax - qh * n) - ql * n;
        let up = bitcast<f32>(bitcast<u32>(q) + 1u);
        let u = up - q;
        let half = u * n * 0.5;
        if (r > half || (r == half && (bitcast<u32>(q) & 1u) == 1u)) {
            q = up;
        } else if (q > 0.0) {
            let dn = bitcast<f32>(bitcast<u32>(q) - 1u);
            let hd = (q - dn) * n * 0.5;
            if (-r > hd || (-r == hd && (bitcast<u32>(q) & 1u) == 1u)) { q = dn; }
        }
    }
    return select(q, -q, x < 0.0);
}

// the end of every kit filter: round the filter's result, then opacity rnd(f + (o - f) op) for the
// whole levels f (before) and o (after) from the host's f64 table (GPUs fuse that expression into one
// fma, which rounds the .5 cases a slider makes the other way)
fn finish(at: vec2<u32>, f: vec3<f32>, v: vec3<f32>) {
    var o = rnd(v);
    let m = u32(P[14]);
    if (m != 0u) {
        let i = vec3<u32>(f) * 256u + vec3<u32>(o);
        o = vec3<f32>(P[m + i.r], P[m + i.g], P[m + i.b]);
    }
    store_levels(at, o);
}
