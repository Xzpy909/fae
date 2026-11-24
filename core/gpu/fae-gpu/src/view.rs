//! Screen output: the chain's result (or the unedited stage, left of the split line / in the
//! before view) resampled to exact screen pixels and drawn into a surface (the preview's canvas).
//!
//! Two passes: a compute pass resamples horizontally into `mid` (screen columns x the image rows
//! the screen needs), a render pass resamples vertically into the target. Lanczos-3, widened by
//! 1/s when shrinking; nearest neighbour from s >= 2 (the pixel view, as the CPU preview did).
//! Pan / zoom only redraw (two cheap passes); the chain doesn't run again.

use crate::{bind_group, compute_pipeline, Gpu, IMG_FORMAT};

pub const VIEW_H_WGSL: &str = include_str!("../shaders/view_h.wgsl");
pub const VIEW_V_WGSL: &str = include_str!("../shaders/view_v.wgsl");

/// Where the image is on the target, in the target's pixels.
#[derive(Clone, Copy, Debug)]
pub struct ViewParams {
    /// target size
    pub cw: u32,
    pub ch: u32,
    /// position of the image's top-left corner
    pub ox: f32,
    pub oy: f32,
    /// target pixels per image pixel
    pub s: f32,
    /// image x (pixels) left of which the unedited stage shows: 0 = none, >= width = all (before)
    pub split: f32,
    /// nearest neighbour from this scale up (the CPU preview: 2)
    pub nearest_from: f32,
}

pub(crate) struct View {
    h: wgpu::ComputePipeline,
    v: wgpu::RenderPipeline,
    uni: wgpu::Buffer,
    mid: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
}

impl Gpu {
    /// Set up the screen output for a target format. Call once (again if the format changes).
    pub fn enable_view(&mut self, format: wgpu::TextureFormat) {
        let d = &self.device;
        let h = compute_pipeline(d, VIEW_H_WGSL, "view_h");
        let m = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("view_v"),
            source: wgpu::ShaderSource::Wgsl(VIEW_V_WGSL.into()),
        });
        let v = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("view_v"),
            layout: None,
            vertex: wgpu::VertexState { module: &m, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &m,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uni = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.view = Some(View { h, v, uni, mid: None });
    }

    /// Draw the current result into `target` (cleared to transparent outside the image). Runs the
    /// chain first if it changed. Submits.
    pub fn draw_view(&mut self, target: &wgpu::TextureView, p: &ViewParams) -> Result<(), String> {
        self.run()?;
        let (iw, ih) = self.size().ok_or("draw: no image")?;
        if self.view.is_none() {
            return Err("draw: enable_view first".into());
        }
        let s = p.s.max(1e-6);
        let nearest = s >= p.nearest_from;
        let ks = s.min(1.0);
        let reach = if nearest { 0.5 } else { 3.0 / ks };
        // the target's pixels the image covers
        let x0 = p.ox.floor().max(0.0) as i64;
        let x1 = ((p.ox + iw as f32 * s).ceil() as i64).min(p.cw as i64);
        let y0 = p.oy.floor().max(0.0) as i64;
        let y1 = ((p.oy + ih as f32 * s).ceil() as i64).min(p.ch as i64);
        let visible = x1 > x0 && y1 > y0;
        // image rows those screen rows need (plus the filter's reach)
        let r0 = if visible { (((y0 as f32 - p.oy) / s - reach - 2.0).floor() as i64).clamp(0, ih as i64 - 1) } else { 0 };
        let r1 = if visible { (((y1 as f32 - p.oy) / s + reach + 2.0).ceil() as i64).clamp(r0 + 1, ih as i64) } else { 1 };
        let mw = if visible { (x1 - x0) as u32 } else { 1 };
        let mh = (r1 - r0) as u32;

        // `mid`: grow-only
        let d = self.device.clone();
        let view = self.view.as_mut().unwrap();
        let need = match &view.mid {
            Some((_, _, w, h)) => *w < mw || *h < mh,
            None => true,
        };
        if need {
            let (w, h) = match &view.mid {
                Some((_, _, w, h)) => ((*w).max(mw), (*h).max(mh)),
                None => (mw, mh),
            };
            let t = d.create_texture(&wgpu::TextureDescriptor {
                label: Some("view_mid"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: IMG_FORMAT,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let tv = t.create_view(&Default::default());
            view.mid = Some((t, tv, w, h));
        }
        let mut u = Vec::with_capacity(64);
        for v in [iw as i32, ih as i32, x0 as i32, r0 as i32] {
            u.extend_from_slice(&v.to_le_bytes());
        }
        for v in [mw, mh] {
            u.extend_from_slice(&v.to_le_bytes());
        }
        for v in [p.ox, p.oy, s, ks, reach, p.split] {
            u.extend_from_slice(&v.to_le_bytes());
        }
        for v in [nearest as u32, 0, 0, 0] {
            u.extend_from_slice(&v.to_le_bytes());
        }
        self.queue.write_buffer(&view.uni, 0, &u);

        let view = self.view.as_ref().unwrap();
        let (res, orig) = self.stage_views().unwrap();
        let mid = &view.mid.as_ref().unwrap().1;
        let mut enc = d.create_command_encoder(&Default::default());
        if visible {
            let bg = bind_group(
                &d,
                &view.h,
                &[
                    wgpu::BindingResource::TextureView(res),
                    wgpu::BindingResource::TextureView(orig),
                    wgpu::BindingResource::TextureView(mid),
                    view.uni.as_entire_binding(),
                ],
            );
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&view.h);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(mw.div_ceil(16), mh.div_ceil(16), 1);
        }
        {
            let bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("view_v"),
                layout: &view.v.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(mid) },
                    wgpu::BindGroupEntry { binding: 1, resource: view.uni.as_entire_binding() },
                ],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("view_v"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if visible {
                pass.set_pipeline(&view.v);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        self.queue.submit([enc.finish()]);
        Ok(())
    }
}
