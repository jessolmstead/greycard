//! The lens correction on the GPU: `greycard_core::develop::lens::correct`
//! with the same models, the same Catmull-Rom resample, the same edges
//! and the same vignetting gain, a thread a pixel in `lens.wgsl`. The
//! magnification and the stats are the reference's own functions on
//! the CPU.
//!
//! The picture goes up in full floats, as [`Context::upload`] puts it,
//! and comes back the same: the result is the base the worker keeps on
//! the CPU, which the guide, the retouch and the export read, so there
//! are no halves on this path. The output is made a band of rows at a
//! time into a texture of about 64 MB, each band read back while the
//! next is drawn, so the run holds the input (720 MB at 45 MP) and
//! two bands rather than a second whole frame.

use greycard_core::develop::lens::{Distortion, LensCorrection, LensStats};
use greycard_core::image::WorkingImage;
use rayon::prelude::*;

use crate::plumbing::{SAMPLED, Uniforms, groups, layout, pipeline, storage_texture, uniform};
use crate::{Context, Error, Result};

/// The shader's uniform, laid out as WGSL lays it out.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    size: [u32; 2],
    y0: u32,
    distortion: u32,
    dims: [f32; 2],
    scale: f32,
    radius_scale: f32,
    vignetting_scale: f32,
    ca: u32,
    vignetting: u32,
    moves: u32,
    dist: [f32; 4],
    dist2: [f32; 4],
    red: [f32; 4],
    blue: [f32; 4],
    vig: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<Params>() == 128);

impl Params {
    fn new(correction: &LensCorrection, width: u32, height: u32, scale: f32) -> Self {
        let mut p = Params {
            size: [width, height],
            dims: [width as f32, height as f32],
            scale,
            radius_scale: correction.radius_scale,
            vignetting_scale: correction.vignetting_scale,
            moves: u32::from(correction.moves()),
            ..Default::default()
        };
        match correction.distortion {
            None => {}
            Some(Distortion::Poly3 { k1 }) => {
                p.distortion = 1;
                p.dist[0] = k1;
            }
            Some(Distortion::Poly5 { k1, k2 }) => {
                p.distortion = 2;
                p.dist[..2].copy_from_slice(&[k1, k2]);
            }
            Some(Distortion::Ptlens { a, b, c }) => {
                p.distortion = 3;
                p.dist[..3].copy_from_slice(&[a, b, c]);
            }
            Some(Distortion::Acm { k, focal_scale }) => {
                p.distortion = 4;
                p.dist.copy_from_slice(&k[..4]);
                p.dist2[0] = k[4];
                p.dist2[1] = focal_scale;
            }
        }
        if let Some(ca) = correction.chromatic_aberration {
            p.ca = 1;
            p.red[..3].copy_from_slice(&ca.red);
            p.blue[..3].copy_from_slice(&ca.blue);
        }
        if let Some(v) = correction.vignetting {
            p.vignetting = 1;
            p.vig[..3].copy_from_slice(&v.k);
        }
        p
    }
}

/// The op's pipeline, built once per device.
pub(crate) struct Pipelines {
    layout: wgpu::BindGroupLayout,
    correct: wgpu::ComputePipeline,
}

impl Pipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lens"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lens.wgsl").into()),
        });
        let layout = layout(
            device,
            "lens",
            &[
                uniform(true),
                SAMPLED,
                storage_texture(
                    wgpu::TextureFormat::Rgba32Float,
                    wgpu::StorageTextureAccess::WriteOnly,
                ),
            ],
        );
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lens"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        Self {
            correct: pipeline(device, &module, &pl, "correct"),
            layout,
        }
    }
}

/// The bytes a band of output is drawn in, about.
const BAND_BYTES: u32 = 64 << 20;

impl Context {
    /// [`greycard_core::develop::lens::correct`] on the GPU: the same
    /// signature and contract, the picture uploaded, corrected and read
    /// back. A picture wider or taller than the device takes is
    /// [`Error::Unsupported`]; a device error is [`Error::Gpu`].
    pub fn correct_lens(
        &self,
        image: &WorkingImage,
        correction: &LensCorrection,
    ) -> Result<(WorkingImage, LensStats)> {
        self.correct_lens_in_bands(image, correction, None)
    }

    /// [`Context::correct_lens`] drawn `band_rows` rows at a time
    /// rather than about 64 MB: for the tests, which check that the
    /// bands meet without a seam.
    #[doc(hidden)]
    pub fn correct_lens_in_bands(
        &self,
        image: &WorkingImage,
        correction: &LensCorrection,
        band_rows: Option<u32>,
    ) -> Result<(WorkingImage, LensStats)> {
        let (w, h) = self.fits(image.width, image.height)?;
        // The reference's own decisions, on the CPU.
        let (wf, hf) = (image.width as f32, image.height as f32);
        let scale = correction.scale_for(wf, hf);
        let stats = LensStats {
            scale,
            resampled: correction.moves(),
            corner_gain: correction
                .vignetting
                .map(|_| correction.gain_at(0.0, 0.0, wf, hf)),
        };
        let params = Params::new(correction, w, h, scale);
        let data = self.scoped("correcting the lens", || {
            self.correct_lens_unscoped(image, params, w, h, band_rows)
        })?;
        let out = WorkingImage::from_data(image.width, image.height, data)
            .map_err(|e| Error::ReadBack(e.to_string()))?;
        Ok((out, stats))
    }

    fn correct_lens_unscoped(
        &self,
        image: &WorkingImage,
        params: Params,
        w: u32,
        h: u32,
        band_rows: Option<u32>,
    ) -> Result<Vec<f32>> {
        let input = self.upload_unscoped(image, w, h)?;
        let row = (w * 16).div_ceil(256) * 256;
        let band = band_rows.unwrap_or(BAND_BYTES / row).clamp(1, h);
        let bands = h.div_ceil(band);
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lens band"),
            size: wgpu::Extent3d {
                width: w,
                height: band,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let uniforms = Uniforms::new(&self.device, "lens params", u64::from(bands));
        let src_view = input.texture.create_view(&Default::default());
        let dst_view = target.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lens"),
            layout: &self.lens.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&dst_view),
                },
            ],
        });
        // Two staging buffers, so a band is drawn and copied while the
        // one before it is unpacked.
        let staging: [wgpu::Buffer; 2] = std::array::from_fn(|_| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lens read back"),
                size: u64::from(row) * u64::from(band),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        let width = w as usize;
        let mut out = vec![0f32; width * h as usize * 3];
        let submit = |i: u32| {
            let y0 = i * band;
            let rows = band.min(h - y0);
            let slot = uniforms.push(&self.queue, &Params { y0, ..params });
            let mut encoder = self.encoder("lens");
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.lens.correct);
                pass.set_bind_group(0, &bind, &[slot]);
                pass.dispatch_workgroups(groups(w, 16), groups(rows, 16), 1);
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &target,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging[i as usize % 2],
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(rows),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: rows,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit(Some(encoder.finish()));
            let (tx, rx) = std::sync::mpsc::channel();
            staging[i as usize % 2]
                .slice(..u64::from(row) * u64::from(rows))
                .map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            rx
        };
        let mut pending = submit(0);
        for i in 0..bands {
            self.wait()?;
            pending
                .recv()
                .map_err(|_| Error::ReadBack("the map callback never came".into()))?
                .map_err(|e| Error::ReadBack(e.to_string()))?;
            // The band after this one into the other buffer, drawn
            // while this one is unpacked.
            let next = (i + 1 < bands).then(|| submit(i + 1));
            let y0 = (i * band) as usize;
            let rows = band.min(h - i * band) as usize;
            let buffer = &staging[i as usize % 2];
            let slice = buffer.slice(..u64::from(row) * rows as u64);
            let data = slice
                .get_mapped_range()
                .map_err(|e| Error::ReadBack(e.to_string()))?;
            out[y0 * width * 3..(y0 + rows) * width * 3]
                .par_chunks_mut(width * 3)
                .zip(data.par_chunks(row as usize))
                .for_each(|(dst, src)| {
                    let src: &[f32] = bytemuck::cast_slice(&src[..width * 16]);
                    for (o, px) in dst
                        .as_chunks_mut::<3>()
                        .0
                        .iter_mut()
                        .zip(src.as_chunks::<4>().0)
                    {
                        *o = [px[0], px[1], px[2]];
                    }
                });
            drop(data);
            buffer.unmap();
            if let Some(rx) = next {
                pending = rx;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    /// The shader parses and validates without a device.
    #[test]
    fn the_shader_parses_and_validates() {
        let source = include_str!("lens.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("lens.wgsl:\n{}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("lens.wgsl: {e:?}"));
    }

    /// The uniform sits where the WGSL puts it.
    #[test]
    fn the_params_are_laid_out_as_the_shader_reads_them() {
        use super::Params;
        use std::mem::offset_of;
        assert_eq!(offset_of!(Params, size), 0);
        assert_eq!(offset_of!(Params, y0), 8);
        assert_eq!(offset_of!(Params, distortion), 12);
        assert_eq!(offset_of!(Params, dims), 16);
        assert_eq!(offset_of!(Params, scale), 24);
        assert_eq!(offset_of!(Params, radius_scale), 28);
        assert_eq!(offset_of!(Params, vignetting_scale), 32);
        assert_eq!(offset_of!(Params, ca), 36);
        assert_eq!(offset_of!(Params, vignetting), 40);
        assert_eq!(offset_of!(Params, moves), 44);
        assert_eq!(offset_of!(Params, dist), 48);
        assert_eq!(offset_of!(Params, dist2), 64);
        assert_eq!(offset_of!(Params, red), 80);
        assert_eq!(offset_of!(Params, blue), 96);
        assert_eq!(offset_of!(Params, vig), 112);
    }
}
