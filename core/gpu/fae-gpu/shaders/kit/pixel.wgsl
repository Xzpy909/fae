// Filter kit template: per-pixel. One pass: out = fx(c, at).
@compute @workgroup_size(16, 16)
fn apply(@builtin(global_invocation_id) g: vec3<u32>) {
    if (!inside(g)) { return; }
    let f = load_levels(g.xy);
    finish(g.xy, f, fx(f, g.xy));
}
