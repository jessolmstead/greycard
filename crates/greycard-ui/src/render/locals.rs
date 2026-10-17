use super::*;

/// What of the picture the view's masks read, as `Params::range.x`
/// has it: switched on or not, since a switched-off adjustment's mask
/// can still be shown, and its weight is made whatever its switch.
pub(super) fn range_reads(locals: &[Local]) -> f32 {
    let locals = &locals[..locals.len().min(MAX_LOCALS)];
    if locals.iter().any(|l| l.mask.reads_color()) {
        2.0
    } else if locals.iter().any(|l| l.mask.reads_picture()) {
        1.0
    } else {
        0.0
    }
}

/// A local adjustment as the shader has it.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct LocalGpu {
    exposure: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    mixer: f32,
    shaded: f32,
    shapes_start: u32,
    shapes_count: u32,
    invert: u32,
    enabled: u32,
    color: f32,
    saturation: f32,
    vibrance: f32,
    /// Keeps what follows 16-byte aligned.
    pad0: f32,
    /// This adjustment's tint as a vector, in x and y.
    tint: [f32; 4],
    /// Its own Texture, Clarity and Dehaze amounts (`Baked::detail`),
    /// put on from the detail planes.
    detail: [f32; 4],
    /// Its own white balance's matrix, rows, in the first three of
    /// each; the first row's fourth is one when it has one.
    white: [[f32; 4]; 3],
    mix_hue: [[f32; 4]; 2],
    mix_sat: [[f32; 4]; 2],
    mix_lum: [[f32; 4]; 2],
}

/// A mask's shape as the shader has it. A brush is a layer of the
/// brush texture array (kind 2), with its raster's aspect in `b.x`;
/// a raster shape still waiting for its raster is kind 3, nothing,
/// and holds no layer. A luminance window is kind 4, its low, high
/// and their feathers in `a`; a color window kind 5, its hue, half
/// its width, its hue feather and its chroma floor in `a` and the
/// floor's feather in `b.x`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ShapeGpu {
    kind: u32,
    flags: u32,
    layer: u32,
    pad: u32,
    a: [f32; 4],
    b: [f32; 4],
}

impl ShapeGpu {
    fn of(c: &greycard_edit::mask::Component, raster: Option<&RasterRef>, layer: u32) -> Self {
        let flags = c.mode as u32 | ((c.invert as u32) << 2);
        match &c.shape {
            Shape::Linear { from, to } => Self {
                kind: 0,
                flags,
                layer: 0,
                pad: 0,
                a: [from[0], from[1], to[0], to[1]],
                b: [0.0; 4],
            },
            Shape::Radial {
                center,
                radius,
                angle,
                feather,
            } => Self {
                kind: 1,
                flags,
                layer: 0,
                pad: 0,
                a: [center[0], center[1], radius[0], radius[1]],
                b: [angle.to_radians(), *feather, 0.0, 0.0],
            },
            Shape::Luminance {
                low,
                high,
                low_feather,
                high_feather,
            } => Self {
                kind: 4,
                flags,
                layer: 0,
                pad: 0,
                a: [*low, *high, low_feather.max(0.0), high_feather.max(0.0)],
                b: [0.0; 4],
            },
            Shape::Color {
                hue,
                width,
                hue_feather,
                chroma,
                chroma_feather,
            } => Self {
                kind: 5,
                flags,
                layer: 0,
                pad: 0,
                a: [*hue, (width * 0.5).max(0.0), hue_feather.max(0.0), *chroma],
                b: [chroma_feather.max(0.0), 0.0, 0.0, 0.0],
            },
            // Without its raster — a model still working, its weights
            // not there, a brush not painted — it is nothing, as the
            // CPU reads a missing raster, and it takes no layer.
            // Never uploaded (`Mask::live` leaves it out); nothing if
            // it were.
            Shape::Unknown => Self {
                kind: 3,
                flags,
                layer: 0,
                pad: 0,
                a: [0.0; 4],
                b: [0.0; 4],
            },
            Shape::Brush { .. }
            | Shape::Subject {}
            | Shape::Background {}
            | Shape::Sky { .. }
            | Shape::Landscape { .. }
            | Shape::Object { .. }
            | Shape::Part { .. } => match raster {
                Some(r) => Self {
                    kind: 2,
                    flags,
                    layer,
                    pad: 0,
                    a: [0.0; 4],
                    b: [r.0.aspect, 0.0, 0.0, 0.0],
                },
                None => Self {
                    kind: 3,
                    flags,
                    layer: 0,
                    pad: 0,
                    a: [0.0; 4],
                    b: [0.0; 4],
                },
            },
        }
    }
}

/// The locals as the shader takes them.
pub(super) struct LocalsGpu {
    pub(super) params: Vec<LocalGpu>,
    pub(super) tables: Vec<[f32; 4]>,
    pub(super) shapes: Vec<ShapeGpu>,
    /// The brushes' rasters, in the order of their layers.
    pub(super) rasters: Vec<RasterRef>,
}

/// The locals' buffers: their parameters, their tables and their
/// shapes, never empty since a storage buffer cannot be; and their
/// brushes' rasters, a layer each.
pub(super) fn locals_gpu(locals: &[Local]) -> LocalsGpu {
    let mut params = Vec::new();
    let mut tables = Vec::new();
    let mut shapes = Vec::new();
    let mut rasters: Vec<RasterRef> = Vec::new();
    for local in locals.iter().take(MAX_LOCALS) {
        let b = &local.baked;
        // The shapes first, so a switched-off one is simply not
        // there: the count is of what was pushed, not of what the
        // mask holds. This is the shader's side of `Mask::live`.
        let start = shapes.len() as u32;
        for (i, c) in local.mask.live() {
            let raster = local.rasters.get(i).and_then(|r| r.as_ref());
            shapes.push(ShapeGpu::of(c, raster, rasters.len() as u32));
            if c.shape.is_raster()
                && let Some(r) = raster
            {
                rasters.push(r.clone());
            }
        }
        params.push(LocalGpu {
            exposure: b.light.exposure,
            contrast: b.light.tone.contrast,
            highlights: b.light.tone.highlights,
            shadows: b.light.tone.shadows,
            whites: b.light.tone.whites,
            blacks: b.light.tone.blacks,
            mixer: if b.mixer.enabled { 1.0 } else { 0.0 },
            shaded: if b.curves.shaded { 1.0 } else { 0.0 },
            shapes_start: start,
            shapes_count: shapes.len() as u32 - start,
            invert: local.mask.invert as u32,
            enabled: local.enabled as u32,
            color: if b.color.enabled { 1.0 } else { 0.0 },
            saturation: b.color.saturation,
            vibrance: b.color.vibrance,
            pad0: 0.0,
            tint: {
                let v = b.tint.vector();
                [v[0], v[1], 0.0, 0.0]
            },
            detail: [b.detail[0], b.detail[1], b.detail[2], 0.0],
            white: match &b.white {
                Some(m) => [
                    [m[0][0], m[0][1], m[0][2], 1.0],
                    [m[1][0], m[1][1], m[1][2], 0.0],
                    [m[2][0], m[2][1], m[2][2], 0.0],
                ],
                None => [[0.0; 4]; 3],
            },
            mix_hue: halves(&b.mixer.hue),
            mix_sat: halves(&b.mixer.saturation),
            mix_lum: halves(&b.mixer.luminance),
        });
        tables.extend_from_slice(&b.curves.tone);
        tables.extend(b.curves.color.iter().map(|c| [c[0], c[1], 0.0, 0.0]));
    }
    if params.is_empty() {
        params.push(bytemuck::Zeroable::zeroed());
        tables.push([0.0; 4]);
    }
    if shapes.is_empty() {
        shapes.push(bytemuck::Zeroable::zeroed());
    }
    LocalsGpu {
        params,
        tables,
        shapes,
        rasters,
    }
}

/// The brushes' rasters on the GPU: one texture array, a layer each,
/// a layer written again only when its raster changed.
pub(super) struct Brushes {
    texture: gpu::Texture,
    /// Each layer's raster: its allocation and version.
    layers: Vec<(usize, u64)>,
}

impl Brushes {
    /// One empty layer, so the binding always has a texture.
    pub(super) fn empty(device: &gpu::Device) -> Self {
        Self {
            texture: Self::texture(device, 1, 1, 1),
            layers: vec![(0, 0)],
        }
    }

    fn texture(device: &gpu::Device, width: u32, height: u32, layers: u32) -> gpu::Texture {
        device.create_texture(&gpu::TextureDescriptor {
            label: Some("brushes"),
            size: gpu::Extent3d {
                width,
                height,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: gpu::TextureFormat::R8Unorm,
            usage: gpu::TextureUsages::TEXTURE_BINDING | gpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    pub(super) fn view(&self) -> gpu::TextureView {
        self.texture.create_view(&gpu::TextureViewDescriptor {
            dimension: Some(gpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    }

    /// Bring the array to `rasters`: made anew when their number or
    /// size changes, each layer written when its raster did.
    pub(super) fn sync(&mut self, device: &gpu::Device, queue: &gpu::Queue, rasters: &[RasterRef]) {
        let Some(first) = rasters.first() else {
            if self.layers.len() != 1 || self.texture.width() != 1 {
                *self = Self::empty(device);
            }
            return;
        };
        let (w, h) = (first.0.width as u32, first.0.height as u32);
        if self.texture.width() != w
            || self.texture.height() != h
            || self.layers.len() != rasters.len()
        {
            self.texture = Self::texture(device, w, h, rasters.len() as u32);
            self.layers = vec![(0, 0); rasters.len()];
        }
        for (i, r) in rasters.iter().enumerate() {
            let key = (Arc::as_ptr(&r.0) as usize, r.0.version);
            if self.layers[i] == key {
                continue;
            }
            if r.0.width as u32 != w || r.0.height as u32 != h {
                continue;
            }
            queue.write_texture(
                gpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: gpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: i as u32,
                    },
                    aspect: gpu::TextureAspect::All,
                },
                r.0.data(),
                gpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w),
                    rows_per_image: Some(h),
                },
                gpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            self.layers[i] = key;
        }
    }
}

// A row of the raster is a whole number of the copy's row alignment.
const _: () = assert!(RASTER_WIDTH.is_multiple_of(256));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finish::Baked;
    use greycard_edit::mask::{Component, Mode, Shape};
    use greycard_edit::{Look, Mask};

    /// What the shader is given for a mask with a shape switched off
    /// is what it is given for the same mask without that shape at
    /// all: the CPU's `Mask::live` and this packing say the same.
    #[test]
    fn a_switched_off_shape_is_not_uploaded() {
        let disc = |center: [f32; 2], mode: Mode| Component {
            shape: Shape::Radial {
                center,
                radius: [0.2, 0.2],
                angle: 0.0,
                feather: 0.0,
            },
            mode,
            ..Default::default()
        };
        let local = |mask: Mask| Local {
            baked: Baked::of(&Look::default()),
            rasters: vec![None; mask.components.len()],
            mask,
            enabled: true,
        };
        let mut off = local(Mask {
            components: vec![
                disc([0.3, 0.5], Mode::Add),
                disc([0.4, 0.5], Mode::Subtract),
                disc([0.7, 0.5], Mode::Add),
            ],
            invert: false,
        });
        off.mask.components[1].enabled = false;
        let without = local(Mask {
            components: vec![disc([0.3, 0.5], Mode::Add), disc([0.7, 0.5], Mode::Add)],
            invert: false,
        });
        let (a, b) = (locals_gpu(&[off]), locals_gpu(&[without]));
        assert_eq!(a.params[0].shapes_count, 2);
        assert_eq!(a.params[0].shapes_count, b.params[0].shapes_count);
        assert_eq!(a.params[0].shapes_start, b.params[0].shapes_start);
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&a.shapes),
            bytemuck::cast_slice::<_, u8>(&b.shapes)
        );
    }

    /// Two locals, the first with a shape off: the second's shapes
    /// still start where its own do, so nothing reads the neighbor's.
    #[test]
    fn the_second_locals_shapes_start_after_the_firsts_live_ones() {
        let disc = |center: [f32; 2]| Component {
            shape: Shape::Radial {
                center,
                radius: [0.2, 0.2],
                angle: 0.0,
                feather: 0.0,
            },
            ..Default::default()
        };
        let mut first = Mask {
            components: vec![disc([0.3, 0.5]), disc([0.4, 0.5])],
            invert: false,
        };
        first.components[0].enabled = false;
        let second = Mask {
            components: vec![disc([0.7, 0.5])],
            invert: false,
        };
        let locals: Vec<Local> = [first, second]
            .into_iter()
            .map(|mask| Local {
                baked: Baked::of(&Look::default()),
                rasters: vec![None; mask.components.len()],
                mask,
                enabled: true,
            })
            .collect();
        let gpu = locals_gpu(&locals);
        assert_eq!(
            (gpu.params[0].shapes_start, gpu.params[0].shapes_count),
            (0, 1)
        );
        assert_eq!(
            (gpu.params[1].shapes_start, gpu.params[1].shapes_count),
            (1, 1)
        );
        assert_eq!(gpu.shapes.len(), 2);
    }

    /// A raster shape still waiting for its raster takes no layer, so
    /// the raster component after it keeps its own paint. Pushed as a
    /// brush with `rasters.len()` it borrowed the next one's.
    #[test]
    fn a_raster_shape_without_its_raster_takes_no_layer() {
        let painted = std::sync::Arc::new(greycard_edit::brush::Raster::new(0.667));
        let mask = Mask {
            components: vec![
                Component {
                    shape: Shape::Subject {},
                    ..Default::default()
                },
                Component {
                    shape: Shape::Brush {
                        strokes: Vec::new(),
                    },
                    ..Default::default()
                },
            ],
            invert: false,
        };
        let local = Local {
            baked: Baked::of(&Look::default()),
            // The subject's raster is not made yet; the brush's is.
            rasters: vec![None, Some(RasterRef(painted))],
            mask,
            enabled: true,
        };
        let gpu = locals_gpu(&[local]);
        assert_eq!(gpu.params[0].shapes_count, 2);
        // Kind 3 is nothing and holds no layer, which is what
        // `Local::weight` reads a missing raster as; its mode and its
        // invert still act on that nothing, as they do there.
        assert_eq!(gpu.shapes[0].kind, 3);
        assert_eq!(gpu.shapes[0].layer, 0);
        // So the brush is the first and only layer, which is its own.
        assert_eq!(gpu.shapes[1].kind, 2);
        assert_eq!(gpu.shapes[1].layer, 0);
        assert_eq!(gpu.rasters.len(), 1);
    }
}
