// Box filter, horizontal pass: raw sums over x - r ..= x + r, mirrored edges (reflect-101, as the
// CPU core / OpenCV), two values per pixel. A sliding sum like the CPU core's, kept exact in
// fixed point (fixed.wgsl, prepended): each thread starts one segment of SEG pixels with a full
// sum, then adds the sample entering and drops the one leaving (~2 reads per pixel instead of
// 2r + 1). Output: the nearest f32 of the exact sum, as the CPU core stores its f64 sums.
// P: [w, h, r]; one thread per segment of a row.
@group(0) @binding(0) var<storage, read> src: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read_write> dst: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read> P: array<f32>;

const SEG: i32 = 64;

fn refl(i: i32, n: i32) -> i32 {
    if (n == 1) { return 0; }
    let p = 2 * n - 2;
    var a = abs(i) % p;
    if (a >= n) { a = p - a; }
    return a;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = i32(P[0]);
    let h = i32(P[1]);
    let r = i32(P[2]);
    let x0 = i32(g.x) * SEG;
    let y = i32(g.y);
    if (x0 >= w || y >= h) { return; }
    let row = y * w;
    var s0 = vec2<u32>(0u);
    var s1 = vec2<u32>(0u);
    for (var k = -r; k <= r; k++) {
        let v = src[row + refl(x0 + k, w)];
        s0 = add64(s0, fx(v.x));
        s1 = add64(s1, fx(v.y));
    }
    let x1 = min(x0 + SEG, w);
    for (var x = x0; x < x1; x++) {
        dst[row + x] = vec2<f32>(to_f32(s0, false), to_f32(s1, false));
        let a = src[row + refl(x + r + 1, w)];
        let b = src[row + refl(x - r, w)];
        s0 = sub64(add64(s0, fx(a.x)), fx(b.x));
        s1 = sub64(add64(s1, fx(a.y)), fx(b.y));
    }
}
