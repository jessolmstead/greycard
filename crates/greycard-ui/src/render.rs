//! The viewport renderer: the engine's image as an `Rgba16Float` texture
//! on Slint's device, drawn through the shader in `viewport.wgsl` into
//! a display-sized texture that Slint shows as an image.

use std::sync::Arc;

use crate::display::Lut3d;
use crate::worker::Halves;
use anyhow::{Context, Result};

use crate::finish::{Local, MAX_LOCALS, RasterRef, Source};
use crate::gpu;
use crate::scope::{self, Scope};
use greycard_core::lut;
use greycard_edit::brush::RASTER_WIDTH;
use greycard_edit::curve::LUT_SIZE;
use greycard_edit::mask::Shape;
use greycard_edit::{BlackWhite, Color, CurveLut, Grain, Light, Mixer, Tint, Vignette};

/// The canvas's four levels, by name, index matching `app.slint`'s
/// `canvas-colors` swatches and the shader's `canvas` uniform:
/// black, then three greys, the third about 18% grey as displayed.
pub const CANVAS_NAMES: [&str; 4] = ["Black", "Dark grey", "Mid grey", "Light grey"];

/// The same four levels' encoded sRGB fractions: the hex `app.slint`
/// paints the Rectangle behind the viewport with (#0f0f0f, #262626,
/// #2e2e2e, #444444), so the shader's margin agrees with it exactly.
const CANVAS_LEVELS: [f32; 4] = [15.0 / 255.0, 38.0 / 255.0, 46.0 / 255.0, 68.0 / 255.0];

fn canvas_index(choice: i32) -> usize {
    (choice.max(0) as usize).min(CANVAS_LEVELS.len() - 1)
}

/// A canvas choice's color, encoded sRGB, for the shader's uniform.
pub fn canvas_rgb(choice: i32) -> [f32; 3] {
    let v = CANVAS_LEVELS[canvas_index(choice)];
    [v, v, v]
}

/// A canvas choice's name, for the settings file.
pub fn canvas_name(choice: i32) -> &'static str {
    CANVAS_NAMES[canvas_index(choice)]
}

/// A name from the settings file back to its choice, the first
/// (black) when it names none of the four.
pub fn canvas_choice(name: &str) -> i32 {
    CANVAS_NAMES.iter().position(|n| *n == name).unwrap_or(0) as i32
}

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
    /// Keeps `m0` 16-byte aligned.
    pad0: f32,
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
    /// texture covers (its size times the source pixels to a texel), so
    /// a source position over it is the texture's uv and the sampler's
    /// bilinear is `Guide::at`; in z whether there is one.
    guide: [f32; 4],
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
}

/// What of the picture the view's masks read, as `Params::range.x`
/// has it: switched on or not, since a switched-off adjustment's mask
/// can still be shown, and its weight is made whatever its switch.
fn range_reads(locals: &[Local]) -> f32 {
    let locals = &locals[..locals.len().min(MAX_LOCALS)];
    if locals.iter().any(|l| l.mask.reads_color()) {
        2.0
    } else if locals.iter().any(|l| l.mask.reads_picture()) {
        1.0
    } else {
        0.0
    }
}

/// Eight band values as two vec4s.
fn halves(v: &[f32; 8]) -> [[f32; 4]; 2] {
    [[v[0], v[1], v[2], v[3]], [v[4], v[5], v[6], v[7]]]
}

/// A local adjustment as the shader has it.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LocalGpu {
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
struct ShapeGpu {
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
            Shape::Brush { .. } | Shape::Subject {} | Shape::Sky { .. } | Shape::Object { .. } => {
                match raster {
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
                }
            }
        }
    }
}

/// The locals as the shader takes them.
struct LocalsGpu {
    params: Vec<LocalGpu>,
    tables: Vec<[f32; 4]>,
    shapes: Vec<ShapeGpu>,
    /// The brushes' rasters, in the order of their layers.
    rasters: Vec<RasterRef>,
}

/// The locals' buffers: their parameters, their tables and their
/// shapes, never empty since a storage buffer cannot be; and their
/// brushes' rasters, a layer each.
fn locals_gpu(locals: &[Local]) -> LocalsGpu {
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
struct Brushes {
    texture: gpu::Texture,
    /// Each layer's raster: its allocation and version.
    layers: Vec<(usize, u64)>,
}

impl Brushes {
    /// One empty layer, so the binding always has a texture.
    fn empty(device: &gpu::Device) -> Self {
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

    fn view(&self) -> gpu::TextureView {
        self.texture.create_view(&gpu::TextureViewDescriptor {
            dimension: Some(gpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    }

    /// Bring the array to `rasters`: made anew when their number or
    /// size changes, each layer written when its raster did.
    fn sync(&mut self, device: &gpu::Device, queue: &gpu::Queue, rasters: &[RasterRef]) {
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

/// The clipping warnings: which ends of the output to paint over in
/// the viewport, the shadows blue and the highlights red, where a
/// channel lands in the histogram's end bins (`scope::Clipping`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Warn {
    pub shadows: bool,
    pub highlights: bool,
}

impl Warn {
    /// As the shader takes it: bit 1 the shadows, bit 2 the highlights.
    fn bits(self) -> u32 {
        (self.shadows as u32) | ((self.highlights as u32) << 1)
    }
}

/// What one frame of the viewport shows of the edit.
#[derive(Debug, Clone)]
pub struct View {
    pub zoom: f32,
    pub center: (f32, f32),
    pub light: Light,
    pub mixer: Mixer,
    /// Global vibrance and saturation, in the same pass as the mixer.
    pub color: Color,
    /// The conversion to black and white, in that pass after them.
    pub bw: BlackWhite,
    /// The global look's tint, last in that pass.
    pub tint: Tint,
    /// The picture's orientation, turn and perspective: the
    /// plane-to-source matrix (`Geometry::matrix`), the perspective's
    /// row (`Geometry::perspective`), the plane's size, and whether a
    /// plane pixel lands between source pixels.
    pub matrix: [[f32; 2]; 2],
    pub persp: [f32; 2],
    pub plane: (f32, f32),
    pub cubic: bool,
    /// The rectangle of the leveled plane the view shows, source
    /// pixels: the crop, or the turned source's bounds while cropping.
    pub frame_origin: (f32, f32),
    pub frame_size: (f32, f32),
    /// The point curves, baked.
    pub curves: CurveLut,
    /// Working-space matrix, rows, for the white balance preview.
    pub white: [[f32; 3]; 3],
    /// The local adjustments, baked, in the edit's order.
    pub locals: Vec<Local>,
    /// A mask to paint over the picture, by index.
    pub show_mask: Option<usize>,
    /// Paint the sharpen's blend mask, which the worker put in the
    /// picture's alpha.
    pub show_sharpen: bool,
    /// Draw the shown mask's weight alone, as grey, rather than the
    /// picture under it: what the GPU check reads back.
    pub mask_alone: bool,
    pub vignette: Vignette,
    pub grain: Grain,
    /// The clipping warnings to paint over the picture.
    pub warn: Warn,
    /// The canvas outside the frame, encoded sRGB: the panel's
    /// choice, matching what the Rectangle behind the viewport
    /// paints (`app.slint`'s `canvas-colors`), so the two agree.
    pub canvas: [f32; 3],
    /// A raw's scene or a picture already rendered: whether the
    /// baseline and the display curve apply (`finish::Source`).
    pub source: Source,
}

/// The look table on the GPU, and what the shader needs beside it.
///
/// The table it holds is remembered so that a strength moved on the
/// slider does not send the whole of it again; it is let go of, with
/// the texture, the moment the edit names no table (`set_look(None)`).
struct LookTable {
    texture: gpu::Texture,
    /// The table the texture holds, to tell a new one from the same
    /// one at another strength. `None` when the texture is the
    /// identity placeholder.
    of: Option<Arc<lut::Lut3d>>,
    /// Strength, nodes an axis, which transfer function, a gamma.
    params: [f32; 4],
    min: [f32; 4],
    max: [f32; 4],
    to_lut: [[f32; 4]; 3],
    from_lut: [[f32; 4]; 3],
}

/// One picture of the compare view: an encoded texture, the view of
/// it, and the rectangle of the target it is drawn into.
pub struct Tile<'a> {
    pub texture: &'a gpu::Texture,
    pub view: View,
    pub rect: (u32, u32, u32, u32),
}

impl View {
    /// A view of nothing at the identity everywhere: what a clear
    /// draws under, and what an encoded picture, which the look never
    /// touches, is drawn with.
    pub fn blank() -> Self {
        View {
            zoom: 1.0,
            center: (0.0, 0.0),
            light: Light::default(),
            mixer: Mixer::default(),
            color: Color::default(),
            bw: BlackWhite::OFF,
            tint: Tint::default(),
            matrix: [[1.0, 0.0], [0.0, 1.0]],
            persp: [0.0, 0.0],
            plane: (1.0, 1.0),
            cubic: false,
            frame_origin: (0.0, 0.0),
            frame_size: (1.0, 1.0),
            curves: greycard_edit::Curves::default().bake(),
            white: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            locals: Vec::new(),
            show_mask: None,
            show_sharpen: false,
            mask_alone: false,
            vignette: Vignette::default(),
            grain: Grain::default(),
            warn: Warn::default(),
            canvas: [0.0; 3],
            source: Source::Scene,
        }
    }
}

pub struct Renderer {
    device: gpu::Device,
    queue: gpu::Queue,
    pipeline: gpu::RenderPipeline,
    layout: gpu::BindGroupLayout,
    sampler: gpu::Sampler,
    source: Option<gpu::Texture>,
    /// The source holds encoded sRGB bytes rather than the engine's
    /// linear picture: the camera's JPEG, shown through the monitor's
    /// table alone.
    encoded: bool,
    target: Option<gpu::Texture>,
    lut: gpu::Texture,
    /// The table an encoded picture goes through: sRGB in, the
    /// monitor's RGB out, no output space and no proof, since the
    /// camera's JPEG is sRGB whatever the export sheet says.
    lut_encoded: gpu::Texture,
    lut_sampler: gpu::Sampler,
    brushes: Brushes,
    /// The tone equalizer's guide plane, R16Float: a stop is a stop, so
    /// half floats hold it to a thousandth of one, and at a texel to
    /// every few dozen source pixels the whole plane is a few megabytes.
    guide: gpu::Texture,
    /// Its `Params::guide`: the source pixels the texture covers, and
    /// whether there is a plane to read.
    guide_cover: [f32; 4],
    /// The look table the picture finishes through.
    look: LookTable,
    /// Working space to the output's, rows: the export's space, so
    /// the viewport shows what the export will hold.
    output: [[f32; 3]; 3],
    oklab: crate::finish::Oklab,
    scopes: Scopes,
}

/// The scopes: the whole image drawn small through the same shader,
/// binned by a compute pass, read back a frame later. The bins are
/// the histogram and then, when the panel asks for one, the waveform
/// or the vectorscope; see [`crate::scope`].
struct Scopes {
    pipeline: gpu::ComputePipeline,
    layout: gpu::BindGroupLayout,
    bins: gpu::Buffer,
    staging: gpu::Buffer,
    mode: gpu::Buffer,
    analysis: Option<gpu::Texture>,
    /// A read back on its way, if one is.
    in_flight: Option<std::sync::mpsc::Receiver<Result<(), gpu::BufferAsyncError>>>,
    /// The last bins read, and which scope asked for them.
    latest: Option<(Scope, Vec<u32>)>,
    /// The analysis picture itself, read back beside the bins for
    /// the navigator: its staging buffer, sized for the picture's
    /// height, the read on its way, and the last picture read, not
    /// yet taken.
    picture_staging: Option<gpu::Buffer>,
    picture_in_flight: Option<std::sync::mpsc::Receiver<Result<(), gpu::BufferAsyncError>>>,
    picture: Option<slint::Image>,
    /// The edit and scope the bins in flight or latest were taken
    /// under; a new analysis only when one of those, or the image,
    /// changes.
    analyzed: Option<(EditKey, Scope)>,
    image_changed: bool,
}

/// What of a view the scopes depend on.
#[derive(Debug, Clone, PartialEq)]
struct EditKey {
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
        }
    }
}

/// Width of the image the scopes are taken from.
const ANALYSIS_WIDTH: u32 = 512;

impl Scopes {
    fn new(device: &gpu::Device) -> Self {
        let shader = device.create_shader_module(gpu::ShaderModuleDescriptor {
            label: Some("scope"),
            source: gpu::ShaderSource::Wgsl(include_str!("scope.wgsl").into()),
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
            size: 16,
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
            in_flight: None,
            latest: None,
            picture_staging: None,
            picture_in_flight: None,
            picture: None,
            analyzed: None,
            image_changed: false,
        }
    }
}

impl Renderer {
    pub fn new(device: &gpu::Device, queue: &gpu::Queue) -> Self {
        let shader = device.create_shader_module(gpu::ShaderModuleDescriptor {
            label: Some("viewport"),
            source: gpu::ShaderSource::Wgsl(include_str!("viewport.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&gpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[
                gpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Buffer {
                        ty: gpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Texture {
                        sample_type: gpu::TextureSampleType::Float { filterable: true },
                        view_dimension: gpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Sampler(gpu::SamplerBindingType::Filtering),
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Texture {
                        sample_type: gpu::TextureSampleType::Float { filterable: true },
                        view_dimension: gpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Sampler(gpu::SamplerBindingType::Filtering),
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Buffer {
                        ty: gpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The locals, their tables and their shapes.
                gpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Buffer {
                        ty: gpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Buffer {
                        ty: gpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                gpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Buffer {
                        ty: gpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The brushes, a layer each.
                gpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Texture {
                        sample_type: gpu::TextureSampleType::Float { filterable: true },
                        view_dimension: gpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                // The tone equalizer's guide plane.
                gpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Texture {
                        sample_type: gpu::TextureSampleType::Float { filterable: true },
                        view_dimension: gpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The look table. Read by node, not sampled: the
                // interpolation is tetrahedral, so it needs no
                // sampler and takes no filtering.
                gpu::BindGroupLayoutEntry {
                    binding: 11,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Texture {
                        sample_type: gpu::TextureSampleType::Float { filterable: true },
                        view_dimension: gpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&gpu::PipelineLayoutDescriptor {
            label: Some("viewport"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&gpu::RenderPipelineDescriptor {
            label: Some("viewport"),
            layout: Some(&pipeline_layout),
            vertex: gpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(gpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(TARGET_FORMAT.into())],
            }),
            primitive: gpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: gpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&gpu::SamplerDescriptor {
            label: Some("viewport"),
            mag_filter: gpu::FilterMode::Linear,
            min_filter: gpu::FilterMode::Linear,
            ..Default::default()
        });
        let lut_sampler = device.create_sampler(&gpu::SamplerDescriptor {
            label: Some("display lut"),
            mag_filter: gpu::FilterMode::Linear,
            min_filter: gpu::FilterMode::Linear,
            address_mode_u: gpu::AddressMode::ClampToEdge,
            address_mode_v: gpu::AddressMode::ClampToEdge,
            address_mode_w: gpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let lut = Self::lut_texture(device, queue, &Lut3d::identity());
        let lut_encoded = Self::lut_texture(device, queue, &Lut3d::identity());
        // Two nodes an axis of identity, so the binding always has a
        // texture; a strength of zero is what makes it do nothing.
        let look = LookTable {
            texture: Self::look_texture(device, queue, &lut::Lut3d::identity(2)),
            of: None,
            params: [0.0, 2.0, 0.0, 0.0],
            min: [0.0; 4],
            max: [1.0, 1.0, 1.0, 0.0],
            to_lut: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            from_lut: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        };
        let guide = Self::guide_texture(device, 1, 1);
        let output = crate::export::Space::Srgb.matrix();
        Self {
            scopes: Scopes::new(device),
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            layout,
            sampler,
            source: None,
            encoded: false,
            target: None,
            lut,
            lut_encoded,
            lut_sampler,
            brushes: Brushes::empty(device),
            guide,
            guide_cover: [1.0, 1.0, 0.0, 0.0],
            look,
            output,
            oklab: crate::finish::Oklab::for_working_space(),
        }
    }

    fn lut_texture(device: &gpu::Device, queue: &gpu::Queue, lut: &Lut3d) -> gpu::Texture {
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
        }
    }

    /// A 3D texture of the look table, its values as they are: a
    /// `.cube` may hand back more than one, so half floats and not a
    /// normalized format.
    fn look_texture(device: &gpu::Device, queue: &gpu::Queue, table: &lut::Lut3d) -> gpu::Texture {
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
        }
    }

    fn make_target(&self, label: &str, width: u32, height: u32) -> gpu::Texture {
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
    fn draw(&self, encoder: &mut gpu::CommandEncoder, target: &gpu::Texture, v: &View) {
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
        let image = source
            .map(|s| [s.width() as f32, s.height() as f32])
            .unwrap_or([1.0, 1.0]);
        let (ox, oy, tw, th) = rect.unwrap_or((0, 0, target.width(), target.height()));
        let m = self.output;
        let w = v.white;
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
            exposure: v.source.baseline() + v.light.exposure,
            // The shape, then 0 the display curve, 1 a clip, for a
            // picture that has had its curve.
            curve: match v.source {
                Source::Scene => 0.0,
                Source::Display => 1.0,
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
            pad0: 0.0,
            m0: [m[0][0], m[0][1], m[0][2], 0.0],
            m1: [m[1][0], m[1][1], m[1][2], 0.0],
            m2: [m[2][0], m[2][1], m[2][2], 0.0],
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
                0.0,
            ],
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
                ],
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// The scopes of the whole image under `v`'s edit: the bins from
    /// the last read back with the scope they were taken for, and a
    /// new read back dispatched if none is on its way. `true` when one
    /// is in flight and a frame later will have it.
    ///
    /// The histogram is always in the first [`scope::HIST`] bins,
    /// whatever `scope` asks for, because the curve editor draws it.
    pub fn analyze(&mut self, v: &View, scope: Scope) -> (Option<(Scope, &[u32])>, bool) {
        // Collect a finished read back.
        let _ = self.device.poll(gpu::PollType::Poll);
        if let Some(rx) = &self.scopes.in_flight
            && let Ok(result) = rx.try_recv()
        {
            self.scopes.in_flight = None;
            if result.is_ok()
                && let Some((_, taken)) = self.scopes.analyzed
                && let Ok(data) = self.scopes.staging.slice(..).get_mapped_range()
            {
                let bins = bytemuck::cast_slice::<u8, u32>(&data)[..taken.bin_count()].to_vec();
                drop(data);
                self.scopes.latest = Some((taken, bins));
            }
            self.scopes.staging.unmap();
        }
        if let Some(rx) = &self.scopes.picture_in_flight
            && let Ok(result) = rx.try_recv()
        {
            self.scopes.picture_in_flight = None;
            if let Some(staging) = &self.scopes.picture_staging {
                if result.is_ok()
                    && let Some(t) = &self.scopes.analysis
                    && let Ok(data) = staging.slice(..).get_mapped_range()
                {
                    let (w, h) = (t.width(), t.height());
                    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                    buf.make_mut_bytes()
                        .copy_from_slice(&data[..(w * h * 4) as usize]);
                    drop(data);
                    self.scopes.picture = Some(slint::Image::from_rgba8(buf));
                }
                staging.unmap();
            }
        }
        let key = (EditKey::of(v), scope);
        let stale = self.scopes.image_changed || self.scopes.analyzed.as_ref() != Some(&key);
        if stale
            && self.scopes.in_flight.is_none()
            && self.scopes.picture_in_flight.is_none()
            && self.source.is_some()
        {
            self.scopes.analyzed = Some(key);
            self.scopes.image_changed = false;
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
            let whole = View {
                zoom: ANALYSIS_WIDTH as f32 / iw as f32,
                center: (iw as f32 / 2.0, ih as f32 / 2.0),
                show_mask: None,
                show_sharpen: false,
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
                ]),
            );
            let mut encoder = self
                .device
                .create_command_encoder(&gpu::CommandEncoderDescriptor {
                    label: Some("scope"),
                });
            self.draw(&mut encoder, &analysis, &whole);
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
            // The picture too, for the navigator. Its rows are 2048
            // bytes, a multiple of the 256 a copy wants, so it lands
            // packed.
            let picture_bytes = (ANALYSIS_WIDTH * 4 * height) as u64;
            let picture_staging = match &self.scopes.picture_staging {
                Some(b) if b.size() == picture_bytes => b.clone(),
                _ => {
                    let b = self.device.create_buffer(&gpu::BufferDescriptor {
                        label: Some("navigator staging"),
                        size: picture_bytes,
                        usage: gpu::BufferUsages::COPY_DST | gpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    self.scopes.picture_staging = Some(b.clone());
                    b
                }
            };
            encoder.copy_texture_to_buffer(
                gpu::TexelCopyTextureInfo {
                    texture: &analysis,
                    mip_level: 0,
                    origin: gpu::Origin3d::ZERO,
                    aspect: gpu::TextureAspect::All,
                },
                gpu::TexelCopyBufferInfo {
                    buffer: &picture_staging,
                    layout: gpu::TexelCopyBufferLayout {
                        offset: 0,
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
            self.scopes.latest.as_ref().map(|(s, b)| (*s, b.as_slice())),
            self.scopes.in_flight.is_some() || self.scopes.picture_in_flight.is_some(),
        )
    }

    /// The analysis picture read back since the last take: the whole
    /// frame under the edit as the screen shows it, 512 wide, for
    /// the navigator.
    pub fn take_navigator(&mut self) -> Option<slint::Image> {
        self.scopes.picture.take()
    }

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

    fn guide_texture(device: &gpu::Device, width: u32, height: u32) -> gpu::Texture {
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

    /// Put the tone equalizer's guide plane on the GPU. Its texels
    /// stand for `scale` source pixels each way, so the shader divides
    /// a source position by what the texture covers to sample it.
    pub fn set_guide(&mut self, guide: &crate::finish::Guide) {
        let (w, h) = (guide.width as u32, guide.height as u32);
        if guide.data.is_empty() || w == 0 || h == 0 {
            self.guide_cover = [1.0, 1.0, 0.0, 0.0];
            return;
        }
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
    }

    /// Show a developed picture that is on the GPU already: an
    /// engine op's texture, the sharpen's mask in its alpha.
    pub fn set_source(&mut self, texture: gpu::Texture) {
        self.source = Some(texture);
        self.encoded = false;
        self.scopes.image_changed = true;
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
    }

    /// Draw the view: `width` by `height` display pixels, `zoom` display
    /// pixels per image pixel, the image pixel `center` in the middle.
    pub fn render(&mut self, width: u32, height: u32, v: &View) -> gpu::Texture {
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
        target
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

/// The display texture's format: plain bytes, encoded by the shader,
/// because Slint's renderer shows every texture's bytes as they are.
const TARGET_FORMAT: gpu::TextureFormat = gpu::TextureFormat::Rgba8Unorm;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finish::Baked;
    use greycard_core::image::WorkingImage;
    use greycard_edit::mask::{Component, Mode, Shape};
    use greycard_edit::{Look, Mask};
    use rayon::prelude::*;

    /// Both shaders parse and validate. Nothing else in `cargo test`
    /// reads the WGSL — the pipelines are built against a real
    /// device, which the test machines have no business needing — so
    /// without this a typo in `viewport.wgsl` is found by a black
    /// viewport rather than by the suite. `Capabilities::all` because
    /// this is asking whether the source is a well-formed module,
    /// not which device would accept it; the device says that.
    #[test]
    fn the_shaders_parse_and_validate() {
        for (name, source) in [
            ("viewport.wgsl", include_str!("viewport.wgsl")),
            ("scope.wgsl", include_str!("scope.wgsl")),
        ] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{name}:\n{}", e.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }

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
    /// A device of our own, off any window; none without an adapter,
    /// and the test says so rather than passing in silence.
    fn device(what: &str) -> Option<(gpu::Device, gpu::Queue)> {
        let instance = gpu::Instance::new(gpu::InstanceDescriptor::new_without_display_handle());
        let got = pollster::block_on(instance.request_adapter(&gpu::RequestAdapterOptions {
            power_preference: gpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok()
        .and_then(|adapter| {
            pollster::block_on(adapter.request_device(&gpu::DeviceDescriptor {
                label: Some("mask test"),
                ..Default::default()
            }))
            .ok()
        });
        if got.is_none() {
            eprintln!("SKIPPED: {what} has no GPU to run on");
            println!("SKIPPED: {what} has no GPU to run on");
        }
        got
    }

    /// A local with `shapes` joined in order and nothing to its look.
    fn local_of(shapes: Vec<(Shape, Mode)>) -> Local {
        Local {
            baked: Baked::of(&Look::default()),
            mask: Mask {
                components: shapes
                    .into_iter()
                    .map(|(shape, mode)| Component {
                        shape,
                        mode,
                        ..Default::default()
                    })
                    .collect(),
                invert: false,
            },
            enabled: true,
            rasters: Vec::new(),
        }
    }

    /// How far the shader's weights for each of `locals` over `image`
    /// at `exposure` stops of global exposure are from the CPU's
    /// (`Local::weight_sampled` on `finish::sample`): the largest and
    /// the mean difference, in weight, over every pixel and local.
    /// The CPU is handed the picture the GPU has, rounded to half
    /// floats, so what is measured is the two implementations; the
    /// read back is eight bits, so half a level (0.002) is the floor.
    /// With `covered`, every mask must take in some of the picture, so
    /// that two blank answers cannot agree.
    fn gpu_against_cpu(
        device: &gpu::Device,
        queue: &gpu::Queue,
        image: &WorkingImage,
        locals: &[Local],
        exposure: f32,
        covered: bool,
    ) -> (f32, f32) {
        use crate::finish::{local_ab, sample};
        let (w, h) = (image.width, image.height);
        let halves = crate::worker::Halves::from_image(image, None);
        let seen = WorkingImage {
            width: w,
            height: h,
            data: image
                .data
                .iter()
                .map(|v| half::f16::from_f32(*v).to_f32())
                .collect(),
        };
        let mut renderer = Renderer::new(device, queue);
        renderer.upload(&halves);
        let ab = local_ab(&seen);
        let stops = exposure;
        let mut worst = 0.0f32;
        let mut sum = 0.0f64;
        for (k, local) in locals.iter().enumerate() {
            let mut view = View::blank();
            view.center = (w as f32 / 2.0, h as f32 / 2.0);
            view.plane = (w as f32, h as f32);
            view.frame_size = (w as f32, h as f32);
            view.light.exposure = exposure;
            view.locals = locals.to_vec();
            view.show_mask = Some(k);
            view.mask_alone = true;
            let target = renderer.render(w as u32, h as u32, &view);
            let shown = renderer.read_back(&target).expect("read back");
            // The largest difference, their sum, how many pixels are
            // more than a level out, and how many the CPU has over half
            // in and part in, so a check of two blank pictures cannot
            // pass.
            let (local_worst, local_sum, over, full, part) = (0..h)
                .into_par_iter()
                .map(|y| {
                    let mut t = (0.0f32, 0.0f64, 0usize, 0usize, 0usize);
                    for x in 0..w {
                        let i = y * w + x;
                        let px = seen.pixel(x, y);
                        let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32);
                        let cpu = local.weight_sampled(u, v, Some(sample(px, Some(ab[i]), stops)));
                        let gpu = f32::from(shown.get_pixel(x as u32, y as u32)[0]) / 255.0;
                        let d = (cpu - gpu).abs();
                        if d > t.0 && std::env::var_os("GREYCARD_MASK_WORST").is_some() {
                            let s = sample(px, Some(ab[i]), stops);
                            eprintln!(
                                "  worse: {d:.4} at {x},{y} px {px:?} cpu {cpu} gpu {gpu} {s:?}"
                            );
                        }
                        t.0 = t.0.max(d);
                        t.1 += f64::from(d);
                        t.2 += usize::from(d > 1.5 / 255.0);
                        t.3 += usize::from(cpu > 0.5);
                        t.4 += usize::from(cpu > 0.0 && cpu < 1.0);
                    }
                    t
                })
                .reduce(
                    || (0.0, 0.0, 0, 0, 0),
                    |a, b| (a.0.max(b.0), a.1 + b.1, a.2 + b.2, a.3 + b.3, a.4 + b.4),
                );
            eprintln!(
                "mask {k}: {:.1}% over half, {:.1}% part in, max difference {local_worst:.4}, \
                 {over} pixels more than a level out",
                100.0 * full as f32 / (w * h) as f32,
                100.0 * part as f32 / (w * h) as f32
            );
            assert!(
                !covered || (full > w * h / 200 && part > w * h / 200),
                "mask {k} is blank"
            );
            worst = worst.max(local_worst);
            sum += local_sum;
        }
        (worst, (sum / (w * h * locals.len()) as f64) as f32)
    }

    /// The masks the checks run: a luminance window, a color one at
    /// the skin preset and one at a blue, and each kind intersected
    /// with a drawn gradient: the skin, which a real frame of browns
    /// and reds has plenty of, and the lightness from 30 up.
    fn range_locals() -> Vec<Local> {
        vec![
            local_of(vec![(
                Shape::Luminance {
                    low: 0.45,
                    high: 0.8,
                    low_feather: 0.1,
                    high_feather: 0.05,
                },
                Mode::Add,
            )]),
            local_of(vec![(Shape::skin(), Mode::Add)]),
            local_of(vec![(Shape::color_at(250.0), Mode::Add)]),
            local_of(vec![
                (
                    Shape::Linear {
                        from: [0.0, 0.7],
                        to: [0.0, 0.0],
                    },
                    Mode::Add,
                ),
                (Shape::skin(), Mode::Intersect),
            ]),
            local_of(vec![
                (
                    Shape::Linear {
                        from: [0.0, 0.0],
                        to: [0.0, 0.8],
                    },
                    Mode::Add,
                ),
                (
                    Shape::Luminance {
                        low: 0.3,
                        high: 1.0,
                        low_feather: 0.1,
                        high_feather: 0.0,
                    },
                    Mode::Intersect,
                ),
            ]),
        ]
    }

    /// A field that runs every hue across and every lightness down,
    /// with a little noise so the local mean has something to do.
    fn field() -> WorkingImage {
        let (w, h) = (360usize, 240usize);
        let ok = greycard_core::color::Oklab::for_working_space();
        let from_lms = greycard_core::color::invert3(ok.to_lms).unwrap();
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let l = 0.1 + 0.85 * y as f32 / h as f32;
                let hue = (x as f32).to_radians();
                let chroma = 0.12 * (0.5 + 0.5 * ((x * 7 + y * 3) as f32 * 0.37).sin());
                let lab = [l, chroma * hue.cos(), chroma * hue.sin()];
                let lms = greycard_core::color::apply3(&greycard_core::color::LAB_TO_LMS, lab)
                    .map(|v| v * v * v);
                data.extend(greycard_core::color::apply3(&from_lms, lms).map(|v| v.max(0.0)));
            }
        }
        WorkingImage {
            width: w,
            height: h,
            data,
        }
    }

    /// The shader's range masks are the CPU's, on the field.
    #[test]
    fn the_shaders_range_masks_are_the_cpus() {
        let Some((device, queue)) = device("the range masks' GPU check") else {
            return;
        };
        let image = field();
        let (worst, mean) = gpu_against_cpu(&device, &queue, &image, &range_locals(), -0.3, true);
        eprintln!("range masks, GPU against CPU: max {worst:.4}, mean {mean:.5}");
        assert!(worst <= 3.0 / 255.0, "max {worst}");
        assert!(mean <= 0.5 / 255.0, "mean {mean}");
    }

    /// A switched-off adjustment's mask can still be shown, and its
    /// range shapes read the picture for it even when no switched-on
    /// one does: not an empty sample, which painted a window from 0
    /// over the whole frame and a color window over none of it.
    #[test]
    fn a_switched_off_range_mask_shows_what_it_would_take() {
        let off = |shape: Shape| {
            let mut l = local_of(vec![(shape, Mode::Add)]);
            l.enabled = false;
            l
        };
        let dark = off(Shape::Luminance {
            low: 0.0,
            high: 0.3,
            low_feather: 0.0,
            high_feather: 0.05,
        });
        let color = off(Shape::color_at(250.0));
        assert_eq!(range_reads(std::slice::from_ref(&dark)), 1.0);
        assert_eq!(range_reads(&[dark.clone(), color.clone()]), 2.0);
        assert_eq!(range_reads(&[local_of(vec![])]), 0.0);
        let Some((device, queue)) = device("the switched-off range mask's check") else {
            return;
        };
        let image = field();
        for local in [dark, color] {
            let (worst, _) = gpu_against_cpu(
                &device,
                &queue,
                &image,
                std::slice::from_ref(&local),
                0.0,
                true,
            );
            assert!(worst <= 3.0 / 255.0, "max {worst}");
        }
    }

    /// The mixer reads the mean the color windows made, brought to its
    /// own exposure, rather than making it again: a picture with the
    /// mixer on and a color window whose look does nothing is the
    /// picture without the window, to the byte.
    #[test]
    fn the_mixer_reads_the_color_windows_mean() {
        let Some((device, queue)) = device("the shared mean's check") else {
            return;
        };
        let image = field();
        let (w, h) = (image.width, image.height);
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        let mut view = View::blank();
        view.center = (w as f32 / 2.0, h as f32 / 2.0);
        view.plane = (w as f32, h as f32);
        view.frame_size = (w as f32, h as f32);
        view.light.exposure = 0.7;
        view.mixer.enabled = true;
        view.mixer.hue[1] = 20.0;
        view.mixer.saturation[5] = 0.6;
        let mut shot = |view: &View| {
            let target = renderer.render(w as u32, h as u32, view);
            renderer.read_back(&target).expect("read back")
        };
        let without = shot(&view);
        view.locals = vec![local_of(vec![(Shape::color_at(250.0), Mode::Add)])];
        let with = shot(&view);
        // Within a level: the mean is one computation shared by the
        // window and the mixer, but the mixer re-exposes it, and a
        // backend that contracts multiplies (Metal on the Mac runner)
        // rounds that a last bit apart from Vulkan's.
        let worst = with
            .as_raw()
            .iter()
            .zip(without.as_raw())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 1,
            "the window changed the mixer's picture by {worst} levels"
        );
        // And the mixer does act, so the check is of something.
        view.mixer.enabled = false;
        assert!(shot(&view) != without);
    }

    /// The Light section switched off keeps the base curve on the GPU
    /// as `finish_pixel` does (issue #9, where off took the curve away
    /// and the picture got darker). The check is against the CPU: the
    /// view for the section off with sliders set, through
    /// `effective()`, draws what `finish_pixel` makes of the default
    /// edit, curve and all. The same sliders on are drawn first, so a
    /// view that ignored them could not pass for one that undid them.
    #[test]
    fn the_light_switch_off_keeps_the_base_curve_on_the_gpu() {
        let Some((device, queue)) = device("the light switch's check") else {
            return;
        };
        let image = field();
        let (w, h) = (image.width, image.height);
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        let mut view = View::blank();
        view.center = (w as f32 / 2.0, h as f32 / 2.0);
        view.plane = (w as f32, h as f32);
        view.frame_size = (w as f32, h as f32);
        let mut shot = |view: &View| {
            let target = renderer.render(w as u32, h as u32, view);
            renderer.read_back(&target).expect("read back")
        };
        let mut set = Light {
            exposure: 1.2,
            ..Light::default()
        };
        set.tone.contrast = 1.4;
        set.tone.whites = 0.5;
        set.tone.blacks = -0.1;
        view.light = set;
        let lit = shot(&view);
        set.enabled = false;
        view.light = set.effective();
        let off = shot(&view);
        assert!(lit != off, "the sliders act, so the check is of something");
        // And that picture is the CPU's, to a level or so: sRGB, the
        // renderer's output until told otherwise, and the display
        // table the identity.
        let seen: Vec<f32> = image
            .data
            .iter()
            .map(|v| half::f16::from_f32(*v).to_f32())
            .collect();
        let global = Baked::global(&greycard_edit::Edit::default(), Source::Scene);
        let to_out = crate::export::Space::Srgb.matrix();
        let mut worst = 0.0f32;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                let px = [seen[i], seen[i + 1], seen[i + 2]];
                let cpu = crate::finish::finish_pixel(px, &global, &[], &to_out);
                let gpu = off.get_pixel(x as u32, y as u32);
                for k in 0..3 {
                    worst = worst.max((cpu[k] - f32::from(gpu[k]) / 255.0).abs());
                }
            }
        }
        eprintln!("light off, GPU against CPU: max difference {worst:.4}");
        assert!(worst < 2.5 / 255.0, "{worst}");
    }

    /// A picture already rendered for a display takes a clip where a
    /// raw takes the curve, on the GPU as `finish_pixel` has it, so the
    /// source's mapping to the shader's mode is checked both ways.
    #[test]
    fn a_rendered_picture_takes_a_clip_on_the_gpu_as_on_the_cpu() {
        let Some((device, queue)) = device("the rendered picture's check") else {
            return;
        };
        let image = field();
        let (w, h) = (image.width, image.height);
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        let seen: Vec<f32> = image
            .data
            .iter()
            .map(|v| half::f16::from_f32(*v).to_f32())
            .collect();
        let to_out = crate::export::Space::Srgb.matrix();
        let mut worst_of = |source: Source| {
            let mut view = View::blank();
            view.center = (w as f32 / 2.0, h as f32 / 2.0);
            view.plane = (w as f32, h as f32);
            view.frame_size = (w as f32, h as f32);
            view.source = source;
            let target = renderer.render(w as u32, h as u32, &view);
            let shown = renderer.read_back(&target).expect("read back");
            let global = Baked::global(&greycard_edit::Edit::default(), source);
            let mut worst = 0.0f32;
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 3;
                    let px = [seen[i], seen[i + 1], seen[i + 2]];
                    let cpu = crate::finish::finish_pixel(px, &global, &[], &to_out);
                    let gpu = shown.get_pixel(x as u32, y as u32);
                    for k in 0..3 {
                        worst = worst.max((cpu[k] - f32::from(gpu[k]) / 255.0).abs());
                    }
                }
            }
            worst
        };
        let display = worst_of(Source::Display);
        let scene = worst_of(Source::Scene);
        eprintln!("GPU against CPU: rendered {display:.4}, raw {scene:.4}");
        assert!(display < 2.5 / 255.0, "{display}");
        assert!(scene < 2.5 / 255.0, "{scene}");
    }

    /// The same on a real frame, developed at the defaults, with the
    /// CPU's time for the masks over it: `GREYCARD_MASK_FRAME` names
    /// the raw. Run it in release, ignored tests included, with the
    /// output shown.
    #[test]
    #[ignore]
    fn a_real_frames_range_masks() {
        use crate::finish::{local_ab, sample};
        let Some(path) = std::env::var_os("GREYCARD_MASK_FRAME") else {
            panic!("set GREYCARD_MASK_FRAME to a raw");
        };
        let frame = greycard_core::decode::decode_path(&path).expect("decode");
        let developed = greycard_core::develop::develop(
            &frame,
            &greycard_core::develop::DevelopSettings::default(),
        )
        .expect("develop");
        let image = developed.image;
        let (w, h) = (image.width, image.height);
        eprintln!("{w} x {h}, {:.1} MP", (w * h) as f32 / 1e6);
        let locals = range_locals();
        // The CPU's cost: the mean a and b once, then each pixel's
        // sample and every mask's weight, as `finish_with` pays it.
        let start = std::time::Instant::now();
        let ab = local_ab(&image);
        let mean_took = start.elapsed();
        let weights: f32 = (0..h)
            .into_par_iter()
            .map(|y| {
                let mut acc = 0.0;
                for x in 0..w {
                    let i = y * w + x;
                    let px = image.pixel(x, y);
                    let s = Some(sample(px, Some(ab[i]), 0.0));
                    let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32);
                    for l in &locals {
                        acc += l.weight_sampled(u, v, s);
                    }
                }
                acc
            })
            .sum();
        eprintln!(
            "CPU: the mean {:.0} ms, then the sample and {} masks a pixel {:.0} ms (sum {weights:.0})",
            mean_took.as_secs_f64() * 1e3,
            locals.len(),
            (start.elapsed() - mean_took).as_secs_f64() * 1e3
        );
        // And what an export's finish pays whole, the least of five:
        // with no local, a luminance window alone (no mean), a color
        // window.
        let global = Baked::global(&greycard_edit::Edit::default(), Source::Scene);
        let to_out = crate::export::Space::Srgb.matrix();
        let once = |locals: &[Local]| {
            let start = std::time::Instant::now();
            let out: Vec<u8> = crate::finish::finish_with(
                &image,
                None,
                &global,
                locals,
                |x, y| ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32),
                |_, _| (0.0, None),
                None,
                None,
                &to_out,
                |v| (v * 255.0).round() as u8,
            );
            std::hint::black_box(out);
            start.elapsed().as_secs_f64() * 1e3
        };
        let finish = |locals: &[Local]| (0..5).map(|_| once(locals)).fold(f64::MAX, f64::min);
        eprintln!(
            "finish: no local {:.0} ms, a luminance window {:.0} ms, a color window {:.0} ms",
            finish(&[]),
            finish(&locals[..1]),
            finish(&locals[1..2])
        );
        let Some((device, queue)) = device("the real frame's GPU check") else {
            return;
        };
        let (worst, mean) = gpu_against_cpu(&device, &queue, &image, &locals, 0.0, true);
        eprintln!("real frame, GPU against CPU: max {worst:.4}, mean {mean:.5}");
        // A few dozen pixels on the whole frame are a few levels out,
        // all in the luminance window's steep high fade, where a small
        // difference in what the shader reads for a pixel is multiplied
        // by the fade's slope; the same pixels cut out on a texture of
        // their own agree to half a level. Which difference in the
        // read that is has not been pinned down.
        assert!(worst <= 4.0 / 255.0, "max {worst}");
        let (cw, ch) = (1024.min(w), 1024.min(h));
        let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
        let mut crop = WorkingImage::new(cw, ch);
        for y in 0..ch {
            let from = ((y0 + y) * w + x0) * 3;
            crop.data[y * cw * 3..(y + 1) * cw * 3]
                .copy_from_slice(&image.data[from..from + cw * 3]);
        }
        let (worst, mean) = gpu_against_cpu(&device, &queue, &crop, &locals, 0.0, false);
        eprintln!("its middle, GPU against CPU: max {worst:.4}, mean {mean:.5}");
        assert!(worst <= 1.5 / 255.0, "max {worst}");
    }
}
