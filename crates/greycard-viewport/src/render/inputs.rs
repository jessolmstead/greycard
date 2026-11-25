use super::*;

impl Renderer {
    pub(super) fn lut_texture(
        device: &gpu::Device,
        queue: &gpu::Queue,
        lut: &Lut3d,
    ) -> gpu::Texture {
        let n = lut.size as u32;
        let texture = device.create_texture(&gpu::TextureDescriptor {
            label: Some("display lut"),
            size: gpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D3,
            format: gpu::TextureFormat::Rgba16Float,
            usage: gpu::TextureUsages::TEXTURE_BINDING | gpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // The alpha carries the proof's gamut mark.
        let halves: Vec<half::f16> = lut
            .rgb
            .iter()
            .enumerate()
            .flat_map(|(i, px)| {
                [
                    half::f16::from_f32(px[0]),
                    half::f16::from_f32(px[1]),
                    half::f16::from_f32(px[2]),
                    if lut.warn.get(i).copied().unwrap_or(false) {
                        half::f16::ONE
                    } else {
                        half::f16::ZERO
                    },
                ]
            })
            .collect();
        queue.write_texture(
            gpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: gpu::Origin3d::ZERO,
                aspect: gpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&halves),
            gpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 8),
                rows_per_image: Some(n),
            },
            gpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
        );
        texture
    }

    /// Use a display table from now on.
    /// The scopes bin the picture before it, but the navigator they
    /// read back beside the bins is drawn through it, so a new one is
    /// a new picture to them.
    pub fn set_display_lut(&mut self, lut: &Lut3d) {
        self.lut = Self::lut_texture(&self.device, &self.queue, lut);
        self.gamut_marks = !lut.warn.is_empty();
        self.scopes.output_changed();
        self.redraw = true;
    }

    /// Use this table for encoded pictures from now on. The scopes
    /// read nothing of those, so they are not told.
    pub fn set_encoded_lut(&mut self, lut: &Lut3d) {
        self.lut_encoded = Self::lut_texture(&self.device, &self.queue, lut);
    }

    /// The output space's matrix from the working space, rows.
    pub fn set_output(&mut self, matrix: [[f32; 3]; 3]) {
        if self.output != matrix {
            self.output = matrix;
            self.scopes.output_changed();
            self.redraw = true;
        }
    }

    /// A 3D texture of the look table, its values as they are: a
    /// `.cube` may hand back more than one, so half floats and not a
    /// normalized format.
    pub(super) fn look_texture(
        device: &gpu::Device,
        queue: &gpu::Queue,
        table: &lut::Lut3d,
    ) -> gpu::Texture {
        let n = table.size as u32;
        let texture = device.create_texture(&gpu::TextureDescriptor {
            label: Some("look lut"),
            size: gpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D3,
            format: gpu::TextureFormat::Rgba16Float,
            usage: gpu::TextureUsages::TEXTURE_BINDING | gpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let halves: Vec<half::f16> = table
            .data
            .iter()
            .flat_map(|px| {
                [
                    half::f16::from_f32(px[0]),
                    half::f16::from_f32(px[1]),
                    half::f16::from_f32(px[2]),
                    half::f16::ZERO,
                ]
            })
            .collect();
        queue.write_texture(
            gpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: gpu::Origin3d::ZERO,
                aspect: gpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&halves),
            gpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 8),
                rows_per_image: Some(n),
            },
            gpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
        );
        texture
    }

    /// The look the viewport finishes through from now on.
    ///
    /// The table is uploaded only when it is another table: the
    /// strength moves on every drag of the slider, and re-sending a
    /// megabyte of half floats for it would be a waste. A look at no
    /// strength still counts as a table and keeps its texture, so
    /// dragging the slider through zero and back costs nothing; only
    /// `None`, which is an edit that names no table at all, drops the
    /// texture and lets go of the table. The look is part of what the
    /// scopes bin, so a change is a new picture to them.
    pub fn set_look(&mut self, look: Option<&lut::Look>) {
        let was = self.look.params;
        match look {
            Some(look) => {
                let table = &look.lut;
                let same = self
                    .look
                    .of
                    .as_ref()
                    .is_some_and(|held| Arc::ptr_eq(held, table));
                if !same {
                    self.look.texture = Self::look_texture(&self.device, &self.queue, table);
                    self.look.of = Some(table.clone());
                    self.scopes.output_changed();
                    self.redraw = true;
                }
                let (which, gamma) = match table.encoding {
                    lut::Encoding::Srgb => (0.0, 0.0),
                    lut::Encoding::Rec709 => (1.0, 0.0),
                    lut::Encoding::Linear => (2.0, 0.0),
                    lut::Encoding::Gamma(g) => (3.0, g),
                };
                let row = |m: &[[f32; 3]; 3]| m.map(|r| [r[0], r[1], r[2], 0.0]);
                self.look.params = [look.strength, table.size as f32, which, gamma];
                let pad = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
                self.look.min = pad(table.domain_min);
                self.look.max = pad(table.domain_max);
                self.look.to_lut = row(look.to_lut());
                self.look.from_lut = row(look.from_lut());
            }
            None => {
                // No table named: let go of it, on both sides. A
                // 64-node table is 2 MB of texture and half a megabyte
                // of floats, and holding either after the section says
                // None is holding it for nothing. The binding always
                // needs something, so it goes back to the 2x2x2
                // identity, and a strength of zero is what the shader
                // reads as no look.
                if self.look.of.is_some() {
                    self.look.texture =
                        Self::look_texture(&self.device, &self.queue, &lut::Lut3d::identity(2));
                    self.look.of = None;
                }
                self.look.params = [0.0, 2.0, 0.0, 0.0];
            }
        }
        if was != self.look.params {
            self.scopes.output_changed();
            self.redraw = true;
        }
    }

    pub(super) fn guide_texture(device: &gpu::Device, width: u32, height: u32) -> gpu::Texture {
        device.create_texture(&gpu::TextureDescriptor {
            label: Some("tone guide"),
            size: gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: gpu::TextureFormat::R16Float,
            usage: gpu::TextureUsages::TEXTURE_BINDING | gpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    pub(super) const TYPICAL_RANGE: [f32; 4] = [
        greycard_render::finish::FRAME_TYPICAL[0],
        greycard_render::finish::FRAME_TYPICAL[1],
        0.0,
        0.0,
    ];

    /// Put the tone equalizer's guide plane on the GPU. Its texels
    /// stand for `scale` source pixels each way, so the shader divides
    /// a source position by what the texture covers to sample it.
    pub fn set_guide(&mut self, guide: &greycard_render::finish::Guide) {
        self.redraw = true;
        let (w, h) = (guide.width as u32, guide.height as u32);
        if guide.data.is_empty() || w == 0 || h == 0 {
            self.guide_cover = [1.0, 1.0, 0.0, 0.0];
            self.frame_range = Self::TYPICAL_RANGE;
            return;
        }
        self.frame_range = [guide.range[0], guide.range[1], 0.0, 0.0];
        self.guide = Self::guide_texture(&self.device, w, h);
        let halves: Vec<half::f16> = guide.data.iter().map(|v| half::f16::from_f32(*v)).collect();
        self.queue.write_texture(
            gpu::TexelCopyTextureInfo {
                texture: &self.guide,
                mip_level: 0,
                origin: gpu::Origin3d::ZERO,
                aspect: gpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&halves),
            gpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 2),
                rows_per_image: Some(h),
            },
            gpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let cover = guide.scale as f32;
        self.guide_cover = [w as f32 * cover, h as f32 * cover, 1.0, 0.0];
        self.scopes.image_changed();
        self.redraw = true;
    }

    pub(super) fn detail_texture(device: &gpu::Device, width: u32, height: u32) -> gpu::Texture {
        device.create_texture(&gpu::TextureDescriptor {
            label: Some("detail planes"),
            size: gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: gpu::TextureFormat::Rgba16Float,
            usage: gpu::TextureUsages::TEXTURE_BINDING | gpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    pub(super) fn sharpen_texture(device: &gpu::Device, width: u32, height: u32) -> gpu::Texture {
        device.create_texture(&gpu::TextureDescriptor {
            label: Some("sharpen delta"),
            size: gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: gpu::TextureFormat::R16Float,
            usage: gpu::TextureUsages::TEXTURE_BINDING | gpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    pub(super) fn sample_texture(device: &gpu::Device, width: u32, height: u32) -> gpu::Texture {
        device.create_texture(&gpu::TextureDescriptor {
            label: Some("range sample"),
            size: gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: gpu::TextureFormat::Rgba16Float,
            usage: gpu::TextureUsages::TEXTURE_BINDING | gpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// The range masks' picture from before the Detail section, from
    /// now on: held, and put on the GPU only while a drawn mask reads
    /// it ([`Self::sync_sample`]).
    fn set_sample(&mut self, sample: Option<&Arc<greycard_core::image::WorkingImage>>) {
        let sample = sample.filter(|s| s.width > 0 && s.height > 0);
        if let (Some(s), Some(h)) = (sample, &self.sample_held)
            && Arc::ptr_eq(s, h)
        {
            return;
        }
        self.sample_held = sample.cloned();
        if self.sample_of.is_some() {
            self.drop_sample();
        }
    }

    /// The range masks' texture back to a texel.
    fn drop_sample(&mut self) {
        self.sample = Self::sample_texture(&self.device, 1, 1);
        self.sample_of = None;
        self.redraw = true;
        self.scopes.image_changed();
    }

    /// The range masks' picture on the GPU while `v` draws a mask that
    /// reads it, an adjustment switched on with a live range shape or
    /// the one whose mask is shown, and back to a texel when none does:
    /// 360 MB at 45 MP held only for a mask that reads it. Uploaded
    /// once a picture; a Detail slider's develop hands on the same one.
    pub(super) fn sync_sample(&mut self, v: &View) {
        let wanted = self.sample_held.is_some()
            && v.locals.iter().take(MAX_LOCALS).enumerate().any(|(k, l)| {
                l.refined.is_none()
                    && l.mask.reads_picture()
                    && (l.enabled || v.show_mask == Some(k))
            });
        if !wanted {
            if self.sample_of.is_some() {
                self.drop_sample();
            }
            return;
        }
        let Some(sample) = self.sample_held.clone() else {
            return;
        };
        if self
            .sample_of
            .as_ref()
            .and_then(|w| w.upgrade())
            .is_some_and(|h| Arc::ptr_eq(&h, &sample))
        {
            return;
        }
        // At the source's size, so reduced as it was where it was past
        // the device's textures.
        let halves = Halves::from_image(&sample, None);
        let (w, h, pixels) = self.fitted::<4>(halves.width, halves.height, &halves.pixels);
        self.sample = Self::sample_texture(&self.device, w, h);
        self.queue.write_texture(
            gpu::TexelCopyTextureInfo {
                texture: &self.sample,
                mip_level: 0,
                origin: gpu::Origin3d::ZERO,
                aspect: gpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&pixels),
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
        self.sample_of = Some(Arc::downgrade(&sample));
        self.redraw = true;
        self.scopes.image_changed();
    }

    /// Whether the range masks read a picture of their own.
    pub(super) fn has_sample(&self) -> bool {
        self.sample_of.is_some()
    }

    /// Put the masks' detail planes on the GPU, or take them down: the
    /// planes of the develop the source is, at its size, read at the
    /// texel the source is read at; and the range masks' picture that
    /// comes with them.
    ///
    /// The same bands landing again with a develop that kept them are
    /// not uploaded again: 360 MB at 45 MP.
    pub fn set_detail(&mut self, planes: Option<&Arc<greycard_render::detail::DetailPlanes>>) {
        self.set_sample(planes.and_then(|p| p.sample.as_ref()));
        let same = |a: &std::sync::Weak<[half::f16]>, b: &Arc<[half::f16]>| {
            a.upgrade().is_some_and(|a| Arc::ptr_eq(&a, b))
        };
        if let (Some(p), Some((data, sharpen, meta))) = (planes, &self.detail_of)
            && same(data, &p.data)
            && match (sharpen, &p.sharpen) {
                (Some(a), Some(b)) => same(a, b),
                (None, None) => true,
                _ => false,
            }
            && *meta == p.meta
        {
            return;
        }
        self.redraw = true;
        self.scopes.image_changed();
        self.detail_of = planes.map(|p| {
            (
                Arc::downgrade(&p.data),
                p.sharpen.as_ref().map(Arc::downgrade),
                p.meta,
            )
        });
        let Some(planes) = planes.filter(|p| p.width > 0 && p.height > 0) else {
            if self.detail_meta[3] != 0.0 {
                self.detail = Self::detail_texture(&self.device, 1, 1);
            }
            if self.detail_clip[3] != 0.0 {
                self.sharpen = Self::sharpen_texture(&self.device, 1, 1);
            }
            self.detail_meta = [1.0, 1.0, 1.0, 0.0];
            self.detail_clip = [-1.0, 0.0, 0.0, 0.0];
            return;
        };
        // The shader reads them only at the source's size; another
        // size is a bug in whoever carried them, said here. The full
        // size: past the device's textures both go up reduced alike.
        if self.source.is_some() {
            greycard_render::detail::fits(
                (planes.width, planes.height),
                self.full.0 as usize,
                self.full.1 as usize,
                "the viewport",
            );
        }
        let (pw, ph) = (planes.width as u32, planes.height as u32);
        let (w, h) = reduce::reduced_size(pw, ph, reduce::factor(pw, ph, self.most_side));
        let extent = gpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        // The sharpen's delta, a lane of its own beside the four.
        match &planes.sharpen {
            Some(delta) => {
                let (_, _, delta) = self.fitted::<1>(pw, ph, delta);
                self.sharpen = Self::sharpen_texture(&self.device, w, h);
                self.queue.write_texture(
                    gpu::TexelCopyTextureInfo {
                        texture: &self.sharpen,
                        mip_level: 0,
                        origin: gpu::Origin3d::ZERO,
                        aspect: gpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&delta),
                    gpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 2),
                        rows_per_image: Some(h),
                    },
                    extent,
                );
            }
            None if self.detail_clip[3] != 0.0 => {
                self.sharpen = Self::sharpen_texture(&self.device, 1, 1);
            }
            None => {}
        }
        // Negated over a picture never sharpened, where the shader
        // holds a mask's share at zero from below.
        let pedestal = match (&planes.sharpen, planes.meta.sharpened) {
            (Some(_), true) => planes.meta.pedestal,
            (Some(_), false) => -planes.meta.pedestal,
            (None, _) => 0.0,
        };
        let a = planes.meta.airlight;
        let clip = planes.meta.clip_level;
        self.detail_clip = [
            if clip.is_finite() { clip } else { -1.0 },
            greycard_core::develop::local_contrast::CLIP_FADE,
            0.0,
            pedestal,
        ];
        if !planes.has_bands() {
            if self.detail_meta[3] != 0.0 {
                self.detail = Self::detail_texture(&self.device, 1, 1);
            }
            self.detail_meta = [a[0], a[1], a[2], 0.0];
            return;
        }
        let (_, _, bands) = self.fitted::<4>(pw, ph, &planes.data);
        self.detail = Self::detail_texture(&self.device, w, h);
        self.queue.write_texture(
            gpu::TexelCopyTextureInfo {
                texture: &self.detail,
                mip_level: 0,
                origin: gpu::Origin3d::ZERO,
                aspect: gpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&bands),
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
        self.detail_meta = [a[0], a[1], a[2], 1.0];
    }

    /// A plane of half floats at the source's size, `C` a texel, as it
    /// goes on the device: itself where it fits, reduced by the
    /// factor the source was ([`reduce`]) where it is past the
    /// device's textures; with the size it goes up at.
    fn fitted<'a, const C: usize>(
        &self,
        width: u32,
        height: u32,
        data: &'a [half::f16],
    ) -> (u32, u32, std::borrow::Cow<'a, [half::f16]>) {
        let k = reduce::factor(width, height, self.most_side);
        if k == 1 {
            return (width, height, data.into());
        }
        let (w, h) = reduce::reduced_size(width, height, k);
        let reduced = reduce::halves::<C>(data, width as usize, height as usize, k as usize);
        (w, h, reduced.into())
    }
}
