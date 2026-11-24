// Perceptual Saturation (the filter's perceptual mode; ps.rs in the CPU core), four entry points:
//   measure: mid = rnd(Warmth / Saturation of the pixel) and its sums per grid cell (atomics)
//   mean:    cell means (sum / count)            blur: Gaussian over the cells, one axis per run
//   apply:   bilinear surround from the blurred grid, push the colour away from it, opacity.
// P: [0] s, [1] opacity, [2] warmth on, [3] opacity table on, [4..7) warmth gains, [8] amount t,
//    [9] grid rows gh, [10] grid columns gw, [11] w, [12] h, [13] kernel length, [16..80) kernel,
//    [80..848) Warmth per channel and level, [848..848+261376) Saturation rounded, by
//    (Y = r + 2g + b) * 256 + level (both computed on the host in f64, see lib.rs),
//    [262224..+65536) opacity table rnd(a + (q - a) * op) for whole levels a, q (CPU core's f64 result).
// sums: per cell r, g, b, count (u32).  grid / grid2: cell means r, g, b (f32).
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> P: array<f32>;
@group(0) @binding(3) var<storage, read_write> sums: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> grid: array<f32>;
@group(0) @binding(5) var<storage, read_write> grid2: array<f32>;
@group(0) @binding(6) var<storage, read> axis: array<f32>;

const PW = array<f32, 9>(0.331, 0.0256, -0.8911, -0.2478, 3.1506, -0.0857, 0.1625, -0.3969, 0.597);
const WARM: u32 = 80u;
const SAT: u32 = 848u;
const MIX: u32 = 262224u;

fn levels(at: vec2<u32>) -> vec3<f32> {
    return clamp(round(textureLoad(src, at, 0).rgb * 255.0), vec3<f32>(0.0), vec3<f32>(255.0));
}

// the filter's Warmth / Saturation, rounded ("mid")
fn mid_of(f: vec3<f32>) -> vec3<f32> {
    let fi = vec3<u32>(f);
    let w = vec3<u32>(u32(P[WARM + fi.r]), u32(P[WARM + 256u + fi.g]), u32(P[WARM + 512u + fi.b]));
    let yy = (w.r + 2u * w.g + w.b) * 256u;
    return vec3<f32>(P[SAT + yy + w.r], P[SAT + yy + w.g], P[SAT + yy + w.b]);
}

var<workgroup> loc: array<atomic<u32>, 16>;      // 2 x 2 cells around the workgroup's first pixel

@compute @workgroup_size(16, 16)
fn measure(@builtin(global_invocation_id) g: vec3<u32>, @builtin(local_invocation_index) li: u32,
           @builtin(workgroup_id) wg: vec3<u32>) {
    if (li < 16u) { atomicStore(&loc[li], 0u); }
    workgroupBarrier();
    let w = u32(P[11]);
    let h = u32(P[12]);
    let gh = u32(P[9]);
    let gw = u32(P[10]);
    let x0 = min(wg.x * 16u, w - 1u);
    let y0 = min(wg.y * 16u, h - 1u);
    let cy0 = y0 * gh / h;
    let cx0 = x0 * gw / w;
    if (g.x < w && g.y < h) {
        let m = vec3<u32>(mid_of(levels(g.xy)));
        let cy = g.y * gh / h;
        let cx = g.x * gw / w;
        let dy = cy - cy0;
        let dx = cx - cx0;
        if (dy < 2u && dx < 2u) {
            let k = (dy * 2u + dx) * 4u;
            atomicAdd(&loc[k], m.r);
            atomicAdd(&loc[k + 1u], m.g);
            atomicAdd(&loc[k + 2u], m.b);
            atomicAdd(&loc[k + 3u], 1u);
        } else {                                // cells smaller than the workgroup (tiny images)
            let c = (cy * gw + cx) * 4u;
            atomicAdd(&sums[c], m.r);
            atomicAdd(&sums[c + 1u], m.g);
            atomicAdd(&sums[c + 2u], m.b);
            atomicAdd(&sums[c + 3u], 1u);
        }
    }
    workgroupBarrier();
    if (li < 16u) {
        let v = atomicLoad(&loc[li]);
        let dy = li / 8u;
        let dx = (li / 4u) % 2u;
        let cy = cy0 + dy;
        let cx = cx0 + dx;
        if (v != 0u && cy < gh && cx < gw) {
            atomicAdd(&sums[(cy * gw + cx) * 4u + li % 4u], v);
        }
    }
}

@compute @workgroup_size(64)
fn mean(@builtin(global_invocation_id) g: vec3<u32>) {
    let cells = u32(P[9]) * u32(P[10]);
    if (g.x >= cells) { return; }
    let n = atomicLoad(&sums[g.x * 4u + 3u]);
    let d = select(1.0, f32(n), n > 1u);
    for (var c = 0u; c < 3u; c++) {
        grid[g.x * 3u + c] = f32(atomicLoad(&sums[g.x * 4u + c])) / d;
    }
}

// mirror without repeating the edge sample
fn reflect(i: i32, n: i32) -> i32 {
    if (n == 1) { return 0; }
    let p = 2 * n - 2;
    var a = abs(i) % p;
    if (a >= n) { a = p - a; }
    return a;
}

// grid -> grid2 along axis[0] (0 = down the rows, 1 = along the columns)
@compute @workgroup_size(64)
fn blur(@builtin(global_invocation_id) g: vec3<u32>) {
    let gh = i32(P[9]);
    let gw = i32(P[10]);
    if (i32(g.x) >= gh * gw) { return; }
    let y = i32(g.x) / gw;
    let x = i32(g.x) % gw;
    let kl = i32(P[13]);
    let r = (kl - 1) / 2;
    var o = vec3<f32>(0.0);
    for (var t = -r; t <= r; t++) {
        let kt = P[16 + u32(t + r)];
        var sy = y;
        var sx = x;
        if (axis[0] == 0.0) { sy = reflect(y + t, gh); } else { sx = reflect(x + t, gw); }
        let c = u32(sy * gw + sx) * 3u;
        o += kt * vec3<f32>(grid[c], grid[c + 1u], grid[c + 2u]);
    }
    let c = g.x * 3u;
    grid2[c] = o.r;
    grid2[c + 1u] = o.g;
    grid2[c + 2u] = o.b;
}

// bilinear position on the grid for pixel i of n: (lower cell, upper cell, weight)
fn taps(i: u32, n: u32, cells: u32) -> vec3<f32> {
    let gg = clamp(((f32(i) + 0.5) * f32(cells)) / f32(n) - 0.5, 0.0, f32(cells - 1u));
    if (cells > 1u) {
        let i0 = min(floor(gg), f32(cells - 2u));
        return vec3<f32>(i0, min(i0 + 1.0, f32(cells - 1u)), gg - i0);
    }
    return vec3<f32>(0.0);
}

fn cell(y: u32, x: u32, gw: u32) -> vec3<f32> {
    let c = (y * gw + x) * 3u;
    return vec3<f32>(grid[c], grid[c + 1u], grid[c + 2u]);
}

@compute @workgroup_size(16, 16)
fn apply(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = u32(P[11]);
    let h = u32(P[12]);
    if (g.x >= w || g.y >= h) { return; }
    let gh = u32(P[9]);
    let gw = u32(P[10]);
    let ty = taps(g.y, h, gh);
    let tx = taps(g.x, w, gw);
    let y0 = u32(ty.x);
    let y1 = u32(ty.y);
    let x0 = u32(tx.x);
    let x1 = u32(tx.y);
    let r0 = cell(y0, x0, gw) * (1.0 - ty.z) + cell(y1, x0, gw) * ty.z;
    let r1 = cell(y0, x1, gw) * (1.0 - ty.z) + cell(y1, x1, gw) * ty.z;
    let sm = r0 * (1.0 - tx.z) + r1 * tx.z;
    let a = levels(g.xy);
    let f = mid_of(a);
    let yy = (f.r + 2.0 * f.g + f.b) / 4.0;
    let ys = (sm.r + 2.0 * sm.g + sm.b) / 4.0;
    let cc = f - yy;
    let d = sm - ys;
    let m = yy / 255.0 - 0.5;
    let nn = sqrt(dot(d, d)) / 128.0;
    let nc = sqrt(dot(cc, cc)) / 128.0;
    let ka = PW[0] + PW[1] * m + PW[6] * nn + PW[7] * nc;
    let kb = PW[2] + PW[3] * m + PW[4] * m * m + PW[8] * nc;
    let ee = PW[5] * (yy - ys);
    let q = floor(clamp(f + P[8] * (cc * ka + d * kb + ee) + 0.5, vec3<f32>(0.0), vec3<f32>(255.0)));
    var o = q;
    if (P[3] != 0.0) {
        let i = vec3<u32>(a) * 256u + vec3<u32>(q);
        o = vec3<f32>(P[MIX + i.r], P[MIX + i.g], P[MIX + i.b]);
    }
    textureStore(dst, vec2<i32>(g.xy), vec4<f32>(o / 255.0, 1.0));
}
