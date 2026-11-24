// Filter kit template: statistics. Before the apply pass the kit counts the step's input into H
// (hist.wgsl of the kit: 4 x 256 bins, R, G, B and the luma rounded to a level; exact integer
// atomics), then runs the filter's `fn stats()` once (one thread) to turn them into up to 64 numbers
// with put(i, v); fx reads them with stat(i).
@group(0) @binding(5) var<storage, read> H: array<u32>;
@group(0) @binding(6) var<storage, read_write> R: array<f32>;

const CH_R: u32 = 0u;
const CH_G: u32 = 1u;
const CH_B: u32 = 2u;
const CH_LUMA: u32 = 3u;

// stats(): pixels of level v in channel c (CH_R, CH_G, CH_B, CH_LUMA)
fn hist(c: u32, v: u32) -> u32 {
    return H[c * 256u + v];
}

// stats(): pixels in the image
fn pixels() -> u32 {
    return u32(P[8]) * u32(P[9]);
}

// stats(): the lowest level with at least q (0..1) of the pixels at or below it in channel c
fn pct(c: u32, q: f32) -> f32 {
    let need = q * f32(pixels());
    var acc = 0u;
    for (var v = 0u; v < 256u; v++) {
        acc += hist(c, v);
        if (f32(acc) >= need) { return f32(v); }
    }
    return 255.0;
}

// stats(): store result i (0..63)
fn put(i: u32, v: f32) {
    R[i] = v;
}

// fx(): result i of stats()
fn stat(i: u32) -> f32 {
    return R[i];
}

@compute @workgroup_size(1)
fn reduce() {
    stats();
}

@compute @workgroup_size(16, 16)
fn apply(@builtin(global_invocation_id) g: vec3<u32>) {
    if (!inside(g)) { return; }
    let f = load_levels(g.xy);
    finish(g.xy, f, fx(f, g.xy));
}
