// Exact sums for the box filters: non-negative f32 values as 64-bit fixed-point numbers with FB
// fraction bits, held as vec2<u32>(low word, high word). Integer adds can't be reordered or fused
// by a shader compiler, so a sliding sum stays exact however long it runs (a float running sum
// drifts, and compilers are free to optimise float error compensation away). Every f32 >= 2^-17
// converts exactly; smaller ones are truncated (an error below 2^-40).
// Prepended to box_h.wgsl and box_v.wgsl by the host.

const FB: i32 = 40;

fn fx(v: f32) -> vec2<u32> {
    let bits = bitcast<u32>(v);
    let ex = i32((bits >> 23u) & 0xffu);
    if (v <= 0.0 || ex == 0) { return vec2<u32>(0u); }        // 0 and subnormals
    let m = (bits & 0x7fffffu) | 0x800000u;                   // 24-bit mantissa
    let sh = ex - 127 - 23 + FB;                              // value = m * 2^(sh - FB)
    if (sh >= 32) { return vec2<u32>(0u, m << u32(sh - 32)); }
    if (sh > 8) { return vec2<u32>(m << u32(sh), m >> u32(32 - sh)); }
    if (sh >= 0) { return vec2<u32>(m << u32(sh), 0u); }
    if (sh > -24) { return vec2<u32>(m >> u32(-sh), 0u); }
    return vec2<u32>(0u);
}

fn add64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let lo = a.x + b.x;
    return vec2<u32>(lo, a.y + b.y + select(0u, 1u, lo < a.x));
}

fn sub64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    return vec2<u32>(a.x - b.x, a.y - b.y - select(0u, 1u, a.x < b.x));
}

fn bitlen(x: u32) -> u32 {
    return 32u - countLeadingZeros(x);
}

// x / 2^FB as the nearest f32 (ties to even); `sticky`: a non-zero remainder below x's last bit
// (for a quotient), which breaks ties upwards
fn to_f32(x: vec2<u32>, sticky: bool) -> f32 {
    let len = select(bitlen(x.x), 32u + bitlen(x.y), x.y != 0u);
    if (len <= 24u) {
        return ldexp(f32(x.x), -FB);           // exact (the sticky part is below 2^-FB: ignored)
    }
    let s = len - 24u;                          // 1..40: bits to drop
    var q: u32;
    var rem: u32;
    var half: u32;
    var low_rest = false;                       // dropped bits below `rem` (when s > 32)
    if (s < 32u) {
        q = (x.y << (32u - s)) | (x.x >> s);
        rem = x.x & ((1u << s) - 1u);
        half = 1u << (s - 1u);
    } else {
        q = x.y >> (s - 32u);
        if (s == 32u) {
            rem = x.x;
        } else {
            rem = x.y & ((1u << (s - 32u)) - 1u);
            low_rest = x.x != 0u;
        }
    }
    if (s == 32u) { half = 0x80000000u; } else if (s > 32u) { half = 1u << (s - 33u); }
    let above = rem > half || (rem == half && (low_rest || sticky));
    let tie = rem == half && !low_rest && !sticky;
    if (above || (tie && (q & 1u) == 1u)) { q += 1u; }
    return ldexp(f32(q), i32(s) - FB);
}
