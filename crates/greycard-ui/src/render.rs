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
            Shape::Brush { .. }
            | Shape::Subject {}
            | Shape::Background {}
            | Shape::Sky { .. }
            | Shape::Object { .. } => match raster {
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
#[derive(Debug, Clone, PartialEq)]
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
    /// Write this local's mask coverage into the alpha, for the
    /// scopes to weigh by (`scope.wgsl`). Only the analysis draw of
    /// [`Renderer::analyze`] sets it; the alpha is 1 otherwise.
    pub weigh_by: Option<usize>,
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
    /// The display curve per channel or AgX, for a scene.
    pub display_curve: greycard_edit::DisplayCurve,
    /// Quarter turns clockwise the frame has been turned since the
    /// picture on the GPU was developed: the view is of the turned
    /// source, and the shader reads the texture through the turn, so
    /// a frame turned shows turned at once, masks and all, while its
    /// develop is still on the way.
    pub source_turn: u8,
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
            weigh_by: None,
            vignette: Vignette::default(),
            grain: Grain::default(),
            warn: Warn::default(),
            canvas: [0.0; 3],
            source: Source::Scene,
            display_curve: greycard_edit::DisplayCurve::Channels,
            source_turn: 0,
        }
    }

    /// A blank view with `edit`'s look on it, as the viewport shows it:
    /// the light through its switch, the mixer as the black and white
    /// leaves it, the curves with the grading baked in, the vignette
    /// and the grain. The one place the viewport reads them from an
    /// edit, so the parity test reads them the same way.
    pub fn with_look(edit: &greycard_edit::Edit) -> Self {
        View {
            light: edit.light.effective(),
            mixer: edit.acting_mixer(),
            color: edit.color,
            bw: edit.bw,
            tint: edit.tint,
            curves: edit.curves.bake_with(&edit.grading),
            display_curve: edit.display_curve,
            vignette: edit.vignette,
            grain: edit.grain,
            ..View::blank()
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
    /// What the target holds: the size and the view it was drawn
    /// for. `render` draws again only for another view or size, or
    /// after the source or a table changed underneath (`redraw`),
    /// so a window at rest costs no frame.
    drawn: Option<(u32, u32, View)>,
    redraw: bool,
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
    /// `latest` came in since `analyze` last handed it out: the bins
    /// go to the window once, or every frame would repaint the scope
    /// and the curve and ask for the next frame, without end.
    fresh: bool,
    /// The analysis picture itself, read back beside the bins for
    /// the navigator: its staging buffer, sized for the picture's
    /// height, the read on its way, and the last picture read, not
    /// yet taken.
    picture_staging: Option<gpu::Buffer>,
    picture_in_flight: Option<std::sync::mpsc::Receiver<Result<(), gpu::BufferAsyncError>>>,
    picture: Option<slint::Image>,
    /// The edit, scope and weighing mask the bins in flight or latest
    /// were taken under; a new analysis only when one of those, or the
    /// image, changes.
    analyzed: Option<(EditKey, Scope, Option<usize>)>,
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
            in_flight: None,
            latest: None,
            fresh: false,
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
            drawn: None,
            redraw: true,
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
            // The shape, then 0 the display curve per channel, 1 a
            // clip, for a picture that has had its curve, 2 AgX.
            curve: match (v.source, v.display_curve) {
                (Source::Scene, greycard_edit::DisplayCurve::Channels) => 0.0,
                (Source::Display, _) => 1.0,
                (Source::Scene, greycard_edit::DisplayCurve::Agx) => 2.0,
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
        // Collect a finished read back.
        let _ = self.device.poll(gpu::PollType::Poll);
        if let Some(rx) = &self.scopes.in_flight
            && let Ok(result) = rx.try_recv()
        {
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
                    // A weighed analysis holds the mask in its alpha;
                    // the navigator shows the picture whole.
                    if matches!(self.scopes.analyzed, Some((_, _, Some(_)))) {
                        for p in buf.make_mut_slice() {
                            p.a = 0xff;
                        }
                    }
                    self.scopes.picture = Some(slint::Image::from_rgba8(buf));
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
            if std::mem::take(&mut self.scopes.fresh) {
                self.scopes.latest.as_ref().map(|(s, b)| (*s, b.as_slice()))
            } else {
                None
            },
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

    /// The developed picture on a grid about `across` pixels wide, a
    /// pixel at each point of it (not a mean), in the working space
    /// at the white it was developed at, as [`Self::sample`] reads
    /// it: the whole frame, small, for the auto white balance. Only
    /// the rows on the grid come back from the GPU. `None` with no
    /// picture.
    pub fn sample_grid(&self, across: u32) -> Result<Option<Vec<[f32; 3]>>> {
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
        Ok(Some(out))
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
        self.redraw = true;
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
        self.redraw = true;
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
        let instance =
            gpu::Instance::new(gpu::InstanceDescriptor::new_without_display_handle_from_env());
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
            // Under CI the skip is a failure: the runner installs a
            // software adapter so that these tests run, and a run
            // that finds none has lost it, not earned a pass.
            assert!(
                std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
                "{what} has no GPU to run on and GREYCARD_REQUIRE_GPU is set"
            );
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
            let target = renderer.render(w as u32, h as u32, &view).0;
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

    /// The scopes' bins as the compute pass makes them, for `scope`
    /// weighed by `weigh_by`: `analyze` until its read back lands.
    fn gpu_bins(
        renderer: &mut Renderer,
        view: &View,
        scope: Scope,
        weigh_by: Option<usize>,
    ) -> Vec<u32> {
        for _ in 0..100 {
            let (bins, in_flight) = renderer.analyze(view, scope, weigh_by);
            if !in_flight
                && let Some((taken, bins)) = bins
                && taken == scope
            {
                return bins.to_vec();
            }
            let _ = renderer.device.poll(gpu::PollType::wait_indefinitely());
        }
        panic!("the scope's read back never landed");
    }

    /// The scopes weighed by a mask, on the GPU, against the CPU's
    /// reference (`scope::weighted_bins`) over the same analysis
    /// picture and coverage: the picture drawn through the view as
    /// `analyze` draws it and read back, its alpha checked against the
    /// mask's own weight, then binned both ways. Unweighted, the bins
    /// are the unweighted reference's and the picture is the same with
    /// the weighing on or off.
    #[test]
    fn the_weighted_scopes_are_the_cpus() {
        let Some((device, queue)) = device("the weighted scopes' GPU check") else {
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
        // A gradient across the top, the skin, and a window above
        // anything the field holds, which takes in nothing.
        let mut locals = vec![
            local_of(vec![(
                Shape::Linear {
                    from: [0.2, 0.0],
                    to: [0.8, 0.0],
                },
                Mode::Add,
            )]),
            local_of(vec![(Shape::skin(), Mode::Add)]),
            local_of(vec![(
                Shape::Luminance {
                    low: 1.5,
                    high: 2.0,
                    low_feather: 0.0,
                    high_feather: 0.0,
                },
                Mode::Add,
            )]),
        ];
        locals[0].baked.light.exposure = 0.5;
        view.locals = locals;
        let _ = renderer.render(w as u32, h as u32, &view).0;
        // The analysis picture as `analyze` draws it.
        let aw = ANALYSIS_WIDTH;
        let ah = (aw * h as u32 / w as u32).max(1);
        let whole = |weigh_by: Option<usize>, show: Option<usize>| View {
            zoom: aw as f32 / w as f32,
            center: (w as f32 / 2.0, h as f32 / 2.0),
            weigh_by,
            show_mask: show,
            mask_alone: show.is_some(),
            ..view.clone()
        };
        let mut shot = |v: &View| {
            let t = renderer.render(aw, ah, v).0;
            renderer.read_back(&t).expect("read back")
        };
        let plain = shot(&whole(None, None));
        let (aw, ah) = (aw as usize, ah as usize);
        let rgb = |img: &image::RgbaImage| -> Vec<[f32; 3]> {
            img.pixels()
                .map(|p| [0, 1, 2].map(|c| f32::from(p[c]) / 255.0))
                .collect()
        };
        assert!(plain.pixels().all(|p| p[3] == 0xff), "alpha is 1 unweighed");
        let pixels = rgb(&plain);
        let mut shots = Vec::new();
        for k in 0..view.locals.len() {
            let weighed = shot(&whole(Some(k), None));
            // The weighing changes the alpha and nothing else.
            assert_eq!(rgb(&weighed), pixels, "mask {k}: the picture moved");
            // And the alpha is the mask's weight, as the mask on show
            // draws it.
            let alone = shot(&whole(None, Some(k)));
            let worst = weighed
                .pixels()
                .zip(alone.pixels())
                .map(|(a, b)| a[3].abs_diff(b[0]))
                .max()
                .unwrap_or(0);
            assert!(worst <= 1, "mask {k}: alpha {worst} levels from the mask");
            let coverage: Vec<f32> = weighed.pixels().map(|p| f32::from(p[3]) / 255.0).collect();
            shots.push(coverage);
        }
        // The first two take in part of the picture and the third none.
        let cover = |c: &[f32]| c.iter().sum::<f32>() / c.len() as f32;
        assert!(
            (0.1..0.9).contains(&cover(&shots[0])),
            "{}",
            cover(&shots[0])
        );
        assert!(
            (0.005..0.9).contains(&cover(&shots[1])),
            "{}",
            cover(&shots[1])
        );
        assert_eq!(cover(&shots[2]), 0.0);
        // The binning is exact arithmetic on the same bytes, and the
        // histogram and waveforms agree to the count; a vectorscope
        // cell edge, where the shader's floating point rounds a last
        // bit apart from the CPU's, moves the odd one, weighed or not
        // (2 counts on Vulkan here, before the weighing too).
        let compare = |what: &str, gpu: &[u32], cpu: &[u32]| {
            let off: u64 = gpu
                .iter()
                .zip(cpu)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            let mass: u64 = cpu.iter().map(|&n| u64::from(n)).sum();
            eprintln!("{what}: {off} of {mass} apart");
            assert!(off <= 8, "{what}: {off} of {mass} apart");
        };
        for scope in Scope::ALL {
            let gpu = gpu_bins(&mut renderer, &view, scope, None);
            let cpu = scope::bins(scope, &pixels, aw, ah);
            compare(&format!("{scope:?} unweighed"), &gpu, &cpu);
            if scope != Scope::Vector {
                assert_eq!(gpu, cpu, "{scope:?} unweighed");
            }
            for (k, coverage) in shots.iter().enumerate() {
                let gpu = gpu_bins(&mut renderer, &view, scope, Some(k));
                let cpu = scope::weighted_bins(scope, &pixels, coverage, aw, ah);
                compare(&format!("{scope:?} weighed by mask {k}"), &gpu, &cpu);
                if scope != Scope::Vector {
                    assert_eq!(gpu, cpu, "{scope:?} weighed by mask {k}");
                }
                if k == 2 {
                    assert!(gpu.iter().all(|&n| n == 0), "{scope:?}: nothing selected");
                }
            }
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
            let target = renderer.render(w as u32, h as u32, view).0;
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

    /// A picture read through a turn it has not caught up with draws
    /// what the picture developed at that turn draws: the texture
    /// turned on the CPU (`orient`, as the develop turns it) and read
    /// straight, against the unturned texture read through
    /// `source_turn`, for each quarter. The mixer's local mean and
    /// the cubic reader both read about a pixel, and a mask sits off
    /// center in the turned picture's own positions, so a reader or a
    /// mask left in the texture's positions shows.
    #[test]
    fn a_texture_read_through_a_turn_is_the_turned_texture() {
        use greycard_core::develop::orient;
        use greycard_core::raw::Orientation;
        let Some((device, queue)) = device("the lagging turn's check") else {
            return;
        };
        let image = field();
        // A guide plane whose texels divide the picture, turned with
        // it by the same map, so the texture's guide read through the
        // turn can be told from the turned one's.
        let scale = 8;
        let guide_of =
            |w: usize, h: usize, at: &dyn Fn(usize, usize) -> f32| crate::finish::Guide {
                width: w / scale,
                height: h / scale,
                scale,
                data: (0..h / scale)
                    .flat_map(|y| (0..w / scale).map(move |x| (x, y)))
                    .map(|(x, y)| at(x, y))
                    .collect(),
            };
        let plain = |x: usize, y: usize| ((x * 7 + y * 3) % 11) as f32 * 0.3 - 1.5;
        let (iw, ih) = (image.width, image.height);
        let guide = guide_of(iw, ih, &plain);
        let mut renderer = Renderer::new(&device, &queue);
        for ((quarters, orientation), cubic) in [
            (1u8, Orientation::Rotate90),
            (2, Orientation::Rotate180),
            (3, Orientation::Rotate270),
        ]
        .into_iter()
        .flat_map(|q| [(q, true), (q, false)])
        {
            let turned = orient(image.clone(), orientation);
            let (w, h) = (turned.width, turned.height);
            let (gw, gh) = (iw / scale, ih / scale);
            let turned_guide = guide_of(w, h, &|x, y| {
                let (tx, ty) = crate::placeholder::texel_of(
                    (x as i64, y as i64),
                    (w as u32 / scale as u32, h as u32 / scale as u32),
                    quarters,
                );
                assert!((tx as usize) < gw && (ty as usize) < gh);
                plain(tx as usize, ty as usize)
            });
            let mut view = View::blank();
            view.light.tone.shadows = 0.6;
            view.light.tone.highlights = -0.5;
            view.center = (w as f32 / 2.0, h as f32 / 2.0);
            view.plane = (w as f32, h as f32);
            view.frame_size = (w as f32, h as f32);
            view.cubic = cubic;
            view.mixer.enabled = true;
            view.mixer.hue[1] = 20.0;
            view.mixer.saturation[5] = 0.6;
            let mut local = local_of(vec![(
                Shape::Radial {
                    center: [0.3, 0.2],
                    radius: [0.15, 0.1],
                    angle: 0.0,
                    feather: 0.3,
                },
                Mode::Add,
            )]);
            local.baked.light.exposure = 1.0;
            view.locals = vec![local];
            let mut shot = |source: &WorkingImage, guide: &crate::finish::Guide, lag: u8| {
                renderer.upload(&crate::worker::Halves::from_image(source, None));
                renderer.set_guide(guide);
                let v = View {
                    source_turn: lag,
                    ..view.clone()
                };
                let target = renderer.render(w as u32, h as u32, &v).0;
                renderer.read_back(&target).expect("read back")
            };
            let developed = shot(&turned, &turned_guide, 0);
            let lagging = shot(&image, &guide, quarters);
            let worst = developed
                .as_raw()
                .iter()
                .zip(lagging.as_raw())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            eprintln!("{quarters} quarters, cubic {cubic}: at most {worst} levels apart");
            assert!(
                worst <= 1,
                "{quarters} quarters, cubic {cubic}: the lagging texture is {worst} levels off"
            );
            // And reading it straight is another picture, so the
            // check is of the turn.
            if quarters % 2 == 0 {
                assert!(shot(&image, &guide, 0) != developed);
                // And the guide is read through the turn too: the
                // texture's own guide read straight is another picture.
                assert!(shot(&image, &turned_guide, quarters) != developed);
            }
        }
    }

    /// A dropper's read during a turn's lag is the read of the develop
    /// at the new turn: the unturned texture sampled at the texel
    /// `texel_of` names is the turned texture sampled at the pixel.
    #[test]
    fn a_pick_through_a_turn_reads_the_pixel_the_turned_develop_has() {
        use greycard_core::develop::orient;
        use greycard_core::raw::Orientation;
        let Some((device, queue)) = device("the lagging pick's check") else {
            return;
        };
        let image = field();
        let mut renderer = Renderer::new(&device, &queue);
        for (lag, orientation) in [
            (1u8, Orientation::Rotate90),
            (2, Orientation::Rotate180),
            (3, Orientation::Rotate270),
        ] {
            let turned = orient(image.clone(), orientation);
            let size = (turned.width as u32, turned.height as u32);
            let points = [
                (0i64, 0i64),
                (7, 3),
                (40, 150),
                (size.0 as i64 - 1, size.1 as i64 - 2),
            ];
            renderer.upload(&crate::worker::Halves::from_image(&turned, None));
            let developed: Vec<_> = points
                .iter()
                .map(|&(x, y)| renderer.sample(x, y, 2).expect("sampled"))
                .collect();
            renderer.upload(&crate::worker::Halves::from_image(&image, None));
            for (k, &(x, y)) in points.iter().enumerate() {
                let (tx, ty) = crate::placeholder::texel_of((x, y), size, lag);
                let lagging = renderer.sample(tx, ty, 2).expect("sampled");
                assert!(
                    lagging.is_some(),
                    "{lag} quarters at {x},{y}: off the picture"
                );
                assert_eq!(lagging, developed[k], "{lag} quarters at {x},{y}");
            }
        }
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
            let target = renderer.render(w as u32, h as u32, view).0;
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

    /// AgX on the GPU is `agx::tone_agx` on the CPU, over the parity
    /// frame's primaries, near blacks and highlights past white, at
    /// exposures from three stops down to four up: the switch reaches
    /// the shader through `with_look`, and both sides read the same
    /// field of the edit.
    #[test]
    fn the_agx_curve_on_the_gpu_is_the_cpus() {
        a_display_curve_on_the_gpu_is_the_cpus(greycard_edit::DisplayCurve::Agx, "the AgX curve");
    }

    /// The gate between a fitted look table and a picture's display
    /// curve, on both sides: a table fitted under AgX applies to a
    /// picture on AgX and is no look at all on one on per channel, the
    /// GPU's picture then being, level for level, its picture with no
    /// look; a general table applies under both; and wherever a table
    /// applies the GPU is the CPU's finish with it. The gate is one
    /// decision (`look::gate`) made before either side is handed a
    /// table, which is what the agreement rests on.
    #[test]
    fn a_fitted_table_is_off_under_another_curve_on_both_sides() {
        use greycard_edit::DisplayCurve;
        let Some((device, queue)) = device("the fitted table's gate") else {
            return;
        };
        let image = parity_frame();
        let (w, h) = (image.width, image.height);
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        let seen = WorkingImage {
            width: w,
            height: h,
            data: image
                .data
                .iter()
                .map(|v| half::f16::from_f32(*v).to_f32())
                .collect(),
        };
        let to_out = crate::export::Space::Srgb.matrix();
        let tagged = |comments: &[&str]| {
            let mut t = (*look_tables()[0]).clone();
            t.comments = comments.iter().map(|c| c.to_string()).collect();
            Arc::new(t)
        };
        let fitted = tagged(&[greycard_edit::look::FITTED, "display_curve: agx"]);
        let general = tagged(&["made by a grading application"]);
        let mut render = |edit: &greycard_edit::Edit, look: Option<&lut::Look>| {
            renderer.set_look(look);
            renderer.set_output(to_out);
            let view = View {
                center: (w as f32 / 2.0, h as f32 / 2.0),
                plane: (w as f32, h as f32),
                frame_size: (w as f32, h as f32),
                ..View::with_look(edit)
            };
            let target = renderer.render(w as u32, h as u32, &view).0;
            let shown = renderer.read_back(&target).expect("read back");
            let gpu: Vec<u8> = shown.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
            let cpu = crate::finish::finish_with(
                &seen,
                None,
                &Baked::global(edit, Source::Scene),
                &[],
                |x, y| ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32),
                |_, _| (0.0, None),
                None,
                look.filter(|l| !l.is_off()),
                &to_out,
                |v: f32| v,
            );
            (cpu, gpu)
        };
        let worst = |cpu: &[f32], gpu: &[u8]| {
            cpu.iter()
                .zip(gpu)
                .map(|(c, g)| (c - f32::from(*g) / 255.0).abs())
                .fold(0.0f32, f32::max)
        };
        let mean_levels = |a: &[u8], b: &[u8]| {
            a.iter()
                .zip(b)
                .map(|(x, y)| (f64::from(*x) - f64::from(*y)).abs())
                .sum::<f64>()
                / a.len() as f64
        };
        for curve in DisplayCurve::ALL {
            let edit = greycard_edit::Edit {
                display_curve: curve,
                ..Default::default()
            };
            let (bare_cpu, bare_gpu) = render(&edit, None);
            for (table, what) in [(&fitted, "fitted under AgX"), (&general, "general")] {
                let resolved = lut::Look::new(table.clone(), 1.0).unwrap();
                let look = greycard_edit::look::gate(Some(resolved), curve);
                let applies = what == "general" || curve == DisplayCurve::Agx;
                assert_eq!(look.is_some(), applies, "{what} under {curve:?}");
                let (cpu, gpu) = render(&edit, look.as_ref());
                let d = worst(&cpu, &gpu);
                let moved = mean_levels(&gpu, &bare_gpu);
                eprintln!(
                    "{what} under {curve:?}: applied {applies}, GPU against CPU {:.2} levels, \
                     {moved:.2} levels from no look on average",
                    d * 255.0
                );
                assert!(d < 2.5 / 255.0, "{what} under {curve:?}: {d}");
                if applies {
                    assert!(moved > 1.0, "{what} under {curve:?} changed nothing");
                } else {
                    assert_eq!(gpu, bare_gpu, "{what} under {curve:?} on the GPU");
                    assert_eq!(cpu, bare_cpu, "{what} under {curve:?} on the CPU");
                }
            }
        }
    }

    fn a_display_curve_on_the_gpu_is_the_cpus(curve: greycard_edit::DisplayCurve, what: &str) {
        let Some((device, queue)) = device(&format!("{what}'s check")) else {
            return;
        };
        let image = parity_frame();
        let (w, h) = (image.width, image.height);
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        let seen: Vec<f32> = image
            .data
            .iter()
            .map(|v| half::f16::from_f32(*v).to_f32())
            .collect();
        let to_out = crate::export::Space::Srgb.matrix();
        let mut edit = greycard_edit::Edit {
            display_curve: curve,
            ..Default::default()
        };
        let mut worst = (0.0f32, [0.0f32; 3], [0.0f32; 3], [0.0f32; 3], 0.0f32);
        // The signed difference's mean and its root mean square over
        // the channels not at either end, in levels: the GPU is read
        // back in 8 bits, so the square root of a twelfth (0.29) is
        // the quantization's own, and a mean well under a hundredth of
        // a level says the curve on the GPU has no bias against the
        // CPU's that the quantization could hide.
        let (mut sum, mut sq, mut n) = (0.0f64, 0.0f64, 0usize);
        for stops in [-3.0f32, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0] {
            edit.light.exposure = stops;
            let view = View {
                center: (w as f32 / 2.0, h as f32 / 2.0),
                plane: (w as f32, h as f32),
                frame_size: (w as f32, h as f32),
                ..View::with_look(&edit)
            };
            let target = renderer.render(w as u32, h as u32, &view).0;
            let shown = renderer.read_back(&target).expect("read back");
            let global = Baked::global(&edit, Source::Scene);
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 3;
                    let px = [seen[i], seen[i + 1], seen[i + 2]];
                    let cpu = crate::finish::finish_pixel(px, &global, &[], &to_out);
                    let gpu = shown.get_pixel(x as u32, y as u32);
                    let gpu = [0, 1, 2].map(|k| f32::from(gpu[k]) / 255.0);
                    for k in 0..3 {
                        let d = (cpu[k] - gpu[k]).abs();
                        if d > worst.0 {
                            worst = (d, px, cpu, gpu, stops);
                        }
                        if cpu[k] > 0.02 && cpu[k] < 0.98 {
                            let signed = f64::from(gpu[k] - cpu[k]) * 255.0;
                            sum += signed;
                            sq += signed * signed;
                            n += 1;
                        }
                    }
                }
            }
        }
        eprintln!(
            "{what}, GPU against CPU: {worst:?}; over {n} channels mean {:+.4} levels, rms {:.3}",
            sum / n as f64,
            (sq / n as f64).sqrt()
        );
        assert!(worst.0 < 1.5 / 255.0, "{worst:?}");
    }

    /// How far each display curve's value on the GPU is from the CPU's
    /// before the point curves, which an 8-bit read back cannot show on
    /// its own: a master point curve rising from 0 to 1 across a
    /// fiftieth of the encoded range magnifies every pixel that lands
    /// in it by fifty to seventy-five, and that curve inverted on the
    /// GPU's level gives the pixel's encoded value to a few
    /// hundred-thousandths. The output space is Rec.2020, the working
    /// space, so the level is the curve's own value. Over the parity
    /// frame at three exposures and five windows, for each curve: how
    /// many channels landed in a window, the root mean square and the
    /// largest gap, in levels of the encoded value. The random parity
    /// test finds such a gap only when a drawn curve is this steep
    /// where a pixel lands, and then reads it as whole levels; this
    /// says how big the gap is for every pixel, whichever curve.
    #[test]
    fn a_display_curves_gap_before_the_point_curves() {
        use greycard_edit::curve::lookup;
        let Some((device, queue)) = device("the display curves' gap") else {
            return;
        };
        let image = parity_frame();
        let (w, h) = (image.width, image.height);
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        renderer.set_output(crate::export::Space::Rec2020.matrix());
        let seen: Vec<f32> = image
            .data
            .iter()
            .map(|v| half::f16::from_f32(*v).to_f32())
            .collect();
        let mut report = Vec::new();
        for curve in greycard_edit::DisplayCurve::ALL {
            let (mut sq, mut sum, mut n, mut worst) = (0.0f64, 0.0f64, 0usize, 0.0f32);
            for window in [0.1f32, 0.3, 0.5, 0.7, 0.9] {
                for stops in [-2.0f32, 0.0, 2.0] {
                    let mut edit = greycard_edit::Edit {
                        display_curve: curve,
                        ..Default::default()
                    };
                    edit.light.exposure = stops;
                    edit.curves.rgb = vec![
                        [0.0, 0.0],
                        [window - 0.01, 0.0],
                        [window + 0.01, 1.0],
                        [1.0, 1.0],
                    ];
                    let view = View {
                        center: (w as f32 / 2.0, h as f32 / 2.0),
                        plane: (w as f32, h as f32),
                        frame_size: (w as f32, h as f32),
                        ..View::with_look(&edit)
                    };
                    let target = renderer.render(w as u32, h as u32, &view).0;
                    let shown = renderer.read_back(&target).expect("read back");
                    let global = Baked::global(&edit, Source::Scene);
                    let look = edit.look();
                    for y in 0..h {
                        for x in 0..w {
                            let i = (y * w + x) * 3;
                            let px = [seen[i], seen[i + 1], seen[i + 2]];
                            let before = crate::finish::pick(
                                px,
                                &look.light,
                                &look.mixer,
                                &look.color,
                                &edit.bw,
                                &look.tint,
                                &global.curves,
                                Source::Scene,
                                curve,
                            )
                            .encoded;
                            let gpu = shown.get_pixel(x as u32, y as u32);
                            for k in 0..3 {
                                let e = before[k];
                                let level = f32::from(gpu[k]) / 255.0;
                                if (e - window).abs() > 0.008 || !(0.02..0.98).contains(&level) {
                                    continue;
                                }
                                // The curve inverted on the GPU's level, by
                                // bisection over the window, where it rises.
                                let (mut lo, mut hi) = (window - 0.011, window + 0.011);
                                for _ in 0..40 {
                                    let mid = 0.5 * (lo + hi);
                                    if lookup(&global.curves, k, mid) < level {
                                        lo = mid;
                                    } else {
                                        hi = mid;
                                    }
                                }
                                let gap = (0.5 * (lo + hi) - e) * 255.0;
                                sq += f64::from(gap * gap);
                                sum += f64::from(gap);
                                n += 1;
                                worst = worst.max(gap.abs());
                            }
                        }
                    }
                }
            }
            report.push((
                curve,
                n,
                (sq / n.max(1) as f64).sqrt(),
                sum / n.max(1) as f64,
                worst,
            ));
        }
        eprintln!("| curve | channels | rms gap | mean gap | largest gap |");
        for (curve, n, rms, mean, worst) in &report {
            eprintln!("| {curve:?} | {n} | {rms:.4} | {mean:+.4} | {worst:.4} |");
        }
        // NVIDIA and lavapipe both: an rms of 0.006 levels and a largest
        // of 0.02 for each of the three curves, which is the chain's
        // float rounding; a tenth of a level is five times that.
        for (curve, n, _, _, worst) in &report {
            assert!(*n > 100, "{curve:?}: only {n} channels landed in a window");
            assert!(
                *worst < 0.1,
                "{curve:?}: {worst} levels before the point curves"
            );
        }
    }

    /// Milliseconds a 4K viewport frame takes on this device with each
    /// display curve, the default edit otherwise; printed, for the
    /// notes. The frame is the parity frame drawn at 3840 by 2160, so
    /// the cost is the shader's per pixel and not the texture's.
    #[test]
    #[ignore = "a timing, run by hand in release"]
    fn the_cost_of_a_4k_frame() {
        let Some((device, queue)) = device("the 4K frame's timing") else {
            return;
        };
        let image = parity_frame();
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        let (w, h) = (3840u32, 2160u32);
        for curve in greycard_edit::DisplayCurve::ALL {
            let edit = greycard_edit::Edit {
                display_curve: curve,
                ..Default::default()
            };
            // The renderer draws again only for another view, so each
            // frame moves the exposure by a thousandth of a stop.
            let mut time = |frames: u32| {
                let start = std::time::Instant::now();
                for i in 0..frames {
                    let mut edit = edit.clone();
                    edit.light.exposure = i as f32 * 1e-3;
                    let view = View {
                        center: (image.width as f32 / 2.0, image.height as f32 / 2.0),
                        plane: (image.width as f32, image.height as f32),
                        frame_size: (w as f32, h as f32),
                        ..View::with_look(&edit)
                    };
                    let (target, _) = renderer.render(w, h, &view);
                    // Read back, so the frame is finished before the
                    // next; the copy of 33 MB is in the figure.
                    let _ = renderer.read_back(&target);
                }
                start.elapsed().as_secs_f64() * 1e3 / f64::from(frames)
            };
            time(3);
            eprintln!(
                "{curve:?}: {:.2} ms a 4K frame, read back included",
                time(20)
            );
        }
    }

    /// The auto white balance's grid is the picture's own pixels at
    /// the grid's points, as the CPU would pick them, row and column.
    #[test]
    fn the_sample_grid_reads_the_pixels_on_its_points() {
        let Some((device, queue)) = device("the sample grid's check") else {
            return;
        };
        let image = field();
        let (w, h) = (image.width, image.height);
        let mut renderer = Renderer::new(&device, &queue);
        assert!(renderer.sample_grid(64).unwrap().is_none());
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        let grid = renderer.sample_grid(64).unwrap().unwrap();
        let step = w / 64;
        let mut want = Vec::new();
        for y in (step / 2..h).step_by(step) {
            for x in (step / 2..w).step_by(step) {
                let i = (y * w + x) * 3;
                want.push([0, 1, 2].map(|k| half::f16::from_f32(image.data[i + k]).to_f32()));
            }
        }
        assert_eq!(grid.len(), want.len());
        assert_eq!(grid, want);
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
            let target = renderer.render(w as u32, h as u32, &view).0;
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

    /// A small seeded generator for the parity test: splitmix64, each
    /// edit seeded from the run's seed and its own index, so one edit
    /// is replayed from those two numbers alone.
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64, k: u64) -> Self {
            let mut r = Rng(seed ^ k.wrapping_mul(0xD1B5_4A32_D192_ED03));
            r.next();
            r
        }

        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// 0 to 1, 24 bits of it.
        fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32
        }

        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.unit()
        }

        fn chance(&mut self, p: f32) -> bool {
            self.unit() < p
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        /// A slider: moved, with chance `p`, else at `rest`; moved, at
        /// `lo` a fifth of the time, at `hi` a fifth, and anywhere
        /// between otherwise, since the ends and their sums are where
        /// the arithmetic breaks first. Both draws are taken either
        /// way, so what follows is the same whatever this one says.
        fn slider(&mut self, p: f32, lo: f32, hi: f32, rest: f32) -> f32 {
            let moved = self.chance(p);
            let u = self.unit();
            let v = if u < 0.2 {
                lo
            } else if u >= 0.8 {
                hi
            } else {
                lo + (hi - lo) * (u - 0.2) / 0.6
            };
            if moved { v } else { rest }
        }

        /// A slider always moved: `slider` with its ends as likely.
        fn moved(&mut self, lo: f32, hi: f32) -> f32 {
            self.slider(1.0, lo, hi, lo)
        }
    }

    /// The parts of an edit the parity test draws, a bit each, so a
    /// failing edit can be drawn again with one part left at its
    /// default to see which part the difference is in.
    const PARTS: [&str; 18] = [
        "light",
        "light switch",
        "point curves",
        "parametric curve",
        "color curves",
        "grading",
        "mixer",
        "color",
        "black and white",
        "tint",
        "vignette",
        "grain",
        "locals",
        "look table",
        "guide plane",
        "display source",
        "output space",
        "display curve",
    ];

    fn part(name: &str) -> u32 {
        1 << PARTS.iter().position(|p| *p == name).expect("a part")
    }

    /// One random edit: the edit, and what the viewport and the
    /// export are handed beside it.
    struct Case {
        edit: greycard_edit::Edit,
        /// The rasters of each adjustment's components, by index.
        rasters: Vec<Vec<Option<RasterRef>>>,
        look: Option<lut::Look>,
        source: Source,
        /// Which guide plane, if any: 0 the picture's own, 1 a coarser
        /// one (the test's `guides`).
        guide: Option<usize>,
        space: crate::export::Space,
    }

    /// A point curve: the ends at x 0 and 1, up to three points
    /// between them from `first` to 0.99 and at least `MIN_GAP` apart,
    /// every y anywhere from 0 to 1, as the curve editor lets them be
    /// dragged (`panel::curve`). A color curve's ends rest at 0.5, a
    /// point curve's on the diagonal.
    fn random_points(rng: &mut Rng, color: bool) -> Vec<greycard_edit::curve::Point> {
        use greycard_edit::curve::MIN_GAP;
        let n = rng.below(4);
        // A color curve's points start a twentieth of the way up, not
        // at the panel's 0.01: excluded. The color curves look their
        // shift up by Oklab lightness, a cube root, whose slope at zero
        // has no bound, so near black a point within a few hundredths
        // of the start turns a few billionths of light into a cast, and
        // the shader's fused multiply-adds in the point curves leave
        // such light where the CPU's separate roundings leave none
        // (seed 10, edit 117: 3 levels after a look table). That is the
        // export's own sensitivity, there on any device.
        let first = if color { 0.05 } else { 0.01 };
        let mut xs: Vec<f32> = (0..n).map(|_| rng.moved(first, 0.99)).collect();
        xs.sort_by(f32::total_cmp);
        let mut kept: Vec<f32> = Vec::new();
        for x in xs {
            if kept.last().is_none_or(|l| x - l >= MIN_GAP) {
                kept.push(x);
            }
        }
        let (rest0, rest1) = if color { (0.5, 0.5) } else { (0.0, 1.0) };
        let (y0, y1) = (
            rng.slider(0.4, 0.0, 1.0, rest0),
            rng.slider(0.4, 0.0, 1.0, rest1),
        );
        let mut points = vec![[0.0, y0]];
        points.extend(kept.into_iter().map(|x| [x, rng.moved(0.0, 1.0)]));
        points.push([1.0, y1]);
        points
    }

    /// A look, every slider over the range the panel gives it
    /// (`edit.slint`, `color.slint`, `curve.slint`), each moved with
    /// chance `p`; the parts in `skip` left at their defaults.
    fn random_look(rng: &mut Rng, p: f32, skip: u32) -> Look {
        use greycard_edit::grading::Wheel;
        let has = |name: &str| skip & part(name) == 0;
        let mut look = Look::default();
        // Light: exposure ±5, contrast 0.5 to 2, highlights, shadows
        // and whites ±2, blacks ±0.3; the section's switch.
        let mut light = Light {
            enabled: !rng.chance(0.15),
            exposure: rng.slider(p, -5.0, 5.0, 0.0),
            ..Light::default()
        };
        light.tone.contrast = rng.slider(p, 0.5, 2.0, 1.0);
        light.tone.highlights = rng.slider(p, -2.0, 2.0, 0.0);
        light.tone.shadows = rng.slider(p, -2.0, 2.0, 0.0);
        light.tone.whites = rng.slider(p, -2.0, 2.0, 0.0);
        light.tone.blacks = rng.slider(p, -0.3, 0.3, 0.0);
        if !has("light switch") {
            light.enabled = true;
        }
        if has("light") {
            look.light = light;
        }
        // The curves: the section's switch, the four channels' points,
        // the parametric's four amounts ±1 and its splits, the two
        // color curves.
        let mut curves = greycard_edit::Curves {
            enabled: !rng.chance(0.1),
            ..Default::default()
        };
        let channels = [
            &mut curves.rgb,
            &mut curves.red,
            &mut curves.green,
            &mut curves.blue,
        ];
        let mut drawn = Vec::new();
        for _ in 0..4 {
            let moved = rng.chance(p);
            let points = random_points(rng, false);
            drawn.push(moved.then_some(points));
        }
        if has("point curves") {
            for (c, d) in channels.into_iter().zip(drawn) {
                if let Some(points) = d {
                    *c = points;
                }
            }
        }
        let mut para = greycard_edit::Parametric {
            highlights: rng.slider(p, -1.0, 1.0, 0.0),
            lights: rng.slider(p, -1.0, 1.0, 0.0),
            darks: rng.slider(p, -1.0, 1.0, 0.0),
            shadows: rng.slider(p, -1.0, 1.0, 0.0),
            ..Default::default()
        };
        for i in 0..3 {
            let moved = rng.chance(p);
            let x = rng.unit();
            if moved {
                para.set_split(i, x);
            }
        }
        if has("parametric curve") {
            curves.parametric = para;
        }
        let rg = rng.chance(p).then(|| random_points(rng, true));
        let by = rng.chance(p).then(|| random_points(rng, true));
        if has("color curves") {
            if let Some(points) = rg {
                curves.red_green = points;
            }
            if let Some(points) = by {
                curves.blue_yellow = points;
            }
        }
        look.curves = curves;
        // Grading: three wheels, hue 0 to 360 and strength 0 to 1, and
        // the balance ±1.
        let wheel = |rng: &mut Rng| {
            let moved = rng.chance(p);
            let w = Wheel {
                hue: rng.range(0.0, 360.0),
                saturation: rng.moved(0.0, 1.0),
            };
            if moved { w } else { Wheel::default() }
        };
        let grading = greycard_edit::Grading {
            enabled: !rng.chance(0.1),
            shadows: wheel(rng),
            midtones: wheel(rng),
            highlights: wheel(rng),
            balance: rng.slider(p, -1.0, 1.0, 0.0),
        };
        if has("grading") {
            look.grading = grading;
        }
        // The mixer: eight bands, hue ±30 degrees, saturation and
        // luminance ±1.
        let mut mixer = Mixer {
            enabled: !rng.chance(0.15),
            ..Mixer::default()
        };
        for b in 0..greycard_edit::mixer::BANDS {
            mixer.hue[b] = rng.slider(p * 0.6, -30.0, 30.0, 0.0);
            mixer.saturation[b] = rng.slider(p * 0.6, -1.0, 1.0, 0.0);
            mixer.luminance[b] = rng.slider(p * 0.6, -1.0, 1.0, 0.0);
        }
        if has("mixer") {
            look.mixer = mixer;
        }
        // Color: vibrance and saturation ±1.
        let color = Color {
            enabled: !rng.chance(0.15),
            saturation: rng.slider(p, -1.0, 1.0, 0.0),
            vibrance: rng.slider(p, -1.0, 1.0, 0.0),
        };
        if has("color") {
            look.color = color;
        }
        // The tint: hue 0 to 360, amount 0 to 1.
        let tint = Tint {
            hue: rng.range(0.0, 360.0),
            amount: rng.slider(p, 0.0, 1.0, 0.0),
        };
        if has("tint") {
            look.tint = tint;
        }
        look
    }

    /// A brush's strokes, a few of them, adds and subtracts and an
    /// erase. The radius is drawn from 0.02 to 0.15 of the width
    /// rather than the panel's 0.002 to 0.5: painting a raster
    /// 2048 texels wide in a debug build costs by the disc's area,
    /// and what is checked is the raster's sampling, the same grid
    /// of texels whatever the strokes were.
    fn random_strokes(rng: &mut Rng, aspect: f32) -> Vec<greycard_edit::brush::Stroke> {
        use greycard_edit::brush::{Op, Stroke};
        let n = 1 + rng.below(3);
        (0..n)
            .map(|i| {
                let op = if i == 0 {
                    Op::Add
                } else {
                    Op::ALL[rng.below(Op::ALL.len())]
                };
                let mut s = Stroke::new(op, rng.range(0.02, 0.15), rng.unit(), rng.unit());
                let points = 2 + rng.below(3);
                s.points = (0..points)
                    .map(|_| [rng.range(-0.1, 1.1), rng.range(-0.1, aspect + 0.1)])
                    .collect();
                s
            })
            .collect()
    }

    /// The rasters the cases draw their raster shapes from, made once:
    /// three brushes and a learned mask (a model's, here a pattern of
    /// soft blobs and hard edges), each a raster of the picture's
    /// aspect, all `RASTER_WIDTH` wide as the app's are.
    fn raster_pool(seed: u64, aspect: f32) -> (Vec<(Shape, RasterRef)>, RasterRef) {
        use greycard_edit::brush::Raster;
        let mut rng = Rng::new(seed, u64::MAX);
        let brushes = (0..3)
            .map(|_| {
                let strokes = random_strokes(&mut rng, aspect);
                let raster = Raster::of(&strokes, aspect);
                (Shape::Brush { strokes }, RasterRef(Arc::new(raster)))
            })
            .collect();
        let w = RASTER_WIDTH;
        let h = ((w as f32 * aspect).round() as usize).max(1);
        let data = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let (u, v) = (x as f32 / w as f32, y as f32 / w as f32);
                let blob = (0.5 + 0.5 * (u * 9.0).sin() * (v * 13.0).cos()).clamp(0.0, 1.0);
                let edge = if (u - 0.6).abs() < 0.15 && v > 0.3 {
                    1.0
                } else {
                    0.0
                };
                ((blob * 0.7 + edge * 0.3) * 255.0).round() as u8
            })
            .collect();
        let learned = RasterRef(Arc::new(Raster::from_data(aspect, w, data)));
        (brushes, learned)
    }

    /// A mask of one to three shapes, each of any kind in any mode,
    /// on, off or inverted, with the raster each needs.
    fn random_mask(
        rng: &mut Rng,
        aspect: f32,
        brushes: &[(Shape, RasterRef)],
        learned: &RasterRef,
    ) -> (Mask, Vec<Option<RasterRef>>) {
        let n = 1 + rng.below(3);
        let mut components = Vec::new();
        let mut rasters = Vec::new();
        for _ in 0..n {
            let (shape, raster) = match rng.below(7) {
                0 => (
                    Shape::Linear {
                        from: [rng.range(-0.2, 1.2), rng.range(-0.2, aspect + 0.2)],
                        to: [rng.range(-0.2, 1.2), rng.range(-0.2, aspect + 0.2)],
                    },
                    None,
                ),
                1 => (
                    Shape::Radial {
                        center: [rng.range(0.0, 1.0), rng.range(0.0, aspect)],
                        radius: [rng.range(0.02, 0.7), rng.range(0.02, 0.7)],
                        angle: rng.range(0.0, 360.0),
                        feather: rng.moved(0.0, 1.0),
                    },
                    None,
                ),
                2 => {
                    let (shape, raster) = &brushes[rng.below(brushes.len())];
                    (shape.clone(), Some(raster.clone()))
                }
                3 => {
                    // A learned shape, its raster there or, now and
                    // then, still on its way.
                    let there = !rng.chance(0.2);
                    (Shape::Subject {}, there.then(|| learned.clone()))
                }
                // The panel's ranges: low and high 0 to 1, their
                // fades 0 to 0.5 (`mask.slint`).
                4 => (
                    Shape::Luminance {
                        low: rng.slider(0.8, 0.0, 1.0, 0.0),
                        high: rng.slider(0.8, 0.0, 1.0, 1.0),
                        low_feather: rng.moved(0.0, 0.5),
                        high_feather: rng.moved(0.0, 0.5),
                    },
                    None,
                ),
                // Hue 0 to 360, width 0 to 180, its fade 0 to 90,
                // chroma 0 to 0.2, its fade 0 to 0.1.
                _ => (
                    Shape::Color {
                        hue: rng.range(0.0, 360.0),
                        width: rng.moved(0.0, 180.0),
                        hue_feather: rng.moved(0.0, 90.0),
                        chroma: rng.moved(0.0, 0.2),
                        chroma_feather: rng.moved(0.0, 0.1),
                    },
                    None,
                ),
            };
            components.push(Component {
                shape,
                mode: Mode::ALL[rng.below(Mode::ALL.len())],
                invert: rng.chance(0.2),
                enabled: !rng.chance(0.1),
            });
            rasters.push(raster);
        }
        let mask = Mask {
            components,
            invert: rng.chance(0.2),
        };
        (mask, rasters)
    }

    /// Three look tables with a cross term, one channel's output
    /// moved by the other two's product, so a lookup that read the
    /// wrong corner of a cell would show: an sRGB one at 9 nodes, a
    /// linear Rec.2020 one at 5 with a domain past one, and a gamma
    /// 2.2 Display P3 one at 17.
    fn look_tables() -> Vec<Arc<lut::Lut3d>> {
        let table = |size: usize,
                     encoding: lut::Encoding,
                     primaries: lut::Primaries,
                     max: f32,
                     f: &dyn Fn([f32; 3]) -> [f32; 3]| {
            let mut t = lut::Lut3d::identity(size);
            t.encoding = encoding;
            t.primaries = primaries;
            t.domain_max = [max; 3];
            let last = (size - 1) as f32;
            let mut i = 0;
            for b in 0..size {
                for g in 0..size {
                    for r in 0..size {
                        let c = [r, g, b].map(|v| v as f32 / last * max);
                        t.data[i] = f(c);
                        i += 1;
                    }
                }
            }
            for px in &mut t.data {
                *px = px.map(|v| half::f16::from_f32(v).to_f32());
            }
            Arc::new(t)
        };
        vec![
            table(
                9,
                lut::Encoding::Srgb,
                lut::Primaries::Srgb,
                1.0,
                &|[r, g, b]| {
                    [
                        0.9 * r + 0.1 * g * b,
                        g * g * 0.8 + 0.2 * r,
                        (b + 0.3 * r * g).min(1.2),
                    ]
                },
            ),
            table(
                5,
                lut::Encoding::Linear,
                lut::Primaries::Rec2020,
                1.25,
                &|[r, g, b]| [r * 0.8 + 0.2 * g * b, g, b * 0.9 + 0.1 * r],
            ),
            table(
                17,
                lut::Encoding::Gamma(2.2),
                lut::Primaries::DisplayP3,
                1.0,
                &|[r, g, b]| {
                    let m = (r + g + b) / 3.0;
                    [
                        m + 1.2 * (r - m) + 0.05 * g * b,
                        m + 0.9 * (g - m),
                        b.sqrt(),
                    ]
                },
            ),
        ]
    }

    /// Edit `k` of the run seeded `seed`, with the parts in `skip` at
    /// their defaults.
    fn random_case(
        seed: u64,
        k: u64,
        skip: u32,
        aspect: f32,
        pool: &(Vec<(Shape, RasterRef)>, RasterRef),
        tables: &[Arc<lut::Lut3d>],
    ) -> Case {
        use greycard_edit::grain::Kind;
        let has = |name: &str| skip & part(name) == 0;
        let mut rng = Rng::new(seed, k);
        let mut edit = greycard_edit::Edit::default();
        edit.set_look(random_look(&mut rng, 0.5, skip));
        // The black and white: eight weights ±1 and a strength 0 to 3.
        let mut bw = BlackWhite {
            enabled: rng.chance(0.3),
            weights: [0.0; 8],
            strength: rng.moved(0.0, greycard_edit::bw::STRENGTH_MAX),
        };
        for b in 0..8 {
            bw.weights[b] = rng.slider(0.5, -1.0, 1.0, 0.0);
        }
        if has("black and white") {
            edit.bw = bw;
        }
        // The vignette: amount ±5 stops, midpoint and feather 0 to 1,
        // roundness ±1.
        let vignette = Vignette {
            enabled: !rng.chance(0.1),
            amount: rng.slider(0.5, -5.0, 5.0, 0.0),
            midpoint: rng.moved(0.0, 1.0),
            feather: rng.moved(0.0, 1.0),
            roundness: rng.moved(-1.0, 1.0),
        };
        if has("vignette") {
            edit.vignette = vignette;
        }
        // The grain: amount 0 to 1, size 0.1 to 2, either kind.
        let grain = Grain {
            enabled: !rng.chance(0.1),
            amount: rng.slider(0.4, 0.0, 1.0, 0.0),
            size: rng.moved(0.1, 2.0),
            kind: Kind::ALL[rng.below(Kind::ALL.len())],
        };
        if has("grain") {
            edit.grain = grain;
        }
        // Two or three local adjustments, each its own look.
        let n = 2 + rng.below(2);
        let mut rasters = Vec::new();
        for id in 0..n {
            let (mask, r) = random_mask(&mut rng, aspect, &pool.0, &pool.1);
            let look = random_look(&mut rng, 0.35, skip);
            let enabled = !rng.chance(0.1);
            if has("locals") {
                edit.adjustments.push(greycard_edit::Adjustment {
                    id: id as u64,
                    enabled,
                    mask,
                    look,
                    ..Default::default()
                });
                rasters.push(r);
            }
        }
        // A look table, at any strength.
        let which = rng.below(tables.len() + 1);
        let strength = rng.moved(0.0, 1.0);
        let look = (has("look table") && which < tables.len())
            .then(|| lut::Look::new(tables[which].clone(), strength).expect("a look"));
        let display = rng.chance(0.25);
        let guide = rng.chance(0.6);
        let coarse = rng.chance(0.5);
        let space = crate::export::Space::ALL[rng.below(crate::export::Space::ALL.len())];
        // AgX half the time: drawn last, so the draws before it are
        // the ones every seed had before.
        if rng.chance(0.5) && has("display curve") {
            edit.display_curve = greycard_edit::DisplayCurve::Agx;
        }
        Case {
            edit,
            rasters,
            look,
            source: if display && has("display source") {
                Source::Display
            } else {
                Source::Scene
            },
            guide: (guide && has("guide plane")).then_some(usize::from(coarse)),
            space: if has("output space") {
                space
            } else {
                crate::export::Space::Srgb
            },
        }
    }

    /// A small frame with the corners of what a picture holds: rows of
    /// near black, an Oklab field from grey to chroma 0.3 at every
    /// hue, the working space's primaries alone at a range of levels
    /// (the other two channels exactly zero), and highlights past
    /// white, some with one channel clipped high; a little noise over
    /// all of it so the local mean has something to do.
    fn parity_frame() -> WorkingImage {
        let (w, h) = (48usize, 32usize);
        let ok = greycard_core::color::Oklab::for_working_space();
        let from_lms = greycard_core::color::invert3(ok.to_lms).unwrap();
        let hash = |x: usize, y: usize, c: usize| {
            let mut z = (x as u64) << 32 | (y as u64) << 8 | c as u64;
            z = (z ^ (z >> 33)).wrapping_mul(0xFF51_AFD7_ED55_8CCD);
            z = (z ^ (z >> 33)).wrapping_mul(0xC4CE_B9FE_1A85_EC53);
            ((z ^ (z >> 33)) >> 40) as f32 / (1u64 << 24) as f32
        };
        let lab_to = |lab: [f32; 3]| {
            let lms = greycard_core::color::apply3(&greycard_core::color::LAB_TO_LMS, lab)
                .map(|v| v * v * v);
            greycard_core::color::apply3(&from_lms, lms).map(|v| v.max(0.0))
        };
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let n = |c| 1.0 + 0.06 * (hash(x, y, c) - 0.5);
                let px = match y {
                    0..=3 => {
                        let level = [0.0, 2e-5, 3e-4, 4e-3][y];
                        [0, 1, 2].map(|c| level * hash(x, y, c) * 2.0)
                    }
                    4..=21 => {
                        let l = 0.12 + 0.85 * (y - 4) as f32 / 17.0;
                        let hue = (x as f32 * 7.5).to_radians();
                        let chroma = 0.3 * ((x % 4) as f32 / 3.0);
                        let c = lab_to([l, chroma * hue.cos(), chroma * hue.sin()]);
                        [0, 1, 2].map(|k| c[k] * n(k))
                    }
                    22..=25 => {
                        let level = 0.01 * 1.3f32.powi((x / 3) as i32 + 4 * (y - 22) as i32 % 16);
                        let mut c = [0.0; 3];
                        c[x % 3] = level;
                        c
                    }
                    _ => {
                        let over = 1.0 + 7.0 * x as f32 / w as f32;
                        let hue = (x as f32 * 31.0).to_radians();
                        let base = lab_to([0.8, 0.08 * hue.cos(), 0.08 * hue.sin()]);
                        let mut c = base.map(|v| v * over);
                        if y % 2 == 0 {
                            c[x % 3] = 6.0;
                        }
                        [0, 1, 2].map(|k| c[k] * n(k))
                    }
                };
                data.extend(px);
            }
        }
        WorkingImage {
            width: w,
            height: h,
            data,
        }
    }

    /// What one edit came to: how far the GPU was at worst from the
    /// nearest of the CPU's answers, where, the CPU's answer and the
    /// GPU's there, how many pixels were steep (their answers spread
    /// wider than `STEEP`), and how many channels' answers spread wider
    /// than `WIDE`.
    struct Outcome {
        worst: f32,
        at: (usize, usize, usize),
        cpu: [f32; 3],
        gpu: [f32; 3],
        steep: usize,
        /// The steep pixels among the lit ones (`lit`), which
        /// `STEEP_SHARE` holds.
        steep_lit: usize,
        wide: usize,
        /// Channels whose answers the finest nudges alone spread
        /// wider than the tolerance (`FINEST`).
        unsettled: usize,
        /// Channels checked again against the finer nudges (`REFINED`).
        refined: usize,
        /// The CPU's answers in the worst channel, as it is first.
        answers: Vec<f32>,
    }

    /// How far each channel of the picture is moved, up and then down,
    /// as a share of the pixel's largest, for the CPU's answers: the
    /// picture as it is and so moved, at each of `NUDGES` of this. The
    /// GPU passes where it lies within the tolerance of one of them,
    /// not merely between the least and the most, so a spread over the
    /// whole scale cannot pass a black. Where the CPU's answer is flat,
    /// they are one answer; where an error of the size the GPU's
    /// arithmetic is allowed moves it far, they spread with it, and they
    /// are graded rather than one step each way because such a pixel's
    /// answer is a continuum the GPU can land anywhere on: at one step
    /// of 1e-5 each way, five edits over seeds 1 to 10 on Mesa fell
    /// between answers 50 to 250 levels apart, and none does graded.
    /// What makes pixels steep, all found by this test:
    ///
    /// - A large stacked gain into the white point, whose exponent runs
    ///   to tens of stops at the sliders' corners with the masks' added:
    ///   it carries a few ULP of the GPU's `log2` and `exp2` to 1e-5 of
    ///   the value and past, and a steep point curve, or the output
    ///   matrix's cancellation into a dark channel where the sRGB curve
    ///   is steepest, makes that whole levels.
    /// - A contrast under one, a power with no bound on its slope at
    ///   zero, on a channel the Oklab pass leaves near zero.
    /// - A tint whose hue is opposite a pixel's own. `Tint::applied`
    ///   turns the short way round, and at 180 degrees both ways are as
    ///   short, so rounding picks the side and the two hues land
    ///   `(1 - share) * 360` degrees apart.
    ///
    /// The brush rasters and the guide plane are read by texel and
    /// interpolated in f32 on both sides (`raster_at`, `guide_at` in
    /// the shader), so neither needs a nudge: through the sampler,
    /// lavapipe's filtered read of a raster came back a third of a
    /// level off, and 45 of the default seed's 400 edits fell outside
    /// the bracket.
    ///
    /// A share of the pixel's largest channel, the nudge does not move
    /// black; and a window's weight at no light can be 6e-8 on one side
    /// and nothing on the other, which the point and color curves near
    /// black make whole levels (default seed, edits 1324 and 2565, 35
    /// and 12 levels, outside the bracket at 5 000 edits; not at 400).
    const NUDGE: f32 = 1e-5;
    const NUDGES: [f32; 4] = [1.0, 0.3, 0.1, 0.03];
    /// The finest nudges' answers, the last of the run's: the smallest
    /// step, both signs, each channel. That step, 3e-7 of the largest
    /// channel, is two to five of its roundings; where it alone spreads
    /// a channel's answers wider than the tolerance, the CPU's answer
    /// there is not settled at f32's precision, and the GPU, whose
    /// arithmetic is allowed about as much, is held to the range of
    /// all the answers rather than to the nearest one. The answers
    /// there are samples of a continuum steep enough that the GPU lands
    /// between them: seed 55 edit 121, a channel the Oklab pass leaves
    /// at a two-thousandth of the largest, taken by a contrast under one
    /// and a point curve rising at zero, answers 57 to 242 levels and
    /// lavapipe at 134, 25 levels from the nearest; and three more past
    /// seed 10, by 2.7 to 4.5 levels, on one driver or two. A black
    /// still fails inside a range that does not reach it, and a pixel
    /// whose finest answers agree is held to the nearest as before.
    const FINEST: usize = 2 * 3;
    /// The most channels of an edit that may be unsettled, and the most
    /// an edit may have on average over the run: the range they are
    /// held to is a looser check than the nearest answer, nearly all of
    /// them are in the frame's empty channels, which the steep cap
    /// does not count, and a change that made those answers chaotic in
    /// a few edits in a hundred would otherwise pass. Over seeds 1 to
    /// 150 the most in one edit is 630 of the frame's 4608 channels
    /// (seed 133, edit 81, every steep pixel of it empty), then 425
    /// (seed 74, edit 395); the most in a run is 631, 1.6 an edit. The
    /// caps are a half and three times over those.
    const UNSETTLED_MAX: usize = 1000;
    const UNSETTLED_MEAN: f64 = 5.0;
    /// How many steps the finer nudges take from `NUDGE` down to the
    /// finest, evenly in their logarithm, each channel alone and all
    /// three together, both signs: run for an edit only where a channel
    /// of the GPU's lies between two of the answers and within the
    /// tolerance of none, which a continuous answer steep at the scale
    /// of the nudges can do (seed 129 edit 102 on lavapipe: a channel
    /// the Oklab pass takes from nothing to 0.055, which the blacks'
    /// crush at 0.054 leaves at 9e-4 and a steep point curve lifts,
    /// answers 186 to 255 levels and the GPU 7.9 from the nearest of
    /// them). A value off every answer of the finer sampling still
    /// fails, so an answer that jumps cannot pass between its sides.
    const REFINED: usize = 64;
    /// How far an encoded channel may be from the CPU's answers.
    const TOLERANCE: f32 = 2.5 / 255.0;
    /// The spread of a channel's answers past which it counts as wide,
    /// a few of which every run has: the run prints how many.
    const WIDE: f32 = 32.0 / 255.0;
    /// The bracket's width from which a pixel counts as steep: wider
    /// than the tolerance, so that the bracket loosens the check by
    /// more than the tolerance itself does. A narrower width, a level,
    /// put a few edits at the sliders' corners over `STEEP_SHARE`.
    const STEEP: f32 = 2.5 / 255.0;
    /// The most of an edit's lit pixels (`lit` in the test: every
    /// channel holds more than the nudge puts in) that may be steep:
    /// past it the bracket is checking too little of the picture for
    /// the edit to count, and it fails. The frame's other pixels, 461
    /// of its 1536 besides the black row (the rows of lone primaries
    /// and the Oklab field's colors clipped at a channel), are steep
    /// wherever the edit has a curve steep at zero, as a point at the
    /// panel's 0.01 rising makes it: counted with them, the steepest
    /// edit of seeds 1 to 10 was 11.1% and the cap of 15% set from it
    /// failed five edits in 40 000 past seed 10 (seed 74 edit 395 at
    /// 21.4%, and every steep pixel of the five with an empty channel,
    /// none lit; the same on every device, since it is the CPU's own
    /// spread). The lit share's steepest over seeds 1 to 150 is 0.78%
    /// (seed 63, edit 133); a break that made answers jump shows in
    /// many times that. `STEEP_MEAN` holds the run as a whole.
    const STEEP_SHARE: f64 = 0.03;
    /// The most of the pixels that may be steep over the whole run, so
    /// that a change that made every answer jump cannot pass by
    /// widening every bracket a little.
    const STEEP_MEAN: f64 = 0.01;

    /// A mask over everything (a luminance window from 0 to 1) with
    /// `look` under it.
    fn everywhere(look: Look) -> greycard_edit::Adjustment {
        greycard_edit::Adjustment {
            mask: Mask {
                components: vec![Component {
                    shape: Shape::Luminance {
                        low: 0.0,
                        high: 1.0,
                        low_feather: 0.0,
                        high_feather: 0.0,
                    },
                    ..Default::default()
                }],
                invert: false,
            },
            look,
            ..Default::default()
        }
    }

    /// `edit` over `image` through both sides, sRGB, eight bits: the
    /// CPU's `finish_with` as the export calls it and the shader's
    /// picture read back.
    fn both_sides(
        device: &gpu::Device,
        queue: &gpu::Queue,
        image: &WorkingImage,
        edit: &greycard_edit::Edit,
    ) -> (Vec<u8>, Vec<u8>) {
        let (w, h) = (image.width, image.height);
        let locals: Vec<Local> = edit
            .adjustments
            .iter()
            .map(|a| Local::of(a, vec![None; a.mask.components.len()]))
            .collect();
        let mut renderer = Renderer::new(device, queue);
        renderer.upload(&crate::worker::Halves::from_image(image, None));
        let view = View {
            center: (w as f32 / 2.0, h as f32 / 2.0),
            plane: (w as f32, h as f32),
            frame_size: (w as f32, h as f32),
            locals: locals.clone(),
            ..View::with_look(edit)
        };
        let target = renderer.render(w as u32, h as u32, &view).0;
        let shown = renderer.read_back(&target).expect("read back");
        let gpu = shown.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let (iw, ih) = (w as f32, h as f32);
        let vignette = edit.vignette;
        let cpu = crate::finish::finish_with(
            image,
            None,
            &Baked::global(edit, Source::Scene),
            &locals,
            |x, y| ((x as f32 + 0.5) / iw, (y as f32 + 0.5) / iw),
            |x, y| {
                let (u, v) = ((x as f32 + 0.5) / iw, (y as f32 + 0.5) / ih);
                let stops = if vignette.is_off() {
                    0.0
                } else {
                    vignette.amount * vignette.at(u, v, iw / ih)
                };
                (stops, None)
            },
            None,
            None,
            &crate::export::Space::Srgb.matrix(),
            |v| (v * 255.0).round() as u8,
        );
        (cpu, gpu)
    }

    /// The stacked corner the random draws found the shader's display
    /// curve going black at: global exposure +5, contrast 2 and whites
    /// +2, the vignette's +5 at the corners, and a mask over everything
    /// at +5 more, on a pixel of 6. The scene comes to around 1e31, past
    /// where the fit's squares overflow; both sides make it white.
    #[test]
    fn the_stacked_corner_is_white_on_both_sides() {
        let Some((device, queue)) = device("the stacked corner's check") else {
            return;
        };
        let (w, h) = (8usize, 6usize);
        let image = WorkingImage {
            width: w,
            height: h,
            data: vec![6.0; w * h * 3],
        };
        let mut edit = greycard_edit::Edit::default();
        edit.light.exposure = 5.0;
        edit.light.tone.contrast = 2.0;
        edit.light.tone.whites = 2.0;
        edit.vignette.amount = 5.0;
        let mut look = Look::default();
        look.light.exposure = 5.0;
        edit.adjustments.push(everywhere(look));
        let (cpu, gpu) = both_sides(&device, &queue, &image, &edit);
        assert!(cpu.iter().all(|&v| v == 255), "the CPU: {cpu:?}");
        assert!(gpu.iter().all(|&v| v == 255), "the GPU: {gpu:?}");
    }

    /// Contrast summed to nothing and under: the picture's 0.5 and one
    /// or two masks over everything at 0.5 sum to 0 and -0.5, which
    /// both sides hold at `MIN_CONTRAST` (a power of zero took black to
    /// mid grey on the CPU, and is undefined in the shader). Black
    /// stays black, and the two sides agree over a ramp.
    #[test]
    fn stacked_flattenings_agree_on_both_sides() {
        let Some((device, queue)) = device("the stacked flattenings' check") else {
            return;
        };
        let (w, h) = (16usize, 2usize);
        let image = WorkingImage {
            width: w,
            height: h,
            data: (0..w * h)
                .flat_map(|i| {
                    let v = if i % w == 0 {
                        0.0
                    } else {
                        0.002 * 2f32.powf((i % w) as f32 * 0.6)
                    };
                    [v, v * 0.8, v * 1.1]
                })
                .collect(),
        };
        for n in [1, 2] {
            let mut edit = greycard_edit::Edit::default();
            edit.light.tone.contrast = 0.5;
            for _ in 0..n {
                let mut look = Look::default();
                look.light.tone.contrast = 0.5;
                edit.adjustments.push(everywhere(look));
            }
            let (cpu, gpu) = both_sides(&device, &queue, &image, &edit);
            assert_eq!(&cpu[..3], &[0, 0, 0], "{n}: the CPU's black");
            assert_eq!(&gpu[..3], &[0, 0, 0], "{n}: the GPU's black");
            let worst = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max();
            assert!(worst <= Some(2), "{n}: {cpu:?} against {gpu:?}");
        }
    }

    /// The viewport's shader against the export's finish over a few
    /// hundred random edits, over a small frame of saturated colors,
    /// near black and clipped highlights. Each edit draws every look
    /// slider the viewport previews over the range its panel gives it,
    /// a fifth of the time at each end (the Light section and its
    /// switch, the point, parametric and color curves, the grading,
    /// the mixer, the color, the black and white, the tint, the
    /// vignette and the grain), two or three local adjustments with
    /// their own looks under masks of gradients, radials, brushes, a
    /// learned raster (Subject) present or missing, and luminance and
    /// color windows, in every mode, a look table, a source of a scene
    /// or a picture already rendered, an output space and a guide
    /// plane.
    ///
    /// Both sides are built from the edit the way the app builds them:
    /// the locals by `Local::of`, as the viewport and the export bake
    /// them, the view's look by `View::with_look`, the guide handed to
    /// the CPU only where `export::reads_guide` says the export reads
    /// it, the vignette and the grain by frame position as
    /// `export::render` has them. The CPU is handed the picture, the
    /// guide and the look tables the GPU holds, rounded to half
    /// floats. Every encoded channel of every pixel is held to 2.5
    /// levels of the CPU's bracket (`NUDGE`).
    ///
    /// Not covered: the white balance the viewport previews
    /// (`View::white`), the geometry (zoom, turns, crop, perspective,
    /// the cubic reader), the export's mask sample from before its
    /// output sharpen (`sampled`), the Background, Sky and Object
    /// shapes (rasters as Subject's is, not drawn), the mask and
    /// clipping overlays, the display table and the soft proof, and
    /// the encoded picture's path.
    ///
    /// `GREYCARD_PARITY_SEED` and `GREYCARD_PARITY_EDITS` change the
    /// run; `GREYCARD_PARITY_ONLY` replays one edit, and
    /// `GREYCARD_PARITY_SKIP` (names from `PARTS`, by commas) leaves
    /// parts at their defaults. The first three failing edits are drawn
    /// again with each part left at its default in turn, and the parts
    /// whose removal brings them within the tolerance are named; an
    /// amplifier's removal does that as well as a cause's.
    #[test]
    fn the_shader_is_the_cpu_over_random_edits() {
        let Some((device, queue)) = device("the random parity check") else {
            return;
        };
        let env = |name: &str| {
            std::env::var(name).ok().and_then(|v| {
                let v = v.trim();
                match v.strip_prefix("0x") {
                    Some(hex) => u64::from_str_radix(hex, 16).ok(),
                    None => v.parse().ok(),
                }
            })
        };
        let seed = env("GREYCARD_PARITY_SEED").unwrap_or(0x6772_6579);
        let edits = env("GREYCARD_PARITY_EDITS").unwrap_or(400);
        let only = env("GREYCARD_PARITY_ONLY");
        let skip = std::env::var("GREYCARD_PARITY_SKIP")
            .map(|names| {
                names
                    .split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .fold(0, |s, n| {
                        assert!(
                            PARTS.contains(&n),
                            "GREYCARD_PARITY_SKIP: no part {n:?}; the parts are {PARTS:?}"
                        );
                        s | part(n)
                    })
            })
            .unwrap_or(0);
        let started = std::time::Instant::now();
        let image = parity_frame();
        let (w, h) = (image.width, image.height);
        let aspect = h as f32 / w as f32;
        let seen = WorkingImage {
            width: w,
            height: h,
            data: image
                .data
                .iter()
                .map(|v| half::f16::from_f32(*v).to_f32())
                .collect(),
        };
        // Two guide planes: the picture's own, which at this size is a
        // texel a pixel, so `guide_at` never falls between texels; and
        // its mean over blocks of four, as a frame larger than
        // `GUIDE_EDGE` has it, read between texels everywhere.
        // Half floats on the GPU (`set_guide`), so the CPU is handed
        // them rounded the same way, as it is the picture and the look
        // tables.
        let native = crate::finish::guide_plane(&seen);
        let coarse = {
            let s = 4;
            let (gw, gh) = (w / s, h / s);
            let data = (0..gh)
                .flat_map(|y| (0..gw).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let block =
                        (0..s * s).map(|k| native.data[(y * s + k / s) * w + x * s + k % s]);
                    block.sum::<f32>() / (s * s) as f32
                })
                .collect();
            crate::finish::Guide {
                width: gw,
                height: gh,
                scale: s,
                data,
            }
        };
        let guides = [native, coarse].map(|mut g| {
            for v in &mut g.data {
                *v = half::f16::from_f32(*v).to_f32();
            }
            g
        });
        let pool = raster_pool(seed, aspect);
        let tables = look_tables();
        let mut renderer = Renderer::new(&device, &queue);
        renderer.upload(&crate::worker::Halves::from_image(&image, None));
        // The picture with each channel in turn moved by `NUDGE` of the
        // pixel's largest, up and down.
        // `FINEST` is the last step's answers: the steps run from the
        // largest down, both signs of each, three channels of each sign.
        assert!(NUDGES.windows(2).all(|p| p[0] > p[1]));
        let nudged: Vec<WorkingImage> = NUDGES
            .into_iter()
            .flat_map(|step| [step, -step])
            .flat_map(|step| (0..3).map(move |c| (step, c)))
            .map(|(step, c)| {
                let mut n = seen.clone();
                for px in n.data.chunks_mut(3) {
                    let top = px.iter().fold(0.0f32, |a, &b| a.max(b));
                    px[c] += step * NUDGE * top;
                }
                n
            })
            .collect();
        assert_eq!(nudged.len(), NUDGES.len() * FINEST);
        // The finer nudges, for a channel the GPU puts between two of
        // the answers and near none (`REFINED`).
        let fine: Vec<WorkingImage> = (0..REFINED)
            .map(|k| 0.03f32.powf(k as f32 / (REFINED - 1) as f32))
            .flat_map(|step| [step, -step])
            .flat_map(|step| (0..4).map(move |c| (step, c)))
            .map(|(step, c)| {
                let mut n = seen.clone();
                for px in n.data.chunks_mut(3) {
                    let top = px.iter().fold(0.0f32, |a, &b| a.max(b));
                    // Each channel alone, and all three together.
                    for (k, v) in px.iter_mut().enumerate() {
                        if c == 3 || c == k {
                            *v += step * NUDGE * top;
                        }
                    }
                }
                n
            })
            .collect();
        // The lit pixels: every channel holds more than the largest
        // nudge puts into it. In the others a channel is empty, or as
        // good as, and the nudge is not a small change of it but light
        // where there was none, which a curve steep at zero (a point at
        // the panel's 0.01, the encoding's 12.92 under it) makes whole
        // levels: an edit with such a curve makes every one of them
        // steep, the same on any device.
        let lit: Vec<bool> = seen
            .data
            .chunks(3)
            .map(|px| {
                let top = px.iter().fold(0.0f32, |a, &b| a.max(b));
                top > 0.0 && px.iter().all(|&v| v > NUDGE * top)
            })
            .collect();
        // One case through both sides.
        let mut run = |case: &Case| -> Outcome {
            let edit = &case.edit;
            let locals: Vec<Local> = edit
                .adjustments
                .iter()
                .zip(&case.rasters)
                .take(MAX_LOCALS)
                .map(|(a, rasters)| Local::of(a, rasters.clone()))
                .collect();
            renderer.set_look(case.look.as_ref());
            renderer.set_output(case.space.matrix());
            let guide = case.guide.map(|k| &guides[k]);
            renderer.set_guide(guide.unwrap_or(&crate::finish::Guide::NONE));
            let view = View {
                center: (w as f32 / 2.0, h as f32 / 2.0),
                plane: (w as f32, h as f32),
                frame_size: (w as f32, h as f32),
                locals: locals.clone(),
                source: case.source,
                ..View::with_look(edit)
            };
            let target = renderer.render(w as u32, h as u32, &view).0;
            let shown = renderer.read_back(&target).expect("read back");
            // The export's side, as `export::render` sets it up: the
            // guide only where it says the export reads one.
            let reads = crate::export::reads_guide(edit);
            let global = Baked::global(edit, case.source);
            let (iw, ih) = (w as f32, h as f32);
            let vignette = edit.vignette;
            let grain = edit.grain;
            let frame_at = |x: usize, y: usize| {
                let (u, v) = ((x as f32 + 0.5) / iw, (y as f32 + 0.5) / ih);
                let stops = if vignette.is_off() {
                    0.0
                } else {
                    vignette.amount * vignette.at(u, v, iw / ih)
                };
                let noise = (!grain.is_off()).then(|| grain.at(u, v, iw / ih));
                (stops, noise)
            };
            let look = case.look.as_ref().filter(|l| !l.is_off());
            let finish = |image: &WorkingImage, global: &Baked| {
                crate::finish::finish_with(
                    image,
                    None,
                    global,
                    &locals,
                    |x, y| ((x as f32 + 0.5) / iw, (y as f32 + 0.5) / iw),
                    frame_at,
                    guide.filter(|_| reads).map(|g| (g, iw)),
                    look,
                    &case.space.matrix(),
                    |v: f32| v,
                )
            };
            let cpu = finish(&seen, &global);
            let around: Vec<Vec<f32>> = nudged.iter().map(|n| finish(n, &global)).collect();
            let mut refined: Option<Vec<Vec<f32>>> = None;
            let mut out = Outcome {
                worst: 0.0,
                at: (0, 0, 0),
                cpu: [0.0; 3],
                gpu: [0.0; 3],
                steep: 0,
                steep_lit: 0,
                wide: 0,
                unsettled: 0,
                refined: 0,
                answers: Vec::new(),
            };
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) * 3;
                    let gpu = shown.get_pixel(x as u32, y as u32);
                    let gpu = [0, 1, 2].map(|c| f32::from(gpu[c]) / 255.0);
                    let here = [cpu[i], cpu[i + 1], cpu[i + 2]];
                    let mut steep = false;
                    for c in 0..3 {
                        // The CPU's answers: the picture as it is and
                        // nudged. The GPU is held to the nearest of
                        // them, not to anything between the least and
                        // the most, so a bracket spread over the whole
                        // scale cannot pass a black. A NaN in the CPU's
                        // own answer fails.
                        let answers =
                            || std::iter::once(here[c]).chain(around.iter().map(|a| a[i + c]));
                        let (lo, hi) = answers()
                            .fold((here[c], here[c]), |(lo, hi), v| (lo.min(v), hi.max(v)));
                        steep |= hi - lo >= STEEP;
                        out.wide += usize::from(hi - lo > WIDE);
                        // Where the finest nudges alone, a few roundings
                        // of the largest channel, spread the answers
                        // wider than the tolerance, the CPU's own answer
                        // is not settled at f32's precision, and the GPU
                        // is held to their range instead (`FINEST`).
                        let (flo, fhi) = std::iter::once(here[c])
                            .chain(around[around.len() - FINEST..].iter().map(|a| a[i + c]))
                            .fold((here[c], here[c]), |(lo, hi), v| (lo.min(v), hi.max(v)));
                        let unsettled = fhi - flo > TOLERANCE;
                        out.unsettled += usize::from(unsettled);
                        let mut d = if here[c].is_nan() {
                            f32::NAN
                        } else if unsettled {
                            (lo - gpu[c]).max(gpu[c] - hi).max(0.0)
                        } else {
                            answers()
                                .map(|v| (v - gpu[c]).abs())
                                .fold(f32::INFINITY, f32::min)
                        };
                        // Between two answers and near none: the finer
                        // sampling of the same neighborhood decides
                        // (`REFINED`).
                        if d > TOLERANCE && gpu[c] > lo && gpu[c] < hi {
                            let refined = refined.get_or_insert_with(|| {
                                fine.iter().map(|n| finish(n, &global)).collect()
                            });
                            d = refined
                                .iter()
                                .map(|a| (a[i + c] - gpu[c]).abs())
                                .fold(d, f32::min);
                            out.refined += 1;
                        }
                        if d > out.worst || (d.is_nan() && !out.worst.is_nan()) {
                            out.worst = d;
                            out.at = (x, y, c);
                            out.cpu = here;
                            out.gpu = gpu;
                            out.answers = answers().collect();
                        }
                    }
                    out.steep += usize::from(steep);
                    out.steep_lit += usize::from(steep && lit[y * w + x]);
                }
            }
            out
        };
        let tolerance = TOLERANCE;
        let pixels = (w * h) as f64;
        let lit_pixels = lit.iter().filter(|l| **l).count() as f64;
        let mut failed = Vec::new();
        let mut overall = 0.0f32;
        let mut steep_sum = 0.0f64;
        let mut wide = 0usize;
        let mut steepest = (0.0f64, 0u64);
        let mut steepest_lit = (0.0f64, 0u64);
        let mut unsettled = 0usize;
        let mut most_unsettled = (0usize, 0u64);
        let mut refined = 0usize;
        let range = match only {
            Some(k) => k..k + 1,
            None => 0..edits,
        };
        for k in range {
            let case = random_case(seed, k, skip, aspect, &pool, &tables);
            let Outcome {
                worst: d,
                at: (x, y, c),
                cpu,
                gpu,
                steep,
                steep_lit,
                wide: n,
                unsettled: u,
                refined: r,
                answers,
            } = run(&case);
            wide += n;
            unsettled += u;
            if u > most_unsettled.0 {
                most_unsettled = (u, k);
            }
            refined += r;
            let share = steep as f64 / pixels;
            let share_lit = steep_lit as f64 / lit_pixels;
            steep_sum += share;
            if share > steepest.0 {
                steepest = (share, k);
            }
            if share_lit > steepest_lit.0 {
                steepest_lit = (share_lit, k);
            }
            if !d.is_nan() {
                overall = overall.max(d);
            }
            let too_steep = share_lit > STEEP_SHARE || u > UNSETTLED_MAX;
            if d <= tolerance && !too_steep {
                continue;
            }
            let i = (y * w + x) * 3;
            eprintln!(
                "seed {seed:#x} edit {k}: {:.1} levels from the CPU's answers at {x},{y} \
                 channel {c} (cpu {cpu:.4?}, gpu {gpu:.4?}); the pixel {:?}; \
                 {:.1}% of pixels steep, {:.1}% of the lit ones; {u} channels unsettled",
                d * 255.0,
                &seen.data[i..i + 3],
                100.0 * share,
                100.0 * share_lit
            );
            eprintln!(
                "  the CPU's answers there, in levels: {:?}",
                answers
                    .iter()
                    .map(|v| (v * 255.0 * 10.0).round() / 10.0)
                    .collect::<Vec<_>>()
            );
            // Which part it is in, for the first few: drawn again
            // without each.
            let mut parts = Vec::new();
            if failed.len() < 3 {
                for (b, name) in PARTS.iter().enumerate() {
                    let without = random_case(seed, k, skip | 1 << b, aspect, &pool, &tables);
                    let o = run(&without);
                    if o.worst <= tolerance
                        && o.steep_lit as f64 / lit_pixels <= STEEP_SHARE
                        && o.unsettled <= UNSETTLED_MAX
                    {
                        parts.push(*name);
                    }
                }
                eprintln!("  within tolerance without: {parts:?}");
            }
            if only.is_some() {
                eprintln!("  {:#?}", case.edit);
                eprintln!(
                    "  look {:?}, source {:?}, guide {:?}, space {:?}",
                    case.look.as_ref().map(|l| (l.lut.size, l.strength)),
                    case.source,
                    case.guide,
                    case.space
                );
            }
            failed.push((k, d, share_lit, parts));
        }
        let count = if only.is_some() { 1 } else { edits };
        let mean = steep_sum / count as f64;
        eprintln!(
            "{count} edits, GPU against CPU: at most {:.2} levels from the CPU's answers, \
             {} failing; steep pixels {:.3}% on average, most {:.2}% (edit {}), \
             of the lit ones most {:.2}% (edit {}); {wide} channels' answers over {:.0} \
             levels apart, {unsettled} unsettled, most {} (edit {}), {refined} refined; \
             in {:.1} s",
            overall * 255.0,
            failed.len(),
            100.0 * mean,
            100.0 * steepest.0,
            steepest.1,
            100.0 * steepest_lit.0,
            steepest_lit.1,
            WIDE * 255.0,
            most_unsettled.0,
            most_unsettled.1,
            started.elapsed().as_secs_f64()
        );
        // A single edit replayed has no run to hold to the mean.
        assert!(
            only.is_some() || mean <= STEEP_MEAN,
            "seed {seed:#x}: {:.2}% of pixels steep over the run",
            100.0 * mean
        );
        let unsettled_mean = unsettled as f64 / count as f64;
        assert!(
            only.is_some() || unsettled_mean <= UNSETTLED_MEAN,
            "seed {seed:#x}: {unsettled_mean:.1} channels an edit unsettled over the run"
        );
        assert!(
            failed.is_empty(),
            "seed {seed:#x}: {} edits failing (over {:.1} levels, over {:.0}% of the lit pixels \
             steep, or over {UNSETTLED_MAX} channels unsettled): {:?}",
            failed.len(),
            tolerance * 255.0,
            STEEP_SHARE * 100.0,
            failed
                .iter()
                .map(|(k, d, s, p)| format!(
                    "{k} ({:.1} levels, {:.1}% of the lit steep: {p:?})",
                    d * 255.0,
                    s * 100.0
                ))
                .collect::<Vec<_>>()
        );
    }
}
