// Film Effect: the preset's 3D table of deltas (n^3 x 3, r major), trilinear by hand (texture
// filtering has ~8-bit weights), rounded; then the Brightness / Contrast table; then opacity.
// P: [n, table on, tone table on, opacity, opacity table on, offset of the 3D table],
//    [16..272) tone table, [272..272+65536) opacity table (when op != 1: for each level before f and
//    level j, rnd(f + (t1[j] or j - f) * op) as the CPU core computes it in f64), then the 3D table.
@group(0) @binding(2) var<storage, read> P: array<f32>;

const T1: u32 = 16u;
const MIX: u32 = 272u;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (!inside(g)) { return; }
    let f = load_levels(g.xy);
    var q = f;
    if (P[1] != 0.0) {
        let n = u32(P[0]);
        let D = u32(P[5]);
        let nm1 = P[0] - 1.0;
        let top = P[0] - 2.0;
        let gg = div3(f * nm1, 255.0);
        let ic = min(floor(gg), vec3<f32>(top));
        let fr = gg - ic;
        let i = vec3<u32>(ic);
        var acc = vec3<f32>(0.0);
        for (var dr = 0u; dr < 2u; dr++) {
            let wr = select(1.0 - fr.r, fr.r, dr == 1u);
            for (var dg = 0u; dg < 2u; dg++) {
                let wg = select(1.0 - fr.g, fr.g, dg == 1u);
                for (var db = 0u; db < 2u; db++) {
                    let wb = select(1.0 - fr.b, fr.b, db == 1u);
                    let w = wr * wg * wb;
                    let k = D + (((i.r + dr) * n + (i.g + dg)) * n + (i.b + db)) * 3u;
                    acc += w * vec3<f32>(P[k], P[k + 1u], P[k + 2u]);
                }
            }
        }
        q = rnd(f + acc);
    }
    if (P[4] != 0.0) {
        // opacity (and the tone table) from the table: exact
        let fi = vec3<u32>(f) * 256u + vec3<u32>(q);
        q = vec3<f32>(P[MIX + fi.r], P[MIX + fi.g], P[MIX + fi.b]);
    } else if (P[2] != 0.0) {
        q = vec3<f32>(P[T1 + u32(q.r)], P[T1 + u32(q.g)], P[T1 + u32(q.b)]);
        q = u8c(q);              // a table value need not be whole: truncate like `as u8`
    }
    store_levels(g.xy, q);
}
