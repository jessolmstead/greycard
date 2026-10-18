use super::*;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    view: [f32; 2],
    image: [f32; 2],
    center: [f32; 2],
    frame_origin: [f32; 2],
    frame_size: [f32; 2],
    /// The plane's size, the rows of the plane-to-source matrix, the
    /// perspective's row in its xy (`Geometry::perspective`), and
    /// whether it resamples.
    plane: [f32; 2],
    turn0: [f32; 2],
    turn1: [f32; 2],
    persp: [f32; 4],
    /// Its x; y paints the sharpen's mask, from the source's alpha;
    /// z says the source is encoded sRGB already (the camera's JPEG
    /// in the culling loupe) and goes to the monitor's table and
    /// nowhere else; w keeps the vec4s that follow aligned.
    cubic: [f32; 4],
    /// Its x: which clipping warnings to paint, 1 the shadows, 2 the
    /// highlights, 3 both (`Warn::bits`).
    clip: [f32; 4],
    zoom: f32,
    exposure: f32,
    curve: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    mixer: f32,
    /// The color curves shift something.
    shaded: f32,
    /// How many local adjustments, and which one's mask to paint (its
    /// index plus one; nothing at zero).
    locals: f32,
    show_mask: f32,
    /// Global vibrance and saturation, in the same pass as the mixer.
    color: f32,
    saturation: f32,
    vibrance: f32,
    /// Quarter turns clockwise the source texture stands behind the
    /// source the view is in (`View::source_turn`). Also keeps `m0`
    /// 16-byte aligned.
    source_turn: f32,
    m0: [f32; 4],
    m1: [f32; 4],
    m2: [f32; 4],
    w0: [f32; 4],
    w1: [f32; 4],
    w2: [f32; 4],
    ok_in: [[f32; 4]; 3],
    ok_out: [[f32; 4]; 3],
    mix_hue: [[f32; 4]; 2],
    mix_sat: [[f32; 4]; 2],
    mix_lum: [[f32; 4]; 2],
    /// Amount (stops), midpoint, feather, roundness.
    vignette: [f32; 4],
    /// Amount, the lattice's level and how far through its octave the
    /// size is (`Grain::level`), then the kind's character
    /// (`grain.rs`): radius; spread, floor, tint, norm.
    grain0: [f32; 4],
    grain1: [f32; 4],
    /// The canvas outside the frame, encoded: the panel's choice, not
    /// the picture's, so it skips the display table.
    canvas: [f32; 4],
    /// The black and white: its switch in x, then the eight bands'
    /// weights. Global only, so no local carries one.
    bw: [f32; 4],
    bw_w: [[f32; 4]; 2],
    /// The global look's tint as a vector, its amount in its hue's
    /// direction, in x and y (`tint.rs`); each local adds its own.
    tint: [f32; 4],
    /// The tone equalizer's guide plane: in xy the source pixels its
    /// texture covers (its size times the source pixels to a texel),
    /// which the shader's `guide_at` reads the source pixels to a texel
    /// from, as `Guide::at` does; in z whether there is one.
    guide: [f32; 4],
    /// The frame's own range, its median and top in scene stops over
    /// mid grey (`Guide::range`), in x and y: where the highlights ramp
    /// sits (`finish::highlights_span`). A typical frame's when there
    /// is no plane, as the CPU has it.
    frame_range: [f32; 4],
    /// The target pixel this view's top left corner sits at, for a
    /// view drawn into part of the target (the compare view's tiles);
    /// zero for the whole of it.
    tile: [f32; 4],
    /// The look table: its strength in x, its nodes an axis in y,
    /// which transfer function its input is in in z (`lut::Encoding`,
    /// in the shader's order) and a plain gamma's exponent in w.
    look: [f32; 4],
    /// What its first and last nodes stand for, per channel.
    look_min: [f32; 4],
    look_max: [f32; 4],
    /// The working space to the table's primaries and back, rows.
    look_in: [[f32; 4]; 3],
    look_out: [[f32; 4]; 3],
    /// The range masks: in x what of the picture a live shape reads,
    /// so the shader samples it once a pixel (`finish::sample`): 0
    /// nothing, 1 its lightness, 2 its color too, which is the mean
    /// about it. In y whether to draw the shown mask's weight alone,
    /// as grey, in place of the picture, for measuring it against the
    /// CPU's; in z the sample's exposure, the global one without the
    /// baseline.
    range: [f32; 4],
    /// The masks' Detail (`crate::detail`): the airlight in xyz and in
    /// w whether there are planes to read; the gains and the dehaze's
    /// strength and floor; the develop's clip level in x (none under
    /// zero) and the fade's start as a share of it in y.
    detail: [f32; 4],
    detail_k: [f32; 4],
    detail_clip: [f32; 4],
    /// The display curve's white where the frame's baseline puts the
    /// sensor's clip (`finish::White`): in scene light, in stops over
    /// mid grey, the shoulder's gain there, and AgX's shoulder scale.
    white: [f32; 4],
    /// AgX fitted to that white (`agx::shader_params`): its gain, its
    /// range, its pivot and its toe scale.
    agx: [f32; 4],
}

impl Renderer {
    pub(super) fn make_target(&self, label: &str, width: u32, height: u32) -> gpu::Texture {
        self.device.create_texture(&gpu::TextureDescriptor {
            label: Some(label),
            size: gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: gpu::TextureUsages::RENDER_ATTACHMENT
                | gpu::TextureUsages::TEXTURE_BINDING
                | gpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// Draw the view into `target` through the viewport shader.
    pub(super) fn draw(&self, encoder: &mut gpu::CommandEncoder, target: &gpu::Texture, v: &View) {
        self.draw_source(encoder, target, v, self.source.as_ref(), self.encoded, None);
    }

    /// Draw the view of `source` into `target`, or into `rect` of it
    /// (x, y, width, height) with the rest left as it is. An
    /// `encoded` source is sRGB bytes already and skips the look.
    fn draw_source(
        &self,
        encoder: &mut gpu::CommandEncoder,
        target: &gpu::Texture,
        v: &View,
        source: Option<&gpu::Texture>,
        encoded: bool,
        rect: Option<(u32, u32, u32, u32)>,
    ) {
        // The source the view is in: the texture's, stood on end by
        // an odd turn the texture has not caught up with.
        let image = source
            .map(|s| {
                let (w, h) = (s.width() as f32, s.height() as f32);
                if v.source_turn % 2 == 1 {
                    [h, w]
                } else {
                    [w, h]
                }
            })
            .unwrap_or([1.0, 1.0]);
        let (ox, oy, tw, th) = rect.unwrap_or((0, 0, target.width(), target.height()));
        let m = self.output;
        let rail = crate::agx::rail_weights_for(&m);
        let w = v.white;
        let display = crate::finish::White::of(v.source, &v.light);
        let agx = crate::agx::shader_params(display.agx);
        let params = Params {
            view: [tw as f32, th as f32],
            image,
            center: [v.center.0, v.center.1],
            frame_origin: [v.frame_origin.0, v.frame_origin.1],
            frame_size: [v.frame_size.0, v.frame_size.1],
            plane: [v.plane.0, v.plane.1],
            turn0: v.matrix[0],
            turn1: v.matrix[1],
            persp: [v.persp[0], v.persp[1], 0.0, 0.0],
            cubic: [
                if v.cubic { 1.0 } else { 0.0 },
                if v.show_sharpen { 1.0 } else { 0.0 },
                if encoded { 1.0 } else { 0.0 },
                0.0,
            ],
            clip: [v.warn.bits() as f32, 0.0, 0.0, 0.0],
            zoom: v.zoom,
            exposure: v.source.baseline(&v.light) + v.light.exposure,
            // The shape, then 0 the display curve per channel, 1 a
            // clip, for a picture that has had its curve, 2 AgX.
            curve: match (v.source, v.display_curve) {
                (Source::Scene(_), greycard_edit::DisplayCurve::Channels) => 0.0,
                (Source::Display, _) => 1.0,
                (Source::Scene(_), greycard_edit::DisplayCurve::Agx) => 2.0,
            },
            contrast: v.light.tone.contrast,
            highlights: v.light.tone.highlights,
            shadows: v.light.tone.shadows,
            whites: v.light.tone.whites,
            blacks: v.light.tone.blacks,
            mixer: if v.mixer.enabled { 1.0 } else { 0.0 },
            shaded: if v.curves.shaded { 1.0 } else { 0.0 },
            locals: v.locals.len().min(MAX_LOCALS) as f32,
            show_mask: v.show_mask.map(|i| i as f32 + 1.0).unwrap_or(0.0),
            color: if v.color.enabled { 1.0 } else { 0.0 },
            saturation: v.color.saturation,
            vibrance: v.color.vibrance,
            source_turn: f32::from(v.source_turn % 4),
            // The fourth lane carries the AgX output rail's luminance
            // weights for this output space (`agx::rail_weights_for`).
            m0: [m[0][0], m[0][1], m[0][2], rail[0]],
            m1: [m[1][0], m[1][1], m[1][2], rail[1]],
            m2: [m[2][0], m[2][1], m[2][2], rail[2]],
            w0: [w[0][0], w[0][1], w[0][2], 0.0],
            w1: [w[1][0], w[1][1], w[1][2], 0.0],
            w2: [w[2][0], w[2][1], w[2][2], 0.0],
            ok_in: self.oklab.to_lms.map(|r| [r[0], r[1], r[2], 0.0]),
            ok_out: self.oklab.from_lms.map(|r| [r[0], r[1], r[2], 0.0]),
            mix_hue: halves(&v.mixer.hue),
            mix_sat: halves(&v.mixer.saturation),
            mix_lum: halves(&v.mixer.luminance),
            // The amounts through the off-tests, as the export reads
            // them: the shader takes a zero amount as nothing to do.
            vignette: [
                if v.vignette.is_off() {
                    0.0
                } else {
                    v.vignette.amount
                },
                v.vignette.midpoint,
                v.vignette.feather,
                v.vignette.roundness,
            ],
            grain0: {
                let c = v.grain.kind.character();
                let (level, f) = v.grain.level();
                let amount = if v.grain.is_off() {
                    0.0
                } else {
                    v.grain.amount
                };
                [amount, level as f32, f, c.radius]
            },
            grain1: {
                let c = v.grain.kind.character();
                [c.spread, c.floor, c.tint, c.norm]
            },
            canvas: [v.canvas[0], v.canvas[1], v.canvas[2], 1.0],
            bw: [
                if v.bw.enabled { 1.0 } else { 0.0 },
                v.bw.strength,
                0.0,
                0.0,
            ],
            bw_w: halves(&v.bw.weights),
            tint: {
                let t = v.tint.vector();
                [t[0], t[1], 0.0, 0.0]
            },
            guide: self.guide_cover,
            frame_range: self.frame_range,
            tile: [ox as f32, oy as f32, 0.0, 0.0],
            look: self.look.params,
            look_min: self.look.min,
            look_max: self.look.max,
            look_in: self.look.to_lut,
            look_out: self.look.from_lut,
            range: [
                range_reads(&v.locals),
                if v.mask_alone { 1.0 } else { 0.0 },
                v.light.exposure,
                v.weigh_by.map(|k| k as f32 + 1.0).unwrap_or(0.0),
            ],
            detail: self.detail_meta,
            detail_k: [
                greycard_core::develop::local_contrast::TEXTURE_GAIN,
                greycard_core::develop::local_contrast::CLARITY_GAIN,
                greycard_core::develop::dehaze::FULL_STRENGTH,
                greycard_core::develop::dehaze::MIN_TRANSMISSION,
            ],
            detail_clip: self.detail_clip,
            white: [display.white, display.stops, display.gain, agx[4]],
            agx: [agx[0], agx[1], agx[2], agx[3]],
        };
        // Each draw has its own uniform buffer, since two draws share a
        // submission.
        let params_buffer = self.device.create_buffer(&gpu::BufferDescriptor {
            label: Some("viewport params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: gpu::BufferUsages::UNIFORM | gpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));
        // The tables as the shader has them: the tone table, then the
        // color shifts padded to vec4s.
        let mut tables = [[0.0f32; 4]; 2 * LUT_SIZE];
        tables[..LUT_SIZE].copy_from_slice(&v.curves.tone);
        for (t, c) in tables[LUT_SIZE..].iter_mut().zip(&v.curves.color) {
            *t = [c[0], c[1], 0.0, 0.0];
        }
        let curves_buffer = self.device.create_buffer(&gpu::BufferDescriptor {
            label: Some("viewport curves"),
            size: std::mem::size_of_val(&tables) as u64,
            usage: gpu::BufferUsages::UNIFORM | gpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&curves_buffer, 0, bytemuck::cast_slice(&tables));
        let LocalsGpu {
            params: local_params,
            tables: local_tables,
            shapes,
            ..
        } = locals_gpu(&v.locals);
        let storage = |label: &str, bytes: &[u8]| {
            let buffer = self.device.create_buffer(&gpu::BufferDescriptor {
                label: Some(label),
                size: bytes.len() as u64,
                usage: gpu::BufferUsages::STORAGE | gpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&buffer, 0, bytes);
            buffer
        };
        let locals_buffer = storage("viewport locals", bytemuck::cast_slice(&local_params));
        let local_tables_buffer =
            storage("viewport local tables", bytemuck::cast_slice(&local_tables));
        let shapes_buffer = storage("viewport shapes", bytemuck::cast_slice(&shapes));
        let view = target.create_view(&Default::default());
        // A tile is drawn over what the target holds; the whole
        // target starts from a clear.
        let load = if rect.is_some() {
            gpu::LoadOp::Load
        } else {
            gpu::LoadOp::Clear(gpu::Color {
                r: 0.004,
                g: 0.004,
                b: 0.004,
                a: 1.0,
            })
        };
        let mut pass = encoder.begin_render_pass(&gpu::RenderPassDescriptor {
            label: Some("viewport"),
            color_attachments: &[Some(gpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: gpu::Operations {
                    load,
                    store: gpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            ..Default::default()
        });
        if rect.is_some() {
            pass.set_viewport(ox as f32, oy as f32, tw as f32, th as f32, 0.0, 1.0);
            pass.set_scissor_rect(ox, oy, tw, th);
        }
        if let Some(source) = source {
            let source_view = source.create_view(&Default::default());
            let lut = if encoded {
                &self.lut_encoded
            } else {
                &self.lut
            };
            let lut_view = lut.create_view(&Default::default());
            let brushes_view = self.brushes.view();
            let guide_view = self.guide.create_view(&Default::default());
            let detail_view = self.detail.create_view(&Default::default());
            let look_view = self.look.texture.create_view(&Default::default());
            let bind_group = self.device.create_bind_group(&gpu::BindGroupDescriptor {
                label: Some("viewport"),
                layout: &self.layout,
                entries: &[
                    gpu::BindGroupEntry {
                        binding: 0,
                        resource: params_buffer.as_entire_binding(),
                    },
                    gpu::BindGroupEntry {
                        binding: 1,
                        resource: gpu::BindingResource::TextureView(&source_view),
                    },
                    gpu::BindGroupEntry {
                        binding: 2,
                        resource: gpu::BindingResource::Sampler(&self.sampler),
                    },
                    gpu::BindGroupEntry {
                        binding: 3,
                        resource: gpu::BindingResource::TextureView(&lut_view),
                    },
                    gpu::BindGroupEntry {
                        binding: 4,
                        resource: gpu::BindingResource::Sampler(&self.lut_sampler),
                    },
                    gpu::BindGroupEntry {
                        binding: 5,
                        resource: curves_buffer.as_entire_binding(),
                    },
                    gpu::BindGroupEntry {
                        binding: 6,
                        resource: locals_buffer.as_entire_binding(),
                    },
                    gpu::BindGroupEntry {
                        binding: 7,
                        resource: local_tables_buffer.as_entire_binding(),
                    },
                    gpu::BindGroupEntry {
                        binding: 8,
                        resource: shapes_buffer.as_entire_binding(),
                    },
                    gpu::BindGroupEntry {
                        binding: 9,
                        resource: gpu::BindingResource::TextureView(&brushes_view),
                    },
                    gpu::BindGroupEntry {
                        binding: 10,
                        resource: gpu::BindingResource::TextureView(&guide_view),
                    },
                    gpu::BindGroupEntry {
                        binding: 11,
                        resource: gpu::BindingResource::TextureView(&look_view),
                    },
                    gpu::BindGroupEntry {
                        binding: 12,
                        resource: gpu::BindingResource::TextureView(&detail_view),
                    },
                ],
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Show a developed picture that is on the GPU already: an
    /// engine op's texture, the sharpen's mask in its alpha.
    pub fn set_source(&mut self, texture: gpu::Texture) {
        self.source = Some(texture);
        self.encoded = false;
        self.scopes.image_changed = true;
        self.redraw = true;
    }

    /// A picture of encoded sRGB bytes, RGBA, on the GPU: the camera's
    /// JPEG for the culling loupe. Shown through the monitor's table
    /// and nothing else of the shader.
    pub fn encoded_texture(&self, width: u32, height: u32, rgba: &[u8]) -> gpu::Texture {
        let texture = self.device.create_texture(&gpu::TextureDescriptor {
            label: Some("camera preview"),
            size: gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: gpu::TextureFormat::Rgba8Unorm,
            usage: gpu::TextureUsages::TEXTURE_BINDING | gpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            gpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: gpu::Origin3d::ZERO,
                aspect: gpu::TextureAspect::All,
            },
            rgba,
            gpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        texture
    }

    /// Draw `tiles` of encoded pictures, each its own view into its
    /// own rectangle of a `width` by `height` target: the culling
    /// loupe, and its compare view. The scopes read nothing of it.
    pub fn render_encoded(
        &mut self,
        width: u32,
        height: u32,
        canvas: [f32; 3],
        tiles: &[Tile],
    ) -> gpu::Texture {
        let target = match &self.target {
            Some(t) if t.width() == width && t.height() == height => t.clone(),
            _ => {
                let t = self.make_target("viewport target", width, height);
                self.target = Some(t.clone());
                t
            }
        };
        let mut encoder = self
            .device
            .create_command_encoder(&gpu::CommandEncoderDescriptor {
                label: Some("viewport"),
            });
        // The clear with nothing drawn, so a tile still waiting for
        // its picture shows the canvas.
        let blank = View {
            canvas,
            ..View::blank()
        };
        self.draw_source(&mut encoder, &target, &blank, None, true, None);
        for tile in tiles {
            self.draw_source(
                &mut encoder,
                &target,
                &tile.view,
                Some(tile.texture),
                true,
                Some(tile.rect),
            );
        }
        self.queue.submit(Some(encoder.finish()));
        target
    }

    /// Put a developed image on the GPU.
    pub fn upload(&mut self, image: &Halves) {
        self.redraw = true;
        let (w, h) = (image.width, image.height);
        let halves = &image.pixels;
        let texture = self.device.create_texture(&gpu::TextureDescriptor {
            label: Some("developed"),
            size: gpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: gpu::TextureFormat::Rgba16Float,
            usage: gpu::TextureUsages::TEXTURE_BINDING
                | gpu::TextureUsages::COPY_DST
                | gpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.queue.write_texture(
            gpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: gpu::Origin3d::ZERO,
                aspect: gpu::TextureAspect::All,
            },
            bytemuck::cast_slice(halves),
            gpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: Some(h),
            },
            gpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.source = Some(texture);
        self.encoded = false;
        self.scopes.image_changed = true;
        self.redraw = true;
    }

    /// Draw the view: `width` by `height` display pixels, `zoom` display
    /// pixels per image pixel, the image pixel `center` in the middle.
    pub fn render(&mut self, width: u32, height: u32, v: &View) -> (gpu::Texture, bool) {
        let same = !self.redraw
            && self
                .drawn
                .as_ref()
                .is_some_and(|(w, h, d)| *w == width && *h == height && d == v);
        if same && let Some(t) = &self.target {
            return (t.clone(), false);
        }
        if std::env::var_os("GREYCARD_UI_TIMING").is_some() {
            let why = if self.redraw {
                "flag".to_string()
            } else if let Some((w, h, d)) = &self.drawn {
                if *w != width || *h != height {
                    "size".to_string()
                } else {
                    let mut f = Vec::new();
                    macro_rules! diff { ($($n:ident),*) => { $( if d.$n != v.$n { f.push(stringify!($n)); } )* } }
                    diff!(
                        zoom,
                        center,
                        light,
                        mixer,
                        color,
                        bw,
                        tint,
                        matrix,
                        persp,
                        plane,
                        cubic,
                        frame_origin,
                        frame_size,
                        curves,
                        white,
                        locals,
                        show_mask,
                        show_sharpen,
                        mask_alone,
                        weigh_by,
                        vignette,
                        grain,
                        warn,
                        canvas,
                        source,
                        display_curve,
                        source_turn
                    );
                    format!("{f:?}")
                }
            } else {
                "first".to_string()
            };
            tracing::info!("viewport drawn: {why}");
        }
        self.redraw = false;
        self.drawn = Some((width, height, v.clone()));
        self.brushes
            .sync(&self.device, &self.queue, &locals_gpu(&v.locals).rasters);
        let target = match &self.target {
            Some(t) if t.width() == width && t.height() == height => t.clone(),
            _ => {
                let t = self.make_target("viewport target", width, height);
                self.target = Some(t.clone());
                t
            }
        };
        let mut encoder = self
            .device
            .create_command_encoder(&gpu::CommandEncoderDescriptor {
                label: Some("viewport"),
            });
        self.draw(&mut encoder, &target, v);
        self.queue.submit(Some(encoder.finish()));
        (target, true)
    }

    /// The target as an 8-bit sRGB image, for a screenshot.
    pub fn read_back(&self, texture: &gpu::Texture) -> Result<image::RgbaImage> {
        let (w, h) = (texture.width(), texture.height());
        let row = (w * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&gpu::BufferDescriptor {
            label: Some("read back"),
            size: (row * h) as u64,
            usage: gpu::BufferUsages::COPY_DST | gpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&gpu::CommandEncoderDescriptor {
                label: Some("read back"),
            });
        encoder.copy_texture_to_buffer(
            gpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: gpu::Origin3d::ZERO,
                aspect: gpu::TextureAspect::All,
            },
            gpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: gpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            gpu::Extent3d {
                width: w,
                height: h,
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
            .context("waiting for the read back")?;
        rx.recv()
            .context("map callback")?
            .context("mapping the read back buffer")?;
        let data = slice
            .get_mapped_range()
            .context("reading the read back buffer")?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            out.extend_from_slice(&data[(y * row) as usize..(y * row + w * 4) as usize]);
        }
        drop(data);
        buffer.unmap();
        image::RgbaImage::from_raw(w, h, out).context("read back size")
    }
}
