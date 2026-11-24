// Sharpen, the blur-based parts (CPU core: sharp.rs blur_map / local_contrast / focus): Adaptive
// Sharpening (with Balance), Local Contrast, Focus. Per channel, out = f(c, blur_c), where blur is a
// Gaussian (mirrored edges, reflect-101) of map[c] on a copy shrunk by k x k block means (k = 1: the
// image itself), read back bilinearly. Four passes, each mirroring the CPU core's loops and summing in
// the same order: means (image -> A), across (A -> B), down (B -> A), apply (image + A -> out).
// A and B: 3 f32 per cell of the grid (w2 x h2).
// P: 0 mode (0: c + g (c - blur), 1: Focus < 0, blur of c^3.8 back through a table, mixed in at P[11]),
//    1 g, 2 k, 3 taps, 4 radius, 5 Balance (-1..1, 0 = off), 6 w2, 7 h2, 8 w, 9 h, 11 mix;
//    16..272 the map of the level; 272.. the kernel (taps); then the back table (4097, mode 1).
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> P: array<f32>;
@group(0) @binding(3) var<storage, read_write> A: array<f32>;
@group(0) @binding(4) var<storage, read_write> B: array<f32>;

const TAB: u32 = 16u;
const KER: u32 = 272u;
const NB: u32 = 4096u;

fn refl(i: i32, n: i32) -> i32 {
    if (i >= 0 && i < n) { return i; }
    if (n == 1) { return 0; }
    let p = 2 * n - 2;
    var a = abs(i) % p;
    if (a >= n) { a = p - a; }
    return a;
}

fn levels(at: vec2<i32>) -> vec3<f32> {
    return clamp(round(textureLoad(src, at, 0).rgb * 255.0), vec3<f32>(0.0), vec3<f32>(255.0));
}

fn get3(i: u32) -> vec3<f32> {
    return vec3<f32>(A[i * 3u], A[i * 3u + 1u], A[i * 3u + 2u]);
}

// x / n correctly rounded for a whole n in 1 .. 4096 (WGSL's division may be off by 2.5 ulp; the
// CPU core's isn't): the hardware quotient, corrected by the exact remainder x - q n (q split in two
// halves of 12 bits, so each product is exact; the subtractions are exact by Sterbenz's lemma).
fn qdiv(x: f32, n: f32) -> f32 {
    let ax = abs(x);
    var q = ax / n;
    for (var it = 0; it < 2; it++) {
        let qh = bitcast<f32>(bitcast<u32>(q) & 0xfffff000u);
        let ql = q - qh;
        let r = (ax - qh * n) - ql * n;          // ax - q n, exact
        let up = bitcast<f32>(bitcast<u32>(q) + 1u);
        let u = up - q;                          // one ulp of q
        let half = u * n * 0.5;
        if (r > half || (r == half && (bitcast<u32>(q) & 1u) == 1u)) {
            q = up;
        } else if (q > 0.0) {
            let dn = bitcast<f32>(bitcast<u32>(q) - 1u);
            let hd = (q - dn) * n * 0.5;
            if (-r > hd || (-r == hd && (bitcast<u32>(q) & 1u) == 1u)) { q = dn; }
        }
    }
    return select(q, -q, x < 0.0);
}

// 1 of 4: block means of map[c] over k x k (partial blocks at the right / bottom average what they have)
@compute @workgroup_size(16, 16)
fn means(@builtin(global_invocation_id) g: vec3<u32>) {
    let k = u32(P[2]);
    let w2 = u32(P[6]);
    let h2 = u32(P[7]);
    if (g.x >= w2 || g.y >= h2) { return; }
    let w = u32(P[8]);
    let h = u32(P[9]);
    let y0 = g.y * k;
    let y1 = min(y0 + k, h);
    let x0 = g.x * k;
    let x1 = min(x0 + k, w);
    var a = vec3<f32>(0.0);
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            let p = vec3<u32>(levels(vec2<i32>(i32(x), i32(y))));
            a += vec3<f32>(P[TAB + p.r], P[TAB + p.g], P[TAB + p.b]);
        }
    }
    let n = f32((y1 - y0) * (x1 - x0));
    let i = (g.y * w2 + g.x) * 3u;
    A[i] = qdiv(a.r, n);
    A[i + 1u] = qdiv(a.g, n);
    A[i + 2u] = qdiv(a.b, n);
}

// 2 of 4: the Gaussian along each row, A -> B
@compute @workgroup_size(16, 16)
fn across(@builtin(global_invocation_id) g: vec3<u32>) {
    let w2 = i32(P[6]);
    let h2 = i32(P[7]);
    let x = i32(g.x);
    let y = i32(g.y);
    if (x >= w2 || y >= h2) { return; }
    let taps = i32(P[3]);
    let r = i32(P[4]);
    var acc = vec3<f32>(0.0);
    for (var t = 0; t < taps; t++) {
        acc += P[KER + u32(t)] * get3(u32(y * w2 + refl(x + t - r, w2)));
    }
    let i = u32(y * w2 + x) * 3u;
    B[i] = acc.r;
    B[i + 1u] = acc.g;
    B[i + 2u] = acc.b;
}

// 3 of 4: the Gaussian down each column, B -> A
@compute @workgroup_size(16, 16)
fn down(@builtin(global_invocation_id) g: vec3<u32>) {
    let w2 = i32(P[6]);
    let h2 = i32(P[7]);
    let x = i32(g.x);
    let y = i32(g.y);
    if (x >= w2 || y >= h2) { return; }
    let taps = i32(P[3]);
    let r = i32(P[4]);
    var acc = vec3<f32>(0.0);
    for (var t = 0; t < taps; t++) {
        let j = u32(refl(y + t - r, h2) * w2 + x) * 3u;
        acc += P[KER + u32(t)] * vec3<f32>(B[j], B[j + 1u], B[j + 2u]);
    }
    let i = u32(y * w2 + x) * 3u;
    A[i] = acc.r;
    A[i + 1u] = acc.g;
    A[i + 2u] = acc.b;
}

// sharp.rs u8r: clamp to 0..255, + 0.5, truncate
fn u8r(v: vec3<f32>) -> vec3<f32> {
    return floor(clamp(v, vec3<f32>(0.0), vec3<f32>(255.0)) + 0.5);
}

// the grid position of image pixel p: the two cells and the weight of the second (bilinear between
// block centres, clamped at the borders)
fn pos(p: u32, k: f32, n2: u32) -> vec3<f32> {
    let f = max(qdiv(f32(p) + 0.5, k) - 0.5, 0.0);
    let i0 = min(u32(f), n2 - 1u);
    let i1 = min(i0 + 1u, n2 - 1u);
    return vec3<f32>(f32(i0), f32(i1), f - f32(i0));
}

fn lum(x: i32, y: i32) -> f32 {
    let p = levels(vec2<i32>(x, y));
    return 0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b;
}

// Balance Sharpening's weight at (x, y): Sobel of the luma (/ 8), t = log2(1 + |grad|)
fn balance(x: i32, y: i32, w: i32, h: i32, b: f32) -> f32 {
    let ym = refl(y - 1, h);
    let yp = refl(y + 1, h);
    let xm = refl(x - 1, w);
    let xp = refl(x + 1, w);
    let gx = (lum(xp, ym) + 2.0 * lum(xp, y) + lum(xp, yp) - lum(xm, ym) - 2.0 * lum(xm, y) - lum(xm, yp)) / 8.0;
    let gy = (lum(xm, yp) + 2.0 * lum(x, yp) + lum(xp, yp) - lum(xm, ym) - 2.0 * lum(x, ym) - lum(xp, ym)) / 8.0;
    let t = log2(1.0 + sqrt(gx * gx + gy * gy));
    var wt: f32;
    if (b > 0.0) {
        wt = 0.634 + 0.283 * t;
    } else {
        wt = 4.942 * exp(-2.257 * t) + 1.102 + -0.285 * t;
    }
    return 1.0 + abs(b) * (wt - 1.0);
}

// 4 of 4: out = f(c, blur)
@compute @workgroup_size(16, 16)
fn apply(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = u32(P[8]);
    let h = u32(P[9]);
    if (g.x >= w || g.y >= h) { return; }
    let k = P[2];
    let w2 = u32(P[6]);
    let h2 = u32(P[7]);
    let v = levels(vec2<i32>(g.xy));
    var b: vec3<f32>;
    if (k == 1.0) {
        b = get3(g.y * w2 + g.x);
    } else {
        let px = pos(g.x, k, w2);
        let py = pos(g.y, k, h2);
        let r0 = u32(py.x) * w2;
        let r1 = u32(py.y) * w2;
        let a00 = get3(r0 + u32(px.x));
        let a01 = get3(r0 + u32(px.y));
        let a10 = get3(r1 + u32(px.x));
        let a11 = get3(r1 + u32(px.y));
        let top = a00 + (a01 - a00) * px.z;
        let bot = a10 + (a11 - a10) * px.z;
        b = top + (bot - top) * py.z;
    }
    var o: vec3<f32>;
    if (P[0] == 0.0) {
        var gw = P[1];
        if (P[5] != 0.0) {
            gw = gw * balance(i32(g.x), i32(g.y), i32(w), i32(h), P[5]);
        }
        o = u8r(v + gw * (v - b));
    } else {
        let back = KER + u32(P[3]);
        var bs: vec3<f32>;
        for (var c = 0; c < 3; c++) {
            let q = sqrt(sqrt(max(b[c], 0.0))) * f32(NB);
            let qi = min(u32(q), NB - 1u);
            bs[c] = P[back + qi] + (P[back + qi + 1u] - P[back + qi]) * (q - f32(qi));
        }
        o = u8r(v + P[11] * (bs - v));
    }
    textureStore(dst, vec2<i32>(g.xy), vec4<f32>(o / 255.0, 1.0));
}
