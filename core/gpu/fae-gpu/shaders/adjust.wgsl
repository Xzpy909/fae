// Adjustments: Mid/Detail (with the smoothed luma from the guided filter passes), Temp / Tint
// matrix, Contrast / Pivot table, Shadows / Highlights tone tables, Color Boost, Saturation - in
// that order, each ending in u8c.
// P: [0] wb on, [1] contrast on, [2] tone tables, [3] boost on, [4] saturation on, [5] saturation,
//    [6..15) wb rows, [15] Mid/Detail on, [16..32) per tone: 1 = ratio (lift), 0 = add;
//    [32] Mid/Detail amount * 0.952 * 255, [33] image width;
//    [48..4144) contrast table, [4144..4400) boost table, [4400..8497) Mid/Detail's fade-in weight
//    by luma (4097 entries over 0..1, linear in between, as the CPU core), then one 4096 table per tone.
// md[i] = the guided filter's (mean a, mean b) at pixel i: smoothed luma = a * y01 + b.
@group(0) @binding(2) var<storage, read> P: array<f32>;
@group(0) @binding(3) var<storage, read> md: array<vec2<f32>>;

const CON: u32 = 48u;
const BOOST: u32 = 4144u;
const MDW: u32 = 4400u;
const TONES: u32 = 8497u;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (!inside(g)) { return; }
    var p = load_levels(g.xy);
    if (P[15] != 0.0) {
        // Mid/Detail: detail = luma - smoothed luma, added to all channels, faded out in the shadows
        let y01 = (0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b) / 255.0;       // the filter's input
        let ab = md[g.y * u32(P[33]) + g.x];
        let base = ab.x * y01 + ab.y;
        let y = (0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b) / 255.0;
        let pos = clamp(y, 0.0, 1.0) * 4096.0;
        let j = min(u32(pos), 4095u);
        let fr = pos - f32(j);
        let wt = P[MDW + j] + (P[MDW + j + 1u] - P[MDW + j]) * fr;
        var d = P[32] * wt * (y - base);
        if (abs(d) < 1.0 / 256.0) { d = 0.0; }      // no change in flat areas (as the CPU core)
        p = u8c(p + d);
    }
    if (P[0] != 0.0) {
        p = u8c(vec3<f32>(
            p.r * P[6] + p.g * P[7] + p.b * P[8],
            p.r * P[9] + p.g * P[10] + p.b * P[11],
            p.r * P[12] + p.g * P[13] + p.b * P[14]));
    }
    if (P[1] != 0.0) {
        let c = vec3<f32>(P[CON + u32(p.r) * 16u], P[CON + u32(p.g) * 16u], P[CON + u32(p.b) * 16u]);
        let d = P[CON + t16(p)] - (0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b);
        p = u8c(c + d);
    }
    let nt = u32(P[2]);
    for (var i = 0u; i < nt; i++) {
        let y = luma(p);
        let y2 = P[TONES + i * 4096u + t16(p)];
        if (P[16u + i] != 0.0) {
            p = u8c(div3(p * y2, max(y, 0.5)));
        } else {
            p = u8c(p + (y2 - y));
        }
    }
    if (P[3] != 0.0) {
        let gn = P[BOOST + u32(max(max(p.r, p.g), p.b) - min(min(p.r, p.g), p.b))];
        let y = luma(p);
        p = u8c(y + (p - y) * gn);
    }
    if (P[4] != 0.0) {
        let y = luma(p);
        p = u8c(y + (p - y) * P[5]);
    }
    store_levels(g.xy, p);
}
