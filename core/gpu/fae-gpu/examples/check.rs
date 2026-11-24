#![allow(non_snake_case)]
//! Native self-check of the GPU core (runs on any adapter, e.g. lavapipe in a container):
//!   cargo run --release --example check
//! The pass graph (Curves x 2, RGB and RGBA upload, re-upload, step changes) against a CPU loop,
//! and the screen output against a CPU Lanczos-3 reference at several scales / offsets / splits.
use fae_gpu::{view::ViewParams, wgpu, Gpu, Options, Step};

fn rng(seed: &mut u32) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed
}

fn image(w: usize, h: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    let mut a = vec![0u8; w * h * 3];
    for y in 0..h {
        for i in 0..w {
            let j = (y * w + i) * 3;
            let n = rng(&mut x);
            a[j] = ((i * 230 / w) as u32 + (n & 15)) as u8;
            a[j + 1] = ((y * 230 / h) as u32 + ((n >> 8) & 15)) as u8;
            a[j + 2] = if (i / 7 + y / 5) % 2 == 0 { 30 } else { 220 };
        }
    }
    a
}

fn luts(seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    (0..768).map(|i| if i % 5 == 0 { 255 - (i % 256) as u8 } else { rng(&mut x) as u8 }).collect()
}

fn lanczos3(x: f64) -> f64 {
    let a = x.abs();
    if a < 1e-5 {
        return 1.0;
    }
    if a >= 3.0 {
        return 0.0;
    }
    let p = std::f64::consts::PI * x;
    3.0 * p.sin() * (p / 3.0).sin() / (p * p)
}

/// CPU reference of view_h + view_v (f64), RGBA8 out (alpha 0 outside the image).
fn ref_view(res: &[u8], orig: &[u8], w: usize, h: usize, p: &ViewParams) -> Vec<u8> {
    let s = p.s as f64;
    let nearest = p.s >= p.nearest_from;
    let ks = s.min(1.0);
    let reach = 3.0 / ks;
    let mut out = vec![0u8; (p.cw * p.ch * 4) as usize];
    let at = |x: i64, y: i64, from_orig: bool, c: usize| -> f64 {
        let x = x.clamp(0, w as i64 - 1) as usize;
        let y = y.clamp(0, h as i64 - 1) as usize;
        let src = if from_orig { orig } else { res };
        src[(y * w + x) * 3 + c] as f64 / 255.0
    };
    for Y in 0..p.ch as i64 {
        for X in 0..p.cw as i64 {
            let fx = (X as f64 + 0.5 - p.ox as f64) / s;
            let fy = (Y as f64 + 0.5 - p.oy as f64) / s;
            if fx < 0.0 || fy < 0.0 || fx >= w as f64 || fy >= h as f64 {
                continue;
            }
            let from_orig = fx < p.split as f64;
            let o = ((Y * p.cw as i64 + X) * 4) as usize;
            for c in 0..3 {
                let v = if nearest {
                    at(fx.floor() as i64, fy.floor() as i64, from_orig, c)
                } else {
                    let (u, vv) = (fx - 0.5, fy - 0.5);
                    let (xl, xh) = ((u - reach).floor() as i64 + 1, (u + reach).floor() as i64);
                    let (yl, yh) = ((vv - reach).floor() as i64 + 1, (vv + reach).floor() as i64);
                    let (mut acc, mut ws) = (0.0, 0.0);
                    for y in yl..=yh {
                        let wy = lanczos3((y as f64 - vv) * ks);
                        let (mut ra, mut rw) = (0.0, 0.0);
                        for x in xl..=xh {
                            let wx = lanczos3((x as f64 - u) * ks);
                            ra += wx * at(x, y, from_orig, c);
                            rw += wx;
                        }
                        acc += wy * ra / rw;
                        ws += wy;
                    }
                    acc / ws
                };
                out[o + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}

fn draw(g: &mut Gpu, p: &ViewParams) -> Vec<u8> {
    let f = wgpu::TextureFormat::Rgba8Unorm;
    let t = g.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: p.cw, height: p.ch, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: f,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    g.draw_view(&t.create_view(&Default::default()), p).unwrap();
    let bpr = (p.cw * 4).div_ceil(256) * 256;
    let buf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (bpr * p.ch) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut e = g.device.create_command_encoder(&Default::default());
    e.copy_texture_to_buffer(
        t.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: None },
        },
        t.size(),
    );
    g.queue.submit([e.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    g.wait().unwrap();
    let v = buf.slice(..).get_mapped_range().unwrap();
    let mut out = vec![0u8; (p.cw * p.ch * 4) as usize];
    for y in 0..p.ch as usize {
        let r = &v[y * bpr as usize..][..p.cw as usize * 4];
        out[y * p.cw as usize * 4..][..p.cw as usize * 4].copy_from_slice(r);
    }
    out
}

/// fixed.wgsl (the box filters' exact sums): f32 -> fixed -> f32 gives the value back, and the
/// sum of two values comes back as the correctly rounded f32 of their exact sum.
fn fixed_point(g: &Gpu) -> usize {
    use wgpu::util::DeviceExt;
    let mut seed = 11u32;
    let mut v: Vec<f32> = Vec::new();
    for _ in 0..20000 {
        let e = (rng(&mut seed) % 30) as i32 - 17; // 2^-17 .. 2^12
        let m = 1.0 + (rng(&mut seed) % (1 << 23)) as f32 / (1 << 23) as f32;
        v.push(m * 2f32.powi(e));
    }
    // ties: 1 + 2^-24 (to even: 1), 1 + 3 * 2^-24 (to even: 1 + 2^-22), powers of two, whole numbers
    v.extend([1.0, 2f32.powi(-24), 1.0, 3.0 * 2f32.powi(-24), 0.5, 0.25, 255.0, 1.0, 2048.0, 2f32.powi(-17)]);
    let n = v.len() / 2;
    let src = format!("{}\n{}", fae_gpu::ops::FIXED_WGSL, r#"
@group(0) @binding(0) var<storage, read> inp: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read_write> out: array<vec4<f32>>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    if (g.x >= arrayLength(&inp)) { return; }
    let p = inp[g.x];
    let s = add64(fx(p.x), fx(p.y));
    out[g.x] = vec4<f32>(to_f32(fx(p.x), false), to_f32(fx(p.y), false), to_f32(s, false),
                         to_f32(sub64(s, fx(p.y)), false));
}"#);
    let d = &g.device;
    let m = d.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(src.into()) });
    let pl = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None, layout: None, module: &m, entry_point: Some("main"), compilation_options: Default::default(), cache: None,
    });
    let bytes: Vec<u8> = v[..2 * n].iter().flat_map(|x| x.to_le_bytes()).collect();
    let inp = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &bytes, usage: wgpu::BufferUsages::STORAGE });
    let size = (n * 16) as u64;
    let out = d.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let rd = d.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pl.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: inp.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: out.as_entire_binding() }],
    });
    let mut e = d.create_command_encoder(&Default::default());
    {
        let mut p = e.begin_compute_pass(&Default::default());
        p.set_pipeline(&pl);
        p.set_bind_group(0, &bg, &[]);
        p.dispatch_workgroups((n as u32).div_ceil(64), 1, 1);
    }
    e.copy_buffer_to_buffer(&out, 0, &rd, 0, size);
    g.queue.submit([e.finish()]);
    rd.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    g.wait().unwrap();
    let r = rd.slice(..).get_mapped_range().unwrap();
    let got: Vec<f32> = r.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let mut bad = 0;
    for i in 0..n {
        let (a, b) = (v[2 * i], v[2 * i + 1]);
        let want = [a, b, (a as f64 + b as f64) as f32, a];
        if got[4 * i..4 * i + 4] != want {
            if bad < 5 {
                println!("     fixed: {a:e} + {b:e}: got {:?}, want {want:?}", &got[4 * i..4 * i + 4]);
            }
            bad += 1;
        }
    }
    println!("{} fixed-point sums: {bad} of {n} differ", if bad == 0 { "OK  " } else { "FAIL" });
    (bad > 0) as usize
}

fn main() {
    let mut fails = 0;
    let mut g = pollster::block_on(Gpu::new(&Options::default())).unwrap();
    println!("{}", g.info.to_json());
    let (w, h) = (203usize, 151usize);
    let img = image(w, h, 7);
    let (l1, l2) = (luts(3), luts(5));
    let cpu = |a: &[u8], l: &[u8]| -> Vec<u8> { a.iter().enumerate().map(|(i, &v)| l[(i % 3) * 256 + v as usize]).collect() };
    let want = cpu(&cpu(&img, &l1), &l2);
    let check = |name: &str, got: &[u8], want: &[u8]| -> usize {
        let bad = got.iter().zip(want).filter(|(a, b)| a != b).count();
        println!("{} {name}: {bad} values differ", if bad == 0 { "OK  " } else { "FAIL" });
        (bad > 0) as usize
    };

    // graph: steps before the upload, RGB
    g.set_steps(&[Step::curves(&l1).unwrap(), Step::curves(&l2).unwrap()]).unwrap();
    g.upload(&img, w as u32, h as u32, 3).unwrap();
    let out = g.readback_start().unwrap().finish_blocking(&g).unwrap();
    fails += check("curves x2, RGB upload", &out, &want);
    // RGBA upload, same size (passes kept)
    let rgba: Vec<u8> = img.chunks(3).flat_map(|p| [p[0], p[1], p[2], 77]).collect();
    g.upload(&rgba, w as u32, h as u32, 4).unwrap();
    let out = g.readback_start().unwrap().finish_blocking(&g).unwrap();
    fails += check("curves x2, RGBA upload", &out, &want);
    // one step only, then none
    g.set_steps(&[Step::curves(&l2).unwrap()]).unwrap();
    let out = g.readback_start().unwrap().finish_blocking(&g).unwrap();
    fails += check("curves x1", &out, &cpu(&img, &l2));
    g.set_steps(&[]).unwrap();
    let out = g.readback_start().unwrap().finish_blocking(&g).unwrap();
    fails += check("no steps = stage", &out, &img);
    // new size: passes rebuilt from the kept steps
    g.set_steps(&[Step::curves(&l1).unwrap()]).unwrap();
    let img2 = image(64, 33, 9);
    g.upload(&img2, 64, 33, 3).unwrap();
    let out = g.readback_start().unwrap().finish_blocking(&g).unwrap();
    fails += check("new size keeps steps", &out, &cpu(&img2, &l1));
    // slider move: same kinds, new values
    g.set_steps(&[Step::curves(&l2).unwrap()]).unwrap();
    let out = g.readback_start().unwrap().finish_blocking(&g).unwrap();
    fails += check("new values", &out, &cpu(&img2, &l2));

    // histograms + RGBA read back: hin = before the (first) Curves, hout / hmx / hmn = final
    g.set_stats(true);
    g.set_steps(&[Step::bw(1.3, 0.8, Some([1.05, 1.0, 0.92])), Step::curves(&l1).unwrap(), Step::curves(&l2).unwrap()]).unwrap();
    g.upload(&img, w as u32, h as u32, 3).unwrap();
    let mid = g.readback_start().unwrap().finish_blocking(&g).unwrap();          // after all steps
    let st = pollster::block_on({ let r = g.stats_start().unwrap(); g.wait().unwrap(); r.finish() }).unwrap();
    let st: Vec<u32> = st.chunks(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    g.set_steps(&[Step::bw(1.3, 0.8, Some([1.05, 1.0, 0.92]))]).unwrap();
    let pre = g.readback_start().unwrap().finish_blocking(&g).unwrap();          // input of the first Curves
    let mut want_h = vec![0u32; fae_gpu::HIST_BINS];
    for (p, q) in pre.chunks(3).zip(mid.chunks(3)) {
        for c in 0..3 {
            want_h[c * 256 + p[c] as usize] += 1;
            want_h[768 + c * 256 + q[c] as usize] += 1;
        }
        want_h[1536 + *q.iter().max().unwrap() as usize] += 1;
        want_h[1792 + *q.iter().min().unwrap() as usize] += 1;
    }
    let hb = st.iter().zip(&want_h).filter(|(a, b)| a != b).count();
    println!("{} histograms: {hb} bins differ", if hb == 0 { "OK  " } else { "FAIL" });
    if hb > 0 {
        fails += 1;
    }
    let rgba = g.readback_rgba_start().unwrap().finish_blocking(&g).unwrap();
    let want_rgba: Vec<u8> = pre.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    let rb = rgba.iter().zip(&want_rgba).filter(|(a, b)| a != b).count();
    println!("{} RGBA read back: {rb} values differ", if rb == 0 { "OK  " } else { "FAIL" });
    if rb > 0 {
        fails += 1;
    }
    g.set_stats(false);

    // screen output
    g.set_steps(&[Step::curves(&l1).unwrap()]).unwrap();
    g.upload(&img, w as u32, h as u32, 3).unwrap();
    g.enable_view(wgpu::TextureFormat::Rgba8Unorm);
    let res = cpu(&img, &l1);
    let cases = [
        ("1:1, whole px offset", ViewParams { cw: 260, ch: 190, ox: 20.0, oy: 13.0, s: 1.0, split: 0.0, nearest_from: 2.0 }),
        ("1:1, split at 80", ViewParams { cw: 260, ch: 190, ox: 20.0, oy: 13.0, s: 1.0, split: 80.0, nearest_from: 2.0 }),
        ("before (all original)", ViewParams { cw: 260, ch: 190, ox: 20.0, oy: 13.0, s: 1.0, split: 1e9, nearest_from: 2.0 }),
        ("0.5x", ViewParams { cw: 140, ch: 100, ox: 10.0, oy: 7.0, s: 0.5, split: 0.0, nearest_from: 2.0 }),
        ("0.237x, fractional offset", ViewParams { cw: 80, ch: 60, ox: 3.3, oy: 5.7, s: 0.237, split: 100.0, nearest_from: 2.0 }),
        ("1.6x (Lanczos up), clipped", ViewParams { cw: 200, ch: 150, ox: -61.4, oy: -20.2, s: 1.6, split: 0.0, nearest_from: 2.0 }),
        ("3x nearest, clipped", ViewParams { cw: 200, ch: 150, ox: -100.0, oy: -50.0, s: 3.0, split: 120.0, nearest_from: 2.0 }),
        ("off screen", ViewParams { cw: 50, ch: 50, ox: 500.0, oy: 0.0, s: 1.0, split: 0.0, nearest_from: 2.0 }),
    ];
    for (name, p) in cases {
        let got = draw(&mut g, &p);
        let want = ref_view(&res, &img, w, h, &p);
        let mut maxd = 0i32;
        let mut over1 = 0usize;
        let mut alpha = 0usize;
        for (a, b) in got.chunks(4).zip(want.chunks(4)) {
            if a[3] != b[3] {
                alpha += 1;
                continue;
            }
            for c in 0..3 {
                let d = (a[c] as i32 - b[c] as i32).abs();
                maxd = maxd.max(d);
                if d > 1 {
                    over1 += 1;
                }
            }
        }
        // rgba16float in between + f32 weights: within 1 level of the f64 reference
        let ok = alpha == 0 && maxd <= 1;
        println!("{} view {name}: max diff {maxd}, {over1} values > 1, {alpha} coverage differences", if ok { "OK  " } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    }
    fails += fixed_point(&g);
    let e = g.take_errors();
    if !e.is_empty() {
        println!("FAIL wgpu errors: {e:?}");
        fails += 1;
    }
    println!("{}", if fails == 0 { "all OK" } else { "FAILURES" });
    std::process::exit(if fails == 0 { 0 } else { 1 });
}
