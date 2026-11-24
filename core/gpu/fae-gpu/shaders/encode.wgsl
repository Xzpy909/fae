// Float image -> packed 8-bit RGB for the encoders (Process) and for tests. One invocation
// writes 4 pixels = 3 whole u32 words. Rounds to the nearest level and clamps.
struct P { w: u32, h: u32, stride: u32, n: u32 }
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<u32>;
@group(0) @binding(2) var<uniform> p: P;

fn q(x: f32) -> u32 {
    return u32(clamp(round(x * 255.0), 0.0, 255.0));
}

fn px(i: u32) -> vec3<u32> {
    if (i >= p.n) { return vec3<u32>(0u); }
    let c = textureLoad(src, vec2<u32>(i % p.w, i / p.w), 0);
    return vec3<u32>(q(c.r), q(c.g), q(c.b));
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    let k = g.x + g.y * p.stride;
    let i = k * 4u;
    if (i >= p.n) { return; }
    let a = px(i);
    let b = px(i + 1u);
    let c = px(i + 2u);
    let d = px(i + 3u);
    let len = arrayLength(&dst);
    let o = k * 3u;
    dst[o] = a.x | (a.y << 8u) | (a.z << 16u) | (b.x << 24u);
    if (o + 1u < len) { dst[o + 1u] = b.y | (b.z << 8u) | (c.x << 16u) | (c.y << 24u); }
    if (o + 2u < len) { dst[o + 2u] = c.z | (d.x << 8u) | (d.y << 16u) | (d.z << 24u); }
}
