// Point correction: luma levels (black / white point: k, o), then the neutral-gray gains.
// P: [levels on, k, anchor (the new luma at 255, see Step::point), -, gain r, gain g, gain b]
@group(0) @binding(2) var<storage, read> P: array<f32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (!inside(g)) { return; }
    var p = load_levels(g.xy);
    if (P[0] != 0.0) {
        let l = luma(p);
        let bl = clamp(P[2] + (l - 255.0) * P[1], 0.0, 255.0);   // white: exactly the anchor
        p = div3(p * bl, max(l, 1.0));      // rgb * new luma / luma (a grey pixel gets exactly bl)
    }
    store_levels(g.xy, u8c(p * vec3<f32>(P[4], P[5], P[6])));
}
