// Test Blur (kit test filter, hidden): the neighbourhood template. Amount > 0: unsharp mask
// (c + a (c - blur)), < 0: soften towards the blur. Radius: the blur's sigma, 0.5 px per step at
// 1440 px height (the descriptor's blur_scale).
fn fx(c: vec3<f32>, at: vec2<u32>) -> vec3<f32> {
    let a = par(P_AMOUNT) / 100.0;
    let b = blur(at);
    if (a >= 0.0) {
        return c + a * (c - b);
    }
    return c + (-a) * (b - c);
}
