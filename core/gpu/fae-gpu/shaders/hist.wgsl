// The UI's histograms, counted with integer atomics (exact, independent of thread order):
// bins [0, 768) hin (R, G, B before the first Curves step), [768, 1536) hout, [1536, 1792) hmx
// (max channel), [1792, 2048) hmn (min channel). Each workgroup counts a strided share of the
// pixels into workgroup memory first, then adds its non-zero bins to the global ones.
@group(0) @binding(0) var img: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> hist: array<atomic<u32>>;

var<workgroup> loc_in: array<atomic<u32>, 768>;
var<workgroup> loc_out: array<atomic<u32>, 1280>;

fn lv(at: u32, w: u32) -> vec3<u32> {
    let c = textureLoad(img, vec2<u32>(at % w, at / w), 0).rgb;
    return vec3<u32>(clamp(round(c * 255.0), vec3<f32>(0.0), vec3<f32>(255.0)));
}

@compute @workgroup_size(256)
fn count_in(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>,
            @builtin(num_workgroups) nw: vec3<u32>) {
    for (var i = li; i < 768u; i += 256u) { atomicStore(&loc_in[i], 0u); }
    workgroupBarrier();
    let s = textureDimensions(img);
    let n = s.x * s.y;
    for (var p = wg.x * 256u + li; p < n; p += nw.x * 256u) {
        let v = lv(p, s.x);
        atomicAdd(&loc_in[v.r], 1u);
        atomicAdd(&loc_in[256u + v.g], 1u);
        atomicAdd(&loc_in[512u + v.b], 1u);
    }
    workgroupBarrier();
    for (var i = li; i < 768u; i += 256u) {
        let c = atomicLoad(&loc_in[i]);
        if (c != 0u) { atomicAdd(&hist[i], c); }
    }
}

@compute @workgroup_size(256)
fn count_out(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>,
             @builtin(num_workgroups) nw: vec3<u32>) {
    for (var i = li; i < 1280u; i += 256u) { atomicStore(&loc_out[i], 0u); }
    workgroupBarrier();
    let s = textureDimensions(img);
    let n = s.x * s.y;
    for (var p = wg.x * 256u + li; p < n; p += nw.x * 256u) {
        let v = lv(p, s.x);
        atomicAdd(&loc_out[v.r], 1u);
        atomicAdd(&loc_out[256u + v.g], 1u);
        atomicAdd(&loc_out[512u + v.b], 1u);
        atomicAdd(&loc_out[768u + max(max(v.r, v.g), v.b)], 1u);
        atomicAdd(&loc_out[1024u + min(min(v.r, v.g), v.b)], 1u);
    }
    workgroupBarrier();
    for (var i = li; i < 1280u; i += 256u) {
        let c = atomicLoad(&loc_out[i]);
        if (c != 0u) { atomicAdd(&hist[768u + i], c); }
    }
}
