// Super Contrast (sc.rs in the CPU core), entry points in run order:
//   hist     channel histograms of the step's input (atomics)
//   reduce   one thread: channel means -> Correct Color Cast tables; percentiles of the cast-corrected
//            histograms -> Correct Contrast line (a, b, k)
//   cellhist Dynamic Contrast: per cell (gh x 12) histograms of R+G+B of "mid" (64 bins)
//   norm / blur (axis) / clip: the cells' clipped, redistributed CDFs ("maps")
//   ct       per cell, the curve at every R+G+B (766 values)
//   apply    mid -> Dynamic Contrast -> Shadows / Highlights table -> opacity
// P: [0] cast, [1] cc, [2] dyn, [3] op, [4] shl on, [5] opacity table on, [6] w, [7] h, [8] gh,
//    [9] dk length, [16..80) dk, [80..336) shl table, [336..336+65536) opacity table (op != 1):
//    rnd(x + (v - x) * op) with v = shl[j] (or j) for the input level x and the level j before the
//    Shadows / Highlights table, the CPU core's f64 values.
// st: [0..768) cast tables (levels), [768] contrast on, [769] a, [770] b, [771] k.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> P: array<f32>;
@group(0) @binding(3) var<storage, read_write> hist: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> st: array<f32>;
@group(0) @binding(5) var<storage, read_write> ch: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read_write> ma: array<f32>;
@group(0) @binding(7) var<storage, read_write> mb: array<f32>;
@group(0) @binding(8) var<storage, read_write> ct: array<f32>;
@group(0) @binding(9) var<storage, read> axis: array<f32>;

const GW: u32 = 12u;
const NB: u32 = 64u;
const SHL: u32 = 80u;
const MIX: u32 = 336u;
const CC_ALO = array<f32, 11>(0.0, 0.2727, 0.4497, 0.5386, 0.6571, 0.7771, 1.0, 1.0, 1.0, 1.0, 1.0);
const CC_BLO = array<f32, 11>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.2556, 0.5307, 0.7731, 0.8910, 1.0);
const CC_AHI = array<f32, 11>(0.0, 0.2118, 0.4149, 0.5963, 0.7711, 0.9293, 1.0, 1.0, 1.0, 1.0, 1.0);
const CC_BHI = array<f32, 11>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1951, 0.3982, 0.6014, 0.8045, 1.0);
const CC_K = array<f32, 11>(1.0, 0.9890, 0.9783, 0.9680, 0.9481, 0.9379, 0.9277, 0.9177, 0.8979, 0.8876, 0.8778);

fn levels(at: vec2<u32>) -> vec3<f32> {
    return clamp(round(textureLoad(src, at, 0).rgb * 255.0), vec3<f32>(0.0), vec3<f32>(255.0));
}

fn rndc(x: f32) -> f32 {
    return floor(clamp(x + 0.5, 0.0, 255.0));
}

fn gh() -> u32 {
    return u32(P[8]);
}

var<workgroup> loc: array<atomic<u32>, 768>;

// ---- 1. channel histograms ----------------------------------------------------------------
@compute @workgroup_size(256)
fn hist_main(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>,
             @builtin(num_workgroups) nw: vec3<u32>) {
    for (var i = li; i < 768u; i += 256u) { atomicStore(&loc[i], 0u); }
    workgroupBarrier();
    let w = u32(P[6]);
    let n = w * u32(P[7]);
    for (var p = wg.x * 256u + li; p < n; p += nw.x * 256u) {
        let v = vec3<u32>(levels(vec2<u32>(p % w, p / w)));
        atomicAdd(&loc[v.r], 1u);
        atomicAdd(&loc[256u + v.g], 1u);
        atomicAdd(&loc[512u + v.b], 1u);
    }
    workgroupBarrier();
    for (var i = li; i < 768u; i += 256u) {
        let c = atomicLoad(&loc[i]);
        if (c != 0u) { atomicAdd(&hist[i], c); }
    }
}

// ---- 2. the global corrections (one thread) -------------------------------------------------
fn cc_tab(t: array<f32, 11>, s: f32) -> f32 {
    var tt = t;
    let i = u32(clamp(floor(s / 10.0), 0.0, 9.0));
    let f = s / 10.0 - f32(i);
    return tt[i] + (tt[i + 1u] - tt[i]) * f;
}

var<private> hs: array<u32, 768>;

// percentile q of channel c's 256 bins (n samples); positions in integers: exact
fn pct(c: u32, n: u32, q: u32) -> f32 {
    let a = n - 1u;
    let i0 = (a / 100u) * q + ((a % 100u) * q) / 100u;
    let fr = f32(((a % 100u) * q) % 100u) / 100.0;
    var v0 = 256u;
    var v1 = 256u;
    var cum = 0u;
    for (var i = 0u; i < 256u; i++) {
        cum += hs[c * 256u + i];
        if (v0 == 256u && cum > i0) { v0 = i; }
        if (v1 == 256u && cum > i0 + 1u) { v1 = i; break; }
    }
    if (i0 >= a) { v1 = v0; }
    return f32(v0) + (f32(v1) - f32(v0)) * fr;
}

@compute @workgroup_size(1)
fn reduce() {
    let n = u32(P[6]) * u32(P[7]);
    // means: sum v * count in 64 bits (two u32)
    var means: vec3<f32>;
    for (var c = 0u; c < 3u; c++) {
        var lo = 0u;
        var hi = 0u;
        for (var v = 0u; v < 256u; v++) {
            let h = atomicLoad(&hist[c * 256u + v]);
            let a = (h & 0xffffu) * v;            // < 2^24
            let b = (h >> 16u) * v;               // < 2^24, times 2^16
            let add_lo = (b & 0xffffu) << 16u;
            var t = lo + a;
            if (t < lo) { hi += 1u; }
            lo = t;
            t = lo + add_lo;
            if (t < lo) { hi += 1u; }
            lo = t;
            hi += b >> 16u;
        }
        means[c] = (f32(hi) * 4294967296.0 + f32(lo)) / f32(n);
    }
    let ccast = P[0];
    let g = (means.r + means.g + means.b) / 3.0;
    for (var c = 0u; c < 3u; c++) {
        let k = 4.0 * ccast * (g - means[c]);
        for (var i = 0u; i < 256u; i++) {
            let x = f32(i);
            let t = x / 255.0;
            st[c * 256u + i] = select(x, rndc(x + k * t * (1.0 - t)), ccast > 0.0);
        }
    }
    let cc = P[1];
    st[768] = 0.0;
    if (cc > 0.0) {
        for (var i = 0u; i < 768u; i++) { hs[i] = 0u; }
        for (var c = 0u; c < 3u; c++) {
            for (var v = 0u; v < 256u; v++) {
                let to = u32(st[c * 256u + v]);
                hs[c * 256u + to] += atomicLoad(&hist[c * 256u + v]);
            }
        }
        let p2 = vec3<f32>(pct(0u, n, 2u), pct(1u, n, 2u), pct(2u, n, 2u));
        let p98 = vec3<f32>(pct(0u, n, 98u), pct(1u, n, 98u), pct(2u, n, 98u));
        let p10 = vec3<f32>(pct(0u, n, 10u), pct(1u, n, 10u), pct(2u, n, 10u));
        let p90 = vec3<f32>(pct(0u, n, 90u), pct(1u, n, 90u), pct(2u, n, 90u));
        let a_lo = min(min(p2.r, p2.g), p2.b);
        let a_hi = max(max(p98.r, p98.g), p98.b);
        let b_lo = min(min(p10.r, p10.g), p10.b) + 1.0;
        let b_hi = max(max(p90.r, p90.g), p90.b) - 1.0;
        let lo = a_lo * cc_tab(CC_ALO, cc) + (b_lo - a_lo) * cc_tab(CC_BLO, cc);
        var hi = 255.0 - (255.0 - a_hi) * cc_tab(CC_AHI, cc) + (b_hi - a_hi) * cc_tab(CC_BHI, cc);
        if (hi - lo < 1.0) { hi = lo + 1.0; }
        let a = 255.0 / (hi - lo);
        st[768] = 1.0;
        st[769] = a;
        st[770] = -lo * a;
        st[771] = cc_tab(CC_K, cc) * a;
    }
}

// Color Cast + Correct Contrast for one pixel
fn mid_px(p: vec3<f32>) -> vec3<f32> {
    let pi = vec3<u32>(p);
    let m = vec3<f32>(st[pi.r], st[256u + pi.g], st[512u + pi.b]);
    if (st[768] == 0.0) { return m; }
    let mm = (m.r + m.g + m.b) / 3.0;
    return vec3<f32>(rndc(st[769] * mm + st[770] + st[771] * (m.r - mm)),
                     rndc(st[769] * mm + st[770] + st[771] * (m.g - mm)),
                     rndc(st[769] * mm + st[770] + st[771] * (m.b - mm)));
}

// ---- 3. Dynamic Contrast ----------------------------------------------------------------
@compute @workgroup_size(16, 16)
fn cellhist(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = u32(P[6]);
    let h = u32(P[7]);
    if (g.x >= w || g.y >= h) { return; }
    let m = mid_px(levels(g.xy));
    let s = u32(m.r + m.g + m.b);
    let cell = (g.y * gh() / h) * GW + g.x * GW / w;
    atomicAdd(&ch[cell * NB + min(s / 12u, NB - 1u)], 1u);
    atomicAdd(&ch[gh() * GW * NB + cell], 1u);
}

@compute @workgroup_size(64)
fn norm(@builtin(global_invocation_id) g: vec3<u32>) {
    let cells = gh() * GW;
    if (g.x >= cells * NB) { return; }
    let cnt = atomicLoad(&ch[cells * NB + g.x / NB]);
    ma[g.x] = f32(atomicLoad(&ch[g.x])) / select(1.0, f32(cnt), cnt > 0u);
}

// mirror that repeats the edge sample
fn dc_reflect(i: i32, n: i32) -> i32 {
    if (n == 1) { return 0; }
    let p = 2 * n;
    let a = ((i % p) + p) % p;
    return select(a, p - 1 - a, a >= n);
}

// ma -> mb along axis[0] (0 = rows, 1 = columns)
@compute @workgroup_size(64)
fn blur(@builtin(global_invocation_id) g: vec3<u32>) {
    let rows = i32(gh());
    let cols = i32(GW);
    if (g.x >= u32(rows * cols) * NB) { return; }
    let b = g.x % NB;
    let cell = i32(g.x / NB);
    let y = cell / cols;
    let x = cell % cols;
    let kl = i32(P[9]);
    let r = (kl - 1) / 2;
    var o = 0.0;
    for (var t = -r; t <= r; t++) {
        var sy = y;
        var sx = x;
        if (axis[0] == 0.0) { sy = dc_reflect(y + t, rows); } else { sx = dc_reflect(x + t, cols); }
        o += P[16 + u32(t + r)] * ma[u32(sy * cols + sx) * NB + b];
    }
    mb[g.x] = o;
}

// clip at 4 / 64 per bin, spread the excess evenly, cumulate: ma -> mb (one thread per cell)
@compute @workgroup_size(64)
fn clip(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= gh() * GW) { return; }
    let lim = 4.0 / f32(NB);
    let o = g.x * NB;
    var ex = 0.0;
    for (var b = 0u; b < NB; b++) {
        let x = ma[o + b];
        if (x > lim) { ex += x - lim; }
    }
    var cum = 0.0;
    for (var b = 0u; b < NB; b++) {
        let wv = min(ma[o + b], lim) + ex / f32(NB);
        cum += wv;
        mb[o + b] = 255.0 * (cum - wv / 2.0);
    }
}

// the cell's curve at every channel sum S (0..765): mb -> ct
@compute @workgroup_size(64)
fn curve(@builtin(global_invocation_id) g: vec3<u32>) {
    let cells = gh() * GW;
    if (g.x >= cells * 766u) { return; }
    let cell = g.x / 766u;
    let s = g.x % 766u;
    let ms = f32(s) / 3.0;
    let fb = clamp((ms + 0.5) * f32(NB) / 256.0 - 0.5, 0.0, f32(NB - 1u));
    let b0 = min(u32(floor(fb)), NB - 2u);
    let wb = fb - f32(b0);
    ct[g.x] = mb[cell * NB + b0] * (1.0 - wb) + mb[cell * NB + b0 + 1u] * wb;
}

fn dc_taps(i: u32, n: u32, cells: u32) -> vec3<f32> {
    let f = clamp((f32(i) + 0.5) / f32(n) * f32(cells) - 0.5, 0.0, f32(cells - 1u));
    let top = select(0u, cells - 2u, cells > 1u);
    let i0 = min(u32(floor(f)), top);
    return vec3<f32>(f32(i0), f32(min(i0 + 1u, cells - 1u)), f - f32(i0));
}

// ---- 4. the final pass -------------------------------------------------------------------
@compute @workgroup_size(16, 16)
fn apply(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = u32(P[6]);
    let h = u32(P[7]);
    if (g.x >= w || g.y >= h) { return; }
    let p = levels(g.xy);
    let m = mid_px(p);
    var res = m;
    let dyn = P[2];
    if (dyn > 0.0) {
        let ty = dc_taps(g.y, h, gh());
        let tx = dc_taps(g.x, w, GW);
        let s = u32(m.r + m.g + m.b);
        let y0 = u32(ty.x);
        let y1 = u32(ty.y);
        let x0 = u32(tx.x);
        let x1 = u32(tx.y);
        let v = (1.0 - ty.z) * (1.0 - tx.z) * ct[(y0 * GW + x0) * 766u + s]
              + (1.0 - ty.z) * tx.z * ct[(y0 * GW + x1) * 766u + s]
              + ty.z * (1.0 - tx.z) * ct[(y1 * GW + x0) * 766u + s]
              + ty.z * tx.z * ct[(y1 * GW + x1) * 766u + s];
        let mm = f32(s) / 3.0;
        let m2 = mm + dyn * 0.4 * (v - mm);
        let gs = 1.0 + dyn * (1.2 - 1.0);
        res = vec3<f32>(rndc(m2 + gs * (m.r - mm)), rndc(m2 + gs * (m.g - mm)), rndc(m2 + gs * (m.b - mm)));
    }
    // res is whole here (j): the Shadows / Highlights table and the opacity come from tables
    if (P[5] != 0.0) {
        let i = vec3<u32>(p) * 256u + vec3<u32>(res);
        res = vec3<f32>(P[MIX + i.r], P[MIX + i.g], P[MIX + i.b]);
    } else if (P[4] != 0.0) {
        res = floor(clamp(vec3<f32>(P[SHL + u32(res.r)], P[SHL + u32(res.g)], P[SHL + u32(res.b)]),
                          vec3<f32>(0.0), vec3<f32>(255.0)));      // `as u8`
    }
    textureStore(dst, vec2<i32>(g.xy), vec4<f32>(res / 255.0, 1.0));
}
