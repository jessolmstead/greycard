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
use greycard_edit::curve::LUT_SIZE;
use greycard_edit::mask::Shape;
use greycard_edit::{BlackWhite, Color, CurveLut, Grain, Light, Mixer, Tint, Vignette};

mod draw;
mod inputs;
mod locals;
mod sample;
mod scopes;
#[cfg(test)]
mod tests;

use self::{
    locals::{Brushes, LocalsGpu, locals_gpu, range_reads},
    scopes::EditKey,
};

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

/// Eight band values as two vec4s.
fn halves(v: &[f32; 8]) -> [[f32; 4]; 2] {
    [[v[0], v[1], v[2], v[3]], [v[4], v[5], v[6], v[7]]]
}

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
            source: Source::SCENE,
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
    /// The wide rasters, a Sky's past the raster rule, in an array of
    /// their own.
    wide: Brushes,
    /// The tone equalizer's guide plane, R16Float: a stop is a stop, so
    /// half floats hold it to a thousandth of one, and at a texel to
    /// every few dozen source pixels the whole plane is a few megabytes.
    guide: gpu::Texture,
    /// Its `Params::guide`: the source pixels the texture covers, and
    /// whether there is a plane to read.
    guide_cover: [f32; 4],
    /// The masks' detail planes, Rgba16Float at the source's size:
    /// texture, clarity and haze (`crate::detail::DetailPlanes`). A
    /// texel of nothing when there are none.
    detail: gpu::Texture,
    /// The planes it holds, to know them again (`set_detail`).
    detail_of: Option<std::sync::Weak<crate::detail::DetailPlanes>>,
    /// Its `Params::detail` and `Params::detail_clip`.
    detail_meta: [f32; 4],
    detail_clip: [f32; 4],
    /// Its `Params::frame_range`.
    frame_range: [f32; 4],
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
    /// The last picture read, kept: what the readout under the
    /// pointer reads, so it lands on the scopes' own traces.
    pixels: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    /// A new source came in since the picture in flight was taken:
    /// that picture is the last image's, perhaps at another shape,
    /// and is not kept for the readout.
    source_changed: bool,
    /// The edit, scope and weighing mask the bins in flight or latest
    /// were taken under; a new analysis only when one of those, or the
    /// image, changes.
    analyzed: Option<(EditKey, Scope, Option<usize>)>,
    image_changed: bool,
}

impl Renderer {
    pub fn new(device: &gpu::Device, queue: &gpu::Queue) -> Self {
        let shader = device.create_shader_module(gpu::ShaderModuleDescriptor {
            label: Some("viewport"),
            source: gpu::ShaderSource::Wgsl(include_str!("../viewport.wgsl").into()),
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
                // The masks' detail planes.
                gpu::BindGroupLayoutEntry {
                    binding: 12,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Texture {
                        sample_type: gpu::TextureSampleType::Float { filterable: true },
                        view_dimension: gpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The wide rasters, a layer each.
                gpu::BindGroupLayoutEntry {
                    binding: 13,
                    visibility: gpu::ShaderStages::FRAGMENT,
                    ty: gpu::BindingType::Texture {
                        sample_type: gpu::TextureSampleType::Float { filterable: true },
                        view_dimension: gpu::TextureViewDimension::D2Array,
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
        let detail = Self::detail_texture(device, 1, 1);
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
            brushes: Brushes::empty(device, "brushes"),
            wide: Brushes::empty(device, "wide rasters"),
            guide,
            guide_cover: [1.0, 1.0, 0.0, 0.0],
            detail,
            detail_of: None,
            detail_meta: [1.0, 1.0, 1.0, 0.0],
            detail_clip: [-1.0, 0.0, 0.0, 0.0],
            frame_range: Self::TYPICAL_RANGE,
            look,
            output,
            oklab: crate::finish::Oklab::for_working_space(),
        }
    }
}

/// The display texture's format: plain bytes, encoded by the shader,
/// because Slint's renderer shows every texture's bytes as they are.
const TARGET_FORMAT: gpu::TextureFormat = gpu::TextureFormat::Rgba8Unorm;
