// Filter kit template: neighbourhood. Before the apply pass the kit blurs the step's input (per
// channel, a Gaussian with mirrored edges, on a copy shrunk by k x k block means when it is large:
// Sharpen's passes, sharp_blur.wgsl means / across / down) into A; fx reads it with blur(at).
// The blur's size: the descriptor's "blur" field x "blur_scale" = sigma in px at 1440 px height.
@group(0) @binding(3) var<storage, read> A: array<f32>;

fn cell(i: u32) -> vec3<f32> {
    return vec3<f32>(A[i * 3u], A[i * 3u + 1u], A[i * 3u + 2u]);
}

// the grid position of image pixel p: the two cells and the weight of the second
fn kpos(p: u32, k: f32, n2: u32) -> vec3<f32> {
    let f = max(qdiv(f32(p) + 0.5, k) - 0.5, 0.0);
    let i0 = min(u32(f), n2 - 1u);
    let i1 = min(i0 + 1u, n2 - 1u);
    return vec3<f32>(f32(i0), f32(i1), f - f32(i0));
}

// the blurred input at pixel `at` (levels, not rounded)
fn blur(at: vec2<u32>) -> vec3<f32> {
    let k = P[2];
    let w2 = u32(P[6]);
    let h2 = u32(P[7]);
    if (k == 1.0) {
        return cell(at.y * w2 + at.x);
    }
    let px = kpos(at.x, k, w2);
    let py = kpos(at.y, k, h2);
    let r0 = u32(py.x) * w2;
    let r1 = u32(py.y) * w2;
    let a00 = cell(r0 + u32(px.x));
    let a01 = cell(r0 + u32(px.y));
    let a10 = cell(r1 + u32(px.x));
    let a11 = cell(r1 + u32(px.y));
    let top = a00 + (a01 - a00) * px.z;
    let bot = a10 + (a11 - a10) * px.z;
    return top + (bot - top) * py.z;
}

@compute @workgroup_size(16, 16)
fn apply(@builtin(global_invocation_id) g: vec3<u32>) {
    if (!inside(g)) { return; }
    let f = load_levels(g.xy);
    finish(g.xy, f, fx(f, g.xy));
}
