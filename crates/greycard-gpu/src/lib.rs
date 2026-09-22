//! greycard-gpu: GPU implementations of greycard-core's engine ops.
//!
//! Every op here has its CPU reference in greycard-core (notes §5),
//! and is held to it by a test that runs both on the same picture and
//! states the tolerance. The reference stays the export's path; this
//! crate is the accelerator for the viewport, where an op re-runs on
//! a slider.
//!
//! A [`Context`] is built either from a device and queue the caller
//! already has (the editor's, which the viewport shares) or from an
//! adapter of its own (the CLI and the tests). greycard-core never
//! sees wgpu; that boundary is this crate's reason to exist.
//!
//! The ops so far: the capture sharpening ([`Context::sharpen`]) and
//! the lateral chromatic aberration correction
//! ([`Context::correct_ca`]).

use std::sync::OnceLock;

use greycard_core::image::WorkingImage;

pub use wgpu;

mod ca;
mod plumbing;
mod sharpen;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no GPU adapter")]
    NoAdapter,
    #[error("the GPU device: {0}")]
    Device(String),
    /// The device reported an error while `what` ran: a validation
    /// failure, out of memory, or an internal one. Caught in an error
    /// scope, so it is this and not the panic wgpu's uncaptured
    /// handler would raise, which on the editor's device is Slint's
    /// to set and not ours.
    #[error("the GPU while {what}: {error}")]
    Gpu { what: &'static str, error: String },
    #[error("{0}")]
    Unsupported(String),
    #[error("reading back from the GPU: {0}")]
    ReadBack(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The texture limit asked of an adapter of our own: 45 MP frames are
/// 8192 wide and more, over the 8192 default.
const WANTED_TEXTURE_DIMENSION: u32 = 16384;

/// A device, its queue, and the ops' pipelines built for it.
pub struct Context {
    device: wgpu::Device,
    queue: wgpu::Queue,
    name: String,
    /// The device's largest texture side; a picture wider than this
    /// cannot be worked here.
    max_dimension: u32,
    sharpen: sharpen::Pipelines,
    ca: ca::Pipelines,
}

impl Context {
    /// On a device the caller holds: the editor's, so the result of
    /// an op is a texture the viewport draws without a copy. An error
    /// building the pipelines is returned, not raised.
    pub fn from_device(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self> {
        let info = device.adapter_info();
        Self::build(device.clone(), queue.clone(), info.name)
    }

    /// On an adapter of our own, the fastest the machine has, or
    /// [`Error::NoAdapter`] when it has none.
    pub fn own() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|_| Error::NoAdapter)?;
        let limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_texture_dimension_2d: WANTED_TEXTURE_DIMENSION.min(limits.max_texture_dimension_2d),
            ..wgpu::Limits::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("greycard-gpu"),
            required_limits,
            ..Default::default()
        }))
        .map_err(|e| Error::Device(e.to_string()))?;
        let name = adapter.get_info().name;
        Self::build(device, queue, name)
    }

    fn build(device: wgpu::Device, queue: wgpu::Queue, name: String) -> Result<Self> {
        let max_dimension = device.limits().max_texture_dimension_2d;
        let sharpen = scoped(&device, "building the sharpen's pipelines", || {
            Ok(sharpen::Pipelines::new(&device))
        })?;
        let ca = scoped(&device, "building the CA correction's pipelines", || {
            Ok(ca::Pipelines::new(&device))
        })?;
        log::info!("greycard-gpu on {name}, textures up to {max_dimension}");
        Ok(Self {
            device,
            queue,
            name,
            max_dimension,
            sharpen,
            ca,
        })
    }

    /// Let go of the working textures the ops keep between runs (the
    /// sharpen's four planes and its atlas, about 900 MB at 45 MP;
    /// the CA correction keeps nothing, its 812 MiB at 45 MP being
    /// made and dropped within a run). For when the op is switched
    /// off or the picture is closed; the next run makes them again.
    pub fn release(&self) {
        self.sharpen.release();
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The adapter's name, for the log.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether a picture of this size can be worked here.
    fn fits(&self, width: usize, height: usize) -> Result<(u32, u32)> {
        let max = self.max_dimension as usize;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(Error::Unsupported(format!(
                "a {width}x{height} picture; this device takes up to {max} a side"
            )));
        }
        Ok((width as u32, height as u32))
    }

    /// A developed picture put on the GPU, in full floats, for the ops
    /// to read: once per base develop, then every slider re-run reads
    /// it there.
    pub fn upload(&self, image: &WorkingImage) -> Result<Image> {
        let (w, h) = self.fits(image.width, image.height)?;
        scoped(&self.device, "uploading the picture", || {
            self.upload_unscoped(image, w, h)
        })
    }

    fn upload_unscoped(&self, image: &WorkingImage, w: u32, h: u32) -> Result<Image> {
        use rayon::prelude::*;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("developed, full floats"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // A band of rows at a time, each submitted before the next: the
        // interleave to four channels is a pass over the picture, and
        // the queue keeps every `write_texture`'s staging copy until
        // the next submit, so without the submits a 45 MP frame would
        // hold another 720 MB on the CPU and as much again in staging
        // while the copy waits.
        let band = (32usize << 20).div_ceil(image.width * 16).max(1);
        let mut rgba = vec![0f32; image.width * band * 4];
        for y0 in (0..image.height).step_by(band) {
            let rows = band.min(image.height - y0);
            let src = &image.data[y0 * image.width * 3..(y0 + rows) * image.width * 3];
            rgba[..rows * image.width * 4]
                .par_chunks_mut(image.width * 4)
                .zip(src.par_chunks(image.width * 3))
                .for_each(|(out, row)| {
                    let (out, _) = out.as_chunks_mut::<4>();
                    let (row, _) = row.as_chunks::<3>();
                    for (o, px) in out.iter_mut().zip(row) {
                        *o = [px[0], px[1], px[2], 1.0];
                    }
                });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: y0 as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&rgba[..rows * image.width * 4]),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 16),
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
        Ok(Image {
            texture,
            width: w,
            height: h,
            threshold: OnceLock::new(),
        })
    }

    /// A texture for an op's result as the viewport draws it: RGBA
    /// half floats, the op's mask in the alpha.
    pub fn viewport_texture(&self, width: u32, height: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("developed"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn encoder(&self, label: &str) -> wgpu::CommandEncoder {
        self.device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) })
    }

    /// Wait for everything submitted so far.
    fn wait(&self) -> Result<()> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map(|_| ())
            .map_err(|e| Error::ReadBack(format!("waiting for the device: {e}")))
    }

    /// `len` bytes of a buffer the GPU wrote, from `offset`, after
    /// everything submitted so far has run.
    fn read_buffer(&self, buffer: &wgpu::Buffer, offset: u64, len: u64) -> Result<Vec<u8>> {
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read back"),
            size: len.max(4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("read back"),
            });
        encoder.copy_buffer_to_buffer(buffer, offset, &staging, 0, len);
        self.queue.submit(Some(encoder.finish()));
        let slice = staging.slice(..len.max(4));
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.wait()?;
        rx.recv()
            .map_err(|_| Error::ReadBack("the map callback never came".into()))?
            .map_err(|e| Error::ReadBack(e.to_string()))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| Error::ReadBack(e.to_string()))?;
        let out = data[..len as usize].to_vec();
        drop(data);
        staging.unmap();
        Ok(out)
    }

    /// A rectangle of a texture, rows packed, `bytes` a texel. In bands
    /// of rows so that no staging buffer outgrows a device's limit.
    fn read_texture(
        &self,
        texture: &wgpu::Texture,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        bytes: u32,
    ) -> Result<Vec<u8>> {
        scoped(&self.device, "reading back", || {
            self.read_texture_unscoped(texture, x, y, width, height, bytes)
        })
    }

    fn read_texture_unscoped(
        &self,
        texture: &wgpu::Texture,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        bytes: u32,
    ) -> Result<Vec<u8>> {
        let row = (width * bytes).div_ceil(256) * 256;
        let band = ((64u32 << 20) / row).max(1);
        let mut out = Vec::with_capacity((width * bytes * height) as usize);
        for y0 in (0..height).step_by(band as usize) {
            let rows = band.min(height - y0);
            let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("read back"),
                size: (row * rows) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("read back"),
                });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y: y + y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(rows),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height: rows,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit(Some(encoder.finish()));
            let slice = staging.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            self.wait()?;
            rx.recv()
                .map_err(|_| Error::ReadBack("the map callback never came".into()))?
                .map_err(|e| Error::ReadBack(e.to_string()))?;
            let data = slice
                .get_mapped_range()
                .map_err(|e| Error::ReadBack(e.to_string()))?;
            for r in 0..rows as usize {
                let line = &data[r * row as usize..][..(width * bytes) as usize];
                out.extend_from_slice(line);
            }
            drop(data);
            staging.unmap();
        }
        Ok(out)
    }
}

/// Run `f` under error scopes for validation, out-of-memory and
/// internal errors, so that whatever the device rejects comes back as
/// [`Error::Gpu`] rather than through wgpu's uncaptured handler, which
/// panics by default and on the editor's device is not ours to set.
/// Scopes pop in the reverse of their pushes.
fn scoped<T>(
    device: &wgpu::Device,
    what: &'static str,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let result = f();
    let caught = [
        pollster::block_on(internal.pop()),
        pollster::block_on(memory.pop()),
        pollster::block_on(validation.pop()),
    ]
    .into_iter()
    .flatten()
    .next();
    match (result, caught) {
        (_, Some(error)) => Err(Error::Gpu {
            what,
            error: error.to_string(),
        }),
        (result, None) => result,
    }
}

/// A developed picture on the GPU, as [`Context::upload`] put it.
pub struct Image {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
    /// The sharpen's automatic contrast threshold, which is a property
    /// of the picture alone: found once, kept for every re-run.
    threshold: OnceLock<f32>,
}

impl Image {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}
