use std::sync::mpsc::TryRecvError;

use super::*;

/// Unmap a staging buffer whose read back was lost: its map callback
/// dropped without being called, the channel's sender with it.
///
/// wgpu can do that. Every poll of the device, on any thread, collects
/// the callbacks of the maps that are done and fires them after; but a
/// `Surface::configure` that gives up because the worker submitted
/// during its wait (see `crate::gpu_errors`) drops what it collected
/// instead (wgpu-core 30, `Device::configure_surface`). The scopes'
/// read back is submitted in one frame and collected in a later one,
/// so a configure between the two can take it, and the channel is left
/// disconnected and empty. Read as "not yet", that would hold the
/// analysis in flight for the rest of the session: the histogram, the
/// navigator and the readout frozen, and a frame asked for after every
/// frame. So a lost read back is let go and the analysis made again.
///
/// The window's other read backs (`sample.rs`, `draw.rs`, femtovg's
/// snapshot) map and wait for the device in one call on the window's
/// thread, the thread that configures, so no configure comes between;
/// the worker's map a band at a time and wait for it, with nothing of
/// theirs left running for a configure to fail on.
///
/// wgpu-core maps the buffer before it hands the callback over, so the
/// unmap is valid, and it lets the next read back map the buffer at
/// once: a map of a buffer wgpu still holds mapped fails, and the
/// scopes would lose one more cycle before the error path's unmap
/// freed it. Had the map failed, the buffer is not mapped and the
/// unmap is a validation error, which the scope takes rather than the
/// device's handler.
fn unmap_lost(device: &gpu::Device, buffer: &gpu::Buffer) {
    let scope = device.push_error_scope(gpu::ErrorFilter::Validation);
    buffer.unmap();
    drop(scope);
}

/// What of a view the scopes depend on.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct EditKey {
    light: Light,
    mixer: Mixer,
    color: Color,
    bw: BlackWhite,
    tint: Tint,
    matrix: [[f32; 2]; 2],
    persp: [f32; 2],
    plane: (f32, f32),
    frame_origin: (f32, f32),
    frame_size: (f32, f32),
    curves: CurveLut,
    white: [[f32; 3]; 3],
    locals: Vec<Local>,
    vignette: Vignette,
    grain: Grain,
    source_turn: u8,
}

impl EditKey {
    fn of(v: &View) -> Self {
        Self {
            light: v.light,
            mixer: v.mixer,
            color: v.color,
            bw: v.bw,
            tint: v.tint,
            matrix: v.matrix,
            persp: v.persp,
            plane: v.plane,
            frame_origin: v.frame_origin,
            frame_size: v.frame_size,
            curves: v.curves,
            white: v.white,
            locals: v.locals.clone(),
            vignette: v.vignette,
            grain: v.grain,
            source_turn: v.source_turn,
        }
    }
}

/// Width of the image the scopes are taken from.
pub(super) const ANALYSIS_WIDTH: u32 = 512;

impl Scopes {
    pub(super) fn new(device: &gpu::Device) -> Self {
        let shader = device.create_shader_module(gpu::ShaderModuleDescriptor {
            label: Some("scope"),
            source: gpu::ShaderSource::Wgsl(include_str!("../scope.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&gpu::BindGroupLayoutDescriptor {
            label: Some("scope"),
            entries: &[
                gpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: gpu::ShaderStages::COMPUTE,
                    ty: gpu::BindingType::Texture {
                        sample_type: gpu::TextureSampleType::Float { filterable: false },
                        view_dimension: gpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: gpu::ShaderStages::COMPUTE,
                    ty: gpu::BindingType::Buffer {
                        ty: gpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: gpu::ShaderStages::COMPUTE,
                    ty: gpu::BindingType::Buffer {
                        ty: gpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&gpu::PipelineLayoutDescriptor {
            label: Some("scope"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = device.create_compute_pipeline(&gpu::ComputePipelineDescriptor {
            label: Some("scope"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let size = (scope::MAX * 4) as u64;
        let bins = device.create_buffer(&gpu::BufferDescriptor {
            label: Some("scope bins"),
            size,
            usage: gpu::BufferUsages::STORAGE
                | gpu::BufferUsages::COPY_SRC
                | gpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&gpu::BufferDescriptor {
            label: Some("scope staging"),
            size,
            usage: gpu::BufferUsages::MAP_READ | gpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mode = device.create_buffer(&gpu::BufferDescriptor {
            label: Some("scope mode"),
            size: 32,
            usage: gpu::BufferUsages::UNIFORM | gpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            layout,
            bins,
            staging,
            mode,
            analysis: None,
            navigator: None,
            in_flight: None,
            latest: None,
            fresh: false,
            picture_staging: None,
            picture_in_flight: None,
            picture: None,
            pixels: None,
            source_changed: false,
            analyzed: None,
            image_changed: false,
        }
    }
}

impl Renderer {
    /// The scopes of the whole image under `v`'s edit: the bins from
    /// the last read back with the scope they were taken for, and a
    /// new read back dispatched if none is on its way. `true` when one
    /// is in flight and a frame later will have it.
    ///
    /// The histogram is always in the first [`scope::HIST`] bins,
    /// whatever `scope` asks for, because the curve editor draws it.
    ///
    /// With `weigh_by`, a local of `v` by index, every bin is weighted
    /// by that local's mask: the analysis draw writes its coverage
    /// into the alpha, the compute pass adds it in place of one, and
    /// the read back is brought to the whole picture's mass
    /// ([`scope::normalize`]), so a small selection does not go dim.
    /// Without it the bins are what they have always been.
    pub fn analyze(
        &mut self,
        v: &View,
        scope: Scope,
        weigh_by: Option<usize>,
    ) -> (Option<(Scope, &[u32])>, bool) {
        let weigh_by = weigh_by.filter(|&k| k < v.locals.len().min(MAX_LOCALS));
        // Collect a finished read back, or let a lost one go and
        // analyze again ([`unmap_lost`]).
        let _ = self.device.poll(gpu::PollType::Poll);
        let bins = self.scopes.in_flight.as_ref().map(|rx| rx.try_recv());
        if let Some(Err(TryRecvError::Disconnected)) = bins {
            self.scopes.in_flight = None;
            unmap_lost(&self.device, &self.scopes.staging);
            self.scopes.image_changed = true;
        }
        if let Some(Ok(result)) = bins {
            self.scopes.in_flight = None;
            if result.is_ok()
                && let Some((_, taken, weighed)) = self.scopes.analyzed
                && let Some(t) = &self.scopes.analysis
                && let Ok(data) = self.scopes.staging.slice(..).get_mapped_range()
            {
                let mut bins = bytemuck::cast_slice::<u8, u32>(&data)[..taken.bin_count()].to_vec();
                drop(data);
                if weighed.is_some() {
                    scope::normalize(&mut bins, (t.width() * t.height()) as usize);
                }
                self.scopes.latest = Some((taken, bins));
                self.scopes.fresh = true;
            }
            self.scopes.staging.unmap();
        }
        let picture = self
            .scopes
            .picture_in_flight
            .as_ref()
            .map(|rx| rx.try_recv());
        if let Some(Err(TryRecvError::Disconnected)) = picture {
            self.scopes.picture_in_flight = None;
            if let Some(staging) = &self.scopes.picture_staging {
                unmap_lost(&self.device, staging);
            }
            self.scopes.image_changed = true;
        }
        if let Some(Ok(result)) = picture {
            self.scopes.picture_in_flight = None;
            if let Some(staging) = &self.scopes.picture_staging {
                if result.is_ok()
                    && let Some(t) = &self.scopes.analysis
                    && let Ok(data) = staging.slice(..).get_mapped_range()
                {
                    // The analysis picture, then the navigator's.
                    let (w, h) = (t.width(), t.height());
                    let bytes = (w * h * 4) as usize;
                    let mut analysis = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                    analysis.make_mut_bytes().copy_from_slice(&data[..bytes]);
                    let mut navigator = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                    navigator
                        .make_mut_bytes()
                        .copy_from_slice(&data[bytes..2 * bytes]);
                    drop(data);
                    // Kept with the mask's coverage in the alpha, which
                    // the readout weighs by as the bins were weighed;
                    // not when a new source came in after it was taken,
                    // which is the last image's picture at its shape.
                    if !self.scopes.source_changed {
                        self.scopes.pixels = Some(analysis);
                    }
                    self.scopes.picture = Some(slint::Image::from_rgba8(navigator));
                }
                staging.unmap();
            }
        }
        let key = (EditKey::of(v), scope, weigh_by);
        let stale = self.scopes.image_changed || self.scopes.analyzed.as_ref() != Some(&key);
        if stale
            && self.scopes.in_flight.is_none()
            && self.scopes.picture_in_flight.is_none()
            && self.source.is_some()
        {
            self.scopes.analyzed = Some(key);
            self.scopes.image_changed = false;
            self.scopes.source_changed = false;
            // The whole frame, small, at the view's edit.
            let (iw, ih) = (
                v.frame_size.0.max(1.0) as u32,
                v.frame_size.1.max(1.0) as u32,
            );
            let height = (ANALYSIS_WIDTH * ih / iw).max(1);
            let analysis = match &self.scopes.analysis {
                Some(t) if t.height() == height => t.clone(),
                _ => {
                    let t = self.make_target("analysis", ANALYSIS_WIDTH, height);
                    self.scopes.analysis = Some(t.clone());
                    t
                }
            };
            let navigator = match &self.scopes.navigator {
                Some(t) if t.height() == height => t.clone(),
                _ => {
                    let t = self.make_target("navigator", ANALYSIS_WIDTH, height);
                    self.scopes.navigator = Some(t.clone());
                    t
                }
            };
            let whole = View {
                zoom: ANALYSIS_WIDTH as f32 / iw as f32,
                center: (iw as f32 / 2.0, ih as f32 / 2.0),
                show_mask: None,
                show_sharpen: false,
                mask_alone: false,
                weigh_by,
                warn: Warn::default(),
                vignette: v.vignette,
                grain: v.grain,
                ..v.clone()
            };
            let bytes = (scope.bin_count() * 4) as u64;
            self.queue.write_buffer(
                &self.scopes.mode,
                0,
                bytemuck::cast_slice(&[
                    scope.kind(),
                    scope::COLUMNS as u32,
                    scope::WHEEL as u32,
                    scope::LEVELS as u32,
                    u32::from(weigh_by.is_some()),
                    0,
                    0,
                    0,
                ]),
            );
            let mut encoder = self
                .device
                .create_command_encoder(&gpu::CommandEncoderDescriptor {
                    label: Some("scope"),
                });
            // The scopes and the readout read the export space's own
            // values, what the file will hold; the navigator is what
            // the viewport shows, through the monitor's table and the
            // proof, and the picture whole, unweighed.
            self.draw(&mut encoder, &analysis, &whole, false);
            self.draw(
                &mut encoder,
                &navigator,
                &View {
                    weigh_by: None,
                    ..whole.clone()
                },
                true,
            );
            encoder.clear_buffer(&self.scopes.bins, 0, Some(bytes));
            {
                let view = analysis.create_view(&Default::default());
                let bind_group = self.device.create_bind_group(&gpu::BindGroupDescriptor {
                    label: Some("scope"),
                    layout: &self.scopes.layout,
                    entries: &[
                        gpu::BindGroupEntry {
                            binding: 0,
                            resource: gpu::BindingResource::TextureView(&view),
                        },
                        gpu::BindGroupEntry {
                            binding: 1,
                            resource: self.scopes.bins.as_entire_binding(),
                        },
                        gpu::BindGroupEntry {
                            binding: 2,
                            resource: self.scopes.mode.as_entire_binding(),
                        },
                    ],
                });
                let mut pass = encoder.begin_compute_pass(&gpu::ComputePassDescriptor {
                    label: Some("scope"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.scopes.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(ANALYSIS_WIDTH.div_ceil(16), height.div_ceil(16), 1);
            }
            encoder.copy_buffer_to_buffer(&self.scopes.bins, 0, &self.scopes.staging, 0, bytes);
            // The pictures too, the analysis for the readout and the
            // navigator's, one after the other. Their rows are 2048
            // bytes, a multiple of the 256 a copy wants, so they land
            // packed.
            let picture_bytes = (ANALYSIS_WIDTH * 4 * height) as u64;
            let picture_staging = match &self.scopes.picture_staging {
                Some(b) if b.size() == 2 * picture_bytes => b.clone(),
                _ => {
                    let b = self.device.create_buffer(&gpu::BufferDescriptor {
                        label: Some("analysis and navigator staging"),
                        size: 2 * picture_bytes,
                        usage: gpu::BufferUsages::COPY_DST | gpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    self.scopes.picture_staging = Some(b.clone());
                    b
                }
            };
            for (k, texture) in [&analysis, &navigator].into_iter().enumerate() {
                encoder.copy_texture_to_buffer(
                    gpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: gpu::Origin3d::ZERO,
                        aspect: gpu::TextureAspect::All,
                    },
                    gpu::TexelCopyBufferInfo {
                        buffer: &picture_staging,
                        layout: gpu::TexelCopyBufferLayout {
                            offset: k as u64 * picture_bytes,
                            bytes_per_row: Some(ANALYSIS_WIDTH * 4),
                            rows_per_image: Some(height),
                        },
                    },
                    gpu::Extent3d {
                        width: ANALYSIS_WIDTH,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.queue.submit(Some(encoder.finish()));
            let (tx, rx) = std::sync::mpsc::channel();
            self.scopes
                .staging
                .slice(..)
                .map_async(gpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            self.scopes.in_flight = Some(rx);
            let (tx, rx) = std::sync::mpsc::channel();
            picture_staging
                .slice(..)
                .map_async(gpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            self.scopes.picture_in_flight = Some(rx);
        }
        (
            if std::mem::take(&mut self.scopes.fresh) {
                self.scopes.latest.as_ref().map(|(s, b)| (*s, b.as_slice()))
            } else {
                None
            },
            self.scopes.in_flight.is_some() || self.scopes.picture_in_flight.is_some(),
        )
    }

    /// The navigator's picture read back since the last take: the
    /// whole frame under the edit as the screen shows it, through the
    /// monitor's table, 512 wide.
    pub fn take_navigator(&mut self) -> Option<slint::Image> {
        self.scopes.picture.take()
    }

    /// The analysis picture's color about a point of the frame, `u`
    /// and `v` fractions of its width and height, within `radius` of
    /// it (a fraction of the frame's width): [`scope::under_pointer`]
    /// on the last picture read, its coverage in the alpha as the bins
    /// were weighed. `None` before the first picture of the image now
    /// on the GPU is read, so a new frame's geometry never reads the
    /// last frame's pixels.
    pub fn analyzed_at(&self, u: f32, v: f32, radius: f32) -> Option<scope::Reading> {
        let buf = self.scopes.pixels.as_ref()?;
        scope::under_pointer(
            buf.as_slice(),
            buf.width() as usize,
            buf.height() as usize,
            u,
            v,
            radius,
        )
    }
}
