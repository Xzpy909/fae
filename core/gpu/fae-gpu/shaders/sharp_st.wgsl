// Sharpen's Structure (CPU core: sharp.rs structure + fine3, md.rs guided_fast):
//   det = 255 (fine + 0.52 (y - mid) + 0.94 (y - coarse)) tilt(y),  out_c = c + d(det) (tanh table)
// y = luma 0..1; fine = -0.67 (y - g3(y, 1e-3)) + 0.88 (y - g3(y, 1e-2)) with 3x3 self-guided filters;
// mid / coarse = fast guided filters: (a, b) on a copy shrunk by k (box filters of box_h / box_v),
// scaled back up bilinearly, a y + b. Passes (host: sharp.rs): prep (per filter: block means of y and
// their squares), the box filters, fine (the 3x3 coefficients), apply.
// P: 0 coarse shrink factor, 1 mid shrink factor (the host fills both in for the stage);
//    16.. the table d(det) over det (4 steps per level, 8193 entries).
// Q (prep): [k, w2, h2].
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> P: array<f32>;
@group(0) @binding(3) var<storage, read_write> grid: array<vec2<f32>>;
@group(0) @binding(4) var<storage, read> Q: array<f32>;
@group(0) @binding(5) var<storage, read_write> F0: array<vec2<f32>>;
@group(0) @binding(6) var<storage, read_write> F1: array<vec2<f32>>;
@group(0) @binding(7) var<storage, read> coarse: array<vec2<f32>>;
@group(0) @binding(8) var<storage, read> mid: array<vec2<f32>>;

const TABLE: u32 = 16u;
const TQ: f32 = 4.0;
const TN: f32 = 4096.0;
const EPS_A: f32 = 1e-3;
const EPS_B: f32 = 1e-2;
const C1A: f32 = -0.67;
const C1B: f32 = 0.88;
const CM: f32 = 0.52;
const C2: f32 = 0.94;
const TILT: f32 = -0.47;

// x / n correctly rounded for a whole n in 1 .. 4096 (as in sharp_blur.wgsl)
fn qdiv(x: f32, n: f32) -> f32 {
    let ax = abs(x);
    var q = ax / n;
    for (var it = 0; it < 2; it++) {
        let qh = bitcast<f32>(bitcast<u32>(q) & 0xfffff000u);
        let ql = q - qh;
        let r = (ax - qh * n) - ql * n;
        let up = bitcast<f32>(bitcast<u32>(q) + 1u);
        let u = up - q;
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

fn levels(at: vec2<i32>) -> vec3<f32> {
    return clamp(round(textureLoad(src, at, 0).rgb * 255.0), vec3<f32>(0.0), vec3<f32>(255.0));
}

// luma 0..1 as the CPU core's structure(): (0.2126 r + 0.7152 g + 0.0722 b) / 255 in f32
fn y01(x: i32, y: i32) -> f32 {
    let p = levels(vec2<i32>(x, y));
    return qdiv(0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b, 255.0);
}

// reflect-101 for the 3x3 window (sharp.rs rf)
fn rf(i: i32, n: i32) -> i32 {
    if (n == 1) { return 0; }
    if (i < 0) { return 1; }
    if (i >= n) { return n - 2; }
    return i;
}

// prep: cell (bx, by) of the copy shrunk by k: mean of y over the block (row sums first, as
// md::guided_fast), and its square
@compute @workgroup_size(16, 16)
fn prep(@builtin(global_invocation_id) g: vec3<u32>) {
    let k = u32(Q[0]);
    let w2 = u32(Q[1]);
    let h2 = u32(Q[2]);
    if (g.x >= w2 || g.y >= h2) { return; }
    let sz = textureDimensions(src);
    let y0 = g.y * k;
    let y1 = min(y0 + k, sz.y);
    let x0 = g.x * k;
    let x1 = min(x0 + k, sz.x);
    var s = 0.0;
    for (var y = y0; y < y1; y++) {
        var row = 0.0;
        for (var x = x0; x < x1; x++) {
            row += y01(i32(x), i32(y));
        }
        s += row;
    }
    let m = qdiv(s, f32((y1 - y0) * (x1 - x0)));
    grid[g.y * w2 + g.x] = vec2<f32>(m, m * m);
}

// fine, 1 of 2: per pixel the 3x3 guided filter coefficients for both eps:
// F0 = (a, b) with eps 1e-3, F1 = (a, b) with eps 1e-2
@compute @workgroup_size(16, 16)
fn fine(@builtin(global_invocation_id) g: vec3<u32>) {
    let sz = vec2<i32>(textureDimensions(src));
    let x = i32(g.x);
    let y = i32(g.y);
    if (x >= sz.x || y >= sz.y) { return; }
    var hs = array<f32, 3>(0.0, 0.0, 0.0);
    var hq = array<f32, 3>(0.0, 0.0, 0.0);
    for (var t = 0; t < 3; t++) {
        let yy = rf(y + t - 1, sz.y);
        let l = y01(rf(x - 1, sz.x), yy);
        let c = y01(x, yy);
        let r = y01(rf(x + 1, sz.x), yy);
        hs[t] = l + c + r;
        hq[t] = l * l + c * c + r * r;
    }
    let m = qdiv(hs[0] + hs[1] + hs[2], 9.0);
    let q = qdiv(hq[0] + hq[1] + hq[2], 9.0);
    let v = q - m * m;
    let a = v / (v + EPS_A);
    let a2 = v / (v + EPS_B);
    let i = u32(y * sz.x + x);
    F0[i] = vec2<f32>(a, m - a * m);
    F1[i] = vec2<f32>(a2, m - a2 * m);
}

// bilinear between block centres (md::guided_fast's pos): cells and the second's weight
fn pos(p: u32, k: f32, n2: u32) -> vec3<f32> {
    let f = max(qdiv(f32(p) + 0.5, k) - 0.5, 0.0);
    let i0 = min(u32(f), n2 - 1u);
    let i1 = min(i0 + 1u, n2 - 1u);
    return vec3<f32>(f32(i0), f32(i1), f - f32(i0));
}

// a y + b of a fast guided filter on the grid shrunk by k (k = 1: the full-size filter)
fn guided(which: u32, x: u32, y: u32, yv: f32, k: u32, w: u32, h: u32) -> f32 {
    if (k == 1u) {
        var c: vec2<f32>;
        if (which == 0u) { c = coarse[y * w + x]; } else { c = mid[y * w + x]; }
        return c.x * yv + c.y;
    }
    let w2 = (w + k - 1u) / k;
    let h2 = (h + k - 1u) / k;
    let px = pos(x, f32(k), w2);
    let py = pos(y, f32(k), h2);
    let r0 = u32(py.x) * w2;
    let r1 = u32(py.y) * w2;
    var c00: vec2<f32>;
    var c01: vec2<f32>;
    var c10: vec2<f32>;
    var c11: vec2<f32>;
    if (which == 0u) {
        c00 = coarse[r0 + u32(px.x)]; c01 = coarse[r0 + u32(px.y)];
        c10 = coarse[r1 + u32(px.x)]; c11 = coarse[r1 + u32(px.y)];
    } else {
        c00 = mid[r0 + u32(px.x)]; c01 = mid[r0 + u32(px.y)];
        c10 = mid[r1 + u32(px.x)]; c11 = mid[r1 + u32(px.y)];
    }
    let top = c00 + (c01 - c00) * px.z;
    let bot = c10 + (c11 - c10) * px.z;
    let ab = top + (bot - top) * py.z;
    return ab.x * yv + ab.y;
}

// fine, 2 of 2, and the rest: the output
@compute @workgroup_size(16, 16)
fn apply(@builtin(global_invocation_id) g: vec3<u32>) {
    let sz = vec2<i32>(textureDimensions(src));
    let x = i32(g.x);
    let y = i32(g.y);
    if (x >= sz.x || y >= sz.y) { return; }
    let yv = y01(x, y);
    // vertical 3-sums of a and b at the three columns, then the horizontal 3-sum
    var va = array<vec4<f32>, 3>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    for (var t = 0; t < 3; t++) {
        let xx = rf(x + t - 1, sz.x);
        var s = vec4<f32>(0.0);
        for (var d = 0; d < 3; d++) {
            let i = u32(rf(y + d - 1, sz.y) * sz.x + xx);
            s += vec4<f32>(F0[i], F1[i]);
        }
        va[t] = s;
    }
    let hsum = va[0] + va[1] + va[2];
    let ga = qdiv(hsum.x, 9.0) * yv + qdiv(hsum.y, 9.0);
    let gb = qdiv(hsum.z, 9.0) * yv + qdiv(hsum.w, 9.0);
    let fine = C1A * (yv - ga) + C1B * (yv - gb);
    let w = u32(sz.x);
    let h = u32(sz.y);
    let co = guided(0u, g.x, g.y, yv, u32(P[0]), w, h);
    let mi = guided(1u, g.x, g.y, yv, u32(P[1]), w, h);
    let u = TILT * (yv - 0.5);
    let tilt = 1.0 + u * (1.0 + u * (0.5 + u * (1.0 / 6.0)));
    let det = 255.0 * (fine + CM * (yv - mi) + C2 * (yv - co)) * tilt;
    let tt = clamp(det * TQ, 1.0 - TN, TN - 1.0) + TN;
    let ti = u32(tt);
    let dd = P[TABLE + ti] + (P[TABLE + ti + 1u] - P[TABLE + ti]) * (tt - f32(ti));
    let p = levels(vec2<i32>(x, y));
    let o = floor(clamp(p + dd, vec3<f32>(0.0), vec3<f32>(255.0)) + 0.5);
    textureStore(dst, vec2<i32>(g.xy), vec4<f32>(o / 255.0, 1.0));
}
