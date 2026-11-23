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
//!
//! A mask's own Radius is a blur of its own: the blur's target holds a
//! layer a radius ([`MOST_BLURS`] at most), each blurred across by a
//! pass of its own, and the last draw blurs each down and sums the
//! terms as `Plan` has them.

use super::*;
use greycard_render::output_sharpen::{MOST_OWN, Plan, half_width};

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

/// The blurs a draw holds at most, a layer of the blur's target each:
/// the picture's radius and the masks' own (`Plan`'s, [`MOST_OWN`]
/// beyond it).
pub(super) const MOST_BLURS: usize = 1 + MOST_OWN;

/// One blur of a draw: its standard deviation in target pixels and
/// taps a side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Blur {
    pub sigma: f32,
    pub n: f32,
}

/// The sharpen as one draw of the viewport takes it: what the shader's
/// `osharp`, `osharp_taps`, and the blurs' and terms' lanes hold.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Taps {
    /// The picture's amount (term 0's before the masks' shares).
    pub amount: f32,
    /// The taps' spacing in target pixels, the same for every blur.
    pub step: f32,
    /// The blurs, a layer each, by radius as `Plan::blurs` has them.
    pub blurs: Vec<Blur>,
    /// Each term's blur (none for the picture's where it does not act)
    /// and threshold: the picture's first, then the masks' own.
    pub terms: Vec<(Option<usize>, f32)>,
    /// The target pixels drawn past each side of the target for the
    /// blurs to read: the widest's reach.
    pub margin: u32,
    /// The plan, for the locals' terms (`locals_gpu`).
    pub plan: Plan,
}

impl Taps {
    /// The sharpen of `v`, or none when it does nothing: the picture's
    /// own, and its masks' shares of it at its radius and at their own
    /// (`Plan::finishing`). A radius is in the pixels of the picture
    /// made, as an export's is in its own: under 1:1 the window's, which
    /// is what an export the size the window shows would be sharpened
    /// at; past 1:1 the full-size picture's, `zoom` target pixels each,
    /// the full-size export seen closer. Above 1:1 the taps are a
    /// picture's pixel apart and as many as the export takes; below it
    /// a target pixel apart.
    pub(super) fn of(v: &View) -> Option<Self> {
        let plan = Plan::finishing(&v.output_sharpen, &v.locals)?;
        let step = v.zoom.max(1.0);
        let (radii, reads) = plan.blurs();
        let blurs: Vec<Blur> = radii
            .iter()
            .map(|&r| Blur {
                sigma: r * step,
                n: half_width(r) as f32,
            })
            .collect();
        let margin = blurs
            .iter()
            .map(|b| (b.n * step).ceil() as u32)
            .max()
            .unwrap_or(1);
        Some(Taps {
            amount: plan.picture.amount,
            step,
            blurs,
            terms: reads.into_iter().zip(plan.thresholds()).collect(),
            margin,
            plan,
        })
    }

    /// The blurs' lanes as the shader reads them: each layer's standard
    /// deviation, and its taps a side.
    pub(super) fn blur_lanes(&self) -> ([f32; MOST_BLURS], [f32; MOST_BLURS]) {
        let (mut sigma, mut n) = ([0.0; MOST_BLURS], [0.0; MOST_BLURS]);
        for (i, b) in self.blurs.iter().enumerate() {
            sigma[i] = b.sigma;
            n[i] = b.n;
        }
        (sigma, n)
    }

    /// The terms' lanes: each term's threshold, and the layer it reads,
    /// -1 for none.
    pub(super) fn term_lanes(&self) -> ([f32; MOST_BLURS], [f32; MOST_BLURS]) {
        let (mut threshold, mut reads) = ([0.0; MOST_BLURS], [-1.0; MOST_BLURS]);
        for (i, (r, t)) in self.terms.iter().enumerate() {
            threshold[i] = *t;
            reads[i] = r.map_or(-1.0, |r| r as f32);
        }
        (threshold, reads)
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
            none: Self::texture(device, "output sharpen none", 1, 1, 1),
        }
    }

    fn texture(
        device: &gpu::Device,
        label: &str,
        width: u32,
        height: u32,
        layers: u32,
    ) -> gpu::Texture {
        device.create_texture(&gpu::TextureDescriptor {
            label: Some(label),
            size: gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: FORMAT,
            usage: gpu::TextureUsages::RENDER_ATTACHMENT | gpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    /// The blurs' target as the shader binds it: every layer, an array
    /// however many it has.
    pub(super) fn layers_view(across: &gpu::Texture) -> gpu::TextureView {
        across.create_view(&gpu::TextureViewDescriptor {
            dimension: Some(gpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    }

    /// The luminance's target and its blurs', `width` by `height`, the
    /// blurs' at least `layers` deep: one kept of that size, or made and
    /// kept, the least lately used past [`KEPT`] let go of. A size is
    /// kept once: one with layers enough serves fewer (a draw reads only
    /// its own), and one made deeper takes the shallower one's place, so
    /// a mask's own radius never pushes another size's targets out (the
    /// scopes').
    pub(super) fn targets(
        &self,
        device: &gpu::Device,
        width: u32,
        height: u32,
        layers: u32,
    ) -> (gpu::Texture, gpu::Texture) {
        let mut held = self.targets.lock().unwrap_or_else(|e| e.into_inner());
        let sized = held
            .iter()
            .position(|(l, _)| l.width() == width && l.height() == height);
        let pair = match sized.map(|i| held.remove(i)) {
            Some(pair) if pair.1.depth_or_array_layers() >= layers => pair,
            shallower => (
                shallower.map_or_else(
                    || Self::texture(device, "output sharpen luminance", width, height, 1),
                    |(lum, _)| lum,
                ),
                Self::texture(device, "output sharpen across", width, height, layers),
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

    /// The sizes of targets kept, and their blurs' layers, for the
    /// tests.
    #[cfg(test)]
    pub(super) fn kept(&self) -> Vec<(u32, u32)> {
        self.kept_layers()
            .into_iter()
            .map(|(w, h, _)| (w, h))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn kept_layers(&self) -> Vec<(u32, u32, u32)> {
        self.targets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(l, a)| (l.width(), l.height(), a.depth_or_array_layers()))
            .collect()
    }

    /// The blurs across of `lum` into `across`, a layer each.
    pub(super) fn across(
        &self,
        device: &gpu::Device,
        queue: &gpu::Queue,
        encoder: &mut gpu::CommandEncoder,
        (lum, across): (&gpu::Texture, &gpu::Texture),
        taps: &Taps,
    ) {
        let lum_view = lum.create_view(&Default::default());
        for (layer, blur) in taps.blurs.iter().enumerate() {
            let params = AcrossParams {
                k: [blur.sigma, taps.step, blur.n, taps.margin as f32],
            };
            let buffer = device.create_buffer(&gpu::BufferDescriptor {
                label: Some("output sharpen across"),
                size: std::mem::size_of::<AcrossParams>() as u64,
                usage: gpu::BufferUsages::UNIFORM | gpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&params));
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
            let view = across.create_view(&gpu::TextureViewDescriptor {
                dimension: Some(gpu::TextureViewDimension::D2),
                base_array_layer: layer as u32,
                array_layer_count: Some(1),
                ..Default::default()
            });
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

    /// Under 1:1 the radius is in the window's pixels, as an export of
    /// the size shown takes it in its own; past 1:1 it follows the
    /// zoom, the full-size export seen closer, the taps a picture's
    /// pixel apart and as many as the export takes.
    #[test]
    fn the_taps_are_the_shown_pictures_and_follow_the_zoom_past_one() {
        assert_eq!(Taps::of(&view(1.0, 0.0, 2.0)), None);
        // At half and at a quarter: six taps a target pixel apart, a
        // margin of six, the radius whole.
        for zoom in [0.5, 0.25] {
            let fit = Taps::of(&view(zoom, 1.0, 2.0)).unwrap();
            assert_eq!(
                (fit.blurs[0].sigma, fit.step, fit.blurs[0].n, fit.margin),
                (2.0, 1.0, 6.0, 6)
            );
        }
        // At 1:1, the full-size export's.
        let one = Taps::of(&view(1.0, 1.0, 2.0)).unwrap();
        assert_eq!(
            (one.blurs[0].sigma, one.step, one.blurs[0].n, one.margin),
            (2.0, 1.0, 6.0, 6)
        );
        // At four: six taps four target pixels apart, a margin of 24.
        let close = Taps::of(&view(4.0, 1.0, 2.0)).unwrap();
        assert_eq!(
            (
                close.blurs[0].sigma,
                close.step,
                close.blurs[0].n,
                close.margin
            ),
            (8.0, 4.0, 6.0, 24)
        );
        let small = Taps::of(&view(0.25, 1.0, 0.5)).unwrap();
        assert_eq!(small.margin, 2);
    }

    /// A mask's own Radius is a layer of its own, and the margin the
    /// widest blur's: the picture's radius and two own ones at 2:1, one
    /// of them shared by two thresholds, are three layers and four
    /// terms; with the picture's off and one own radius, one layer and
    /// the picture's term reading none.
    #[test]
    fn a_masks_own_radius_is_a_layer_of_its_own() {
        let local = |share: f32, radius: Option<f32>, threshold: Option<f32>| {
            let mut a = greycard_edit::Adjustment::default();
            a.mask.components.push(Default::default());
            a.look.detail.output_sharpen = share;
            a.look.detail.output_sharpen_radius = radius;
            a.look.detail.output_sharpen_threshold = threshold;
            Local::of(&a, vec![None], None)
        };
        let mut v = view(2.0, 1.0, 1.0);
        v.locals = vec![
            local(0.5, Some(3.0), None),
            local(0.5, Some(0.5), Some(0.0)),
            local(0.5, Some(3.0), Some(0.05)),
        ];
        let t = Taps::of(&v).unwrap();
        assert_eq!(
            t.blurs,
            vec![
                Blur { sigma: 2.0, n: 3.0 },
                Blur { sigma: 1.0, n: 2.0 },
                Blur { sigma: 6.0, n: 9.0 },
            ]
        );
        assert_eq!(t.margin, 18);
        assert_eq!(
            t.term_lanes(),
            ([0.01, 0.0, 0.01, 0.05], [0.0, 1.0, 2.0, 2.0])
        );
        v.output_sharpen.amount = 0.0;
        v.locals = vec![local(1.0, Some(0.5), None)];
        let t = Taps::of(&v).unwrap();
        assert_eq!(t.blurs, vec![Blur { sigma: 1.0, n: 2.0 }]);
        assert_eq!(
            t.term_lanes(),
            ([0.01, 0.01, 0.0, 0.0], [-1.0, 0.0, -1.0, -1.0])
        );
        assert_eq!(t.margin, 4);
    }
}
