// Test Pixel (kit test filter, hidden): every per-pixel feature the kit offers. Lift raises the
// shadows towards white, Ramp darkens (or brightens) towards the bottom of the image, Tint moves
// red against blue around the luma.
fn fx(c: vec3<f32>, at: vec2<u32>) -> vec3<f32> {
    let lift = par(P_LIFT) / 100.0;
    let ramp = par(P_RAMP) / 100.0;
    let tint = par(P_TINT) / 100.0;
    var v = c + (255.0 - c) * (lift * 0.5);
    let t = (f32(at.y) + 0.5) / f32(size().y);
    v = v * (1.0 - ramp * 0.5 * t);
    let y = luma(c);
    v = v + vec3<f32>(1.0, 0.0, -1.0) * (tint * 0.25 * min(y, 255.0 - y));
    return v;
}
