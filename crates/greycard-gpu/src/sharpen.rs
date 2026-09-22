//! The capture sharpening on the GPU: the same deconvolution as
//! `greycard_core::develop::sharpen`, tile for tile and block for
//! block, so that the viewport shows what the export will hold.
//!
//! Three shaders: `sharpen_planes.wgsl` works the whole picture
//! (luminance, L*, the clip mask, the tile statistics behind the
//! automatic threshold, the blend mask and its blur); `sharpen_tiles.wgsl`
//! runs the Richardson–Lucy iterations in an atlas of padded tiles, a
//! batch of them at a time; `sharpen_apply.wgsl` scales the channels
//! by the result. What the reference decides on the CPU between its
//! stages (which tile is flattest, and the threshold from it) is
//! decided on the CPU here too, from small read backs, with the
//! reference's own functions.
//!
//! The picture's planes are R32Float textures rather than storage
//! buffers: a 45 MP plane is 180 MB, over the 128 MB a storage buffer
//! binding is allowed by default on the device the editor shares, and
//! a texture is limited by its side, not its bytes.
//!
//! How faithful, pass by pass. The deconvolution's blurs, the ratio
//! and the multiply, the blend blur, the contrast measure and the
//! sigmoid, the clip mask and its dilation, and the final scaling take
//! the reference's operations in the reference's order, so they are
//! the same to within the GPU compiler's freedom to fuse a multiply
//! and an add. Three places are not order-faithful: `l_star` uses a
//! power and a Newton step where the reference has `cbrt`; and the
//! tile statistics behind the automatic threshold and the row sums
//! behind the stats reduce a strided partial per thread and then a
//! tree, where the reference sums in sequence. A last-bit difference
//! is invisible in a pixel, but two decisions read it: a block's early
//! stop (an estimate a bit under half its blended start stops an
//! iteration earlier, and that block's picture is then a whole
//! iteration different) and the threshold search (the flattest tile,
//! and the threshold's 0.01 step). Neither has moved on any picture
//! tried; the tests hold the pictures to 1e-4 and the threshold to
//! equality, so if either ever does, the suite says so.

use std::sync::Mutex;

use greycard_core::develop::sharpen::{
    self as reference, Radius, SharpenOptions, SharpenStats, Threshold,
};
use greycard_core::image::WorkingImage;

use crate::plumbing::{
    SAMPLED, STORAGE_BUFFER, Uniforms, groups, layout, pipeline, storage_texture, uniform,
};
use crate::{Context, Error, Image, Result};

/// The shaders' uniform, laid out as WGSL lays it out.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    size: [u32; 2],
    tile: u32,
    border: u32,
    full: u32,
    cols: u32,
    rows: u32,
    c0: u32,
    b0: u32,
    iterations: u32,
    stop_early: u32,
    half: u32,
    taps: u32,
    threshold: f32,
    clip_level: f32,
    _align: u32,
    origin: [u32; 2],
    skip: u32,
    tsize: u32,
    count: [u32; 2],
    offset: u32,
    _pad: u32,
    kernel: [[f32; 4]; 4],
}

const _: () = assert!(std::mem::size_of::<Params>() == 160);

/// The atlas's size, per plane: how many padded tiles are worked at
/// once. Larger is fewer batches; 96 MB is under a hundredth of a
/// second of traffic and holds a fifth of a 45 MP frame's tiles.
const ATLAS_BUDGET: u64 = 96 << 20;

/// The widest border a tile can carry (`border_for` gives 8 at most),
/// so an atlas made for a size fits any radius.
const MAX_BORDER: u32 = 8;

/// The op's pipelines, built once per device, and the working
/// textures kept between runs on a picture of the same size.
pub(crate) struct Pipelines {
    planes_layout: wgpu::BindGroupLayout,
    tiles_layout: wgpu::BindGroupLayout,
    apply_layout: wgpu::BindGroupLayout,
    prepare: wgpu::ComputePipeline,
    row_sum: wgpu::ComputePipeline,
    tile_index: wgpu::ComputePipeline,
    dilate: wgpu::ComputePipeline,
    contrast_blend: wgpu::ComputePipeline,
    blur_rows: wgpu::ComputePipeline,
    blur_cols: wgpu::ComputePipeline,
    init_sharpened: wgpu::ComputePipeline,
    setup_blocks: wgpu::ComputePipeline,
    fill_estimate: wgpu::ComputePipeline,
    blur_ratio: wgpu::ComputePipeline,
    blur_multiply: wgpu::ComputePipeline,
    check: wgpu::ComputePipeline,
    commit_rest: wgpu::ComputePipeline,
    apply_half: wgpu::ComputePipeline,
    apply_full: wgpu::ComputePipeline,
    /// Stand-ins for the apply's outputs not in use.
    spare_half: wgpu::Texture,
    spare_full: wgpu::Texture,
    spare_mask: wgpu::Texture,
    work: Mutex<Option<Work>>,
}

impl Pipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = |label: &str, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let planes = module("sharpen planes", include_str!("sharpen_planes.wgsl"));
        let tiles = module("sharpen tiles", include_str!("sharpen_tiles.wgsl"));
        let apply = module("sharpen apply", include_str!("sharpen_apply.wgsl"));
        use wgpu::StorageTextureAccess::{ReadWrite, WriteOnly};
        use wgpu::TextureFormat::{R32Float, Rgba16Float, Rgba32Float};
        let planes_layout = layout(
            device,
            "sharpen planes",
            &[
                uniform(true),
                SAMPLED,
                storage_texture(R32Float, ReadWrite),
                storage_texture(R32Float, ReadWrite),
                storage_texture(R32Float, ReadWrite),
                storage_texture(R32Float, ReadWrite),
                STORAGE_BUFFER,
            ],
        );
        let tiles_layout = layout(
            device,
            "sharpen tiles",
            &[
                uniform(true),
                SAMPLED,
                SAMPLED,
                storage_texture(R32Float, ReadWrite),
                storage_texture(R32Float, ReadWrite),
                storage_texture(R32Float, ReadWrite),
                STORAGE_BUFFER,
                STORAGE_BUFFER,
            ],
        );
        let apply_layout = layout(
            device,
            "sharpen apply",
            &[
                uniform(false),
                SAMPLED,
                SAMPLED,
                SAMPLED,
                SAMPLED,
                storage_texture(Rgba16Float, WriteOnly),
                storage_texture(Rgba32Float, WriteOnly),
                storage_texture(R32Float, WriteOnly),
            ],
        );
        let pipeline_layout = |label: &str, l: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(l)],
                ..Default::default()
            })
        };
        let planes_pl = pipeline_layout("sharpen planes", &planes_layout);
        let tiles_pl = pipeline_layout("sharpen tiles", &tiles_layout);
        let apply_pl = pipeline_layout("sharpen apply", &apply_layout);
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
            prepare: pipeline(device, &planes, &planes_pl, "prepare"),
            row_sum: pipeline(device, &planes, &planes_pl, "row_sum"),
            tile_index: pipeline(device, &planes, &planes_pl, "tile_index"),
            dilate: pipeline(device, &planes, &planes_pl, "dilate"),
            contrast_blend: pipeline(device, &planes, &planes_pl, "contrast_blend"),
            blur_rows: pipeline(device, &planes, &planes_pl, "blur_rows"),
            blur_cols: pipeline(device, &planes, &planes_pl, "blur_cols"),
            init_sharpened: pipeline(device, &planes, &planes_pl, "init_sharpened"),
            setup_blocks: pipeline(device, &tiles, &tiles_pl, "setup_blocks"),
            fill_estimate: pipeline(device, &tiles, &tiles_pl, "fill_estimate"),
            blur_ratio: pipeline(device, &tiles, &tiles_pl, "blur_ratio"),
            blur_multiply: pipeline(device, &tiles, &tiles_pl, "blur_multiply"),
            check: pipeline(device, &tiles, &tiles_pl, "check"),
            commit_rest: pipeline(device, &tiles, &tiles_pl, "commit_rest"),
            apply_half: pipeline(device, &apply, &apply_pl, "apply_half"),
            apply_full: pipeline(device, &apply, &apply_pl, "apply_full"),
            spare_half: spare("spare half", Rgba16Float),
            spare_full: spare("spare full", Rgba32Float),
            spare_mask: spare("spare mask", R32Float),
            planes_layout,
            tiles_layout,
            apply_layout,
            work: Mutex::new(None),
        }
    }

    /// Drop the working textures; see [`Context::release`].
    pub(crate) fn release(&self) {
        *self.work.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// The working textures and buffers for a picture of one size: the
/// four planes, the tile atlas, and the small buffers beside them.
/// Kept between runs, since a slider re-run is on the same picture.
struct Work {
    width: u32,
    height: u32,
    tile: u32,
    lum: wgpu::Texture,
    lstar: wgpu::Texture,
    blend: wgpu::Texture,
    /// The blur's scratch, then the sharpened luminance.
    tmp: wgpu::Texture,
    estimate: wgpu::Texture,
    ratio: wgpu::Texture,
    /// The atlas's capacity: padded tiles across and down.
    cap_cols: u32,
    cap_rows: u32,
    settled: wgpu::Buffer,
    left: wgpu::Buffer,
    /// Row sums of the clip mask, then of the blend mask, then the
    /// tile indices of a threshold search.
    partials: wgpu::Buffer,
}

impl Work {
    fn new(device: &wgpu::Device, max_dimension: u32, width: u32, height: u32) -> Self {
        let plane = |label: &str, w: u32, h: u32| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let tile = reference::tile_for(height as usize) as u32;
        let full_cap = tile + 2 * MAX_BORDER;
        let tiles_across = width.div_ceil(tile);
        let bands = height.div_ceil(tile);
        let cap_cols = tiles_across.min(max_dimension / full_cap).max(1);
        let cap_tiles = (ATLAS_BUDGET / (u64::from(full_cap) * u64::from(full_cap) * 4)).max(1);
        let cap_rows = (cap_tiles / u64::from(cap_cols)) as u32;
        let cap_rows = cap_rows.clamp(1, bands.min(max_dimension / full_cap).max(1));
        let blocks = tile / reference::BLOCK as u32;
        let buffer = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        // The row sums of two planes, then the largest of the three
        // threshold searches: the coarse grid, the fine grid, or the
        // search around the fine grid's best, which is every offset
        // within a step and so has a size of its own on a small
        // picture, where it outnumbers both grids.
        let skip = reference::FINE_SKIP as u32;
        let coarse = reference::COARSE_TILE as u32;
        let around = (2 * skip + 1) * (2 * skip + 1);
        let searches = [
            (width / coarse) * (height / coarse),
            (width / skip) * (height / skip),
            around,
        ]
        .into_iter()
        .max()
        .unwrap_or(around);
        Self {
            width,
            height,
            tile,
            lum: plane("sharpen luminance", width, height),
            lstar: plane("sharpen L*", width, height),
            blend: plane("sharpen blend", width, height),
            tmp: plane("sharpen scratch", width, height),
            estimate: plane("sharpen estimate", cap_cols * full_cap, cap_rows * full_cap),
            ratio: plane("sharpen ratio", cap_cols * full_cap, cap_rows * full_cap),
            cap_cols,
            cap_rows,
            settled: buffer(
                "sharpen settled",
                u64::from(cap_cols * cap_rows * blocks * blocks) * 4,
            ),
            left: buffer("sharpen left", u64::from(cap_cols * cap_rows) * 4),
            partials: buffer("sharpen partials", u64::from(2 * height + searches) * 4),
        }
    }
}

/// Where the result goes: the viewport's half-float texture, or full
/// floats and the mask apart for a read back.
enum Output<'a> {
    Half(&'a wgpu::Texture),
    Full {
        rgb: &'a wgpu::Texture,
        mask: &'a wgpu::Texture,
    },
}

/// A grid of tiles for the threshold search: tiles of side `size`
/// from `origin`, stepping by `skip`, `count` along each axis.
#[derive(Clone, Copy)]
struct Grid {
    origin: [u32; 2],
    skip: u32,
    size: u32,
    count: [u32; 2],
}

/// The first finite minimum of a search's indices, in the order the
/// reference walks its grid, with its index.
fn flattest(indices: &[f32]) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (i, &v) in indices.iter().enumerate() {
        if v.is_finite() && best.is_none_or(|(_, b)| v < b) {
            best = Some((i, v));
        }
    }
    best
}

impl Context {
    /// [`greycard_core::develop::sharpen::sharpen_with_mask`] on a
    /// picture already on the GPU, the result into `out`, a texture
    /// from [`Context::viewport_texture`] of the picture's size, the
    /// blend mask in its alpha. The stats come back as the reference
    /// gives them; the picture and its mask stay on the GPU.
    pub fn sharpen(
        &self,
        image: &Image,
        options: &SharpenOptions,
        measured: Option<f32>,
        clip_level: f32,
        out: &wgpu::Texture,
    ) -> Result<SharpenStats> {
        if out.width() != image.width || out.height() != image.height {
            return Err(Error::Unsupported(format!(
                "a {}x{} output for a {}x{} picture",
                out.width(),
                out.height(),
                image.width,
                image.height
            )));
        }
        crate::scoped(&self.device, "sharpening", || {
            self.sharpen_into(image, options, measured, clip_level, Output::Half(out))
        })
    }

    /// The reference's own signature: sharpen a working image in
    /// place and hand back the stats and the mask. Uploads, runs and
    /// reads back, so it is for the CLI and the tests, not a slider.
    pub fn sharpen_image(
        &self,
        image: &mut WorkingImage,
        options: &SharpenOptions,
        measured: Option<f32>,
        clip_level: f32,
    ) -> Result<(SharpenStats, Vec<f32>)> {
        let uploaded = self.upload(image)?;
        let (w, h) = (uploaded.width, uploaded.height);
        let make = |label: &str, format: wgpu::TextureFormat| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let (rgb, mask, stats) = crate::scoped(&self.device, "sharpening", || {
            let rgb = make("sharpened, full floats", wgpu::TextureFormat::Rgba32Float);
            let mask = make("sharpen mask", wgpu::TextureFormat::R32Float);
            let stats = self.sharpen_into(
                &uploaded,
                options,
                measured,
                clip_level,
                Output::Full {
                    rgb: &rgb,
                    mask: &mask,
                },
            )?;
            Ok((rgb, mask, stats))
        })?;
        let rgba = self.read_texture(&rgb, 0, 0, w, h, 16)?;
        let rgba: &[f32] = bytemuck::cast_slice(&rgba);
        let (pixels, _) = image.data.as_chunks_mut::<3>();
        let (fours, _) = rgba.as_chunks::<4>();
        for (px, four) in pixels.iter_mut().zip(fours) {
            px.copy_from_slice(&four[..3]);
        }
        let mask = self.read_texture(&mask, 0, 0, w, h, 4)?;
        Ok((stats, bytemuck::cast_slice(&mask).to_vec()))
    }

    fn sharpen_into(
        &self,
        image: &Image,
        options: &SharpenOptions,
        measured: Option<f32>,
        clip_level: f32,
        out: Output<'_>,
    ) -> Result<SharpenStats> {
        let p = &self.sharpen;
        let (w, h) = (image.width, image.height);
        let n = f64::from(w) * f64::from(h);
        let radius = match options.radius {
            Radius::Auto => measured.unwrap_or(reference::DEFAULT_RADIUS),
            Radius::Fixed(r) => r,
        }
        .clamp(reference::MIN_RADIUS, reference::MAX_RADIUS);

        // The working set for this size, kept from the last run.
        let mut guard = p.work.lock().unwrap_or_else(|e| e.into_inner());
        let work = match guard.as_ref() {
            Some(work) if work.width == w && work.height == h => guard.as_ref().unwrap(),
            _ => {
                *guard = Some(Work::new(&self.device, self.max_dimension, w, h));
                guard.as_ref().unwrap()
            }
        };

        // The deconvolution's tiling, the reference's.
        let kernel = reference::kernel(radius);
        let half = (kernel.len() / 2) as u32;
        let border = reference::border_for(half as usize, options.iterations) as u32;
        let tile = work.tile;
        let full = tile + 2 * border;
        let tiles_across = w.div_ceil(tile);
        let bands = h.div_ceil(tile);
        let blocks = tile / reference::BLOCK as u32;
        let mut batches = Vec::new();
        for c0 in (0..tiles_across).step_by(work.cap_cols as usize) {
            for b0 in (0..bands).step_by(work.cap_rows as usize) {
                batches.push((
                    c0,
                    b0,
                    work.cap_cols.min(tiles_across - c0),
                    work.cap_rows.min(bands - b0),
                ));
            }
        }
        let uniforms = Uniforms::new(&self.device, "sharpen params", 16 + batches.len() as u64);
        let mut deconv = [[0f32; 4]; 4];
        for (i, k) in kernel.iter().enumerate() {
            deconv[i / 4][i % 4] = *k;
        }
        let base = Params {
            size: [w, h],
            tile,
            border,
            full,
            iterations: options.iterations as u32,
            stop_early: options.stop_early as u32,
            half,
            taps: kernel.len() as u32,
            clip_level,
            kernel: deconv,
            ..Default::default()
        };

        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let planes_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sharpen planes"),
            layout: &p.planes_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view(&image.texture)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view(&work.lum)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&view(&work.lstar)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&view(&work.blend)),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&view(&work.tmp)),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: work.partials.as_entire_binding(),
                },
            ],
        });

        // Phase one: luminance, L* and the clip mask, and the clip
        // mask's row sums.
        let mut encoder = self.encoder("sharpen");
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&p.prepare);
            pass.set_bind_group(0, &planes_group, &[uniforms.push(&self.queue, &base)]);
            pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
            pass.set_pipeline(&p.row_sum);
            pass.set_bind_group(
                0,
                &planes_group,
                &[uniforms.push(&self.queue, &Params { offset: 0, ..base })],
            );
            pass.dispatch_workgroups(h, 1, 1);
        }
        self.queue.submit(Some(encoder.finish()));

        // The contrast threshold: fixed, remembered, or searched for.
        let threshold = match options.contrast {
            Threshold::Fixed(t) => t.max(0.0),
            Threshold::Auto => match image.threshold.get() {
                Some(t) => *t,
                None => {
                    let t = self.auto_threshold(work, &planes_group, &uniforms, &base)?;
                    let _ = image.threshold.set(t);
                    t
                }
            },
        };

        // Phase two: the blend mask, its row sums, the sharpened
        // luminance's start, the iterations batch by batch, and the
        // result.
        let tiles_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sharpen tiles"),
            layout: &p.tiles_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view(&work.lum)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view(&work.blend)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&view(&work.tmp)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&view(&work.estimate)),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&view(&work.ratio)),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: work.settled.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: work.left.as_entire_binding(),
                },
            ],
        });
        let (out_half, out_full, out_mask) = match out {
            Output::Half(t) => (t, &p.spare_full, &p.spare_mask),
            Output::Full { rgb, mask } => (&p.spare_half, rgb, mask),
        };
        let apply_slot = uniforms.push(&self.queue, &base);
        let apply_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sharpen apply"),
            layout: &p.apply_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniforms.buffer,
                        offset: u64::from(apply_slot),
                        size: Some(std::num::NonZeroU64::new(crate::plumbing::SLOT).unwrap()),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view(&image.texture)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view(&work.lum)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&view(&work.tmp)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&view(&work.blend)),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&view(out_half)),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&view(out_full)),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&view(out_mask)),
                },
            ],
        });

        let mut encoder = self.encoder("sharpen");
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            // The clip mask's zeros widened, into `tmp`.
            pass.set_pipeline(&p.dilate);
            pass.set_bind_group(0, &planes_group, &[uniforms.push(&self.queue, &base)]);
            pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
            let soft = threshold > 0.0 && w >= 5 && h >= 5;
            if soft {
                pass.set_pipeline(&p.contrast_blend);
                pass.set_bind_group(
                    0,
                    &planes_group,
                    &[uniforms.push(&self.queue, &Params { threshold, ..base })],
                );
                pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
                // The blend's blur, one side of its kernel.
                let sigma = reference::BLEND_BLUR_SIGMA;
                let reach = (3.0 * sigma).ceil() as usize;
                let mut side: Vec<f32> = (0..=reach)
                    .map(|i| (-(i as f32 * i as f32) / (2.0 * sigma * sigma)).exp())
                    .collect();
                let norm = side[0] + 2.0 * side[1..].iter().sum::<f32>();
                for k in &mut side {
                    *k /= norm;
                }
                let mut taps = [[0f32; 4]; 4];
                for (i, k) in side.iter().enumerate() {
                    taps[i / 4][i % 4] = *k;
                }
                let blur = uniforms.push(
                    &self.queue,
                    &Params {
                        half: reach as u32,
                        taps: side.len() as u32,
                        kernel: taps,
                        ..base
                    },
                );
                pass.set_pipeline(&p.blur_rows);
                pass.set_bind_group(0, &planes_group, &[blur]);
                pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
                pass.set_pipeline(&p.blur_cols);
                pass.set_bind_group(0, &planes_group, &[blur]);
                pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
            }
            if !soft {
                // No threshold: the dilated clip mask is the blend.
                drop(pass);
                encoder.copy_texture_to_texture(
                    work.tmp.as_image_copy(),
                    work.blend.as_image_copy(),
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
                pass = encoder.begin_compute_pass(&Default::default());
            }
            pass.set_pipeline(&p.row_sum);
            pass.set_bind_group(
                0,
                &planes_group,
                &[uniforms.push(&self.queue, &Params { offset: h, ..base })],
            );
            pass.dispatch_workgroups(h, 1, 1);
            pass.set_pipeline(&p.init_sharpened);
            pass.set_bind_group(0, &planes_group, &[uniforms.push(&self.queue, &base)]);
            pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
        }
        for &(c0, b0, cols, rows) in &batches {
            let tiles = cols * rows;
            let slot = uniforms.push(
                &self.queue,
                &Params {
                    cols,
                    rows,
                    c0,
                    b0,
                    ..base
                },
            );
            encoder.clear_buffer(&work.left, 0, None);
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &tiles_group, &[slot]);
            pass.set_pipeline(&p.setup_blocks);
            pass.dispatch_workgroups(blocks, blocks, tiles);
            pass.set_pipeline(&p.fill_estimate);
            pass.dispatch_workgroups(groups(full, 16), groups(full, 16), tiles);
            for iteration in 0..options.iterations {
                pass.set_pipeline(&p.blur_ratio);
                pass.dispatch_workgroups(groups(full, 16), groups(full, 16), tiles);
                pass.set_pipeline(&p.blur_multiply);
                pass.dispatch_workgroups(groups(full, 16), groups(full, 16), tiles);
                if options.stop_early && iteration + 1 < options.iterations {
                    pass.set_pipeline(&p.check);
                    pass.dispatch_workgroups(blocks, blocks, tiles);
                }
            }
            pass.set_pipeline(&p.commit_rest);
            pass.dispatch_workgroups(tile / 16, tile / 16, tiles);
        }
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(match out {
                Output::Half(_) => &p.apply_half,
                Output::Full { .. } => &p.apply_full,
            });
            pass.set_bind_group(0, &apply_group, &[]);
            pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
        }
        self.queue.submit(Some(encoder.finish()));

        // The stats: the row sums, totaled in double as the reference
        // totals its planes.
        let sums = self.read_buffer(&work.partials, 0, u64::from(2 * h) * 4)?;
        let sums: &[f32] = bytemuck::cast_slice(&sums);
        let total = |rows: &[f32]| rows.iter().map(|&v| f64::from(v)).sum::<f64>();
        // Rounded as the reference rounds: the mean to single, then
        // the subtraction in single.
        let clipped = 1.0 - (total(&sums[..h as usize]) / n) as f32;
        let blend_mean = (total(&sums[h as usize..]) / n) as f32;
        Ok(SharpenStats {
            radius,
            threshold,
            blend_mean,
            clipped,
            iterations: options.iterations,
        })
    }

    /// The tile indices of a grid of tiles, in the reference's
    /// row-major order; a tile off the picture is infinite.
    fn tile_indices(
        &self,
        work: &Work,
        group: &wgpu::BindGroup,
        uniforms: &Uniforms,
        base: &Params,
        grid: Grid,
    ) -> Result<Vec<f32>> {
        let Grid {
            origin,
            skip,
            size,
            count,
        } = grid;
        let n = u64::from(count[0]) * u64::from(count[1]);
        if n == 0 {
            return Ok(Vec::new());
        }
        let offset = 2 * work.height;
        let mut encoder = self.encoder("sharpen");
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.sharpen.tile_index);
            pass.set_bind_group(
                0,
                group,
                &[uniforms.push(
                    &self.queue,
                    &Params {
                        origin,
                        skip,
                        tsize: size,
                        count,
                        offset,
                        ..*base
                    },
                )],
            );
            pass.dispatch_workgroups(count[0], count[1], 1);
        }
        self.queue.submit(Some(encoder.finish()));
        let bytes = self.read_buffer(&work.partials, u64::from(offset) * 4, n * 4)?;
        Ok(bytemuck::cast_slice(&bytes).to_vec())
    }

    /// The reference's `contrast_threshold` on one tile of L*, read
    /// back.
    fn threshold_of_tile(&self, work: &Work, x: u32, y: u32, size: u32) -> Result<f32> {
        let bytes = self.read_texture(&work.lstar, x, y, size, size, 4)?;
        let tile: &[f32] = bytemuck::cast_slice(&bytes);
        Ok(reference::contrast_threshold(
            tile,
            size as usize,
            0,
            0,
            size as usize,
        ))
    }

    /// RawTherapee's automatic contrast threshold, the reference's
    /// `auto_threshold` with the tile statistics made on the GPU: the
    /// flattest tile of a coarse grid if one is flat enough, else the
    /// flattest of a finer grid and of every offset around it.
    fn auto_threshold(
        &self,
        work: &Work,
        group: &wgpu::BindGroup,
        uniforms: &Uniforms,
        base: &Params,
    ) -> Result<f32> {
        let (w, h) = (work.width, work.height);
        let coarse = reference::COARSE_TILE as u32;
        let count = [w / coarse, h / coarse];
        let grid = Grid {
            origin: [0, 0],
            skip: coarse,
            size: coarse,
            count,
        };
        let indices = self.tile_indices(work, group, uniforms, base, grid)?;
        if let Some((i, index)) = flattest(&indices)
            && index <= reference::FLAT_ENOUGH
        {
            let (x, y) = (
                (i as u32 % count[0]) * coarse,
                (i as u32 / count[0]) * coarse,
            );
            return self.threshold_of_tile(work, x, y, coarse);
        }
        let fine = reference::FINE_TILE as u32;
        let skip = reference::FINE_SKIP as u32;
        let count = [(w / skip).saturating_sub(3), (h / skip).saturating_sub(3)];
        let grid = Grid {
            origin: [0, 0],
            skip,
            size: fine,
            count,
        };
        let indices = self.tile_indices(work, group, uniforms, base, grid)?;
        let Some((i, _)) = flattest(&indices) else {
            return Ok(0.0);
        };
        let (bx, by) = ((i as u32 % count[0]) * skip, (i as u32 / count[0]) * skip);
        let x0 = bx.saturating_sub(skip);
        let y0 = by.saturating_sub(skip);
        let x1 = (bx + skip).min(w.saturating_sub(fine));
        let y1 = (by + skip).min(h.saturating_sub(fine));
        let count = [x1 - x0 + 1, y1 - y0 + 1];
        let grid = Grid {
            origin: [x0, y0],
            skip: 1,
            size: fine,
            count,
        };
        let indices = self.tile_indices(work, group, uniforms, base, grid)?;
        match flattest(&indices) {
            Some((i, index)) if index <= reference::FLAT_ENOUGH_SECOND_PASS => {
                let (x, y) = (x0 + i as u32 % count[0], y0 + i as u32 / count[0]);
                self.threshold_of_tile(work, x, y, fine)
            }
            _ => Ok(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    /// The shaders parse and validate without a device, so a typo is
    /// found by the suite and not by a black viewport.
    #[test]
    fn the_shaders_parse_and_validate() {
        for (name, source) in [
            ("sharpen_planes.wgsl", include_str!("sharpen_planes.wgsl")),
            ("sharpen_tiles.wgsl", include_str!("sharpen_tiles.wgsl")),
            ("sharpen_apply.wgsl", include_str!("sharpen_apply.wgsl")),
        ] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{name}:\n{}", e.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }

    /// The uniform's fields sit where the WGSL `Params` puts them:
    /// vec2s on 8, the vec4 array on 16, and nothing shifted by a
    /// field added in one place and not the other.
    #[test]
    fn the_params_are_laid_out_as_the_shaders_read_them() {
        use super::Params;
        use std::mem::offset_of;
        assert_eq!(offset_of!(Params, size), 0);
        assert_eq!(offset_of!(Params, tile), 8);
        assert_eq!(offset_of!(Params, border), 12);
        assert_eq!(offset_of!(Params, full), 16);
        assert_eq!(offset_of!(Params, cols), 20);
        assert_eq!(offset_of!(Params, rows), 24);
        assert_eq!(offset_of!(Params, c0), 28);
        assert_eq!(offset_of!(Params, b0), 32);
        assert_eq!(offset_of!(Params, iterations), 36);
        assert_eq!(offset_of!(Params, stop_early), 40);
        assert_eq!(offset_of!(Params, half), 44);
        assert_eq!(offset_of!(Params, taps), 48);
        assert_eq!(offset_of!(Params, threshold), 52);
        assert_eq!(offset_of!(Params, clip_level), 56);
        assert_eq!(offset_of!(Params, origin), 64);
        assert_eq!(offset_of!(Params, skip), 72);
        assert_eq!(offset_of!(Params, tsize), 76);
        assert_eq!(offset_of!(Params, count), 80);
        assert_eq!(offset_of!(Params, offset), 88);
        assert_eq!(offset_of!(Params, kernel), 96);
        assert_eq!(std::mem::size_of::<Params>(), 160);
    }

    #[test]
    fn the_flattest_is_the_first_finite_minimum() {
        let inf = f32::INFINITY;
        assert_eq!(super::flattest(&[inf, 2.0, 1.0, 1.0, inf]), Some((2, 1.0)));
        assert_eq!(super::flattest(&[inf, inf]), None);
        assert_eq!(super::flattest(&[]), None);
    }
}
