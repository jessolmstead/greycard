//! The local contrast (Texture and Clarity) on the GPU: the passes of
//! `greycard_core::develop::local_contrast`, one kernel each, in the
//! reference's order, so that a Detail slider costs about what a
//! sharpen move does and the viewport shows what the export will.
//!
//! The order is the reference's: the log luminance is taken once,
//! from the picture before either band; Texture's guided filter runs
//! on it and its gain is kept in a plane; the log is then low-passed
//! in place for Clarity's band's top, and Clarity's guided filter
//! runs on that, exact at full size under the reference's
//! `COARSE_FROM_RADIUS` and on the reference's grid of blocks from it,
//! with the same block sums, the same count-weighted short blocks,
//! the same grid radius and the same bilinear taps. One last pass
//! puts both gains on the picture: Texture's first, then Clarity's,
//! whose clip fade reads the brightest channel after Texture's gain.
//!
//! The box means are separable, a pass along the rows and one down
//! the columns, each reading its window whole from workgroup memory,
//! never a summed-area table. The result agrees with the reference to
//! rounding and not to the bit: `log2` and `exp2` differ by device,
//! and the reference's running sums drift in f32 where a window read
//! whole does not. The tests hold the pictures to 5e-5 relative of
//! an f64 port of the reference, and to the reference within its own
//! drift.
//!
//! The planes are R32Float textures, only those the run's sliders
//! need: Texture's gain when Texture is on, the full-size slope and
//! intercept when Texture or Clarity's exact filter takes them, the
//! grid's when Clarity takes the grid. On a discrete GPU they are kept
//! between runs on a picture of one size, as the sharpen keeps its
//! own, and [`Context::release`] lets them go; on an integrated or
//! unified-memory GPU, which shares the machine's memory, they are
//! made for each run and let go after it. The output is an [`Image`]
//! the sharpen reads, made anew each run so that its automatic
//! threshold is found on this picture and not kept from the last; or
//! the viewport's half-float texture when the sharpen is off.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use greycard_core::develop::local_contrast::{
    self as reference, LocalContrastOptions, LocalContrastStats,
};
use greycard_core::image::WorkingImage;

use crate::plumbing::{SAMPLED, Uniforms, groups, layout_at, pipeline, storage_texture, uniform};
use crate::{Context, Error, Image, Result};

/// The shaders' uniform, laid out as WGSL lays it out.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    size: [u32; 2],
    radius: u32,
    square: u32,
    epsilon: f32,
    k: f32,
    clip_level: f32,
    has_clip: u32,
    grid: [u32; 2],
    step: u32,
    mode: u32,
    luma: [f32; 4],
    fade: [f32; 4],
    floor: f32,
    clip_fade: f32,
    _pad: [u32; 2],
}

const _: () = assert!(std::mem::size_of::<Params>() == 96);

/// The apply's modes, as the shader reads them.
const MODE_TEXTURE: u32 = 1;
const MODE_CLARITY: u32 = 2;
const MODE_COARSE: u32 = 4;

/// The widest window a box pass reads: the shader's workgroup memory
/// is sized for it. The widest a run asks for is the grid's radius,
/// 102 on a 16384-pixel frame (Texture's is 8 there, and Clarity's
/// exact filter stops under 64); the grid's passes 128 from a long
/// edge of about 41,000 pixels, past any device's texture limit. So
/// the `Unsupported` it guards cannot be reached on a picture the
/// device takes; it stays as a guard should the radii change.
const MAX_RADIUS: usize = 128;

/// The row pass's run of pixels a workgroup, and the column pass's
/// tile, the shader's.
const ROW_TILE: u32 = 256;
const COL_W: u32 = 8;
const COL_H: u32 = 32;

/// The op's pipelines, built once per device, and the working planes
/// kept between runs on a picture of the same size when `keep` is
/// set.
pub(crate) struct Pipelines {
    plane_layout: wgpu::BindGroupLayout,
    blocks_layout: wgpu::BindGroupLayout,
    coef_layout: wgpu::BindGroupLayout,
    grid_coef_layout: wgpu::BindGroupLayout,
    gain_layout: wgpu::BindGroupLayout,
    apply_layout: wgpu::BindGroupLayout,
    log_lum: wgpu::ComputePipeline,
    box_rows: wgpu::ComputePipeline,
    box_cols: wgpu::ComputePipeline,
    coefficients: wgpu::ComputePipeline,
    texture_gain: wgpu::ComputePipeline,
    block_sums: wgpu::ComputePipeline,
    grid_coefficients: wgpu::ComputePipeline,
    grid_divide: wgpu::ComputePipeline,
    apply_half: wgpu::ComputePipeline,
    apply_full: wgpu::ComputePipeline,
    /// Stand-ins for bindings a run has no plane for.
    spare_plane: wgpu::Texture,
    spare_half: wgpu::Texture,
    spare_full: wgpu::Texture,
    keep: AtomicBool,
    work: Mutex<Option<Work>>,
}

impl Pipelines {
    pub(crate) fn new(device: &wgpu::Device, keep: bool) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("local contrast"),
            source: wgpu::ShaderSource::Wgsl(include_str!("local_contrast.wgsl").into()),
        });
        use wgpu::StorageTextureAccess::{ReadWrite, WriteOnly};
        use wgpu::TextureFormat::{R32Float, Rgba16Float, Rgba32Float};
        let write = || storage_texture(R32Float, WriteOnly);
        let both = || storage_texture(R32Float, ReadWrite);
        // The bindings as the shader numbers them; each layout the
        // subset its kernels use.
        let plane_layout = layout_at(
            device,
            "local contrast plane",
            &[(0, uniform(true)), (1, SAMPLED), (2, write())],
        );
        let blocks_layout = layout_at(
            device,
            "local contrast blocks",
            &[
                (0, uniform(true)),
                (1, SAMPLED),
                (2, write()),
                (3, write()),
                (4, write()),
            ],
        );
        let coef_layout = layout_at(
            device,
            "local contrast coefficients",
            &[(0, uniform(true)), (5, both()), (6, both())],
        );
        let grid_coef_layout = layout_at(
            device,
            "local contrast grid coefficients",
            &[
                (0, uniform(true)),
                (5, both()),
                (6, both()),
                (7, SAMPLED),
                (8, SAMPLED),
            ],
        );
        let gain_layout = layout_at(
            device,
            "local contrast gain",
            &[
                (0, uniform(true)),
                (2, write()),
                (9, SAMPLED),
                (10, SAMPLED),
                (11, SAMPLED),
                (12, SAMPLED),
            ],
        );
        let apply_layout = layout_at(
            device,
            "local contrast apply",
            &[
                (0, uniform(true)),
                (9, SAMPLED),
                (10, SAMPLED),
                (11, SAMPLED),
                (12, SAMPLED),
                (13, SAMPLED),
                (14, SAMPLED),
                (15, SAMPLED),
                (16, storage_texture(Rgba16Float, WriteOnly)),
                (17, storage_texture(Rgba32Float, WriteOnly)),
            ],
        );
        let pipeline_layout = |label: &str, l: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(l)],
                ..Default::default()
            })
        };
        let plane_pl = pipeline_layout("local contrast plane", &plane_layout);
        let blocks_pl = pipeline_layout("local contrast blocks", &blocks_layout);
        let coef_pl = pipeline_layout("local contrast coefficients", &coef_layout);
        let grid_coef_pl = pipeline_layout("local contrast grid coefficients", &grid_coef_layout);
        let gain_pl = pipeline_layout("local contrast gain", &gain_layout);
        let apply_pl = pipeline_layout("local contrast apply", &apply_layout);
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
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        Self {
            log_lum: pipeline(device, &module, &plane_pl, "log_lum"),
            box_rows: pipeline(device, &module, &plane_pl, "box_rows"),
            box_cols: pipeline(device, &module, &plane_pl, "box_cols"),
            coefficients: pipeline(device, &module, &coef_pl, "coefficients"),
            texture_gain: pipeline(device, &module, &gain_pl, "texture_gain"),
            block_sums: pipeline(device, &module, &blocks_pl, "block_sums"),
            grid_coefficients: pipeline(device, &module, &grid_coef_pl, "grid_coefficients"),
            grid_divide: pipeline(device, &module, &grid_coef_pl, "grid_divide"),
            apply_half: pipeline(device, &module, &apply_pl, "apply_half"),
            apply_full: pipeline(device, &module, &apply_pl, "apply_full"),
            spare_plane: spare("local contrast spare plane", R32Float),
            spare_half: spare("local contrast spare half", Rgba16Float),
            spare_full: spare("local contrast spare full", Rgba32Float),
            plane_layout,
            blocks_layout,
            coef_layout,
            grid_coef_layout,
            gain_layout,
            apply_layout,
            keep: AtomicBool::new(keep),
            work: Mutex::new(None),
        }
    }

    /// Drop the working planes; see [`Context::release`].
    pub(crate) fn release(&self) {
        *self.work.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub(crate) fn keeps(&self) -> bool {
        self.keep.load(Ordering::Relaxed)
    }

    pub(crate) fn set_keeps(&self, keep: bool) {
        self.keep.store(keep, Ordering::Relaxed);
        if !keep {
            self.release();
        }
    }

    /// The bytes the kept planes take; see [`Context::kept_bytes`].
    pub(crate) fn kept_bytes(&self) -> u64 {
        let guard = self.work.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map_or(0, Work::bytes)
    }
}

/// The working planes for a picture of one size: the reference's
/// `log` and its scratch, and those the run's sliders need beside
/// them: the slope and intercept (`a`, `b`) for Texture or Clarity's
/// exact filter, Texture's gain, and the coarse grid's planes when
/// Clarity takes the grid at this size.
struct Work {
    width: u32,
    height: u32,
    log: wgpu::Texture,
    tmp: wgpu::Texture,
    a: Option<wgpu::Texture>,
    b: Option<wgpu::Texture>,
    gain: Option<wgpu::Texture>,
    grid: Option<Grid>,
}

/// The coarse grid's planes: the block sums of the log and its
/// square, each block's share of a full block, a window's share
/// (`held`), and a scratch for the box passes.
struct Grid {
    width: u32,
    height: u32,
    step: u32,
    radius: u32,
    mean: wgpu::Texture,
    sq: wgpu::Texture,
    share: wgpu::Texture,
    held: wgpu::Texture,
    tmp: wgpu::Texture,
}

/// Which of the optional planes a run needs.
#[derive(Clone, Copy)]
struct Needs {
    /// The full-size slope and intercept.
    ab: bool,
    gain: bool,
    grid: bool,
}

fn plane(device: &wgpu::Device, label: &str, w: u32, h: u32) -> wgpu::Texture {
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
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

impl Grid {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Option<Self> {
        let (_, clarity_radius) = reference::radii(width as usize, height as usize);
        let step = reference::clarity_step(clarity_radius);
        (step > 1).then(|| {
            let radius = reference::coarse_radius(clarity_radius, step) as u32;
            let step = step as u32;
            let (gw, gh) = (width.div_ceil(step), height.div_ceil(step));
            Grid {
                width: gw,
                height: gh,
                step,
                radius,
                mean: plane(device, "local contrast grid mean", gw, gh),
                sq: plane(device, "local contrast grid squares", gw, gh),
                share: plane(device, "local contrast grid share", gw, gh),
                held: plane(device, "local contrast grid held", gw, gh),
                tmp: plane(device, "local contrast grid scratch", gw, gh),
            }
        })
    }
}

impl Work {
    fn new(device: &wgpu::Device, width: u32, height: u32, needs: Needs) -> Self {
        let mut work = Self {
            width,
            height,
            log: plane(device, "local contrast log", width, height),
            tmp: plane(device, "local contrast scratch", width, height),
            a: None,
            b: None,
            gain: None,
            grid: None,
        };
        work.fit(device, needs);
        work
    }

    /// Make the planes `needs` asks for that are missing, and let go
    /// of those it does not.
    fn fit(&mut self, device: &wgpu::Device, needs: Needs) {
        let (w, h) = (self.width, self.height);
        let want = |slot: &mut Option<wgpu::Texture>, on: bool, label: &str| {
            if !on {
                *slot = None;
            } else if slot.is_none() {
                *slot = Some(plane(device, label, w, h));
            }
        };
        want(&mut self.a, needs.ab, "local contrast a");
        want(&mut self.b, needs.ab, "local contrast b");
        want(&mut self.gain, needs.gain, "local contrast texture gain");
        if !needs.grid {
            self.grid = None;
        } else if self.grid.is_none() {
            self.grid = Grid::new(device, w, h);
        }
    }

    fn bytes(&self) -> u64 {
        use crate::plumbing::texture_bytes;
        let grid = self.grid.as_ref().map_or(0, |g| {
            [&g.mean, &g.sq, &g.share, &g.held, &g.tmp]
                .into_iter()
                .map(texture_bytes)
                .sum()
        });
        [
            Some(&self.log),
            Some(&self.tmp),
            self.a.as_ref(),
            self.b.as_ref(),
            self.gain.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(texture_bytes)
        .sum::<u64>()
            + grid
    }
}

/// Where the result goes: the viewport's half-float texture, or full
/// floats for the sharpen or a read back.
enum Output<'a> {
    Half(&'a wgpu::Texture),
    Full(&'a wgpu::Texture),
}

/// The box passes a run makes, each a pipeline and a bind group of
/// its source and destination, dispatched on a plane of one size.
struct Passes<'a> {
    ctx: &'a Context,
    uniforms: &'a Uniforms,
    base: Params,
}

impl Passes<'_> {
    fn group(&self, src: &wgpu::Texture, dst: &wgpu::Texture) -> wgpu::BindGroup {
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        self.ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("local contrast plane"),
                layout: &self.ctx.local_contrast.plane_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view(src)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&view(dst)),
                    },
                ],
            })
    }

    /// The box mean of `src` into `dst` by way of `tmp`, on a plane
    /// of `size`; `square` sums the squares.
    #[allow(clippy::too_many_arguments)]
    fn box_mean(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        src: &wgpu::Texture,
        tmp: &wgpu::Texture,
        dst: &wgpu::Texture,
        size: [u32; 2],
        radius: u32,
        square: bool,
    ) {
        let p = &self.ctx.local_contrast;
        let rows = self.uniforms.push(
            &self.ctx.queue,
            &Params {
                size,
                radius,
                square: square as u32,
                ..self.base
            },
        );
        let cols = self.uniforms.push(
            &self.ctx.queue,
            &Params {
                size,
                radius,
                square: 0,
                ..self.base
            },
        );
        pass.set_pipeline(&p.box_rows);
        pass.set_bind_group(0, &self.group(src, tmp), &[rows]);
        pass.dispatch_workgroups(groups(size[0], ROW_TILE), size[1], 1);
        pass.set_pipeline(&p.box_cols);
        pass.set_bind_group(0, &self.group(tmp, dst), &[cols]);
        pass.dispatch_workgroups(groups(size[0], COL_W), groups(size[1], COL_H), 1);
    }

    /// The reference's `guided_filter` of `input` by itself at full
    /// size: the averaged intercept left in `a` and the averaged slope
    /// in `b`, for the base `a + b * input` to be taken where it is
    /// read.
    #[allow(clippy::too_many_arguments)]
    fn guided_filter(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        input: &wgpu::Texture,
        work: &Work,
        size: [u32; 2],
        radius: u32,
        epsilon: f32,
    ) {
        let p = &self.ctx.local_contrast;
        let (a, b) = match (work.a.as_ref(), work.b.as_ref()) {
            (Some(a), Some(b)) => (a, b),
            _ => unreachable!("the run's needs made the slope and intercept"),
        };
        // a: the mean; b: the mean of the squares.
        self.box_mean(pass, input, &work.tmp, a, size, radius, false);
        self.box_mean(pass, input, &work.tmp, b, size, radius, true);
        // b: the slope; a: the intercept.
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let coef = self
            .ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("local contrast coefficients"),
                layout: &p.coef_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&view(a)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&view(b)),
                    },
                ],
            });
        pass.set_pipeline(&p.coefficients);
        pass.set_bind_group(
            0,
            &coef,
            &[self.uniforms.push(
                &self.ctx.queue,
                &Params {
                    size,
                    epsilon,
                    ..self.base
                },
            )],
        );
        pass.dispatch_workgroups(groups(size[0], 16), groups(size[1], 16), 1);
        // Each averaged over the windows.
        self.box_mean(pass, b, &work.tmp, b, size, radius, false);
        self.box_mean(pass, a, &work.tmp, a, size, radius, false);
    }

    /// The reference's `coarse_guided_filter`'s stages on the grid:
    /// the averaged slope left in `grid.sq` and the averaged intercept
    /// in `grid.mean`, over the pixels their windows hold, for the
    /// apply to take back to full size.
    fn coarse_guided_filter(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        input: &wgpu::Texture,
        grid: &Grid,
        full: [u32; 2],
        epsilon: f32,
    ) {
        let p = &self.ctx.local_contrast;
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let size = [grid.width, grid.height];
        let blocks = self
            .ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("local contrast blocks"),
                layout: &p.blocks_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view(input)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&view(&grid.mean)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&view(&grid.sq)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&view(&grid.share)),
                    },
                ],
            });
        pass.set_pipeline(&p.block_sums);
        pass.set_bind_group(
            0,
            &blocks,
            &[self.uniforms.push(
                &self.ctx.queue,
                &Params {
                    size: full,
                    grid: size,
                    step: grid.step,
                    ..self.base
                },
            )],
        );
        pass.dispatch_workgroups(groups(size[0], 16), groups(size[1], 16), 1);
        // A window's pixels as a share of its blocks, then the
        // filter's first stages on the grid.
        let r = grid.radius;
        self.box_mean(pass, &grid.share, &grid.tmp, &grid.held, size, r, false);
        self.box_mean(pass, &grid.mean, &grid.tmp, &grid.mean, size, r, false);
        self.box_mean(pass, &grid.sq, &grid.tmp, &grid.sq, size, r, false);
        let coef = self
            .ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("local contrast grid coefficients"),
                layout: &p.grid_coef_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniforms.binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&view(&grid.mean)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&view(&grid.sq)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&view(&grid.held)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&view(&grid.share)),
                    },
                ],
            });
        let slot = self.uniforms.push(
            &self.ctx.queue,
            &Params {
                size,
                epsilon,
                ..self.base
            },
        );
        pass.set_pipeline(&p.grid_coefficients);
        pass.set_bind_group(0, &coef, &[slot]);
        pass.dispatch_workgroups(groups(size[0], 16), groups(size[1], 16), 1);
        self.box_mean(pass, &grid.sq, &grid.tmp, &grid.sq, size, r, false);
        self.box_mean(pass, &grid.mean, &grid.tmp, &grid.mean, size, r, false);
        pass.set_pipeline(&p.grid_divide);
        pass.set_bind_group(0, &coef, &[slot]);
        pass.dispatch_workgroups(groups(size[0], 16), groups(size[1], 16), 1);
    }
}

impl Context {
    /// [`greycard_core::develop::local_contrast::local_contrast`] on a
    /// picture already on the GPU: the result a new [`Image`] for the
    /// sharpen to read, in full floats, with its own threshold to find.
    /// The stats come back as the reference gives them.
    pub fn local_contrast(
        &self,
        image: &Image,
        options: &LocalContrastOptions,
        clip_level: f32,
    ) -> Result<(Image, LocalContrastStats)> {
        let (w, h) = (image.width, image.height);
        crate::scoped(&self.device, "the local contrast", || {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("after the local contrast, full floats"),
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
            let stats =
                self.local_contrast_into(image, options, clip_level, Output::Full(&texture))?;
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
    pub fn local_contrast_to_viewport(
        &self,
        image: &Image,
        options: &LocalContrastOptions,
        clip_level: f32,
        out: &wgpu::Texture,
    ) -> Result<LocalContrastStats> {
        if out.width() != image.width || out.height() != image.height {
            return Err(Error::Unsupported(format!(
                "a {}x{} output for a {}x{} picture",
                out.width(),
                out.height(),
                image.width,
                image.height
            )));
        }
        crate::scoped(&self.device, "the local contrast", || {
            self.local_contrast_into(image, options, clip_level, Output::Half(out))
        })
    }

    /// The reference's own signature: the local contrast on a working
    /// image in place. Uploads, runs and reads back, so it is for the
    /// CLI and the tests, not a slider.
    pub fn local_contrast_image(
        &self,
        image: &mut WorkingImage,
        options: &LocalContrastOptions,
        clip_level: f32,
    ) -> Result<LocalContrastStats> {
        let uploaded = self.upload(image)?;
        let (out, stats) = self.local_contrast(&uploaded, options, clip_level)?;
        let (w, h) = (out.width, out.height);
        let rgba = self.read_texture(&out.texture, 0, 0, w, h, 16)?;
        let rgba: &[f32] = bytemuck::cast_slice(&rgba);
        let (pixels, _) = image.data.as_chunks_mut::<3>();
        let (fours, _) = rgba.as_chunks::<4>();
        for (px, four) in pixels.iter_mut().zip(fours) {
            px.copy_from_slice(&four[..3]);
        }
        Ok(stats)
    }

    fn local_contrast_into(
        &self,
        image: &Image,
        options: &LocalContrastOptions,
        clip_level: f32,
        out: Output<'_>,
    ) -> Result<LocalContrastStats> {
        let p = &self.local_contrast;
        let (w, h) = (image.width, image.height);
        let (texture_radius, clarity_radius) = reference::radii(w as usize, h as usize);
        let stats = LocalContrastStats {
            texture_radius,
            clarity_radius,
        };
        let step = reference::clarity_step(clarity_radius);
        let widest = if step > 1 {
            texture_radius.max(reference::coarse_radius(clarity_radius, step))
        } else {
            texture_radius.max(clarity_radius)
        };
        // Not reached on a picture any device takes; see MAX_RADIUS.
        if widest > MAX_RADIUS {
            return Err(Error::Unsupported(format!(
                "a box radius of {widest} pixels; the shader takes up to {MAX_RADIUS}"
            )));
        }
        let texture_on = options.texture != 0.0;
        let clarity_on = options.clarity != 0.0;
        let needs = Needs {
            ab: texture_on || (clarity_on && step == 1),
            gain: texture_on,
            grid: clarity_on && step > 1,
        };

        // The planes this run needs: on a discrete GPU those kept from
        // the last run, made up to this run's needs; else made for this
        // run alone, and let go once it is submitted (the device keeps
        // them alive until it has run).
        let mut guard = p.work.lock().unwrap_or_else(|e| e.into_inner());
        let mut own = None;
        let work: &Work = if p.keeps() {
            match guard.as_mut() {
                Some(work) if work.width == w && work.height == h => work.fit(&self.device, needs),
                _ => {
                    *guard = None;
                    *guard = Some(Work::new(&self.device, w, h, needs));
                }
            }
            guard.as_ref().expect("made, or kept")
        } else {
            *guard = None;
            own.insert(Work::new(&self.device, w, h, needs))
        };

        let (shadow, highlight) = (
            reference::CLARITY_SHADOW_FADE,
            reference::CLARITY_HIGHLIGHT_FADE,
        );
        let base = Params {
            size: [w, h],
            clip_level,
            has_clip: clip_level.is_finite() as u32,
            luma: [
                reference::LUMA[0],
                reference::LUMA[1],
                reference::LUMA[2],
                reference::MID_GREY,
            ],
            fade: [shadow.0, shadow.1, highlight.0, highlight.1],
            floor: reference::FLOOR,
            clip_fade: reference::CLIP_FADE,
            ..Default::default()
        };
        let uniforms = Uniforms::new(&self.device, "local contrast params", 48);
        let passes = Passes {
            ctx: self,
            uniforms: &uniforms,
            base,
        };
        let size = [w, h];
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let or_spare = |t: Option<&'_ wgpu::Texture>| view(t.unwrap_or(&p.spare_plane));

        let mut encoder = self.encoder("local contrast");
        let mut mode = 0;
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            // The log luminance, once, from the picture before either
            // band.
            pass.set_pipeline(&p.log_lum);
            pass.set_bind_group(
                0,
                &passes.group(&image.texture, &work.log),
                &[uniforms.push(&self.queue, &base)],
            );
            pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
            if texture_on {
                mode |= MODE_TEXTURE;
                let k = options.texture.clamp(-1.0, 1.0) * reference::TEXTURE_GAIN;
                passes.guided_filter(
                    &mut pass,
                    &work.log,
                    work,
                    size,
                    texture_radius as u32,
                    reference::TEXTURE_EPSILON,
                );
                let gain = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("local contrast gain"),
                    layout: &p.gain_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniforms.binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&or_spare(
                                work.gain.as_ref(),
                            )),
                        },
                        wgpu::BindGroupEntry {
                            binding: 9,
                            resource: wgpu::BindingResource::TextureView(&view(&image.texture)),
                        },
                        wgpu::BindGroupEntry {
                            binding: 10,
                            resource: wgpu::BindingResource::TextureView(&view(&work.log)),
                        },
                        wgpu::BindGroupEntry {
                            binding: 11,
                            resource: wgpu::BindingResource::TextureView(&or_spare(
                                work.a.as_ref(),
                            )),
                        },
                        wgpu::BindGroupEntry {
                            binding: 12,
                            resource: wgpu::BindingResource::TextureView(&or_spare(
                                work.b.as_ref(),
                            )),
                        },
                    ],
                });
                pass.set_pipeline(&p.texture_gain);
                pass.set_bind_group(
                    0,
                    &gain,
                    &[uniforms.push(&self.queue, &Params { k, ..base })],
                );
                pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
            }
            if clarity_on {
                mode |= MODE_CLARITY;
                // The log is done with: low-passed in place, it is the
                // top of Clarity's band, and the guided filter of it
                // the bottom.
                for _ in 0..2 {
                    passes.box_mean(
                        &mut pass,
                        &work.log,
                        &work.tmp,
                        &work.log,
                        size,
                        texture_radius as u32,
                        false,
                    );
                }
                match work.grid.as_ref() {
                    Some(grid) => {
                        mode |= MODE_COARSE;
                        passes.coarse_guided_filter(
                            &mut pass,
                            &work.log,
                            grid,
                            size,
                            reference::CLARITY_EPSILON,
                        );
                    }
                    None => passes.guided_filter(
                        &mut pass,
                        &work.log,
                        work,
                        size,
                        clarity_radius as u32,
                        reference::CLARITY_EPSILON,
                    ),
                }
            }
            // Both gains onto the picture.
            let k = options.clarity.clamp(-1.0, 1.0) * reference::CLARITY_GAIN;
            let (out_half, out_full) = match out {
                Output::Half(t) => (t, &p.spare_full),
                Output::Full(t) => (&p.spare_half, t),
            };
            let (grid_intercept, grid_slope, step) = match work.grid.as_ref() {
                Some(g) => (&g.mean, &g.sq, g.step),
                None => (&p.spare_plane, &p.spare_plane, 1),
            };
            let apply = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("local contrast apply"),
                layout: &p.apply_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::TextureView(&view(&image.texture)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: wgpu::BindingResource::TextureView(&view(&work.log)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: wgpu::BindingResource::TextureView(&or_spare(work.a.as_ref())),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: wgpu::BindingResource::TextureView(&or_spare(work.b.as_ref())),
                    },
                    wgpu::BindGroupEntry {
                        binding: 13,
                        resource: wgpu::BindingResource::TextureView(&or_spare(work.gain.as_ref())),
                    },
                    wgpu::BindGroupEntry {
                        binding: 14,
                        resource: wgpu::BindingResource::TextureView(&view(grid_intercept)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 15,
                        resource: wgpu::BindingResource::TextureView(&view(grid_slope)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 16,
                        resource: wgpu::BindingResource::TextureView(&view(out_half)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 17,
                        resource: wgpu::BindingResource::TextureView(&view(out_full)),
                    },
                ],
            });
            pass.set_pipeline(match out {
                Output::Half(_) => &p.apply_half,
                Output::Full(_) => &p.apply_full,
            });
            pass.set_bind_group(
                0,
                &apply,
                &[uniforms.push(
                    &self.queue,
                    &Params {
                        k,
                        mode,
                        step,
                        ..base
                    },
                )],
            );
            pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
        }
        self.queue.submit(Some(encoder.finish()));
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    /// The shader parses and validates without a device, so a typo is
    /// found by the suite and not by a black viewport.
    #[test]
    fn the_shader_parses_and_validates() {
        let source = include_str!("local_contrast.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("local_contrast.wgsl:\n{}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("local_contrast.wgsl: {e:?}"));
    }

    /// The uniform's fields sit where the WGSL `Params` puts them.
    #[test]
    fn the_params_are_laid_out_as_the_shader_reads_them() {
        use super::Params;
        use std::mem::offset_of;
        assert_eq!(offset_of!(Params, size), 0);
        assert_eq!(offset_of!(Params, radius), 8);
        assert_eq!(offset_of!(Params, square), 12);
        assert_eq!(offset_of!(Params, epsilon), 16);
        assert_eq!(offset_of!(Params, k), 20);
        assert_eq!(offset_of!(Params, clip_level), 24);
        assert_eq!(offset_of!(Params, has_clip), 28);
        assert_eq!(offset_of!(Params, grid), 32);
        assert_eq!(offset_of!(Params, step), 40);
        assert_eq!(offset_of!(Params, mode), 44);
        assert_eq!(offset_of!(Params, luma), 48);
        assert_eq!(offset_of!(Params, fade), 64);
        assert_eq!(offset_of!(Params, floor), 80);
        assert_eq!(offset_of!(Params, clip_fade), 84);
        assert_eq!(std::mem::size_of::<Params>(), 96);
    }

    /// The shader's workgroup memory holds the widest window the host
    /// lets through.
    #[test]
    fn the_tiles_hold_the_widest_window() {
        use super::{COL_H, COL_W, MAX_RADIUS, ROW_TILE};
        let r = MAX_RADIUS as u32;
        assert!(ROW_TILE + 2 * r <= 512);
        assert!(COL_W * (COL_H + 2 * r) <= 2304);
    }
}
