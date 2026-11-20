use std::sync::mpsc::TryRecvError;

use super::*;

/// Unmap a staging buffer whose read back was lost: its map callback
/// dropped without being called, the channel's sender with it.
///
/// wgpu can do that. Every poll of the device, on any thread, collects
/// the callbacks of the maps that are done and fires them after; but a
/// `Surface::configure` that gives up because the worker submitted
/// during its wait (see the window's `gpu_errors`) drops what it collected
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
    output_sharpen: greycard_edit::OutputSharpen,
    source_turn: u8,
    /// The curve the picture is finished with, and where the source's
    /// white is: a scope taken under the last ones reads the wrong
    /// tones.
    source: Source,
    display_curve: greycard_edit::DisplayCurve,
    profile_tone: Option<greycard_render::profile_tone::ProfileTone>,
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
            output_sharpen: v.output_sharpen,
            source_turn: v.source_turn,
            source: v.source,
            display_curve: v.display_curve,
            profile_tone: v.profile_tone.clone(),
        }
    }
}

/// Width of the image the scopes are taken from.
pub(super) const ANALYSIS_WIDTH: u32 = 512;

/// The scopes' image of a frame of `frame` (width, height) on a device
/// whose textures stop at `most` a side: [`ANALYSIS_WIDTH`] wide (or
/// the device's side, on one smaller) and as tall as the frame's shape
/// makes it, as it always was, or, for a frame that would make it
/// taller than the device takes, that tall and as narrow as its
/// shape makes it.
/// In whole 64s across, so a row is still the 256 bytes' multiple a
/// copy wants: 64 at the least, the bottom of a frame past 128 to 1
/// left out.
pub(super) fn analysis_size(frame: (f32, f32), most: u32) -> (u32, u32) {
    let (iw, ih) = (
        u64::from(frame.0.max(1.0) as u32),
        u64::from(frame.1.max(1.0) as u32),
    );
    let tallest = u64::from(most.max(1));
    let widest = u64::from(ANALYSIS_WIDTH.min(most / 64 * 64).max(64));
    let height = (widest * ih / iw).max(1);
    if height <= tallest {
        return (widest as u32, height as u32);
    }
    let width = (tallest * iw / ih / 64 * 64).clamp(64, widest);
    let height = (width * ih / iw).clamp(1, tallest);
    (width as u32, height as u32)
}

/// Where in the look a histogram is taken short of its end, the
/// export space's encoded values the scopes bin: where the curves
/// take the picture in, the encoded working-space values, before the
/// look table, the output sharpen, the export space's matrix, and the
/// grain ([`Renderer::histogram_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    /// Before every curve: what a local's curves take in, and the
    /// global ones when no local's curve acts.
    BeforeCurves,
    /// After the locals' curves are blended in, before the global
    /// curves: what those take in.
    BeforeGlobalCurves,
    /// After every point curve, the locals' and the global ones,
    /// before the color curves: what those read their lightness from.
    AfterCurves,
}

/// What a read back's bins were taken under: the view's edit, the
/// scope binned, where in the look (none for its end), the local the
/// bins are weighed by, and the frame of the source they were taken
/// of (the count of frames opened when that source came in,
/// [`Renderer::another_picture`]). A new analysis only when one of
/// these, or the image under it, changes.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Taken {
    key: EditKey,
    scope: Scope,
    tap: Option<Tap>,
    weigh_by: Option<usize>,
    picture: u64,
}

/// One analysis and its read back: the whole frame drawn small,
/// binned on the GPU, and the bins mapped back a frame or more later.
pub(super) struct Readback {
    bins: gpu::Buffer,
    staging: gpu::Buffer,
    mode: gpu::Buffer,
    /// The whole frame drawn small, which the bins are taken of.
    pub(super) target: Option<gpu::Texture>,
    /// A read back on its way, if one is.
    pub(super) in_flight: Option<std::sync::mpsc::Receiver<Result<(), gpu::BufferAsyncError>>>,
    /// What the read back in flight, or the latest bins, were taken
    /// under.
    taken: Option<Taken>,
    /// The last bins read, and which scope asked for them.
    latest: Option<(Scope, Vec<u32>)>,
    /// `latest` came in since it was last handed out: the bins go to
    /// the window once, or every frame would repaint the scope and the
    /// curve and ask for the next frame, without end.
    fresh: bool,
    /// The image under it changed since it was last sent (the source,
    /// a table, a mask's raster): analyze again whatever the key says.
    changed: bool,
}

impl Readback {
    fn new(device: &gpu::Device, label: &str, bins: usize) -> Self {
        let size = (bins * 4) as u64;
        let buffer = |what: &str, usage| {
            device.create_buffer(&gpu::BufferDescriptor {
                label: Some(&format!("{label} {what}")),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            bins: buffer(
                "bins",
                gpu::BufferUsages::STORAGE
                    | gpu::BufferUsages::COPY_SRC
                    | gpu::BufferUsages::COPY_DST,
            ),
            staging: buffer(
                "staging",
                gpu::BufferUsages::MAP_READ | gpu::BufferUsages::COPY_DST,
            ),
            mode: device.create_buffer(&gpu::BufferDescriptor {
                label: Some(&format!("{label} mode")),
                size: 32,
                usage: gpu::BufferUsages::UNIFORM | gpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            target: None,
            in_flight: None,
            taken: None,
            latest: None,
            fresh: false,
            changed: false,
        }
    }

    /// Collect a finished read back into `latest`. A lost one (its map
    /// callback dropped, [`unmap_lost`]) is let go and the analysis
    /// made again; one `wanted` turns down is let go too.
    fn collect(&mut self, device: &gpu::Device, wanted: impl Fn(&Taken) -> bool) {
        match self.in_flight.as_ref().map(|rx| rx.try_recv()) {
            Some(Err(TryRecvError::Disconnected)) => {
                self.in_flight = None;
                unmap_lost(device, &self.staging);
                self.changed = true;
            }
            Some(Ok(result)) => {
                self.in_flight = None;
                if result.is_ok()
                    && let Some(taken) = &self.taken
                    && wanted(taken)
                    && let Some(t) = &self.target
                    && let Ok(data) = self.staging.slice(..).get_mapped_range()
                {
                    let mut bins =
                        bytemuck::cast_slice::<u8, u32>(&data)[..taken.scope.bin_count()].to_vec();
                    drop(data);
                    if taken.weigh_by.is_some() {
                        scope::normalize(&mut bins, (t.width() * t.height()) as usize);
                    }
                    self.latest = Some((taken.scope, bins));
                    self.fresh = true;
                }
                self.staging.unmap();
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    /// Whether an analysis under `taken` is to be sent: the image
    /// changed or the key did, and none is on its way.
    fn wants(&self, taken: &Taken) -> bool {
        self.in_flight.is_none() && (self.changed || self.taken.as_ref() != Some(taken))
    }

    /// The read back of the bins just submitted, under `taken`.
    fn sent(&mut self, taken: Taken) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.staging
            .slice(..)
            .map_async(gpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.in_flight = Some(rx);
        self.taken = Some(taken);
        self.changed = false;
    }

    /// The bins read since the last take, once.
    fn take_fresh(&mut self) -> Option<(Scope, &[u32])> {
        if std::mem::take(&mut self.fresh) {
            self.latest.as_ref().map(|(s, b)| (*s, b.as_slice()))
        } else {
            None
        }
    }
}

/// Which of the scopes' read backs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// The scopes', at the export space's encoded values.
    Scopes,
    /// [`Renderer::histogram_at`]'s.
    Tap,
}

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
        Self {
            pipeline,
            layout,
            main: Readback::new(device, "scope", scope::MAX),
            tap: Readback::new(device, "tapped histogram", scope::HIST),
            navigator: None,
            picture_staging: None,
            picture_in_flight: None,
            picture: None,
            pixels: None,
            source_changed: false,
            pictures: 0,
            source_picture: 0,
        }
    }

    /// The image under every analysis changed: each is made again.
    pub(super) fn image_changed(&mut self) {
        self.main.changed = true;
        self.tap.changed = true;
    }

    /// What follows the curves changed (the display table, the export
    /// space, the look table): the scopes, which read past them, are
    /// made again; a tapped histogram, before them, is not.
    pub(super) fn output_changed(&mut self) {
        self.main.changed = true;
    }

    /// A new source came in: the picture in flight is the last one's
    /// and is not kept for the readout. The source is of the frame
    /// open now ([`Renderer::another_picture`]); a develop of the same
    /// frame is the same frame to the tapped histogram.
    pub(super) fn new_source(&mut self) {
        self.image_changed();
        self.source_changed = true;
        self.source_picture = self.pictures;
    }

    fn slot(&self, slot: Slot) -> &Readback {
        match slot {
            Slot::Scopes => &self.main,
            Slot::Tap => &self.tap,
        }
    }

    fn slot_mut(&mut self, slot: Slot) -> &mut Readback {
        match slot {
            Slot::Scopes => &mut self.main,
            Slot::Tap => &mut self.tap,
        }
    }
}

impl Renderer {
    /// The scopes of the whole image under `v`'s edit: the bins from
    /// the last read back with the scope they were taken for, and a
    /// new read back dispatched if none is on its way. `true` when one
    /// is in flight and a frame later will have it.
    ///
    /// The histogram is in the first [`scope::HIST`] bins whatever
    /// `scope` asks for: the clipping lamps read it.
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
        self.sync_sample(v);
        let weigh_by = weigh_by.filter(|&k| k < v.locals.len().min(MAX_LOCALS));
        // Collect a finished read back, or let a lost one go and
        // analyze again ([`unmap_lost`]).
        let _ = crate::lost::poll(&self.device, gpu::PollType::Poll);
        // The scopes show what lands, as they always have: they are of
        // the picture on screen, the last frame's until the open one's
        // develop comes in.
        self.scopes.main.collect(&self.device, |_| true);
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
            self.scopes.main.changed = true;
        }
        if let Some(Ok(result)) = picture {
            self.scopes.picture_in_flight = None;
            if let Some(staging) = &self.scopes.picture_staging {
                if result.is_ok()
                    && let Some(t) = &self.scopes.main.target
                    && let Ok(data) = staging.slice(..).get_mapped_range()
                {
                    // The analysis picture, then the navigator's.
                    let (w, h) = (t.width(), t.height());
                    let bytes = (w * h * 4) as usize;
                    let analysis = Rgba8::from_bytes(w, h, &data[..bytes]);
                    let navigator = Rgba8::from_bytes(w, h, &data[bytes..2 * bytes]);
                    drop(data);
                    // Kept with the mask's coverage in the alpha, which
                    // the readout weighs by as the bins were weighed;
                    // not when a new source came in after it was taken,
                    // which is the last image's picture at its shape.
                    if !self.scopes.source_changed {
                        self.scopes.pixels = analysis;
                    }
                    self.scopes.picture = navigator;
                }
                staging.unmap();
            }
        }
        let taken = Taken {
            key: EditKey::of(v),
            scope,
            tap: None,
            weigh_by,
            picture: self.scopes.source_picture,
        };
        if self.scopes.main.wants(&taken)
            && self.scopes.picture_in_flight.is_none()
            && self.source.is_some()
        {
            self.scopes.source_changed = false;
            let whole = whole_frame(v, weigh_by, self.most_side);
            let (mut encoder, analysis) = self.bin_frame(Slot::Scopes, &whole, &taken);
            // The scopes and the readout read the export space's own
            // values, what the file will hold; the navigator is what
            // the viewport shows, through the monitor's table and the
            // proof, and the picture whole, unweighed.
            let (width, height) = (analysis.width(), analysis.height());
            let navigator = match &self.scopes.navigator {
                Some(t) if t.height() == height && t.width() == width => t.clone(),
                _ => {
                    let t = self.make_target("navigator", width, height);
                    self.scopes.navigator = Some(t.clone());
                    t
                }
            };
            self.draw(
                &mut encoder,
                &navigator,
                &View {
                    weigh_by: None,
                    ..whole
                },
                true,
            );
            // The pictures too, the analysis for the readout and the
            // navigator's, one after the other. Their rows are 2048
            // bytes (a multiple of 256 at any width `analysis_size`
            // gives), the multiple a copy wants, so they land packed.
            let picture_bytes = u64::from(width * 4 * height);
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
                            bytes_per_row: Some(width * 4),
                            rows_per_image: Some(height),
                        },
                    },
                    gpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.queue.submit(Some(encoder.finish()));
            self.scopes.main.sent(taken);
            let (tx, rx) = std::sync::mpsc::channel();
            picture_staging
                .slice(..)
                .map_async(gpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            self.scopes.picture_in_flight = Some(rx);
        }
        let in_flight =
            self.scopes.main.in_flight.is_some() || self.scopes.picture_in_flight.is_some();
        (self.scopes.main.take_fresh(), in_flight)
    }

    /// The histogram of the whole frame under `v` where `tap` says the
    /// curves take the picture in, weighed as [`Self::analyze`] weighs
    /// it: its first [`scope::HIST`] bins, of the encoded working-space
    /// values rather than the export space's. The bins from the last
    /// read back, once, and `true` while one is in flight.
    ///
    /// Its own read back beside the scopes', keyed on the edit without
    /// what lies past the tap (the curves after it, the grain, the
    /// output sharpen), so an edit of those dispatches nothing; nor
    /// does a new display table, export space, or look table.
    pub fn histogram_at(
        &mut self,
        v: &View,
        tap: Tap,
        weigh_by: Option<usize>,
    ) -> (Option<&[u32]>, bool) {
        self.sync_sample(v);
        let weigh_by = weigh_by.filter(|&k| k < v.locals.len().min(MAX_LOCALS));
        let _ = crate::lost::poll(&self.device, gpu::PollType::Poll);
        // What is past the tap does nothing to what it reads: the
        // curves past it (the global ones, the locals' too before
        // every curve, only the color curves after every point curve),
        // and the grain and the output sharpen past all of them.
        let mut before = View {
            grain: Grain::default(),
            output_sharpen: OutputSharpen::default(),
            ..v.clone()
        };
        let level = greycard_edit::Curves::default().bake();
        let colorless = |lut: &mut CurveLut| {
            lut.color = level.color;
            lut.shaded = false;
        };
        match tap {
            Tap::BeforeCurves => {
                before.curves = level;
                for local in &mut before.locals {
                    local.baked.curves = level;
                }
            }
            Tap::BeforeGlobalCurves => before.curves = level,
            Tap::AfterCurves => {
                colorless(&mut before.curves);
                for local in &mut before.locals {
                    colorless(&mut local.baked.curves);
                }
            }
        }
        let taken = Taken {
            key: EditKey::of(&before),
            scope: Scope::Rgb,
            tap: Some(tap),
            weigh_by,
            picture: self.scopes.source_picture,
        };
        // Bins taken of another frame's source, at another tap, or
        // under another weighing are of another curve's input: let go,
        // never drawn behind this one.
        let open = self.scopes.pictures;
        self.scopes.tap.collect(&self.device, |landed| {
            landed.picture == open && landed.tap == taken.tap && landed.weigh_by == taken.weigh_by
        });
        // Nothing is sent over the last frame's source after another
        // is opened: its own develop is waited for.
        if self.scopes.tap.wants(&taken)
            && self.source.is_some()
            && self.scopes.source_picture == open
        {
            let whole = whole_frame(&before, weigh_by, self.most_side);
            let (encoder, _) = self.bin_frame(Slot::Tap, &whole, &taken);
            self.queue.submit(Some(encoder.finish()));
            self.scopes.tap.sent(taken);
        }
        let in_flight = self.scopes.tap.in_flight.is_some();
        (
            self.scopes.tap.take_fresh().map(|(_, b)| &b[..scope::HIST]),
            in_flight,
        )
    }

    /// Another frame is opened: until its develop comes in as the
    /// source, the tapped histogram sends nothing, and one on its way,
    /// of the last frame's source, is let go when it lands rather than
    /// drawn behind this frame's curve. The scopes are unchanged: they
    /// describe the picture on screen.
    pub fn another_picture(&mut self) {
        self.scopes.pictures += 1;
    }

    /// The tapped histogram is not wanted for now (the curve editor
    /// hidden): one on its way is let go when it lands, and the next
    /// call takes its own afresh, so it never draws bins taken before.
    pub fn let_histogram_go(&mut self) {
        let tap = &mut self.scopes.tap;
        tap.taken = None;
        tap.latest = None;
        tap.fresh = false;
    }

    /// Draw `whole` into `slot`'s target, made at the frame's shape,
    /// bin it for `taken`'s scope and weighing, and copy the bins to
    /// the staging buffer: the encoder, for more before the submit,
    /// and the target drawn.
    fn bin_frame(
        &mut self,
        slot: Slot,
        whole: &View,
        taken: &Taken,
    ) -> (gpu::CommandEncoder, gpu::Texture) {
        let (width, height) = analysis_size(whole.frame_size, self.most_side);
        let target = match &self.scopes.slot(slot).target {
            Some(t) if t.height() == height && t.width() == width => t.clone(),
            _ => {
                let t = self.make_target("analysis", width, height);
                self.scopes.slot_mut(slot).target = Some(t.clone());
                t
            }
        };
        let readback = self.scopes.slot(slot);
        let bytes = (taken.scope.bin_count() * 4) as u64;
        self.queue.write_buffer(
            &readback.mode,
            0,
            bytemuck::cast_slice(&[
                taken.scope.kind(),
                scope::COLUMNS as u32,
                scope::WHEEL as u32,
                scope::LEVELS as u32,
                u32::from(taken.weigh_by.is_some()),
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
        match taken.tap {
            None => self.draw(&mut encoder, &target, whole, false),
            Some(tap) => self.draw_tap(&mut encoder, &target, whole, tap),
        }
        encoder.clear_buffer(&readback.bins, 0, Some(bytes));
        {
            let view = target.create_view(&Default::default());
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
                        resource: readback.bins.as_entire_binding(),
                    },
                    gpu::BindGroupEntry {
                        binding: 2,
                        resource: readback.mode.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&gpu::ComputePassDescriptor {
                label: Some("scope"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.scopes.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        }
        encoder.copy_buffer_to_buffer(&readback.bins, 0, &readback.staging, 0, bytes);
        (encoder, target)
    }

    /// The navigator's picture read back since the last take: the
    /// whole frame under the edit as the screen shows it, through the
    /// monitor's table, 512 wide.
    pub fn take_navigator(&mut self) -> Option<Rgba8> {
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
            &buf.pixels,
            buf.width as usize,
            buf.height as usize,
            u,
            v,
            radius,
        )
    }
}

/// `v`'s whole frame drawn [`analysis_size`] wide for an analysis,
/// weighed by `weigh_by`, with none of the overlays the viewport
/// paints over it.
fn whole_frame(v: &View, weigh_by: Option<usize>, most: u32) -> View {
    let (iw, ih) = (v.frame_size.0.max(1.0), v.frame_size.1.max(1.0));
    let (width, _) = analysis_size(v.frame_size, most);
    View {
        zoom: width as f32 / iw.floor(),
        // A frame narrower than the analysis is read as it always
        // was, between its pixels: the blocks past 1:1 are for the
        // eye, and the scopes count colors.
        pixel: 0.0,
        center: (iw.floor() / 2.0, ih.floor() / 2.0),
        show_mask: None,
        show_sharpen: false,
        mask_alone: false,
        weigh_by,
        warn: Warn::default(),
        ..v.clone()
    }
}
