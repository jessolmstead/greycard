//! The lateral chromatic aberration correction on the GPU: the same
//! measure, fit, resample and color-shift guard as
//! `greycard_core::develop::ca`, with the decisions on the CPU. That
//! reference, and so this, is a port of RawTherapee's
//! `rtengine/CA_correct_RT.cc` (copyright 2008-2010 Emil Martinec;
//! 2018 Ingo Weyrich for the iterated correction and the color-shift
//! avoidance; GPL-3.0-or-later); the algorithm and its constants are
//! theirs.
//!
//! One shader, `ca.wgsl`, holds the data-parallel stages: the green
//! interpolated at every red and blue site, the tiles' quadratic-fit
//! sums (a workgroup a tile, the sums in the reference's sequential
//! order), the resample by the fitted shifts (a thread a pixel), and
//! the guard's factors, its six box-blur passes (a thread a line,
//! the reference's running sum) and its multiply. Between the sums
//! and the resample the reference's own `fit_votes` runs on the CPU
//! from a read back of twelve floats a tile: the 3x3 median, the
//! variance gate, the polynomial fit in double and the solve, so the
//! tiles that vote and the polynomial they give are decided by the
//! reference's code on numbers that differ from the reference's only
//! by the GPU's rounding of the sums' terms.
//!
//! No atlas. The reference works in tiles of 128 with a border of 8
//! it computes and discards, but every value a tile's interior reads
//! is determined by the picture alone (the reflected mosaic, and the
//! interpolated green within four pixels of the interior, which the
//! border covers), so the green is one plane padded by the border,
//! and the tiles' interiors are the picture cut into 112-pixel
//! blocks. The mosaic and its two corrected versions are `R32Float`
//! planes, the padded green another, the factors and their scratch
//! half-size planes: 812 MiB at 45 MP, made for the run and dropped
//! at its end. The op runs once per base develop and uploads its
//! mosaic each time, so there is nothing to keep between runs, and
//! the sharpen's working set, which a slider re-reads, is held
//! instead.
//!
//! Where rounding could show. The green, the filters, the terms of
//! the sums, the resample and the factors take the reference's
//! operations in the reference's order, so they differ from it by
//! the GPU compiler's freedom to fuse a multiply and an add. The
//! sums add their terms in the reference's order, each term rounded
//! to single before the add as the reference rounds it. The blur is
//! the reference's running sum, in its order, with no multiply to
//! fuse. A last bit in a sum could move a tile across the variance
//! gate, and a last bit in a resampled value could flip one of the
//! reference's per-pixel guards (which sample wins, whether the
//! correction overshot), and there the difference is a step and not
//! a rounding; the tests hold the decisions to equality and count
//! the pixels that step.

use greycard_core::develop::ca::{
    self as reference, BlockVote, CaOptions, CaStats, Coefficients, Resample,
};
use greycard_core::raw::{CfaColor, CfaPattern};

use crate::plumbing::{
    SAMPLED, STORAGE_BUFFER, STORAGE_BUFFER_READ, Uniforms, groups, layout_at, pipeline,
    storage_texture, uniform,
};
use crate::{Context, Error, Result};

/// The shader's uniform, laid out as WGSL lays it out.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    size: [u32; 2],
    half: [u32; 2],
    tiles: [u32; 2],
    radius: u32,
    norm: f32,
    colors: [u32; 4],
    plane: u32,
    from_tmp: u32,
    axis: u32,
    _pad: u32,
}

const _: () = assert!(std::mem::size_of::<Params>() == 64);

/// One tile's resample parameters as the shader reads them: the
/// reference's [`Resample`], per color.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct TileResample {
    vfloor: [i32; 2],
    vceil: [i32; 2],
    hfloor: [i32; 2],
    hceil: [i32; 2],
    vfrac: [f32; 2],
    hfrac: [f32; 2],
    dir_v: [i32; 2],
    dir_h: [i32; 2],
}

const _: () = assert!(std::mem::size_of::<TileResample>() == 64);

impl From<Resample> for TileResample {
    fn from(r: Resample) -> Self {
        let i = |a: [isize; 2]| a.map(|v| v as i32);
        Self {
            vfloor: i(r.vfloor),
            vceil: i(r.vceil),
            hfloor: i(r.hfloor),
            hceil: i(r.hceil),
            vfrac: r.vfrac,
            hfrac: r.hfrac,
            dir_v: i(r.dir_v),
            dir_h: i(r.dir_h),
        }
    }
}

const BORDER: u32 = reference::BORDER as u32;

/// The op's pipelines, built once per device.
pub(crate) struct Pipelines {
    /// The measure and resample kernels' bindings, and the guard's:
    /// a device's default allows four storage textures a stage, and
    /// the two sets together are five.
    pass_layout: wgpu::BindGroupLayout,
    guard_layout: wgpu::BindGroupLayout,
    interpolate_green: wgpu::ComputePipeline,
    vote: wgpu::ComputePipeline,
    correct: wgpu::ComputePipeline,
    factors: wgpu::ComputePipeline,
    blur_lines: wgpu::ComputePipeline,
    apply_factors: wgpu::ComputePipeline,
}

impl Pipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ca"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ca.wgsl").into()),
        });
        let plane = storage_texture(
            wgpu::TextureFormat::R32Float,
            wgpu::StorageTextureAccess::ReadWrite,
        );
        let pass_layout = layout_at(
            device,
            "ca pass",
            &[
                (0, uniform(true)),
                (2, SAMPLED),
                (3, plane),
                (4, plane),
                (8, STORAGE_BUFFER),
                (9, STORAGE_BUFFER_READ),
            ],
        );
        let guard_layout = layout_at(
            device,
            "ca guard",
            &[
                (0, uniform(true)),
                (1, SAMPLED),
                (3, plane),
                (5, plane),
                (6, plane),
                (7, plane),
            ],
        );
        let pipeline_layout = |label: &str, l: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(l)],
                ..Default::default()
            })
        };
        let pass_pl = pipeline_layout("ca pass", &pass_layout);
        let guard_pl = pipeline_layout("ca guard", &guard_layout);
        Self {
            interpolate_green: pipeline(device, &module, &pass_pl, "interpolate_green"),
            vote: pipeline(device, &module, &pass_pl, "vote"),
            correct: pipeline(device, &module, &pass_pl, "correct"),
            factors: pipeline(device, &module, &guard_pl, "factors"),
            blur_lines: pipeline(device, &module, &guard_pl, "blur_lines"),
            apply_factors: pipeline(device, &module, &guard_pl, "apply_factors"),
            pass_layout,
            guard_layout,
        }
    }
}

/// The working textures and buffers for one run on a mosaic.
struct Work {
    width: u32,
    height: u32,
    /// The mosaic as uploaded.
    original: wgpu::Texture,
    /// The passes' results, alternating.
    planes: [wgpu::Texture; 2],
    /// Green at every site, padded by the border.
    green: wgpu::Texture,
    /// The color-shift factors, red and blue, at half size.
    factor: [wgpu::Texture; 2],
    /// The blur's scratch, at half size.
    tmp: wgpu::Texture,
    /// Twelve sums a tile.
    sums: wgpu::Buffer,
    /// A [`TileResample`] a tile.
    resample: wgpu::Buffer,
    /// Tiles across and down.
    tiles: [u32; 2],
}

impl Work {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let plane = |label: &str, w: u32, h: u32, usage: wgpu::TextureUsages| {
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
                usage,
                view_formats: &[],
            })
        };
        use wgpu::TextureUsages as U;
        let (hw, hh) = (width.div_ceil(2), height.div_ceil(2));
        let tiles = [
            reference::origins(width as usize).len() as u32,
            reference::origins(height as usize).len() as u32,
        ];
        let n = u64::from(tiles[0]) * u64::from(tiles[1]);
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4),
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            width,
            height,
            original: plane("ca mosaic", width, height, U::TEXTURE_BINDING | U::COPY_DST),
            planes: [
                plane(
                    "ca corrected a",
                    width,
                    height,
                    U::STORAGE_BINDING | U::TEXTURE_BINDING | U::COPY_SRC,
                ),
                plane(
                    "ca corrected b",
                    width,
                    height,
                    U::STORAGE_BINDING | U::TEXTURE_BINDING | U::COPY_SRC,
                ),
            ],
            green: plane(
                "ca green",
                width + 2 * BORDER,
                height + 2 * BORDER,
                U::STORAGE_BINDING,
            ),
            factor: [
                plane("ca factor red", hw, hh, U::STORAGE_BINDING),
                plane("ca factor blue", hw, hh, U::STORAGE_BINDING),
            ],
            tmp: plane("ca blur scratch", hw, hh, U::STORAGE_BINDING),
            sums: buffer(
                "ca sums",
                n * 12 * 4,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            resample: buffer(
                "ca resample",
                n * std::mem::size_of::<TileResample>() as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            tiles,
        }
    }
}

fn entry(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(v),
    }
}

/// The sum layout the shader writes: `[direction][term][color]`.
fn coefficients_of(sums: &[f32]) -> Coefficients {
    let mut coeff = Coefficients::default();
    for (dir, terms) in coeff.iter_mut().enumerate() {
        for (term, colors) in terms.iter_mut().enumerate() {
            for (color, v) in colors.iter_mut().enumerate() {
                *v = sums[(dir * 3 + term) * 2 + color];
            }
        }
    }
    coeff
}

impl Context {
    /// [`greycard_core::develop::ca::correct_ca`] on the GPU: the same
    /// signature and contract, the mosaic uploaded, corrected and read
    /// back. The decisions (which tiles vote, the polynomial) are the
    /// reference's own functions on the sums read back; see the
    /// module doc for what can differ.
    pub fn correct_ca(
        &self,
        samples: &[f32],
        width: usize,
        height: usize,
        pattern: &CfaPattern,
        options: &CaOptions,
    ) -> Result<(Vec<f32>, CaStats)> {
        if !reference::is_bayer(pattern) {
            return Err(Error::Unsupported(format!(
                "CA correction needs a 2x2 Bayer pattern, got {pattern}"
            )));
        }
        if samples.len() != width * height {
            return Err(Error::Unsupported(format!(
                "{width}x{height} mosaic with {} samples",
                samples.len()
            )));
        }
        if width < reference::MIN_SIDE || height < reference::MIN_SIDE {
            return Ok((samples.to_vec(), CaStats::default()));
        }
        let (w, h) = self.ca_fits(width, height)?;
        crate::scoped(&self.device, "correcting chromatic aberration", || {
            self.correct_ca_unscoped(samples, w, h, pattern, options)
        })
    }

    /// The reference's `measure_votes` on the GPU: the first pass's
    /// votes, one a tile in row-major order, for a check tile by tile.
    pub fn ca_votes(
        &self,
        samples: &[f32],
        width: usize,
        height: usize,
        pattern: &CfaPattern,
    ) -> Result<Vec<BlockVote>> {
        if !reference::is_bayer(pattern) {
            return Err(Error::Unsupported(format!(
                "CA correction needs a 2x2 Bayer pattern, got {pattern}"
            )));
        }
        if samples.len() != width * height {
            return Err(Error::Unsupported(format!(
                "{width}x{height} mosaic with {} samples",
                samples.len()
            )));
        }
        let (w, h) = self.ca_fits(width, height)?;
        crate::scoped(&self.device, "measuring chromatic aberration", || {
            let work = Work::new(&self.device, w, h);
            self.upload_mosaic(&work.original, samples, w, h);
            let uniforms = Uniforms::new(&self.device, "ca params", 1);
            let base = self.ca_params(&work, pattern);
            let bind = self.ca_pass_group(&work, &uniforms, &work.original, &work.planes[0]);
            self.measure(&work, &bind, &uniforms, &base)
        })
    }

    /// [`Context::fits`] for the mosaic and for its green plane, which
    /// is padded by the border on every side.
    fn ca_fits(&self, width: usize, height: usize) -> Result<(u32, u32)> {
        let (w, h) = self.fits(width, height)?;
        self.fits(
            width + 2 * reference::BORDER,
            height + 2 * reference::BORDER,
        )?;
        Ok((w, h))
    }

    /// The uniform every kernel reads: the sizes, the tile grid, the
    /// blur and the pattern's colors.
    fn ca_params(&self, work: &Work, pattern: &CfaPattern) -> Params {
        let radius = reference::box_radius(reference::SHIFT_BLUR_SIGMA);
        let color = |r: usize, c: usize| match pattern.color_at(r, c) {
            CfaColor::Red => 0u32,
            CfaColor::Green => 1,
            _ => 2,
        };
        Params {
            size: [work.width, work.height],
            half: [work.width.div_ceil(2), work.height.div_ceil(2)],
            tiles: work.tiles,
            radius: radius as u32,
            norm: 1.0 / (2 * radius + 1) as f32,
            colors: [color(0, 0), color(0, 1), color(1, 0), color(1, 1)],
            ..Default::default()
        }
    }

    /// The measure and resample kernels' bindings: the mosaic a pass
    /// reads and the plane it writes.
    fn ca_pass_group(
        &self,
        work: &Work,
        uniforms: &Uniforms,
        cur: &wgpu::Texture,
        next: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let views = [view(cur), view(next), view(&work.green)];
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ca pass"),
            layout: &self.ca.pass_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.binding(),
                },
                entry(2, &views[0]),
                entry(3, &views[1]),
                entry(4, &views[2]),
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: work.sums.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: work.resample.as_entire_binding(),
                },
            ],
        })
    }

    /// The green and the tiles' sums on the mosaic `bind` reads, and
    /// the votes from them read back.
    fn measure(
        &self,
        work: &Work,
        bind: &wgpu::BindGroup,
        uniforms: &Uniforms,
        base: &Params,
    ) -> Result<Vec<BlockVote>> {
        let (w, h) = (work.width, work.height);
        let tiles = work.tiles;
        let n_tiles = (tiles[0] * tiles[1]) as usize;
        let mut encoder = self.encoder("ca");
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, bind, &[uniforms.push(&self.queue, base)]);
            pass.set_pipeline(&self.ca.interpolate_green);
            pass.dispatch_workgroups(groups(w + 2 * BORDER, 16), groups(h + 2 * BORDER, 16), 1);
            pass.set_pipeline(&self.ca.vote);
            pass.dispatch_workgroups(tiles[0], tiles[1], 1);
        }
        self.queue.submit(Some(encoder.finish()));
        let sums = self.read_buffer(&work.sums, 0, (n_tiles * 12 * 4) as u64)?;
        let sums: &[f32] = bytemuck::cast_slice(&sums);
        Ok(sums
            .as_chunks::<12>()
            .0
            .iter()
            .map(|s| reference::vote_of(coefficients_of(s)))
            .collect())
    }

    fn correct_ca_unscoped(
        &self,
        samples: &[f32],
        w: u32,
        h: u32,
        pattern: &CfaPattern,
        options: &CaOptions,
    ) -> Result<(Vec<f32>, CaStats)> {
        let p = &self.ca;
        let work = &Work::new(&self.device, w, h);
        self.upload_mosaic(&work.original, samples, w, h);

        let base = self.ca_params(work, pattern);
        let iterations = options.iterations.max(1);
        let uniforms = Uniforms::new(&self.device, "ca params", 16 + 2 * iterations as u64);
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        // The guard's bindings: the upload, the corrected plane it
        // multiplies in place, the factors and their scratch.
        let guard_group = |corrected: &wgpu::Texture| {
            let views = [
                view(&work.original),
                view(corrected),
                view(&work.factor[0]),
                view(&work.factor[1]),
                view(&work.tmp),
            ];
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ca guard"),
                layout: &p.guard_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.binding(),
                    },
                    entry(1, &views[0]),
                    entry(3, &views[1]),
                    entry(5, &views[2]),
                    entry(6, &views[3]),
                    entry(7, &views[4]),
                ],
            })
        };
        let tiles = work.tiles;
        let n_tiles = (tiles[0] * tiles[1]) as usize;

        // The passes: measure on the GPU, decide on the CPU, resample
        // on the GPU; the mosaic alternates between the two planes.
        let mut stats = CaStats::default();
        let mut current: Option<usize> = None;
        for it in 0..iterations {
            let cur = current.map_or(&work.original, |i| &work.planes[i]);
            let next_i = current.map_or(0, |i| 1 - i);
            let bind = self.ca_pass_group(work, &uniforms, cur, &work.planes[next_i]);
            let votes = self.measure(work, &bind, &uniforms, &base)?;
            let fit = match reference::fit_votes(&votes, tiles[1] as usize, tiles[0] as usize) {
                Ok(fit) => fit,
                Err(no) => {
                    stats.corrected = false;
                    stats.blocks = no.blocks();
                    stats.order = 0;
                    break;
                }
            };
            let params: Vec<TileResample> = (0..n_tiles)
                .map(|i| {
                    let (tv, th) = (i / tiles[0] as usize, i % tiles[0] as usize);
                    reference::resample_for(fit.shift_at(tv + 1, th + 1)).into()
                })
                .collect();
            self.queue
                .write_buffer(&work.resample, 0, bytemuck::cast_slice(&params));
            let mut encoder = self.encoder("ca");
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_bind_group(0, &bind, &[uniforms.push(&self.queue, &base)]);
                pass.set_pipeline(&p.correct);
                pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
            }
            self.queue.submit(Some(encoder.finish()));
            if it == 0 {
                stats.max_shift = fit.max_shift;
            }
            stats.corrected = true;
            stats.blocks = fit.blocks;
            stats.order = fit.order;
            current = Some(next_i);
        }

        let Some(cur_i) = current else {
            // No pass corrected: the mosaic as it came.
            return Ok((samples.to_vec(), stats));
        };

        if options.avoid_color_shift && stats.corrected {
            // The corrected plane is multiplied in place.
            let bind = guard_group(&work.planes[cur_i]);
            let [hw, hh] = base.half;
            let mut encoder = self.encoder("ca");
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_bind_group(0, &bind, &[uniforms.push(&self.queue, &base)]);
                pass.set_pipeline(&p.factors);
                pass.dispatch_workgroups(groups(hw, 16), groups(hh, 16), 1);
                // Three passes along the rows, then three down the
                // columns, the reference's order: plane to scratch,
                // scratch to plane, plane to scratch, and again with
                // the axes swapped, ending in the plane.
                pass.set_pipeline(&p.blur_lines);
                for plane in 0..2u32 {
                    for (axis, from_tmp) in [(0, 0), (0, 1), (0, 0), (1, 1), (1, 0), (1, 1)] {
                        let lines = if axis == 0 { hh } else { hw };
                        let slot = uniforms.push(
                            &self.queue,
                            &Params {
                                plane,
                                from_tmp,
                                axis,
                                ..base
                            },
                        );
                        pass.set_bind_group(0, &bind, &[slot]);
                        pass.dispatch_workgroups(groups(lines, 64), 1, 1);
                    }
                }
                pass.set_bind_group(0, &bind, &[uniforms.push(&self.queue, &base)]);
                pass.set_pipeline(&p.apply_factors);
                pass.dispatch_workgroups(groups(w, 16), groups(h, 16), 1);
            }
            self.queue.submit(Some(encoder.finish()));
        }

        let bytes = self.read_texture(&work.planes[cur_i], 0, 0, w, h, 4)?;
        Ok((bytemuck::cast_slice(&bytes).to_vec(), stats))
    }

    /// The mosaic into its texture, a band of rows at a time and a
    /// submit after each, so the queue's staging copies do not pile
    /// up to the frame's whole size.
    fn upload_mosaic(&self, texture: &wgpu::Texture, samples: &[f32], w: u32, h: u32) {
        let band = ((32usize << 20) / (w as usize * 4)).max(1);
        for y0 in (0..h as usize).step_by(band) {
            let rows = band.min(h as usize - y0);
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: y0 as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&samples[y0 * w as usize..(y0 + rows) * w as usize]),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(rows as u32),
                },
                wgpu::Extent3d {
                    width: w,
                    height: rows as u32,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit([]);
        }
    }
}

#[cfg(test)]
mod tests {
    /// The shader parses and validates without a device.
    #[test]
    fn the_shader_parses_and_validates() {
        let source = include_str!("ca.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("ca.wgsl:\n{}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("ca.wgsl: {e:?}"));
    }

    /// The uniform and the per-tile struct sit where the WGSL puts
    /// them.
    #[test]
    fn the_params_are_laid_out_as_the_shader_reads_them() {
        use super::{Params, TileResample};
        use std::mem::offset_of;
        assert_eq!(offset_of!(Params, size), 0);
        assert_eq!(offset_of!(Params, half), 8);
        assert_eq!(offset_of!(Params, tiles), 16);
        assert_eq!(offset_of!(Params, radius), 24);
        assert_eq!(offset_of!(Params, norm), 28);
        assert_eq!(offset_of!(Params, colors), 32);
        assert_eq!(offset_of!(Params, plane), 48);
        assert_eq!(offset_of!(Params, from_tmp), 52);
        assert_eq!(offset_of!(Params, axis), 56);
        assert_eq!(std::mem::size_of::<Params>(), 64);
        assert_eq!(offset_of!(TileResample, vfloor), 0);
        assert_eq!(offset_of!(TileResample, vceil), 8);
        assert_eq!(offset_of!(TileResample, hfloor), 16);
        assert_eq!(offset_of!(TileResample, hceil), 24);
        assert_eq!(offset_of!(TileResample, vfrac), 32);
        assert_eq!(offset_of!(TileResample, hfrac), 40);
        assert_eq!(offset_of!(TileResample, dir_v), 48);
        assert_eq!(offset_of!(TileResample, dir_h), 56);
        assert_eq!(std::mem::size_of::<TileResample>(), 64);
    }

    /// The sums come back in the shader's order.
    #[test]
    fn the_sums_are_read_in_the_shaders_order() {
        let sums: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let c = super::coefficients_of(&sums);
        assert_eq!(c[0][0][0], 0.0);
        assert_eq!(c[0][0][1], 1.0);
        assert_eq!(c[0][1][0], 2.0);
        assert_eq!(c[0][2][1], 5.0);
        assert_eq!(c[1][0][0], 6.0);
        assert_eq!(c[1][2][1], 11.0);
    }
}
