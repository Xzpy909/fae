// Filter kit block: the histograms of a statistics filter's input, R, G, B and the luma rounded to a
// level (exact: the integer luma of common.wgsl), 4 x 256 bins, integer atomics (independent of
// thread order). Each workgroup counts a strided share into workgroup memory first.
@group(0) @binding(0) var img: texture_2d<f32>;
@group(0) @binding(5) var<storage, read_write> H: array<atomic<u32>>;

var<workgroup> loc: array<atomic<u32>, 1024>;

@compute @workgroup_size(256)
fn count(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>,
         @builtin(num_workgroups) nw: vec3<u32>) {
    for (var i = li; i < 1024u; i += 256u) { atomicStore(&loc[i], 0u); }
    workgroupBarrier();
    let s = textureDimensions(img);
    let n = s.x * s.y;
    for (var p = wg.x * 256u + li; p < n; p += nw.x * 256u) {
        let c = textureLoad(img, vec2<u32>(p % s.x, p / s.x), 0).rgb;
        let v = vec3<u32>(clamp(round(c * 255.0), vec3<f32>(0.0), vec3<f32>(255.0)));
        atomicAdd(&loc[v.r], 1u);
        atomicAdd(&loc[256u + v.g], 1u);
        atomicAdd(&loc[512u + v.b], 1u);
        let l = v.r * 2126u + v.g * 7152u + v.b * 722u;
        atomicAdd(&loc[768u + (l + 5000u) / 10000u], 1u);
    }
    workgroupBarrier();
    for (var i = li; i < 1024u; i += 256u) {
        let c = atomicLoad(&loc[i]);
        if (c != 0u) { atomicAdd(&H[i], c); }
    }
}
