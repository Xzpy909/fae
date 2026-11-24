// Mid/Detail, 1 of 5: the luma 0..1 of each pixel and its square, the guided filter's input
// (luma01 in the CPU core: f32 arithmetic). out[i] = (y, y * y).
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> out: array<vec2<f32>>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    let s = textureDimensions(src);
    if (g.x >= s.x || g.y >= s.y) { return; }
    let p = clamp(round(textureLoad(src, g.xy, 0).rgb * 255.0), vec3<f32>(0.0), vec3<f32>(255.0));
    let y = (0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b) / 255.0;
    out[g.y * s.x + g.x] = vec2<f32>(y, y * y);
}
