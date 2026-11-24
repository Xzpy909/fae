// Test Statistics (kit test filter, hidden): the statistics template. An auto-levels stretch: black
// and white at the Clip percentiles of the luma (or of each channel), Strength mixes it in.
fn stats() {
    let q = par(P_CLIP) / 100.0;
    for (var c = 0u; c < 4u; c++) {
        let lo = pct(c, q);
        let hi = max(pct(c, 1.0 - q), lo + 1.0);
        put(c * 2u, lo);
        put(c * 2u + 1u, hi);
    }
}

fn fx(c: vec3<f32>, at: vec2<u32>) -> vec3<f32> {
    let s = par(P_STRENGTH) / 100.0;
    var lo = vec3<f32>(stat(6u));
    var hi = vec3<f32>(stat(7u));
    if (par(P_CHANNELS) == 1.0) {          // the select's second option: per channel
        lo = vec3<f32>(stat(0u), stat(2u), stat(4u));
        hi = vec3<f32>(stat(1u), stat(3u), stat(5u));
    }
    let v = (c - lo) * 255.0 / (hi - lo);
    return c + (v - c) * s;
}
