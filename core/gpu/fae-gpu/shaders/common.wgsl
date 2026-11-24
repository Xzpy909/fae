// Shared by the per-pixel steps (prepended to each step's shader by the host).
// The images between passes hold 8-bit levels as k / 255 in rgba16float (exact after rounding).
// Every step works on whole levels and ends like the CPU core's stages: clamp to 0..255, then
// truncate (u8c) or round (rnd). Luma and the table indices from it are exact (integers), as in
// the CPU core; the rest is f32 (the CPU core: f64), so a value that lands within ~1e-5 of a whole
// level can still truncate the other way: +-1 level on a few pixels in 100 000.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;

fn load_levels(at: vec2<u32>) -> vec3<f32> {
    return clamp(round(textureLoad(src, at, 0).rgb * 255.0), vec3<f32>(0.0), vec3<f32>(255.0));
}

fn store_levels(at: vec2<u32>, p: vec3<f32>) {
    textureStore(dst, vec2<i32>(at), vec4<f32>(p / 255.0, 1.0));
}

// clip(x, 0, 255) then truncate
fn u8c(v: vec3<f32>) -> vec3<f32> {
    return floor(clamp(v, vec3<f32>(0.0), vec3<f32>(255.0)));
}

// clip(floor(x + 0.5), 0, 255)
fn rnd(v: vec3<f32>) -> vec3<f32> {
    return floor(clamp(v + 0.5, vec3<f32>(0.0), vec3<f32>(255.0)));
}

// 0.2126 r + 0.7152 g + 0.0722 b for whole levels, computed exactly: 10000 * luma is an integer
// (< 2^24, exact in f32), split into its whole part (exact) and the fraction (k / 10000, never
// closer than 2e-4 to a whole level, so f32 rounding can't move it across one).
fn luma(p: vec3<f32>) -> f32 {
    let l = u32(p.r) * 2126u + u32(p.g) * 7152u + u32(p.b) * 722u;
    return f32(l / 10000u) + f32(l % 10000u) * 1e-4;
}

// min(floor(luma * 16 + 0.5), 4095) of whole levels, in integers (exact): index into the
// 4096-entry tone / contrast tables
fn t16(p: vec3<f32>) -> u32 {
    let l = u32(p.r) * 2126u + u32(p.g) * 7152u + u32(p.b) * 722u;
    return min((l * 16u + 5000u) / 10000u, 4095u);
}

// x / y with exact quotients exact: WGSL division may be off by 2.5 ulp (GPUs use a reciprocal),
// which turns 255 * 37 / 37 into 254.99998 -> 254. If the rounded quotient times y gives x back
// exactly, it is the quotient (what correctly rounded division returns).
fn div(x: f32, y: f32) -> f32 {
    let q = x / y;
    let r = round(q);
    return select(q, r, r * y == x);
}

fn div3(x: vec3<f32>, y: f32) -> vec3<f32> {
    return vec3<f32>(div(x.r, y), div(x.g, y), div(x.b, y));
}

fn inside(g: vec3<u32>) -> bool {
    let s = textureDimensions(src);
    return g.x < s.x && g.y < s.y;
}
