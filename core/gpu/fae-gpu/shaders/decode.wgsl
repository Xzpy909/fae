// Stage pixels (packed 8-bit RGB or RGBA, `bpp` bytes per pixel, in a u32 array) -> rgba16float,
// 0..1. Once per stage; the chain's passes then read and write float images. The alpha (RGBA: the
// image's own, RGB: 1) stays in `orig` only: the steps work on RGB, the screen output takes the
// alpha from `orig` (transparent images show the page's background, view_h.wgsl).
struct P { w: u32, h: u32, bpp: u32, n: u32 }
@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> p: P;

fn byte(i: u32) -> f32 {
    return f32((src[i >> 2u] >> ((i & 3u) * 8u)) & 255u);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= p.w || g.y >= p.h) { return; }
    let i = (g.y * p.w + g.x) * p.bpp;
    let a = select(255.0, byte(i + 3u), p.bpp == 4u);
    textureStore(dst, vec2<i32>(g.xy), vec4<f32>(byte(i), byte(i + 1u), byte(i + 2u), a) / 255.0);
}
