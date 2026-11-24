// Screen output, pass 2 of 2 (vertical): draws into the canvas, one fragment per screen pixel.
// Outside the image the canvas stays transparent (the page's background and shadow show); so do the
// image's own transparent parts. `mid` and the output are premultiplied RGBA (the canvas's alpha mode).
struct V {
    iw: i32, ih: i32,
    x0: i32, r0: i32,
    mw: u32, mh: u32,
    ox: f32, oy: f32,
    s: f32,
    ks: f32,
    reach: f32,
    split: f32,
    nearest: u32,
    pad0: u32, pad1: u32, pad2: u32,
}
@group(0) @binding(0) var mid: texture_2d<f32>;
@group(0) @binding(1) var<uniform> v: V;

fn lanczos3(x: f32) -> f32 {
    let a = abs(x);
    if (a < 1e-5) { return 1.0; }
    if (a >= 3.0) { return 0.0; }
    let p = 3.14159265358979 * x;
    return 3.0 * sin(p) * sin(p / 3.0) / (p * p);
}

fn row(y: i32, col: i32) -> vec4<f32> {
    let r = clamp(clamp(y, 0, v.ih - 1) - v.r0, 0, i32(v.mh) - 1);
    return textureLoad(mid, vec2<i32>(col, r), 0);
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // pos.xy = screen pixel centre
    let fx = (pos.x - v.ox) / v.s;
    let fy = (pos.y - v.oy) / v.s;
    if (fx < 0.0 || fy < 0.0 || fx >= f32(v.iw) || fy >= f32(v.ih)) { discard; }
    let col = i32(pos.x) - v.x0;
    if (col < 0 || col >= i32(v.mw)) { discard; }
    let u = fy - 0.5;
    var c: vec4<f32>;
    if (v.nearest != 0u) {
        c = row(i32(floor(fy)), col);
    } else {
        let lo = i32(floor(u - v.reach)) + 1;
        let hi = i32(floor(u + v.reach));
        var acc = vec4<f32>(0.0);
        var ws = 0.0;
        for (var y = lo; y <= hi; y++) {
            let w = lanczos3((f32(y) - u) * v.ks);
            acc += w * row(y, col);
            ws += w;
        }
        c = acc / ws;
    }
    let a = clamp(c.a, 0.0, 1.0);
    return vec4<f32>(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(a)), a);
}
