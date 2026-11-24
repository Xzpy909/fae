// Curves: one 256-entry table per channel (the `luts` of the edit), 8-bit in, 8-bit out.
// rgba16float holds k/255 to within 0.0005, so round(x * 255) gives back the exact 8-bit level:
// on 8-bit input this pass is bit-exact with the CPU core.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> lut: array<u32, 768>;

fn level(x: f32) -> u32 {
    return u32(clamp(round(x * 255.0), 0.0, 255.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    let size = textureDimensions(src);
    if (g.x >= size.x || g.y >= size.y) { return; }
    let c = textureLoad(src, g.xy, 0);
    let r = f32(lut[level(c.r)]);
    let gg = f32(lut[256u + level(c.g)]);
    let b = f32(lut[512u + level(c.b)]);
    textureStore(dst, vec2<i32>(g.xy), vec4<f32>(r, gg, b, 255.0) / 255.0);
}
