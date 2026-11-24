// The Perceptual Saturation filter without its perceptual mode: Warmth gains, Saturation around
// (r + 2g + b) / 4, then opacity, rounded.
// P: [s, opacity, warmth on, opacity table on, gain r, gain g, gain b, -],
//    [8..776) Warmth per channel and level, [776..262152) Saturation rounded by (Y = r + 2g + b) * 256
//    + level (host f64: at opacity 100 % the result is exactly that), then (op != 1) the opacity
//    table rnd(f + (l - f) * op) for whole levels f, l (used when l is whole)
@group(0) @binding(2) var<storage, read> P: array<f32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (!inside(g)) { return; }
    let f = load_levels(g.xy);
    let fi = vec3<u32>(f);
    let wi = vec3<u32>(u32(P[8u + fi.r]), u32(P[264u + fi.g]), u32(P[520u + fi.b]));
    let yy = (wi.r + 2u * wi.g + wi.b) * 256u;
    if (P[1] == 1.0) {
        store_levels(g.xy, vec3<f32>(P[776u + yy + wi.r], P[776u + yy + wi.g], P[776u + yy + wi.b]));
        return;
    }
    let w = vec3<f32>(wi);
    let y = (w.r + 2.0 * w.g + w.b) / 4.0;
    let l = clamp(y + (w - y) * P[0], vec3<f32>(0.0), vec3<f32>(255.0));
    var o = rnd(f + (l - f) * P[1]);
    if (P[3] != 0.0) {
        let i = vec3<u32>(f) * 256u + vec3<u32>(l);
        let t = vec3<f32>(P[262152u + i.r], P[262152u + i.g], P[262152u + i.b]);
        o = select(o, t, l == floor(l));
    }
    store_levels(g.xy, o);
}
