use super::*;

impl Renderer {
    pub fn has_image(&self) -> bool {
        self.source.is_some()
    }

    /// The developed picture's mean over the square of `reach` pixels
    /// each way about (`x`, `y`), in the working space at the white
    /// it was developed at, read back from the GPU; `None` with no
    /// picture, or the point off it.
    pub fn sample(&self, x: i64, y: i64, reach: u32) -> Result<Option<[f32; 3]>> {
        let Some(source) = &self.source else {
            return Ok(None);
        };
        let (w, h) = (source.width() as i64, source.height() as i64);
        if x < 0 || y < 0 || x >= w || y >= h {
            return Ok(None);
        }
        let reach = reach as i64;
        let (x0, y0) = ((x - reach).max(0), (y - reach).max(0));
        let (x1, y1) = ((x + reach + 1).min(w), (y + reach + 1).min(h));
        let (cw, ch) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let row = (cw * 8).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&gpu::BufferDescriptor {
            label: Some("sample"),
            size: (row * ch) as u64,
            usage: gpu::BufferUsages::COPY_DST | gpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&gpu::CommandEncoderDescriptor {
                label: Some("sample"),
            });
        encoder.copy_texture_to_buffer(
            gpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: gpu::Origin3d {
                    x: x0 as u32,
                    y: y0 as u32,
                    z: 0,
                },
                aspect: gpu::TextureAspect::All,
            },
            gpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: gpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(ch),
                },
            },
            gpu::Extent3d {
                width: cw,
                height: ch,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(gpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(gpu::PollType::wait_indefinitely())
            .context("waiting for the sample")?;
        rx.recv()
            .context("map callback")?
            .context("mapping the sample buffer")?;
        let data = slice
            .get_mapped_range()
            .context("reading the sample buffer")?;
        let mut sum = [0.0f32; 3];
        for r in 0..ch as usize {
            let line = &data[r * row as usize..][..(cw * 8) as usize];
            for px in bytemuck::cast_slice::<u8, half::f16>(line)
                .as_chunks::<4>()
                .0
            {
                for k in 0..3 {
                    sum[k] += px[k].to_f32();
                }
            }
        }
        drop(data);
        buffer.unmap();
        let n = (cw * ch) as f32;
        Ok(Some(sum.map(|v| v / n)))
    }

    /// The developed picture on a grid about `across` pixels wide, a
    /// pixel at each point of it (not a mean), in the working space
    /// at the white it was developed at, as [`Self::sample`] reads
    /// it: the whole frame, small, for the auto white balance, row
    /// after row, and how many points a row has. Only the rows on the
    /// grid come back from the GPU. `None` with no picture.
    pub fn sample_grid(&self, across: u32) -> Result<Option<(Vec<[f32; 3]>, usize)>> {
        let Some(source) = &self.source else {
            return Ok(None);
        };
        let (w, h) = (source.width(), source.height());
        let step = (w / across.max(1)).max(1);
        let rows: Vec<u32> = (step / 2..h).step_by(step as usize).collect();
        let row = (w * 8).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&gpu::BufferDescriptor {
            label: Some("sample grid"),
            size: u64::from(row) * rows.len() as u64,
            usage: gpu::BufferUsages::COPY_DST | gpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&gpu::CommandEncoderDescriptor {
                label: Some("sample grid"),
            });
        for (i, &y) in rows.iter().enumerate() {
            encoder.copy_texture_to_buffer(
                gpu::TexelCopyTextureInfo {
                    texture: source,
                    mip_level: 0,
                    origin: gpu::Origin3d { x: 0, y, z: 0 },
                    aspect: gpu::TextureAspect::All,
                },
                gpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: gpu::TexelCopyBufferLayout {
                        offset: u64::from(row) * i as u64,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(1),
                    },
                },
                gpu::Extent3d {
                    width: w,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.queue.submit(Some(encoder.finish()));
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(gpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(gpu::PollType::wait_indefinitely())
            .context("waiting for the sample grid")?;
        rx.recv()
            .context("map callback")?
            .context("mapping the sample grid")?;
        let data = slice
            .get_mapped_range()
            .context("reading the sample grid")?;
        let mut out = Vec::with_capacity(rows.len() * (w / step) as usize);
        for i in 0..rows.len() {
            let line = &data[i * row as usize..][..(w * 8) as usize];
            let texels = bytemuck::cast_slice::<u8, half::f16>(line)
                .as_chunks::<4>()
                .0;
            out.extend(
                texels
                    .iter()
                    .skip((step / 2) as usize)
                    .step_by(step as usize)
                    .map(|px| [px[0].to_f32(), px[1].to_f32(), px[2].to_f32()]),
            );
        }
        drop(data);
        buffer.unmap();
        let width = (step / 2..w).step_by(step as usize).count();
        Ok(Some((out, width)))
    }
}
