//! Whole-image work of the steps that need more than the pixel itself: the passes that run before
//! a step's per-pixel pass (G3). Mid/Detail: the guided filter of the luma, as the CPU core's md.rs
//! (box means with mirrored edges, radius 26 px at 1440 px height, eps 0.01).

use crate::{bind_group, bind_group_at, Gpu, Pass, Pipe};

pub const PS_WGSL: &str = include_str!("../shaders/ps.wgsl");
pub const PS_GH: u32 = 45;
pub const SC_WGSL: &str = include_str!("../shaders/sc.wgsl");
/// Dynamic Contrast's cells across
pub const DC_GW: u32 = 12;

/// Dynamic Contrast's cells down a w x h image
pub fn dc_rows(w: u32, h: u32) -> u32 {
    (((h * DC_GW) as f64 / w as f64 + 0.5).floor() as u32).max(1)
}

/// Perceptual Saturation's grid (rows, columns) for a w x h image (ps.rs grid_size).
pub fn ps_grid(w: u32, h: u32) -> (u32, u32) {
    let gw = ((PS_GH as f64 * w as f64) / h as f64 + 0.5).floor() as u32;
    (PS_GH, gw.max(1))
}

pub const MD_PREP_WGSL: &str = include_str!("../shaders/md_prep.wgsl");
pub const BOX_H_WGSL: &str = include_str!("../shaders/box_h.wgsl");
pub const BOX_V_WGSL: &str = include_str!("../shaders/box_v.wgsl");
/// exact fixed-point sums, prepended to the box filters
pub const FIXED_WGSL: &str = include_str!("../shaders/fixed.wgsl");

pub const MD_GAIN: f64 = 0.952;
pub const MD_K: f64 = 0.706;
pub const MD_GAMMA: f64 = 0.662;
pub const MD_EPS: f32 = 0.01;
pub const MD_RADIUS_1440: f64 = 26.0;

/// Window radius for an image `height` pixels high (26 px at 1440p), rounded half to even.
pub fn md_radius(height: u32) -> u32 {
    ((MD_RADIUS_1440 * height as f64 / 1440.0).round_ties_even() as u32).max(1)
}

fn b(x: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
    x.as_entire_binding()
}

impl Gpu {
    fn storage(&self, label: &str, bytes: u64) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.max(16),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Perceptual Saturation, the whole step (image `inp` -> `out`): measure (cell sums), mean,
    /// blur down the rows, blur along the columns, apply. Returns passes, kept buffers, buffers to
    /// zero before each run.
    pub(crate) fn ps_passes(&mut self, inp: usize, out: usize, params: &wgpu::Buffer)
                            -> (Vec<Pass>, Vec<wgpu::Buffer>, Vec<wgpu::Buffer>) {
        let s = self.stage.as_ref().unwrap();
        let (w, h) = (s.w, s.h);
        let (gh, gw) = ps_grid(w, h);
        let cells = (gh * gw) as u64;
        let sums = self.storage("ps sums", cells * 16);
        let grid = self.storage("ps grid", cells * 12);
        let grid2 = self.storage("ps grid2", cells * 12);
        let ax0 = self.small_storage(&[0.0]);
        let ax1 = self.small_storage(&[1.0]);
        let d = &self.device;
        let px = |p: Pipe| &self.pipes[p as usize];
        let tv = |i: usize| wgpu::BindingResource::TextureView(&s.views[i]);
        let g16 = (w.div_ceil(16), h.div_ceil(16), 1);
        let gc = ((cells as u32).div_ceil(64), 1, 1);
        let passes = vec![
            Pass { pipe: Pipe::PsMeasure, groups: g16,
                   bg: bind_group_at(d, px(Pipe::PsMeasure), &[(0, tv(inp)), (2, params.as_entire_binding()), (3, sums.as_entire_binding())]) },
            Pass { pipe: Pipe::PsMean, groups: gc,
                   bg: bind_group_at(d, px(Pipe::PsMean), &[(2, params.as_entire_binding()), (3, sums.as_entire_binding()), (4, grid.as_entire_binding())]) },
            Pass { pipe: Pipe::PsBlur, groups: gc,
                   bg: bind_group_at(d, px(Pipe::PsBlur), &[(2, params.as_entire_binding()), (4, grid.as_entire_binding()), (5, grid2.as_entire_binding()), (6, ax0.as_entire_binding())]) },
            Pass { pipe: Pipe::PsBlur, groups: gc,
                   bg: bind_group_at(d, px(Pipe::PsBlur), &[(2, params.as_entire_binding()), (4, grid2.as_entire_binding()), (5, grid.as_entire_binding()), (6, ax1.as_entire_binding())]) },
            Pass { pipe: Pipe::PsApply, groups: g16,
                   bg: bind_group_at(d, px(Pipe::PsApply), &[(0, tv(inp)), (1, tv(out)), (2, params.as_entire_binding()), (4, grid.as_entire_binding())]) },
        ];
        (passes, vec![grid, grid2, ax0, ax1], vec![sums])
    }

    /// Super Contrast, the whole step (`inp` -> `out`). See sc.wgsl for the passes.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn sc_passes(&mut self, inp: usize, out: usize, params: &wgpu::Buffer, dyn_on: bool)
                            -> (Vec<Pass>, Vec<wgpu::Buffer>, Vec<wgpu::Buffer>) {
        let s = self.stage.as_ref().unwrap();
        let (w, h) = (s.w, s.h);
        let cells = (dc_rows(w, h) * DC_GW) as u64;
        let hist = self.storage("sc hist", 768 * 4);
        let st = self.storage("sc stats", 772 * 4);
        let ch = self.storage("sc cell hist", (cells * 64 + cells) * 4);
        let ma = self.storage("sc maps a", cells * 64 * 4);
        let mb = self.storage("sc maps b", cells * 64 * 4);
        let ct = self.storage("sc curves", cells * 766 * 4);
        let ax0 = self.small_storage(&[0.0]);
        let ax1 = self.small_storage(&[1.0]);
        let d = &self.device;
        let px = |p: Pipe| &self.pipes[p as usize];
        let tv = |i: usize| wgpu::BindingResource::TextureView(&s.views[i]);
        let g16 = (w.div_ceil(16), h.div_ceil(16), 1);
        let n64 = |n: u64| ((n as u32).div_ceil(64), 1, 1);
        let pr = || b(params);
        let mut passes = vec![
            Pass { pipe: Pipe::ScHist, groups: (s.hist_groups, 1, 1),
                   bg: bind_group_at(d, px(Pipe::ScHist), &[(0, tv(inp)), (2, pr()), (3, b(&hist))]) },
            Pass { pipe: Pipe::ScReduce, groups: (1, 1, 1),
                   bg: bind_group_at(d, px(Pipe::ScReduce), &[(2, pr()), (3, b(&hist)), (4, b(&st))]) },
        ];
        if dyn_on {
            passes.extend([
                Pass { pipe: Pipe::ScCellHist, groups: g16,
                       bg: bind_group_at(d, px(Pipe::ScCellHist), &[(0, tv(inp)), (2, pr()), (4, b(&st)), (5, b(&ch))]) },
                Pass { pipe: Pipe::ScNorm, groups: n64(cells * 64),
                       bg: bind_group_at(d, px(Pipe::ScNorm), &[(2, pr()), (5, b(&ch)), (6, b(&ma))]) },
                Pass { pipe: Pipe::ScBlur, groups: n64(cells * 64),
                       bg: bind_group_at(d, px(Pipe::ScBlur), &[(2, pr()), (6, b(&ma)), (7, b(&mb)), (9, b(&ax0))]) },
                Pass { pipe: Pipe::ScBlur, groups: n64(cells * 64),
                       bg: bind_group_at(d, px(Pipe::ScBlur), &[(2, pr()), (6, b(&mb)), (7, b(&ma)), (9, b(&ax1))]) },
                Pass { pipe: Pipe::ScClip, groups: n64(cells),
                       bg: bind_group_at(d, px(Pipe::ScClip), &[(2, pr()), (6, b(&ma)), (7, b(&mb))]) },
                Pass { pipe: Pipe::ScCurve, groups: n64(cells * 766),
                       bg: bind_group_at(d, px(Pipe::ScCurve), &[(2, pr()), (7, b(&mb)), (8, b(&ct))]) },
            ]);
        }
        passes.push(Pass { pipe: Pipe::ScApply, groups: g16,
                           bg: bind_group_at(d, px(Pipe::ScApply), &[(0, tv(inp)), (1, tv(out)), (2, pr()), (4, b(&st)), (8, b(&ct))]) });
        (passes, vec![st, ma, mb, ct, ax0, ax1], vec![hist, ch])
    }



    fn small_storage(&self, v: &[f32]) -> wgpu::Buffer {
        let b = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pass params"),
            size: (v.len() * 4).max(16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.queue.write_buffer(&b, 0, &bytes);
        b
    }

    /// The stage's two scratch planes (vec2<f32> per pixel), made on first use.
    fn scratch(&mut self) -> [wgpu::Buffer; 2] {
        let d = self.device.clone();
        let s = self.stage.as_mut().unwrap();
        if s.scratch.is_none() {
            let size = s.w as u64 * s.h as u64 * 8;
            let mk = || {
                d.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("scratch"),
                    size,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                })
            };
            s.scratch = Some([mk(), mk()]);
        }
        s.scratch.clone().unwrap()
    }

    /// Mid/Detail's guided filter of image `inp`: prep (y, y^2) -> box -> (a, b) -> box -> (mean a,
    /// mean b) in scratch plane 0, which the Adjustments pass reads. Returns the passes, the small
    /// parameter buffers to keep, and the binding of the result.
    pub(crate) fn md_passes(&mut self, inp: usize) -> (Vec<Pass>, Vec<wgpu::Buffer>, wgpu::Buffer) {
        let [b0, b1] = self.scratch();
        let s = self.stage.as_ref().unwrap();
        let (w, h) = (s.w, s.h);
        let r = md_radius(h) as f32;
        let ph = self.small_storage(&[w as f32, h as f32, r]);
        let pv0 = self.small_storage(&[w as f32, h as f32, r, 0.0, MD_EPS]);
        let pv1 = self.small_storage(&[w as f32, h as f32, r, 1.0, MD_EPS]);
        let d = &self.device;
        let px = |p: Pipe| &self.pipes[p as usize];
        let g16 = (w.div_ceil(16), h.div_ceil(16), 1);
        // box filters: one thread per segment of 64 pixels (shaders' SEG), 64 threads per group
        let grow = (w.div_ceil(64).div_ceil(64), h, 1);
        let gcol = (w.div_ceil(64), h.div_ceil(64), 1);
        let passes = vec![
            Pass { pipe: Pipe::MdPrep, bg: bind_group(d, px(Pipe::MdPrep), &[wgpu::BindingResource::TextureView(&s.views[inp]), b0.as_entire_binding()]), groups: g16 },
            Pass { pipe: Pipe::BoxH, bg: bind_group(d, px(Pipe::BoxH), &[b0.as_entire_binding(), b1.as_entire_binding(), ph.as_entire_binding()]), groups: grow },
            Pass { pipe: Pipe::BoxV, bg: bind_group(d, px(Pipe::BoxV), &[b1.as_entire_binding(), b0.as_entire_binding(), pv0.as_entire_binding()]), groups: gcol },
            Pass { pipe: Pipe::BoxH, bg: bind_group(d, px(Pipe::BoxH), &[b0.as_entire_binding(), b1.as_entire_binding(), ph.as_entire_binding()]), groups: grow },
            Pass { pipe: Pipe::BoxV, bg: bind_group(d, px(Pipe::BoxV), &[b1.as_entire_binding(), b0.as_entire_binding(), pv1.as_entire_binding()]), groups: gcol },
        ];
        (passes, vec![ph, pv0, pv1], b0)
    }
}
