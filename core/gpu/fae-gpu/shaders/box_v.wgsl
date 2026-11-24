// Box filter, vertical pass over the horizontal sums, scaled to the mean (1 / (2r+1)^2), mirrored
// edges; exact sliding sums per segment of SEG rows in fixed point (fixed.wgsl, prepended), as in
// box_h.wgsl. The sum is rounded to f32, then scaled (an exact division agreed with the CPU core
// on 4 % more values but cost 70 % more time).
// mode 0: the guided filter's coefficients from (mean y, mean y^2):
//   v = m2 - m^2, a = v / (v + eps), b = m - a m   -> (a, b)
// mode 1: the means themselves.  P: [w, h, r, mode, eps]; one thread per column and segment.
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
    let x = i32(g.x);
    let y0 = i32(g.y) * SEG;
    if (x >= w || y0 >= h) { return; }
    var s0 = vec2<u32>(0u);
    var s1 = vec2<u32>(0u);
    for (var k = -r; k <= r; k++) {
        let v = src[refl(y0 + k, h) * w + x];
        s0 = add64(s0, fx(v.x));
        s1 = add64(s1, fx(v.y));
    }
    let kk = f32(2 * r + 1);
    let scale = 1.0 / (kk * kk);
    let y1 = min(y0 + SEG, h);
    for (var y = y0; y < y1; y++) {
        let mean = vec2<f32>(to_f32(s0, false), to_f32(s1, false)) * scale;
        var o = mean;
        if (P[3] == 0.0) {
            let v = mean.y - mean.x * mean.x;
            let a = v / (v + P[4]);
            o = vec2<f32>(a, mean.x - a * mean.x);
        }
        dst[y * w + x] = o;
        let a = src[refl(y + r + 1, h) * w + x];
        let b = src[refl(y - r, h) * w + x];
        s0 = sub64(add64(s0, fx(a.x)), fx(b.x));
        s1 = sub64(add64(s1, fx(a.y)), fx(b.y));
    }
}
