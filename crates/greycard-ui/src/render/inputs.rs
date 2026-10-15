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
    /// The table is the last step of what the scopes bin, so a new
    /// one is a new picture to them.
    pub fn set_display_lut(&mut self, lut: &Lut3d) {
        self.lut = Self::lut_texture(&self.device, &self.queue, lut);
        self.scopes.image_changed = true;
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
            self.scopes.image_changed = true;
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
                    self.scopes.image_changed = true;
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
            self.scopes.image_changed = true;
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
        crate::finish::FRAME_TYPICAL[0],
        crate::finish::FRAME_TYPICAL[1],
        0.0,
        0.0,
    ];

    /// Put the tone equalizer's guide plane on the GPU. Its texels
    /// stand for `scale` source pixels each way, so the shader divides
    /// a source position by what the texture covers to sample it.
    pub fn set_guide(&mut self, guide: &crate::finish::Guide) {
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
        self.scopes.image_changed = true;
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

    /// Put the masks' detail planes on the GPU, or take them down: the
    /// planes of the develop the source is, at its size, read at the
    /// texel the source is read at.
    ///
    /// The same planes landing again with a develop that kept them are
    /// not uploaded again: 360 MB at 45 MP.
    pub fn set_detail(&mut self, planes: Option<&Arc<crate::detail::DetailPlanes>>) {
        if let (Some(p), Some(up)) = (planes, self.detail_of.as_ref().and_then(|w| w.upgrade()))
            && Arc::ptr_eq(p, &up)
        {
            return;
        }
        self.redraw = true;
        self.scopes.image_changed = true;
        self.detail_of = planes.map(Arc::downgrade);
        let Some(planes) = planes.filter(|p| p.width > 0 && p.height > 0) else {
            if self.detail_meta[3] != 0.0 {
                self.detail = Self::detail_texture(&self.device, 1, 1);
            }
            self.detail_meta = [1.0, 1.0, 1.0, 0.0];
            self.detail_clip = [-1.0, 0.0, 0.0, 0.0];
            return;
        };
        // The shader reads them only at the source's size; another
        // size is a bug in whoever carried them, said here.
        if let Some(source) = &self.source {
            crate::detail::fits(
                (planes.width, planes.height),
                source.width() as usize,
                source.height() as usize,
                "the viewport",
            );
        }
        let (w, h) = (planes.width as u32, planes.height as u32);
        self.detail = Self::detail_texture(&self.device, w, h);
        self.queue.write_texture(
            gpu::TexelCopyTextureInfo {
                texture: &self.detail,
                mip_level: 0,
                origin: gpu::Origin3d::ZERO,
                aspect: gpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&planes.data),
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
        let a = planes.meta.airlight;
        self.detail_meta = [a[0], a[1], a[2], 1.0];
        let clip = planes.meta.clip_level;
        self.detail_clip = [
            if clip.is_finite() { clip } else { -1.0 },
            greycard_core::develop::local_contrast::CLIP_FADE,
            0.0,
            0.0,
        ];
    }
}
