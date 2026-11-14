//! The output sharpen on the GPU (`greycard_render::output_sharpen` is
//! the reference): the viewport's shader draws the picture's encoded
//! luminance into a target of its own, this blurs it across, and the
//! shader's last draw blurs that down and sharpens. Run only while the
//! sharpen is on; off, the viewport is the one draw it always was, and
//! the targets are let go of.
//!
//! The luminance is drawn with a margin of the blur's reach on every
//! side of the target, so a pixel at the window's edge reads the
//! picture past it as the export does; a tap is held only at the
//! picture's own edge. That edge is the shader's own: it marks the
//! canvas in the luminance (`OSHARP_PAST`), and the taps stop at the
//! last texel it drew as picture, so no edge reckoned apart can
//! disagree with it where the edge falls on a texel's center.

use super::*;

/// The luminance and its blur: one channel of full floats, so the blur
/// is the CPU's to its rounding. Renderable on every WebGPU device; not
/// filterable without a feature, so the shaders read it by
/// `textureLoad` and interpolate it themselves (`osharp_tap`).
const FORMAT: gpu::TextureFormat = gpu::TextureFormat::R32Float;

/// The sizes of targets kept at once: the viewport's and the scopes'
/// (with the navigator's, the same size), and one to spare for a
/// window being resized.
const KEPT: usize = 3;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct AcrossParams {
    k: [f32; 4],
}

/// The sharpen as one draw of the viewport takes it: what the shader's
/// `osharp` and `osharp_taps` hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Taps {
    pub amount: f32,
    /// The blur's standard deviation in target pixels.
    pub sigma: f32,
    pub threshold: f32,
    /// The taps' spacing in target pixels and taps a side.
    pub step: f32,
    pub n: f32,
    /// The target pixels drawn past each side of the target for the
    /// blur to read: its reach.
    pub margin: u32,
}

impl Taps {
    /// The sharpen of `v`, or none when it does nothing. The radius is
    /// in the full-size picture's pixels, which `zoom` target pixels
    /// each are, so the viewport at any zoom shows what an export at
    /// that size would hold. Above 1:1 the taps are a picture's pixel
    /// apart and as many as the export takes; below it a target pixel
    /// apart.
    pub(super) fn of(v: &View) -> Option<Self> {
        let p = greycard_render::output_sharpen::Params::of(&v.output_sharpen, 1.0)?;
        let sigma = p.sigma * v.zoom;
        let step = v.zoom.max(1.0);
        let n = ((3.0 * sigma / step).ceil() as usize)
            .clamp(1, greycard_render::output_sharpen::MOST_TAPS);
        let margin = (n as f32 * step).ceil() as u32;
        Some(Taps {
            amount: p.amount,
            sigma,
            threshold: p.threshold,
            step,
            n: n as f32,
            margin,
        })
    }
}

pub(super) struct OutputSharpen {
    /// The viewport's shader into the luminance's target.
    pub lum_pipeline: gpu::RenderPipeline,
    across_pipeline: gpu::RenderPipeline,
    across_layout: gpu::BindGroupLayout,
    /// The luminance and its blur across, by size, the last used
    /// first: the viewport and the scopes draw at sizes of their own,
    /// and neither makes the other's again.
    targets: std::sync::Mutex<Vec<(gpu::Texture, gpu::Texture)>>,
    /// A texel of nothing, bound while there is no blur to read.
    pub none: gpu::Texture,
}

impl OutputSharpen {
    pub(super) fn new(
        device: &gpu::Device,
        shader: &gpu::ShaderModule,
        layout: &gpu::PipelineLayout,
    ) -> Self {
        let lum_pipeline = device.create_render_pipeline(&gpu::RenderPipelineDescriptor {
            label: Some("output sharpen luminance"),
            layout: Some(layout),
            vertex: gpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(gpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            primitive: gpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: gpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let across_shader = device.create_shader_module(gpu::ShaderModuleDescriptor {
            label: Some("output sharpen across"),
            source: gpu::ShaderSource::Wgsl(include_str!("../osharp.wgsl").into()),
        });
        let across_layout = device.create_bind_group_layout(&gpu::BindGroupLayoutDescriptor {
            label: Some("output sharpen across"),
            entries: &[
                gpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Buffer {
                        ty: gpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Texture {
                        sample_type: gpu::TextureSampleType::Float { filterable: false },
                        view_dimension: gpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let across_pipeline_layout =
            device.create_pipeline_layout(&gpu::PipelineLayoutDescriptor {
                label: Some("output sharpen across"),
                bind_group_layouts: &[Some(&across_layout)],
                ..Default::default()
            });
        let across_pipeline = device.create_render_pipeline(&gpu::RenderPipelineDescriptor {
            label: Some("output sharpen across"),
            layout: Some(&across_pipeline_layout),
            vertex: gpu::VertexState {
                module: &across_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(gpu::FragmentState {
                module: &across_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            primitive: gpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: gpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            lum_pipeline,
            across_pipeline,
            across_layout,
            targets: std::sync::Mutex::new(Vec::new()),
            none: Self::texture(device, "output sharpen none", 1, 1),
        }
    }

    fn texture(device: &gpu::Device, label: &str, width: u32, height: u32) -> gpu::Texture {
        device.create_texture(&gpu::TextureDescriptor {
            label: Some(label),
            size: gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: FORMAT,
            usage: gpu::TextureUsages::RENDER_ATTACHMENT | gpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    /// The luminance's target and its blur's, `width` by `height`:
    /// one kept of that size, or made and kept, the least lately used
    /// past [`KEPT`] let go of.
    pub(super) fn targets(
        &self,
        device: &gpu::Device,
        width: u32,
        height: u32,
    ) -> (gpu::Texture, gpu::Texture) {
        let mut held = self.targets.lock().unwrap_or_else(|e| e.into_inner());
        let pair = match held
            .iter()
            .position(|(l, _)| l.width() == width && l.height() == height)
        {
            Some(i) => held.remove(i),
            None => (
                Self::texture(device, "output sharpen luminance", width, height),
                Self::texture(device, "output sharpen across", width, height),
            ),
        };
        held.insert(0, pair.clone());
        held.truncate(KEPT);
        pair
    }

    /// Let go of every target: the sharpen is off.
    pub(super) fn release(&self) {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// How many sizes of targets are kept, for the tests.
    #[cfg(test)]
    pub(super) fn kept(&self) -> Vec<(u32, u32)> {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(l, _)| (l.width(), l.height()))
            .collect()
    }

    /// The blur across of `lum` into `across`.
    pub(super) fn across(
        &self,
        device: &gpu::Device,
        queue: &gpu::Queue,
        encoder: &mut gpu::CommandEncoder,
        (lum, across): (&gpu::Texture, &gpu::Texture),
        taps: &Taps,
    ) {
        let params = AcrossParams {
            k: [taps.sigma, taps.step, taps.n, taps.margin as f32],
        };
        let buffer = device.create_buffer(&gpu::BufferDescriptor {
            label: Some("output sharpen across"),
            size: std::mem::size_of::<AcrossParams>() as u64,
            usage: gpu::BufferUsages::UNIFORM | gpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&params));
        let lum_view = lum.create_view(&Default::default());
        let bind_group = device.create_bind_group(&gpu::BindGroupDescriptor {
            label: Some("output sharpen across"),
            layout: &self.across_layout,
            entries: &[
                gpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                gpu::BindGroupEntry {
                    binding: 1,
                    resource: gpu::BindingResource::TextureView(&lum_view),
                },
            ],
        });
        let view = across.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&gpu::RenderPassDescriptor {
            label: Some("output sharpen across"),
            color_attachments: &[Some(gpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: gpu::Operations {
                    load: gpu::LoadOp::Clear(gpu::Color::BLACK),
                    store: gpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.across_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(zoom: f32, amount: f32, radius: f32) -> View {
        View {
            zoom,
            center: (300.0, 200.0),
            frame_size: (600.0, 400.0),
            output_sharpen: greycard_edit::OutputSharpen {
                amount,
                radius,
                ..greycard_edit::OutputSharpen::default()
            },
            ..View::blank()
        }
    }

    /// The radius follows the zoom, as an export's follows its size;
    /// above 1:1 the taps are a picture's pixel apart and as many as
    /// the export takes.
    #[test]
    fn the_taps_follow_the_zoom() {
        assert_eq!(Taps::of(&view(1.0, 0.0, 2.0)), None);
        // At half: three taps a target pixel apart, a margin of three.
        let fit = Taps::of(&view(0.5, 1.0, 2.0)).unwrap();
        assert_eq!((fit.sigma, fit.step, fit.n, fit.margin), (1.0, 1.0, 3.0, 3));
        // At four: six taps four target pixels apart, a margin of 24.
        let close = Taps::of(&view(4.0, 1.0, 2.0)).unwrap();
        assert_eq!(
            (close.sigma, close.step, close.n, close.margin),
            (8.0, 4.0, 6.0, 24)
        );
        let small = Taps::of(&view(0.25, 1.0, 1.0)).unwrap();
        assert_eq!(small.margin, 1);
    }
}
