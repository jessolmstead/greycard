//! The dehaze on the GPU: the halves of
//! `greycard_core::develop::dehaze` that scale with the picture, so
//! that an edit with the Dehaze on keeps the local contrast and the
//! sharpen here too, and a Dehaze move costs about what a sharpen
//! move does.
//!
//! The reference splits in two. The fit works on a reduced copy of
//! the picture, a long edge of about 1536 (each reduced pixel the
//! mean of a block of four by four at 24 MP): the dark channel, the
//! airlight from its quantiles, the guided filter's line on the grid.
//! It is small and awkward to put on a GPU (two selections), and stays
//! the reference's own code on the CPU. The apply is per full-size
//! pixel: the line read bilinearly off the grid and closed by the
//! pixel's own luminance, bounded below by its own dark channel, and
//! the recovery `J = (I - A) / t + A`. That is the part a 24 MP or
//! 45 MP picture pays for, and it runs here.
//!
//! So a run is: the block sums here ([`Context::dehaze_reduced`]),
//! read back (18 MB at 24 MP) and divided by the reference into its
//! [`Reduced`] copy; the reference's [`Reduced::model`] on that; and
//! the apply here from the model's two planes and its taps
//! ([`Context::dehaze`]). The sums are added in the reference's order,
//! one invocation a block, so on the same pixels the reduced copy,
//! and with it the airlight and the whole model, is the reference's to
//! the bit; the apply agrees with the reference's to rounding, since
//! its divisions are not correctly rounded here and a device may fuse
//! a multiply and an add. The tests hold it to an f64 evaluation of
//! the same model, by a bound worked out from each pixel's magnitudes.
//!
//! The apply also sums and takes the least of the transmissions it
//! applied, a pair a workgroup, so the dehaze's stats mean what the
//! reference's do: the full-size transmission's mean and least.
//!
//! Nothing is kept between runs: the sums, the model and the pairs
//! are a few tens of MB and made for each run.

use std::sync::OnceLock;

use greycard_core::develop::dehaze::{
    self as reference, DehazeOptions, DehazeStats, Model, Reduced,
};
use greycard_core::image::WorkingImage;

use crate::plumbing::{
    SAMPLED, STORAGE_BUFFER, STORAGE_BUFFER_READ, groups, layout_at, pipeline, storage_texture,
    uniform,
};
use crate::{Context, Error, Image, Result};

/// The shader's uniform, laid out as WGSL lays it out.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    size: [u32; 2],
    grid: [u32; 2],
    factor: u32,
    strength: f32,
    min_transmission: f32,
    _pad: u32,
    airlight: [f32; 4],
    luma: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<Params>() == 64);

/// A tap as the shader reads it.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Tap {
    i0: u32,
    i1: u32,
    t: f32,
    _pad: u32,
}

/// The apply's workgroup, the shader's: a pair of stats a workgroup.
const GROUP: u32 = 16;

/// The op's pipelines, built once per device.
pub(crate) struct Pipelines {
    sums_layout: wgpu::BindGroupLayout,
    apply_layout: wgpu::BindGroupLayout,
    block_sums: wgpu::ComputePipeline,
    apply_half: wgpu::ComputePipeline,
    apply_full: wgpu::ComputePipeline,
    /// Stand-ins for the output a run does not write.
    spare_half: wgpu::Texture,
    spare_full: wgpu::Texture,
}

impl Pipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dehaze"),
            source: wgpu::ShaderSource::Wgsl(include_str!("dehaze.wgsl").into()),
        });
        use wgpu::StorageTextureAccess::WriteOnly;
        use wgpu::TextureFormat::{Rgba16Float, Rgba32Float};
        // The bindings as the shader numbers them; each layout the
        // subset its kernels use.
        let sums_layout = layout_at(
            device,
            "dehaze block sums",
            &[(0, uniform(false)), (1, SAMPLED), (2, STORAGE_BUFFER)],
        );
        let apply_layout = layout_at(
            device,
            "dehaze apply",
            &[
                (0, uniform(false)),
                (1, SAMPLED),
                (3, STORAGE_BUFFER_READ),
                (4, STORAGE_BUFFER_READ),
                (5, STORAGE_BUFFER_READ),
                (6, STORAGE_BUFFER_READ),
                (7, STORAGE_BUFFER),
                (8, storage_texture(Rgba16Float, WriteOnly)),
                (9, storage_texture(Rgba32Float, WriteOnly)),
            ],
        );
        let pipeline_layout = |label: &str, l: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(l)],
                ..Default::default()
            })
        };
        let sums_pl = pipeline_layout("dehaze block sums", &sums_layout);
        let apply_pl = pipeline_layout("dehaze apply", &apply_layout);
        let spare = |label: &str, format: wgpu::TextureFormat| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            })
        };
        Self {
            block_sums: pipeline(device, &module, &sums_pl, "block_sums"),
            apply_half: pipeline(device, &module, &apply_pl, "apply_half"),
            apply_full: pipeline(device, &module, &apply_pl, "apply_full"),
            spare_half: spare("dehaze spare half", Rgba16Float),
            spare_full: spare("dehaze spare full", Rgba32Float),
            sums_layout,
            apply_layout,
        }
    }
}

/// Where the result goes: the viewport's half-float texture, or full
/// floats for the sharpen or a read back.
enum Output<'a> {
    Half(&'a wgpu::Texture),
    Full(&'a wgpu::Texture),
}

impl Context {
    /// The reduced copy the dehaze's fit is made on, of a picture on
    /// the GPU: its block sums made here, read back, and divided by
    /// the reference, [`Reduced::from_block_sums`]. Waits for the GPU,
    /// since the fit needs the copy before anything after it can run.
    pub fn dehaze_reduced(&self, image: &Image) -> Result<Reduced> {
        let (w, h) = (image.width, image.height);
        let factor = reference::reduce_factor(w as usize, h as usize) as u32;
        let (gw, gh) = (w.div_ceil(factor), h.div_ceil(factor));
        let len = u64::from(gw) * u64::from(gh) * 3 * 4;
        let sums = crate::scoped(&self.device, "the dehaze's block sums", || {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("dehaze block sums"),
                size: len,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let params = self.dehaze_params(Params {
                size: [w, h],
                grid: [gw, gh],
                factor,
                ..Default::default()
            });
            let view = image.texture.create_view(&Default::default());
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("dehaze block sums"),
                layout: &self.dehaze.sums_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            });
            let mut encoder = self.encoder("dehaze block sums");
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.dehaze.block_sums);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(groups(gw, GROUP), groups(gh, GROUP), 1);
            }
            self.queue.submit(Some(encoder.finish()));
            self.read_buffer(&buffer, 0, len)
        })?;
        let sums: Vec<f32> = bytemuck::cast_slice(&sums).to_vec();
        Ok(Reduced::from_block_sums(w as usize, h as usize, sums))
    }

    /// [`greycard_core::develop::dehaze::dehaze_with_model`] on a
    /// picture already on the GPU, with a model the reference fitted
    /// (from [`Context::dehaze_reduced`]'s copy of this picture): the
    /// result a new [`Image`] for the sharpen to read, in full floats,
    /// with its own threshold to find, and the stats the reference
    /// gives. Waits for the GPU, for the stats.
    pub fn dehaze(&self, image: &Image, model: &Model) -> Result<(Image, DehazeStats)> {
        let (w, h) = (image.width, image.height);
        crate::scoped(&self.device, "the dehaze", || {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("after the dehaze, full floats"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let stats = self.dehaze_into(image, model, Output::Full(&texture))?;
            Ok((
                Image {
                    texture,
                    width: w,
                    height: h,
                    threshold: OnceLock::new(),
                },
                stats,
            ))
        })
    }

    /// The same, the result into `out`, a texture from
    /// [`Context::viewport_texture`] of the picture's size, its alpha
    /// zero: for the viewport when the sharpen is off.
    pub fn dehaze_to_viewport(
        &self,
        image: &Image,
        model: &Model,
        out: &wgpu::Texture,
    ) -> Result<DehazeStats> {
        if out.width() != image.width || out.height() != image.height {
            return Err(Error::Unsupported(format!(
                "a {}x{} output for a {}x{} picture",
                out.width(),
                out.height(),
                image.width,
                image.height
            )));
        }
        crate::scoped(&self.device, "the dehaze", || {
            self.dehaze_into(image, model, Output::Half(out))
        })
    }

    /// The reference's own signature: the dehaze on a working image in
    /// place, the fit on the CPU between the GPU's halves. Uploads,
    /// runs and reads back, so it is for the CLI and the tests, not a
    /// slider.
    pub fn dehaze_image(
        &self,
        image: &mut WorkingImage,
        options: &DehazeOptions,
    ) -> Result<DehazeStats> {
        let uploaded = self.upload(image)?;
        let Some(model) = self.dehaze_reduced(&uploaded)?.model(options) else {
            return Ok(reference::identity_stats(options));
        };
        let (out, stats) = self.dehaze(&uploaded, &model)?;
        *image = self.download(&out)?;
        Ok(stats)
    }

    fn dehaze_params(&self, params: Params) -> wgpu::Buffer {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dehaze params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&buffer, 0, bytemuck::bytes_of(&params));
        buffer
    }

    /// A storage buffer holding `data`, for the shader to read.
    fn dehaze_buffer<T: bytemuck::Pod>(&self, label: &str, data: &[T]) -> wgpu::Buffer {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (bytes.len() as u64).max(16),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buffer, 0, bytes);
        buffer
    }

    fn dehaze_into(&self, image: &Image, model: &Model, out: Output<'_>) -> Result<DehazeStats> {
        let p = &self.dehaze;
        let (w, h) = (image.width, image.height);
        let (gw, gh, factor) = model.grid();
        if (w as usize).div_ceil(factor) != gw || (h as usize).div_ceil(factor) != gh {
            return Err(Error::Unsupported(format!(
                "a model fitted on a {gw}x{gh} grid at {factor} for a {w}x{h} picture"
            )));
        }
        let taps = |full: u32, reduced: usize| -> Vec<Tap> {
            reference::taps(full as usize, reduced, factor)
                .into_iter()
                .map(|(i0, i1, t)| Tap {
                    i0: i0 as u32,
                    i1: i1 as u32,
                    t,
                    _pad: 0,
                })
                .collect()
        };
        let a = model.airlight();
        let params = self.dehaze_params(Params {
            size: [w, h],
            grid: [gw as u32, gh as u32],
            factor: factor as u32,
            strength: model.strength(),
            min_transmission: reference::MIN_TRANSMISSION,
            _pad: 0,
            airlight: [a[0], a[1], a[2], 0.0],
            luma: [
                reference::LUMA[0],
                reference::LUMA[1],
                reference::LUMA[2],
                0.0,
            ],
        });
        let slope = self.dehaze_buffer("dehaze slope", model.slope());
        let intercept = self.dehaze_buffer("dehaze intercept", model.intercept());
        let columns = self.dehaze_buffer("dehaze columns", &taps(w, gw));
        let rows = self.dehaze_buffer("dehaze rows", &taps(h, gh));
        let (gx, gy) = (groups(w, GROUP), groups(h, GROUP));
        let partial_len = u64::from(gx) * u64::from(gy) * 8;
        let partial = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dehaze transmission sums"),
            size: partial_len,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let (out_half, out_full) = match out {
            Output::Half(t) => (t, &p.spare_full),
            Output::Full(t) => (&p.spare_half, t),
        };
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dehaze apply"),
            layout: &p.apply_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view(&image.texture)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: slope.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: intercept.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: columns.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: rows.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: partial.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&view(out_half)),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&view(out_full)),
                },
            ],
        });
        let mut encoder = self.encoder("dehaze apply");
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(match out {
                Output::Half(_) => &p.apply_half,
                Output::Full(_) => &p.apply_full,
            });
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        self.queue.submit(Some(encoder.finish()));
        // The pairs, summed as the reference sums its rows, in f64.
        let pairs = self.read_buffer(&partial, 0, partial_len)?;
        let pairs: &[f32] = bytemuck::cast_slice(&pairs);
        let (sum, least) = pairs
            .as_chunks::<2>()
            .0
            .iter()
            .fold((0f64, f32::INFINITY), |(s, m), pair| {
                (s + f64::from(pair[0]), m.min(pair[1]))
            });
        let mean = (sum / (f64::from(w) * f64::from(h))) as f32;
        Ok(model.stats(mean, least))
    }
}

#[cfg(test)]
mod tests {
    /// The shader parses and validates without a device, so a typo is
    /// found by the suite and not by a black viewport.
    #[test]
    fn the_shader_parses_and_validates() {
        let source = include_str!("dehaze.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("dehaze.wgsl:\n{}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("dehaze.wgsl: {e:?}"));
    }

    /// The uniform's and the taps' fields sit where the WGSL structs
    /// put them.
    #[test]
    fn the_params_are_laid_out_as_the_shader_reads_them() {
        use super::{Params, Tap};
        use std::mem::offset_of;
        assert_eq!(offset_of!(Params, size), 0);
        assert_eq!(offset_of!(Params, grid), 8);
        assert_eq!(offset_of!(Params, factor), 16);
        assert_eq!(offset_of!(Params, strength), 20);
        assert_eq!(offset_of!(Params, min_transmission), 24);
        assert_eq!(offset_of!(Params, airlight), 32);
        assert_eq!(offset_of!(Params, luma), 48);
        assert_eq!(std::mem::size_of::<Params>(), 64);
        assert_eq!(offset_of!(Tap, i0), 0);
        assert_eq!(offset_of!(Tap, i1), 4);
        assert_eq!(offset_of!(Tap, t), 8);
        assert_eq!(std::mem::size_of::<Tap>(), 16);
    }
}
