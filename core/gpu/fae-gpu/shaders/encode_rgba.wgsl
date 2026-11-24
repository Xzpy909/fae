// Float image -> 8-bit RGBA (alpha 255), one u32 per pixel: the preview's S.outData (readouts,
// loupe, the Alt views drawn on the 2D canvas).
struct P { w: u32, h: u32, stride: u32, n: u32 }
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<u32>;
@group(0) @binding(2) var<uniform> p: P;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    let i = g.x + g.y * p.stride;
    if (i >= p.n) { return; }
    let c = vec3<u32>(clamp(round(textureLoad(src, vec2<u32>(i % p.w, i / p.w), 0).rgb * 255.0),
                            vec3<f32>(0.0), vec3<f32>(255.0)));
    dst[i] = c.r | (c.g << 8u) | (c.b << 16u) | (255u << 24u);
}
