// Screen output, pass 1 of 2 (horizontal): the image resampled to exact screen columns, for the
// image rows the screen needs. Lanczos-3, widened by 1/s when shrinking (s = screen px per image
// px), so every image pixel contributes; nearest neighbour when zoomed in to s >= 2 (pixel view).
// Columns left of `split` (image x) come from the unedited stage: the original | edited split view.
// Transparent images: the alpha comes from the stage (`orig`; the steps compute RGB only); filtered
// premultiplied (no dark fringes), `mid` holds premultiplied RGBA.
struct V {
    iw: i32, ih: i32,        // image size
    x0: i32, r0: i32,        // first screen column / first image row held in `mid`
    mw: u32, mh: u32,        // size of the part of `mid` in use
    ox: f32, oy: f32,        // screen position of the image's top-left corner (device px)
    s: f32,                  // screen px per image px
    ks: f32,                 // kernel scale: min(s, 1)
    reach: f32,              // filter reach in image px: 3 / ks
    split: f32,              // image x left of which the unedited stage shows
    nearest: u32,
    pad0: u32, pad1: u32, pad2: u32,
}
@group(0) @binding(0) var res: texture_2d<f32>;
@group(0) @binding(1) var orig: texture_2d<f32>;
@group(0) @binding(2) var mid: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var<uniform> v: V;

fn lanczos3(x: f32) -> f32 {
    let a = abs(x);
    if (a < 1e-5) { return 1.0; }
    if (a >= 3.0) { return 0.0; }
    let p = 3.14159265358979 * x;
    return 3.0 * sin(p) * sin(p / 3.0) / (p * p);
}

fn px(x: i32, y: i32, from_orig: bool) -> vec4<f32> {
    let c = vec2<i32>(clamp(x, 0, v.iw - 1), y);
    let o = textureLoad(orig, c, 0);
    var rgb = o.rgb;
    if (!from_orig) { rgb = textureLoad(res, c, 0).rgb; }
    return vec4<f32>(rgb * o.a, o.a);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= v.mw || g.y >= v.mh) { return; }
    let X = f32(v.x0 + i32(g.x)) + 0.5;               // screen column centre
    let u = (X - v.ox) / v.s - 0.5;                    // in image px (pixel centres at integers)
    let y = clamp(v.r0 + i32(g.y), 0, v.ih - 1);
    let from_orig = (X - v.ox) / v.s < v.split;
    var c: vec4<f32>;
    if (v.nearest != 0u) {
        c = px(i32(floor(u + 0.5)), y, from_orig);
    } else {
        let lo = i32(floor(u - v.reach)) + 1;
        let hi = i32(floor(u + v.reach));
        var acc = vec4<f32>(0.0);
        var ws = 0.0;
        for (var x = lo; x <= hi; x++) {
            let w = lanczos3((f32(x) - u) * v.ks);
            acc += w * px(x, y, from_orig);
            ws += w;
        }
        c = acc / ws;
    }
    textureStore(mid, vec2<i32>(g.xy), c);
}
