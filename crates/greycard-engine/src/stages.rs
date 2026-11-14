//! The list of stages a develop walks, derived once from the edit and
//! what the develop is for.
//!
//! Every develop of an edit, the viewport's, an export's, the reduced
//! picture's, the models', and the prefetch's, is one list of steps
//! made here: [`needs`] before the decode says what must be had, and
//! [`stages`] after it says what runs, in order, with every option
//! resolved from the edit, so two lists compare by `==` and a walk can
//! start from the last point a list before it kept. A stage the
//! purpose leaves out of what the edit asks for is named in
//! [`Steps::skipped`] with its reason.

use std::sync::Arc;

use greycard_core::TempTint;
use greycard_core::color::Profile;
use greycard_core::color::WhitePoint;
use greycard_core::develop::ca::{self, CaFit, CaOptions};
use greycard_core::develop::defringe::DefringeOptions;
use greycard_core::develop::dehaze::DehazeOptions;
use greycard_core::develop::denoise::DenoiseOptions;
use greycard_core::develop::dual::DualContrast;
use greycard_core::develop::gain_map::MapPlacement;
use greycard_core::develop::lens::LensCorrection;
use greycard_core::develop::local_contrast::LocalContrastOptions;
use greycard_core::develop::reduced;
use greycard_core::develop::sharpen::SharpenOptions;
use greycard_core::develop::{DemosaicMethod, DevelopSettings, HighlightMode};
use greycard_core::opcodes::Warp;
use greycard_core::raw::{CfaPattern, Orientation, RawFrame, SensorLayout, Shot};
use greycard_edit::geometry::Geometry;
use greycard_edit::lens::FileCorrects;
use greycard_edit::retouch::Retouch;
use greycard_edit::{DetailWants, Edit};

use greycard_render::export;

/// What a develop is for: all that varies between the callers.
#[derive(Debug, Clone, PartialEq)]
pub enum Purpose {
    /// The open frame on screen: on the GPU where one is lent, ending
    /// at the viewport's shader.
    Viewport,
    /// A file under the sheet's settings, ending at the file: every
    /// stage with a device version on the GPU where one is lent, as the
    /// viewport runs them, read back in full floats for the finish on
    /// the CPU.
    Export(Box<export::Settings>),
    /// The edit's pictures of a frame not open: binned, the masks from
    /// the cache, finished to a 2048 sRGB picture.
    Reduced,
    /// What the learned masks and the person test are made on: the
    /// reduced develop without the retouch, through the Detail
    /// section, as they see it today.
    ModelsSee,
    /// A neighbor's base for the shelf, made ahead of its open. Its
    /// list is the viewport's to the base, the learned denoiser kept
    /// in it so the shelf's base serves the open; the prefetch's
    /// refusal of a learned tier and of a picture's file, which it
    /// leaves to the open, stays with its caller.
    Prefetch,
    /// A frame of a merge, as the merge reads it: camera RGB in its
    /// sensor units, all on the CPU, nothing clipped after the gains,
    /// ending at the demosaic.
    MergeInput(MergeInput),
}

/// What a merge asks of one frame's walk.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeInput {
    /// The reference frame's white as camera-space gains, which every
    /// other frame takes; none for the reference, at its own as-shot
    /// white. The merge divides the gains back out, so only the
    /// demosaic and the reconstruction see them.
    pub white: Option<[f64; 3]>,
    /// Whether the highlights are reconstructed, on the darkest frame
    /// alone: where every frame clipped, its reconstruction is the
    /// merge's. Every other frame's clipped samples are dropped.
    pub reconstruct: bool,
    /// The learned denoiser in the demosaic's place, when the merge
    /// asks for it.
    pub denoiser: Option<&'static greycard_ai::Model>,
    /// Where the frame's CA fit comes from.
    pub ca: MergeCa,
    /// The lens's corrections run on the frame in camera space, after
    /// the demosaic: a panorama's vignetting, whose distortion the
    /// merge composes into its one warp. None for a bracket, whose
    /// file keeps its lens for the develop.
    pub lens: Vec<LensCorrection>,
}

/// Where a merge's frame takes its CA fit from. Frames that differ in
/// exposure and noise fit corrections that differ, and a merge of
/// them shows the difference: one fit, made on the reference, applied
/// to every frame corrects them alike.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeCa {
    /// The frame's own, as any develop fits it.
    Own,
    /// The frame's own, handed back on the walk's result for the
    /// others: the reference's.
    Make,
    /// Another frame's, applied.
    Take(CaFitRef),
    /// None: for a measurement of the others.
    Off,
}

/// A CA fit compared by pointer.
#[derive(Debug, Clone)]
pub struct CaFitRef(pub Arc<CaFit>);

impl PartialEq for CaFitRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Purpose {
    /// Whether the develop is the reduced one, binned and with its
    /// skips.
    pub fn is_reduced(&self) -> bool {
        matches!(self, Purpose::Reduced | Purpose::ModelsSee)
    }
}

/// What the decode says that the list depends on.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub input: InputKind,
    /// The body and the shot, for the lens's profile.
    pub identity: Identity,
    /// The frame's quarter turns, clockwise, on top of the camera's
    /// tag.
    pub turn: u8,
}

/// A raw or a picture already rendered, with what the list needs of
/// each.
#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    Raw {
        /// The mosaic's pattern; none for a linear raw.
        pattern: Option<CfaPattern>,
        /// The size the develop crops the sensor to, unturned.
        crop: (usize, usize),
        /// The camera's orientation tag.
        tag: Orientation,
        /// The warps a DNG corrects its lens with, and where the crop
        /// sits in their image; none for every other raw.
        warp: Option<Box<WarpOptions>>,
        /// A merge of ours: its noise is no one frame's, so the
        /// denoisers stand down on it.
        merged: bool,
        /// The stops a merge of ours stored its samples under its
        /// reference frame (`baseline::headroom`); 0 for any other raw.
        headroom: f32,
        /// A merge whose lens was corrected in the merge (a
        /// panorama): it is in no one frame's geometry, and the lens
        /// profile stands down on it.
        lens_corrected: bool,
        /// A monochrome sensor's: one channel developed as three equal
        /// ones, which no lateral aberration correction may part.
        monochrome: bool,
    },
    /// A picture is turned by its tag at decode, so its size is the
    /// size it stands at. Its white is its color space's, as a
    /// temperature and tint: what its white balance is an offset from.
    Picture {
        size: (usize, usize),
        white: TempTint,
    },
}

/// What a file says of its body and its lens.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub make: String,
    pub model: String,
    pub shot: Shot,
}

impl Facts {
    /// A merge's headroom in stops (`baseline::headroom`); 0 for any
    /// other input.
    pub fn headroom(&self) -> f32 {
        match self.input {
            InputKind::Raw { headroom, .. } => headroom,
            _ => 0.0,
        }
    }

    /// A raw's facts at `turn`.
    pub fn raw(frame: &RawFrame, turn: u8) -> Self {
        let crop = frame.effective_crop();
        Self {
            input: InputKind::Raw {
                pattern: match &frame.layout {
                    SensorLayout::Cfa(p) => Some(p.clone()),
                    _ => None,
                },
                crop: (crop.width, crop.height),
                tag: frame.orientation,
                warp: greycard_core::develop::warp::area_of(frame).map(|area| {
                    Box::new(WarpOptions {
                        warps: frame.dng.warps.clone(),
                        area,
                    })
                }),
                merged: frame.dng.merged.is_some(),
                headroom: greycard_edit::baseline::headroom(frame),
                lens_corrected: frame.dng.merged.as_ref().is_some_and(|m| m.lens_corrected),
                monochrome: frame.layout == SensorLayout::Monochrome,
            },
            identity: Identity {
                make: frame.make.clone(),
                model: frame.model.clone(),
                shot: frame.shot.clone(),
            },
            turn: turn % 4,
        }
    }

    /// A picture's facts at `turn`, its size as it was decoded and
    /// `white` its color space's.
    pub fn picture(
        size: (usize, usize),
        white: TempTint,
        make: &str,
        model: &str,
        shot: &Shot,
        turn: u8,
    ) -> Self {
        Self {
            input: InputKind::Picture { size, white },
            identity: Identity {
                make: make.to_string(),
                model: model.to_string(),
                shot: shot.clone(),
            },
            turn: turn % 4,
        }
    }

    pub fn is_raw(&self) -> bool {
        matches!(self.input, InputKind::Raw { .. })
    }

    /// What the file's own warps correct of its lens.
    pub fn file_corrects(&self) -> FileCorrects {
        match &self.input {
            InputKind::Raw { warp: Some(w), .. } => FileCorrects::of(&w.warps),
            _ => FileCorrects::default(),
        }
    }

    /// Whether the develop measures and corrects the mosaic's lateral
    /// aberration itself when asked to: a mosaic the automatic
    /// correction knows, and no warp of the file's that moves each
    /// color.
    pub fn measures_ca(&self) -> bool {
        matches!(&self.input, InputKind::Raw { pattern: Some(p), .. } if ca::corrects(p))
            && !self.file_corrects().chromatic_aberration
    }

    /// Whether the input is a monochrome sensor's raw.
    pub fn is_monochrome(&self) -> bool {
        matches!(
            self.input,
            InputKind::Raw {
                monochrome: true,
                ..
            }
        )
    }

    /// The orientation the develop shows a raw in: the camera's tag
    /// with the frame's turns on it.
    pub fn orientation(&self) -> Option<Orientation> {
        match &self.input {
            InputKind::Raw { tag, .. } => Some(tag.turned(i32::from(self.turn))),
            InputKind::Picture { .. } => None,
        }
    }

    /// The full develop's size as it stands, turned.
    pub fn full(&self) -> (usize, usize) {
        match &self.input {
            InputKind::Raw { crop, .. } => {
                if self.orientation().is_some_and(swaps) {
                    (crop.1, crop.0)
                } else {
                    *crop
                }
            }
            InputKind::Picture { size, .. } => {
                if self.turn % 2 == 1 {
                    (size.1, size.0)
                } else {
                    *size
                }
            }
        }
    }

    /// The bin of the reduced develop under `geometry`: the mosaic's
    /// cell (or two for a picture or a linear raw) while the framed
    /// picture keeps that many times the 2048 on its long edge, else
    /// one. As the reduced develop chose it, with X-Trans taken up to
    /// a whole cell.
    pub fn bin_under(&self, geometry: &Geometry) -> usize {
        let cell = match &self.input {
            InputKind::Raw {
                pattern: Some(p), ..
            } => reduced::cell_of(p),
            _ => reduced::BIN,
        };
        let full = self.full();
        let (w, h) = framed_size((full.0 as u32, full.1 as u32), geometry);
        let bin = reduced::bin_for_cell(cell, w as usize, h as usize);
        match &self.input {
            InputKind::Raw {
                pattern: Some(p), ..
            } if bin > 1 && p.is_xtrans() => {
                bin.div_ceil(reduced::XTRANS_BIN) * reduced::XTRANS_BIN
            }
            InputKind::Raw { pattern: None, .. } => 1,
            _ => bin,
        }
    }

    /// The size of the picture a bin of `bin` makes, as it stands.
    fn binned(&self, bin: usize) -> (usize, usize) {
        let (w, h) = self.full();
        if bin <= 1 {
            return (w, h);
        }
        match &self.input {
            // The mosaic's rows and columns past the last whole cell
            // are left out, before the turn.
            InputKind::Raw { crop, .. } => {
                let (cw, ch) = (crop.0 / bin, crop.1 / bin);
                if self.orientation().is_some_and(swaps) {
                    (ch, cw)
                } else {
                    (cw, ch)
                }
            }
            InputKind::Picture { .. } => ((w / bin).max(1), (h / bin).max(1)),
        }
    }
}

/// Whether an orientation stands a picture on its side.
fn swaps(o: Orientation) -> bool {
    matches!(
        o,
        Orientation::Transpose
            | Orientation::Rotate90
            | Orientation::Transverse
            | Orientation::Rotate270
    )
}

/// The engine's settings before the demosaic: the edit's white, its
/// camera profile, its demosaic, and the defaults it fixes (the
/// highlights, the hot pixels, the dual demosaic's contrast). The CA
/// is a stage of its own and the orientation is the move into the
/// working space's.
#[derive(Debug, Clone)]
pub struct PrepareOptions {
    pub settings: DevelopSettings,
    /// Whether the frame's noise is measured: the learned denoiser
    /// asks for it whatever the settings say.
    pub measure_noise: bool,
    /// Whether the samples are clipped at the ceiling after the gains:
    /// every develop but a merge's frames.
    pub clip: bool,
}

impl PartialEq for PrepareOptions {
    fn eq(&self, other: &Self) -> bool {
        // Every field named, so one added to the settings later cannot
        // be left out of the cache's key unseen.
        let DevelopSettings {
            white_point,
            profile,
            profile_look,
            look_baseline,
            demosaic,
            highlights,
            segments,
            chromatic_aberration,
            dual_contrast,
            denoise,
            hot_pixels,
            orientation,
        } = &self.settings;
        let b = &other.settings;
        // A camera profile is read once and kept, so the same file is
        // the same pointer; a hundred kilobytes is not compared.
        same_profile(profile, &b.profile)
            && *profile_look == b.profile_look
            && *look_baseline == b.look_baseline
            && *white_point == b.white_point
            && *demosaic == b.demosaic
            && *highlights == b.highlights
            && *segments == b.segments
            && *chromatic_aberration == b.chromatic_aberration
            && *dual_contrast == b.dual_contrast
            && *denoise == b.denoise
            && *hot_pixels == b.hot_pixels
            && *orientation == b.orientation
            && self.measure_noise == other.measure_noise
            && self.clip == other.clip
    }
}

fn same_profile(a: &Option<Arc<Profile>>, b: &Option<Arc<Profile>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// A DNG's rectilinear warps and where the develop's crop sits in the
/// image they are defined over, at the full develop's scale.
#[derive(Debug, Clone, PartialEq)]
pub struct WarpOptions {
    pub warps: Vec<Warp>,
    pub area: MapPlacement,
}

/// One transform, its options resolved from the edit.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// Mosaic to mosaic: the levels, the noise, the hot pixels, the
    /// gains, the highlights.
    Prepare(Box<PrepareOptions>),
    /// The lateral chromatic aberration, on the mosaic.
    Ca(CaOptions),
    /// The CA, its fit handed back on the walk's result for another
    /// frame to take.
    CaFit(CaOptions),
    /// The CA by a fit another frame made.
    CaApply(CaFitRef),
    /// Mosaic to camera RGB.
    Demosaic(DemosaicMethod, DualContrast),
    /// Mosaic to camera RGB at a bin's fraction, in place of the
    /// demosaic.
    Bin(usize),
    /// Mosaic to the network's camera RGB and the plain demosaic's
    /// beside it.
    Learned(&'static greycard_ai::Model),
    /// The profiled denoise, on camera RGB.
    Denoise(DenoiseOptions),
    /// A DNG's own lens correction, on camera RGB: a resample, the
    /// camera's geometry before any turn, crop, or lens profile.
    Warp(Box<WarpOptions>),
    /// Camera RGB into the working space: the matrix, the camera
    /// profile's map and look table, the orientation. Each of a pair.
    IntoWorking {
        profile: Option<ProfileRef>,
        /// Whether the profile's look table goes on after its map.
        profile_look: bool,
        /// The baseline it is read at
        /// ([`DevelopSettings::look_baseline`]).
        look_baseline: f32,
        orientation: Orientation,
    },
    /// The learned pair blended by strength: the luminance's, then
    /// the chroma's.
    Blend {
        luma: f32,
        chroma: f32,
    },
    /// A picture input's white balance: the adaptation from the light
    /// its own white is (`own`) to the one its offset names (`light`),
    /// a matrix on the linear picture, first, where a raw's white
    /// balance sits.
    White {
        own: TempTint,
        light: TempTint,
    },
    /// A picture input's turn.
    Orient(Orientation),
    /// A picture input reduced by a whole factor, in place of the bin.
    Reduce(usize),
    /// The lens's corrections, in order; working to working from here
    /// on, the same size.
    Lens(Vec<LensCorrection>),
    Defringe(DefringeOptions),
    /// The retouch, its fills made or refused.
    Retouch(Retouch, Fills),
    LocalContrast(LocalContrastOptions),
    Dehaze(DehazeOptions),
    /// The capture sharpen, and the delta beside it for the masks'
    /// Sharpen when one asks.
    Sharpen(SharpenOptions, Delta),
    /// Working to what the purpose ends at.
    Finish(Finish),
}

/// What the sharpen makes for the masks' Sharpen: nothing; its delta
/// beside the sharpened picture; or its delta alone, the picture left
/// unsharpened, when the global Sharpen is off and a mask asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delta {
    No,
    Also,
    Only,
}

impl Delta {
    /// Whether the picture is sharpened.
    pub fn sharpens(self) -> bool {
        self != Delta::Only
    }

    /// Whether the delta is made.
    pub fn made(self) -> bool {
        self != Delta::No
    }
}

/// A camera profile compared by pointer.
#[derive(Debug, Clone)]
pub struct ProfileRef(pub Arc<Profile>);

impl PartialEq for ProfileRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Stage {
    /// Whether the stage has a device version a plan can place on the
    /// GPU: the CA, the lens when it moves a pixel, the local
    /// contrast, the dehaze, and the sharpen.
    pub fn device(&self) -> bool {
        match self {
            Stage::Ca(_) | Stage::LocalContrast(_) | Stage::Dehaze(_) | Stage::Sharpen(..) => true,
            Stage::Lens(c) => c.iter().any(LensCorrection::moves),
            _ => false,
        }
    }

    /// Whether the stage has a device version that takes a picture in
    /// memory and hands one back: the CA, the lens when it moves a
    /// pixel, and a demosaic with a device version. Every walk with a
    /// device runs these there ([`crate::walk::plan`]).
    pub fn in_memory_on_device(&self) -> bool {
        match self {
            Stage::Ca(_) | Stage::Lens(_) => self.device(),
            _ => self.demosaics_on_device(),
        }
    }

    /// Whether the stage is a demosaic with a device version: the Bayer
    /// ones but bilinear. The viewport, the prefetch, and the export run
    /// it there, so a base the one makes is the same demosaic the others
    /// would.
    pub fn demosaics_on_device(&self) -> bool {
        matches!(
            self,
            Stage::Demosaic(
                DemosaicMethod::Rcd
                    | DemosaicMethod::RcdVng4
                    | DemosaicMethod::Vng4
                    | DemosaicMethod::Amaze
                    | DemosaicMethod::AmazeVng4,
                _
            )
        )
    }

    /// Whether the stage's picture at one turn is its picture at
    /// another turned, to the bit. The lens is radial and the
    /// defringe symmetric in exact arithmetic, but a turn changes the
    /// order of their sums, so in floats neither is. A reduce drops
    /// the pixels past the last whole block, which a turn moves from
    /// the right and the bottom to other edges. Every stage is named,
    /// so a new one is decided here and not taken to commute.
    pub fn commutes_with_turn(&self) -> bool {
        match self {
            Stage::Lens(_) | Stage::Defringe(_) | Stage::Reduce(_) => false,
            Stage::Prepare(_)
            | Stage::Ca(_)
            | Stage::CaFit(_)
            | Stage::CaApply(_)
            | Stage::Demosaic(..)
            | Stage::Bin(_)
            | Stage::Learned(_)
            | Stage::Denoise(_)
            | Stage::Warp(_)
            | Stage::IntoWorking { .. }
            | Stage::Blend { .. }
            | Stage::White { .. }
            | Stage::Orient(_)
            | Stage::Retouch(..)
            | Stage::LocalContrast(_)
            | Stage::Dehaze(_)
            | Stage::Sharpen(..)
            | Stage::Finish(_) => true,
        }
    }

    /// The stage's name, for the log and for a skip.
    pub fn name(&self) -> &'static str {
        match self {
            Stage::Prepare(_) => "prepare",
            Stage::Ca(_) | Stage::CaFit(_) | Stage::CaApply(_) => "CA",
            Stage::Demosaic(..) => "demosaic",
            Stage::Bin(_) => "bin",
            Stage::Learned(_) => "learned denoiser",
            Stage::Denoise(_) => "denoise",
            Stage::Warp(_) => "the file's warp",
            Stage::IntoWorking { .. } => "move into the working space",
            Stage::Blend { .. } => "blend",
            Stage::White { .. } => "white balance",
            Stage::Orient(_) => "orient",
            Stage::Reduce(_) => "reduce",
            Stage::Lens(_) => "lens",
            Stage::Defringe(_) => "defringe",
            Stage::Retouch(..) => "retouch",
            Stage::LocalContrast(_) => "local contrast",
            Stage::Dehaze(_) => "dehaze",
            Stage::Sharpen(..) => "sharpen",
            Stage::Finish(_) => "finish",
        }
    }
}

/// Whether a retouch's fills are made by their model or read from
/// those kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fills {
    Run,
    /// The reduced develop's: a fill kept on disk at the full develop's
    /// size, `full`, is read and taken down by `factor`, the bin; one
    /// not kept refuses, the patch left as it was and the picture not
    /// the edit's. No model runs.
    Read {
        factor: usize,
        full: (usize, usize),
    },
}

/// Where the walk ends.
#[derive(Debug, Clone, PartialEq)]
pub enum Finish {
    /// The viewport's shader runs it: the walk yields the picture and
    /// what the shader reads.
    Shader,
    /// The render crate's, on the CPU, then the border and the mark.
    Cpu {
        settings: Box<export::Settings>,
        fit: Fit,
    },
}

/// The size the CPU's finish makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// The export's long edge, its settings say.
    LongEdge,
    /// Exactly this long edge: the edit's pictures.
    Exact(u32),
}

/// How the learned masks' rasters are had at the tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Masks {
    /// Made when a job asks: the open frame's.
    OnAsk,
    /// Made now if they are not yet: an export's.
    Make,
    /// Read from the cache alone: the reduced develop's.
    CacheOnly,
}

/// A product read off the picture where it stands; not a transform.
#[derive(Debug, Clone, PartialEq)]
pub enum Tap {
    /// The tone equalizer's read of the scene.
    Guide,
    /// The masks' detail bands, the haze read after the global's local
    /// contrast when one is asked for; beside the walk on a thread of
    /// their own, or inline.
    Planes {
        wants: DetailWants,
        haze_after: Option<LocalContrastOptions>,
        beside: bool,
    },
    /// The learned masks' rasters.
    Masks(Masks),
    /// The picture the refined masks are made against, before the
    /// sharpen: only for an edit that refines a mask.
    MaskPicture,
    /// The picture the range masks read, from before the Detail
    /// section, for a finish on the CPU; the viewport's develop hands
    /// on the patched picture it keeps.
    Sample,
    /// The picture the capture sharpen reads what is flat from: the
    /// one before the Detail section's local contrast, dehazed with
    /// the picture after it. Only where the local contrast runs and a
    /// sharpen follows; without it the sharpen reads its own input,
    /// which is that picture.
    SharpenGuide,
}

/// Where a walk leaves what it has for the next to start from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// The learned denoiser's pair, before the blend.
    Pair,
    /// The base: everything before the retouch.
    Base,
    /// The base with its retouch.
    Patched,
    /// The picture before the sharpen.
    PreSharpen,
    /// The developed picture, before its finish.
    Developed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Op(Stage),
    Tap(Tap),
    Keep(Keep),
}

/// The name a list's skips give the learned denoiser.
pub const LEARNED_DENOISER: &str = "the learned denoiser";

/// Why a stage the edit asks for is not in a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The reduced develop leaves it out.
    Reduced,
    /// What the models see leaves it out.
    ModelsSee,
    /// The frame cannot take it; says why.
    Unavailable(&'static str),
}

/// A develop's steps in order, and what of the edit it leaves out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Steps {
    pub steps: Vec<Step>,
    /// What the purpose left out of what the edit asks, by name, and
    /// why; a list with any is kept under the reduced recipe.
    pub skipped: Vec<(&'static str, Skip)>,
}

impl Steps {
    fn op(&mut self, stage: Stage) {
        self.steps.push(Step::Op(stage));
    }

    fn op_if(&mut self, stage: Option<Stage>) {
        if let Some(stage) = stage {
            self.op(stage);
        }
    }

    fn tap(&mut self, tap: Tap) {
        self.steps.push(Step::Tap(tap));
    }

    fn keep(&mut self, keep: Keep) {
        self.steps.push(Step::Keep(keep));
    }

    fn skip(&mut self, name: &'static str, why: Skip) {
        self.skipped.push((name, why));
    }

    /// The stages alone, in order.
    pub fn ops(&self) -> impl Iterator<Item = &Stage> {
        self.steps.iter().filter_map(|s| match s {
            Step::Op(stage) => Some(stage),
            _ => None,
        })
    }

    /// Where the list keeps `keep`, if it does.
    pub fn at(&self, keep: Keep) -> Option<usize> {
        self.steps.iter().position(|s| *s == Step::Keep(keep))
    }

    /// Whether the picture this list makes is kept under the reduced
    /// recipe: the reduced develop or the models' left something of
    /// the edit out. A stage the frame cannot take is not one: an
    /// X-Trans frame's learned tier stands the full develop in, which
    /// is the edit's picture.
    pub fn reduced_recipe(&self) -> bool {
        self.skipped
            .iter()
            .any(|(_, why)| matches!(why, Skip::Reduced | Skip::ModelsSee))
    }

    /// The list a learned denoiser that is not to be had stands in
    /// with: the engine's own demosaic in its place, the noise
    /// measured as the edit's settings ask and no pair to blend. The
    /// learned tier stops the profiled denoise, so none comes in.
    pub fn without_learned(&self) -> Steps {
        let mut out = Steps {
            steps: Vec::with_capacity(self.steps.len()),
            skipped: self.skipped.clone(),
        };
        for step in &self.steps {
            match step {
                Step::Op(Stage::Prepare(p)) => out.op(Stage::Prepare(Box::new(PrepareOptions {
                    settings: p.settings.clone(),
                    measure_noise: p.measure_noise && !p.clip,
                    clip: p.clip,
                }))),
                Step::Op(Stage::Learned(_)) => {
                    let Some(Stage::Prepare(p)) = self.ops().next() else {
                        unreachable!("a learned denoiser follows a prepare");
                    };
                    out.op(Stage::Demosaic(
                        p.settings.demosaic,
                        p.settings.dual_contrast,
                    ));
                }
                Step::Keep(Keep::Pair) | Step::Op(Stage::Blend { .. }) => {}
                step => out.steps.push(step.clone()),
            }
        }
        out
    }
}

/// What a develop must have before the decode.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Needs {
    /// The learned denoiser's model.
    pub denoiser: Option<&'static greycard_ai::Model>,
    /// The look the edit names, when it has strength.
    pub look: Option<String>,
    /// The learned shapes the edit draws, by adjustment id and
    /// component.
    pub masks: Vec<(u64, usize)>,
    /// Whether the retouch has a patch a model fills.
    pub fills: bool,
}

/// What `edit` needs under `purpose` before the decode: the models it
/// runs, the look, and the masks' rasters.
pub fn needs(edit: &Edit, purpose: &Purpose) -> Needs {
    let denoiser = match purpose {
        Purpose::Viewport | Purpose::Export(_) | Purpose::Prefetch => edit
            .noise
            .tier()
            .map(|t| greycard_ai::denoiser(t).expect("the edit's tiers are the registry's")),
        Purpose::Reduced | Purpose::ModelsSee => None,
        Purpose::MergeInput(m) => {
            return Needs {
                denoiser: m.denoiser,
                ..Needs::default()
            };
        }
    };
    let look = match &edit.look_lut.lut {
        greycard_edit::look::LutChoice::Named(name) if edit.look_lut.strength > 0.0 => {
            Some(name.clone())
        }
        _ => None,
    };
    let fills = !matches!(purpose, Purpose::ModelsSee | Purpose::Prefetch)
        && !edit.retouch.is_empty()
        && edit
            .retouch
            .patches
            .iter()
            .any(|p| p.method == greycard_edit::retouch::Method::Fill);
    Needs {
        denoiser,
        look,
        masks: learned_shapes(edit),
        fills,
    }
}

/// The learned shapes `edit` draws: an adjustment on, its mask live,
/// and a shape the models are prompted for.
pub fn learned_shapes(edit: &Edit) -> Vec<(u64, usize)> {
    let mut out = Vec::new();
    for a in &edit.adjustments {
        if !a.enabled || a.mask.is_empty() {
            continue;
        }
        for (i, c) in a.mask.live() {
            if c.shape.is_learned() && prompted(&c.shape) {
                out.push((a.id, i));
            }
        }
    }
    out
}

/// The steps of `edit`'s develop for `purpose` on the frame `facts`
/// describe, with the lens corrections `lenses` holds for it.
pub fn stages(
    edit: &Edit,
    purpose: &Purpose,
    facts: &Facts,
    lenses: Option<&greycard_lens::Database>,
) -> Steps {
    if let Purpose::MergeInput(merge) = purpose {
        return merge_input(edit, merge, facts);
    }
    let reduced = purpose.is_reduced();
    let bin = if reduced {
        facts.bin_under(&edit.geometry)
    } else {
        1
    };
    let mut s = Steps::default();
    let mut settings = edit.settings(&greycard_edit_io::Stores::user());
    // Only where a look table may go on, so the list of an edit with
    // none (and its base, which keeps no baseline then) is as it was.
    if edit.camera.applies_look_table() {
        settings.look_baseline = edit.look_baseline(
            &facts.identity.make,
            &facts.identity.model,
            facts.headroom(),
        );
    }
    match &facts.input {
        InputKind::Raw {
            pattern,
            warp,
            merged,
            ..
        } => {
            let tier = edit
                .noise
                .tier()
                .map(|t| greycard_ai::denoiser(t).expect("the edit's tiers are the registry's"));
            let xtrans = pattern.as_ref().is_some_and(CfaPattern::is_xtrans);
            let learned = match tier {
                Some(_) if reduced => {
                    s.skip(LEARNED_DENOISER, Skip::Reduced);
                    None
                }
                Some(_) if xtrans => {
                    s.skip(
                        LEARNED_DENOISER,
                        Skip::Unavailable("the learned denoiser has no X-Trans version yet"),
                    );
                    None
                }
                Some(_) if pattern.is_none() => {
                    s.skip(
                        LEARNED_DENOISER,
                        Skip::Unavailable(
                            "the learned denoiser needs a mosaic, and this file has none",
                        ),
                    );
                    None
                }
                tier => tier,
            };
            // A merge's noise is the weighted mean of several frames', not
            // one frame's: the profiled denoise's model is a frame's.
            let denoise = settings.denoise.filter(|_| learned.is_none() && !*merged);
            if reduced && denoise.is_some() {
                s.skip("the denoise", Skip::Reduced);
            }
            let denoise = denoise.filter(|_| !reduced);
            s.op(Stage::Prepare(Box::new(PrepareOptions {
                settings: DevelopSettings {
                    chromatic_aberration: None,
                    denoise,
                    orientation: None,
                    ..settings.clone()
                },
                measure_noise: learned.is_some(),
                clip: true,
            })));
            // A file whose warps correct each color has corrected its
            // lateral aberration itself, as Adobe's reader takes it: the
            // automatic correction would move red and blue a second time.
            s.op_if(
                settings
                    .chromatic_aberration
                    .filter(|_| facts.measures_ca())
                    .map(Stage::Ca),
            );
            match (bin, learned) {
                (b, _) if b > 1 && pattern.is_some() => s.op(Stage::Bin(b)),
                (_, Some(model)) => s.op(Stage::Learned(model)),
                // The method the mosaic takes, as the prepare resolves
                // it: where it runs is decided by what it is.
                _ => s.op(Stage::Demosaic(
                    pattern
                        .as_ref()
                        .map_or(settings.demosaic, |p| settings.demosaic.for_mosaic(p)),
                    settings.dual_contrast,
                )),
            }
            s.op_if(
                denoise
                    .filter(|_| bin == 1 || pattern.is_none())
                    .map(Stage::Denoise),
            );
            // The file's own correction of its lens, on the camera's
            // geometry: after the denoise, which reads the demosaic's
            // noise as measured, and before the turn.
            s.op_if(warp.clone().map(Stage::Warp));
            s.op(Stage::IntoWorking {
                profile: settings.profile.clone().map(ProfileRef),
                profile_look: settings.profile_look,
                look_baseline: settings.look_baseline,
                orientation: facts.orientation().expect("a raw's orientation"),
            });
            if learned.is_some() {
                s.keep(Keep::Pair);
                s.op(Stage::Blend {
                    luma: edit.noise.learned_strength,
                    chroma: edit.noise.learned_chroma_strength,
                });
            }
        }
        InputKind::Picture { white, .. } => {
            s.op_if(quarters(facts.turn).map(Stage::Orient));
            s.op_if((bin > 1).then_some(Stage::Reduce(bin)));
            // As shot is the offset of zero, which is no stage; a kelvin
            // light is a raw's, and a picture has none to name. A matrix
            // on each pixel, it commutes with the turn and the reduce, so
            // it runs after them, on the fewest pixels, and before
            // everything else.
            s.op_if(
                greycard_edit::white::picture_light(&edit.white_balance, *white)
                    .filter(|light| light != white)
                    .map(|light| Stage::White { own: *white, light }),
            );
        }
    }
    let (w, h) = facts.binned(bin);
    // A file that corrects its own lens has the last word on it, and
    // the aberration measured on the mosaic over the profile's; a
    // panorama's lens was corrected in its merge, and no one frame's
    // profile fits its canvas.
    let measured = settings.chromatic_aberration.is_some() && facts.measures_ca();
    let mut lens = edit
        .lens
        .over_file(facts.file_corrects())
        .over_measured_ca(measured)
        .over_monochrome(facts.is_monochrome());
    if matches!(
        facts.input,
        InputKind::Raw {
            lens_corrected: true,
            ..
        }
    ) && lens.enabled
        && lens.wants_profile()
    {
        lens.profile = false;
        s.skip(
            "the lens profile",
            Skip::Unavailable("a panorama's lens was corrected when it was merged"),
        );
    }
    if !lens.is_identity(lenses.is_some()) {
        s.op(Stage::Lens(corrections(w, h, facts, &lens, lenses)));
    }
    s.op_if(edit.lens.defringe().map(|mut o| {
        // The radius is in pixels, and the binned picture's are larger.
        if bin > 1 {
            o.radius /= bin as f32;
        }
        Stage::Defringe(o)
    }));
    if !learned_shapes(edit).is_empty() {
        s.tap(Tap::Masks(match purpose {
            Purpose::Viewport | Purpose::Prefetch => Masks::OnAsk,
            Purpose::Export(_) => Masks::Make,
            Purpose::Reduced | Purpose::ModelsSee => Masks::CacheOnly,
            Purpose::MergeInput(_) => unreachable!("a merge's frame ends at the demosaic"),
        }));
    }
    s.keep(Keep::Base);
    if *purpose == Purpose::Prefetch {
        return s;
    }
    if !edit.retouch.is_empty() {
        if *purpose == Purpose::ModelsSee {
            s.skip("the retouch", Skip::ModelsSee);
        } else {
            s.op(Stage::Retouch(
                edit.retouch.clone(),
                if reduced {
                    Fills::Read {
                        factor: bin,
                        full: facts.binned(1),
                    }
                } else {
                    Fills::Run
                },
            ));
        }
    }
    if *purpose != Purpose::ModelsSee {
        s.tap(Tap::Guide);
    }
    s.keep(Keep::Patched);
    let wants = edit.detail_wants();
    let sample = wants.sample;
    if sample && matches!(purpose, Purpose::Export(_) | Purpose::Reduced) {
        s.tap(Tap::Sample);
    }
    // The refined masks' picture, from before the Detail section and
    // the sharpen: a refined edge is the picture's own, and a refined
    // range window takes in what an unrefined one does.
    if greycard_render::refine::refines(edit) && *purpose != Purpose::ModelsSee {
        s.tap(Tap::MaskPicture);
    }
    let wants = greycard_edit::DetailWants {
        sample: false,
        ..wants
    };
    let global = edit.detail.options();
    if wants.bands() && *purpose != Purpose::ModelsSee {
        s.tap(Tap::Planes {
            wants,
            haze_after: global.filter(|_| wants.haze),
            beside: !reduced,
        });
    }
    // Texture, Clarity, and Structure lift the grain of the flattest
    // patch the sharpen's automatic threshold reads, and the contrast
    // its blend mask reads: the sharpen decides what is flat from the
    // picture before them.
    if edit.sharpen_reads_guide() && !reduced && *purpose != Purpose::ModelsSee {
        s.tap(Tap::SharpenGuide);
    }
    s.op_if(global.map(Stage::LocalContrast));
    s.op_if(edit.detail.dehaze_options().map(Stage::Dehaze));
    if *purpose == Purpose::ModelsSee {
        return s;
    }
    s.keep(Keep::PreSharpen);
    // A mask's Sharpen reads the sharpen's delta, made by the global's
    // settings whether or not the global sharpens the picture.
    match (edit.sharpen.options(), wants.sharpen) {
        (Some(_), _) | (None, true) if reduced => s.skip("the capture sharpen", Skip::Reduced),
        (Some(options), masks) => s.op(Stage::Sharpen(
            options,
            if masks { Delta::Also } else { Delta::No },
        )),
        (None, true) => s.op(Stage::Sharpen(edit.sharpen.settings(), Delta::Only)),
        (None, false) => {}
    }
    s.keep(Keep::Developed);
    s.op(Stage::Finish(match purpose {
        Purpose::Viewport => Finish::Shader,
        Purpose::Export(settings) => Finish::Cpu {
            settings: settings.clone(),
            fit: Fit::LongEdge,
        },
        _ => Finish::Cpu {
            settings: Box::new(reduced_settings()),
            fit: Fit::Exact(BIG),
        },
    }));
    s
}

/// A merge's frame: the prepare with nothing clipped after the gains,
/// the noise measured for the weights, the highlights reconstructed
/// only where asked; the CA, by its own fit or the one the merge
/// hands it; the demosaic, or the learned denoiser in its place; a
/// DNG's own warps. No lens, no turn: the merge is in the camera's
/// geometry, and its file carries the lens to the develop.
fn merge_input(edit: &Edit, merge: &MergeInput, facts: &Facts) -> Steps {
    let mut s = Steps::default();
    let InputKind::Raw { pattern, warp, .. } = &facts.input else {
        return s;
    };
    let settings = edit.settings(&greycard_edit_io::Stores::user());
    s.op(Stage::Prepare(Box::new(PrepareOptions {
        settings: DevelopSettings {
            white_point: merge
                .white
                .map_or(WhitePoint::AsShot, WhitePoint::Coefficients),
            highlights: if merge.reconstruct {
                HighlightMode::InpaintOpposed
            } else {
                HighlightMode::Clip
            },
            chromatic_aberration: None,
            denoise: None,
            orientation: None,
            ..settings.clone()
        },
        measure_noise: true,
        clip: false,
    })));
    s.op_if(
        settings
            .chromatic_aberration
            .filter(|_| facts.measures_ca())
            .and_then(|options| match &merge.ca {
                MergeCa::Own => Some(Stage::Ca(options)),
                MergeCa::Make => Some(Stage::CaFit(options)),
                MergeCa::Take(fit) => Some(Stage::CaApply(fit.clone())),
                MergeCa::Off => None,
            }),
    );
    let bayer = pattern
        .as_ref()
        .is_some_and(greycard_core::develop::rcd::is_bayer);
    match merge.denoiser {
        Some(model) if bayer => s.op(Stage::Learned(model)),
        Some(_) => s.skip(
            LEARNED_DENOISER,
            Skip::Unavailable("the learned denoiser needs a 2x2 Bayer mosaic"),
        ),
        None => {}
    }
    if !matches!(s.ops().last(), Some(Stage::Learned(_))) {
        s.op(Stage::Demosaic(settings.demosaic, settings.dual_contrast));
    }
    s.op_if(warp.clone().map(Stage::Warp));
    if !merge.lens.is_empty() {
        s.op(Stage::Lens(merge.lens.clone()));
    }
    s
}

/// Whether `a` and `b` make the same base of the frame `facts`
/// describes: the same steps to it, as a prefetch would take them.
pub fn same_base(
    a: &Edit,
    b: &Edit,
    facts: &Facts,
    lenses: Option<&greycard_lens::Database>,
) -> bool {
    stages(a, &Purpose::Prefetch, facts, lenses) == stages(b, &Purpose::Prefetch, facts, lenses)
}

/// Whether `a` and `b` develop the same picture of the frame `facts`
/// describes, with the same masks' bands beside it: the viewport's
/// lists of the two up to the finish, their keeps, the learned masks'
/// tap, and the refined masks' picture left out (a save makes the
/// learned rasters and that picture on its own, and the window asks
/// for them apart from a develop).
pub fn same_develop(
    a: &Edit,
    b: &Edit,
    facts: &Facts,
    lenses: Option<&greycard_lens::Database>,
) -> bool {
    let developed = |e: &Edit| -> Vec<Step> {
        stages(e, &Purpose::Viewport, facts, lenses)
            .steps
            .into_iter()
            .take_while(|s| !matches!(s, Step::Op(Stage::Finish(_))))
            .filter(|s| {
                !matches!(
                    s,
                    Step::Keep(_) | Step::Tap(Tap::Masks(_) | Tap::MaskPicture)
                )
            })
            .collect()
    };
    developed(a) == developed(b)
}

/// The long edge of the edit's pictures.
pub const BIG: u32 = 2048;

/// The edit's pictures' finish: sRGB, no output sharpen.
fn reduced_settings() -> export::Settings {
    export::Settings {
        sharpen: export::Sharpen::Off,
        ..Default::default()
    }
}

/// The lens's corrections for a picture `w` by `h` of the frame, the
/// section as it applies over the file's own correction.
fn corrections(
    w: usize,
    h: usize,
    facts: &Facts,
    lens: &greycard_edit::Lens,
    db: Option<&greycard_lens::Database>,
) -> Vec<LensCorrection> {
    let id = &facts.identity;
    let identity = (
        id.make.as_str(),
        id.model.as_str(),
        &id.shot,
        facts.is_raw(),
    );
    // A picture was turned by its tag at decode, which is not known
    // here: its turns since are what the lens sees.
    let orientation = facts
        .orientation()
        .or_else(|| quarters(facts.turn))
        .unwrap_or_default();
    let profile = crate::lens::profile_correction((w, h), orientation, identity, lens, db);
    lens.corrections(profile)
}

/// A whole number of quarter turns clockwise as an orientation, for
/// a picture that was already turned by its own tag at decode; None
/// for no turn at all.
pub fn quarters(turn: u8) -> Option<Orientation> {
    match turn % 4 {
        1 => Some(Orientation::Rotate90),
        2 => Some(Orientation::Rotate180),
        3 => Some(Orientation::Rotate270),
        _ => None,
    }
}

/// Whether the shape has anything for a model to go on.
pub fn prompted(shape: &greycard_edit::mask::Shape) -> bool {
    match shape {
        greycard_edit::mask::Shape::Subject {}
        | greycard_edit::mask::Shape::Background {}
        | greycard_edit::mask::Shape::Sky { .. }
        | greycard_edit::mask::Shape::Part { .. } => true,
        greycard_edit::mask::Shape::Landscape { .. } => !shape.is_unknown(),
        greycard_edit::mask::Shape::Object { picks, boxes } => {
            !picks.is_empty() || !boxes.is_empty()
        }
        _ => false,
    }
}

/// The size of a develop of `full` once `geometry` has framed it, as
/// `geometry::apply` cuts it.
pub fn framed_size(full: (u32, u32), geometry: &Geometry) -> (u32, u32) {
    if geometry.is_identity_on(full.0 as f32, full.1 as f32) {
        return full;
    }
    let frame = geometry.frame(full.0 as f32, full.1 as f32);
    (frame.size.0 as u32, frame.size.1 as u32)
}

#[cfg(test)]
mod tests {
    use greycard_edit::Adjustment;
    use greycard_edit::mask::Shape;
    use greycard_edit::retouch::{Method, Patch};

    use super::*;

    /// A 24 MP Bayer raw, landscape, at `turn`.
    fn raw(turn: u8) -> Facts {
        Facts {
            input: InputKind::Raw {
                pattern: Some(CfaPattern::rggb()),
                crop: (6000, 4000),
                tag: Orientation::Normal,
                warp: None,
                merged: false,
                headroom: 0.0,
                lens_corrected: false,
                monochrome: false,
            },
            identity: identity(),
            turn,
        }
    }

    fn xtrans() -> Facts {
        Facts {
            input: InputKind::Raw {
                pattern: Some(CfaPattern::xtrans()),
                crop: (7728, 5152),
                tag: Orientation::Normal,
                warp: None,
                merged: false,
                headroom: 0.0,
                lens_corrected: false,
                monochrome: false,
            },
            ..raw(0)
        }
    }

    fn picture(turn: u8) -> Facts {
        Facts::picture(
            (6000, 4000),
            greycard_core::picture::white_temp_tint(greycard_core::illuminant::D65),
            "Canon",
            "EOS R5",
            &Shot::default(),
            turn,
        )
    }

    fn identity() -> Identity {
        Identity {
            make: "Canon".into(),
            model: "EOS R5".into(),
            shot: Shot::default(),
        }
    }

    /// An edit that asks for every stage: the defringe, a retouch,
    /// the profiled denoise, the Detail section, and the sharpen.
    fn everything() -> Edit {
        let mut edit = Edit::default();
        edit.noise.profiled = true;
        edit.lens.defringe = true;
        edit.detail.clarity = 0.4;
        edit.detail.dehaze = 0.25;
        edit.retouch.patches.push(Patch {
            method: Method::Fill,
            ..Patch::default()
        });
        edit
    }

    /// A DCP's look table is read where the render's baseline puts the
    /// picture: the body's, an older edit's own where it keeps one, and
    /// a merge's headroom on top, as `Edit::look_baseline` (which the
    /// command line uses too) has it.
    #[test]
    fn the_look_table_is_read_at_the_baseline_and_a_merges_headroom() {
        let read_at = |edit: &Edit, facts: &Facts| {
            stages(edit, &Purpose::Viewport, facts, None)
                .ops()
                .find_map(|o| match o {
                    Stage::IntoWorking { look_baseline, .. } => Some(*look_baseline),
                    _ => None,
                })
                .expect("a raw goes into the working space")
        };
        let body = greycard_edit::baseline::for_make_model("Canon", "EOS R5");
        // No table, nothing read: the list is the one it was.
        assert_eq!(read_at(&Edit::default(), &raw(0)), 0.0);
        // Nor with a hand-set switch on the file's own profile.
        let mut own = Edit::default();
        own.camera.look_table = true;
        assert_eq!(read_at(&own, &raw(0)), 0.0);
        let mut edit = Edit::default();
        edit.camera
            .choose(greycard_edit::camera::ProfileChoice::Named(
                "Natural".into(),
            ));
        assert_eq!(read_at(&edit, &raw(0)), body);
        let merge = Facts {
            input: InputKind::Raw {
                pattern: Some(CfaPattern::rggb()),
                crop: (6000, 4000),
                tag: Orientation::Normal,
                warp: None,
                merged: true,
                headroom: 2.0,
                lens_corrected: false,
                monochrome: false,
            },
            ..raw(0)
        };
        assert_eq!(read_at(&edit, &merge), body + 2.0);
        assert_eq!(edit.look_baseline("Canon", "EOS R5", 2.0), body + 2.0);
        let mut old = edit.clone();
        old.light.baseline = Some(0.3);
        assert_eq!(read_at(&old, &merge), 2.3);
    }

    fn names(s: &Steps) -> Vec<String> {
        s.steps
            .iter()
            .map(|step| match step {
                Step::Op(Stage::Bin(n)) => format!("bin {n}"),
                Step::Op(Stage::Reduce(n)) => format!("reduce {n}"),
                Step::Op(Stage::Retouch(_, Fills::Read { .. })) => "retouch, fills read".into(),
                Step::Op(Stage::Finish(Finish::Shader)) => "shader".into(),
                Step::Op(Stage::Finish(Finish::Cpu { fit, .. })) => format!("finish {fit:?}"),
                Step::Op(Stage::IntoWorking { .. }) => "into working".into(),
                Step::Op(stage) => stage.name().into(),
                Step::Tap(Tap::Guide) => "guide".into(),
                Step::Tap(Tap::Planes { beside, .. }) => format!("planes beside {beside}"),
                Step::Tap(Tap::Masks(m)) => format!("masks {m:?}"),
                Step::Tap(Tap::MaskPicture) => "mask picture".into(),
                Step::Tap(Tap::Sample) => "sample".into(),
                Step::Tap(Tap::SharpenGuide) => "sharpen guide".into(),
                Step::Keep(k) => format!("keep {k:?}"),
            })
            .collect()
    }

    fn export() -> Purpose {
        Purpose::Export(Box::default())
    }

    /// The sharpen's guide is tapped just before the local contrast,
    /// where Texture, Clarity, or Structure runs and a sharpen
    /// follows; an edit with all three at zero has the list it had.
    #[test]
    fn the_sharpen_guide_is_tapped_before_the_local_contrast() {
        let guide = |e: &Edit, p: &Purpose| {
            let steps = stages(e, p, &raw(0), None).steps;
            let tap = steps
                .iter()
                .position(|s| matches!(s, Step::Tap(Tap::SharpenGuide)));
            if let Some(i) = tap {
                assert!(matches!(steps[i + 1], Step::Op(Stage::LocalContrast(_))));
            }
            tap.is_some()
        };
        let mut edit = Edit::default();
        edit.detail.dehaze = 0.5;
        assert!(edit.sharpen.options().is_some(), "the sharpen is on");
        for p in [Purpose::Viewport, export(), Purpose::Reduced] {
            assert!(!guide(&edit, &p), "Detail at zero: {p:?}");
        }
        for set in [
            |e: &mut Edit| e.detail.texture = 0.5,
            |e: &mut Edit| e.detail.clarity = -0.3,
            |e: &mut Edit| e.detail.structure = 0.2,
        ] {
            let mut e = edit.clone();
            set(&mut e);
            assert!(guide(&e, &Purpose::Viewport));
            assert!(guide(&e, &export()));
            // Neither the reduced develop nor the models' picture
            // sharpens.
            assert!(!guide(&e, &Purpose::Reduced));
            assert!(!guide(&e, &Purpose::ModelsSee));
            e.sharpen.enabled = false;
            assert!(!guide(&e, &Purpose::Viewport), "nothing to guide");
        }
    }

    /// The range masks' picture from before Detail is tapped for a
    /// finish on the CPU alone, and only for a range shape under a
    /// Detail section that does something; the viewport's list does
    /// not change, and no band asks for it.
    #[test]
    fn the_sample_is_tapped_for_a_finish_on_the_cpu() {
        use greycard_edit::mask::{Component, Shape};
        let mut edit = Edit::default();
        edit.detail.dehaze = 0.5;
        let mut a = greycard_edit::Adjustment::default();
        a.mask.components.push(Component {
            shape: Shape::LUMINANCE,
            ..Component::default()
        });
        edit.adjustments.push(a);
        let has = |e: &Edit, p: &Purpose| {
            stages(e, p, &raw(0), None)
                .steps
                .iter()
                .any(|s| matches!(s, Step::Tap(Tap::Sample)))
        };
        assert!(has(&edit, &export()));
        assert!(has(&edit, &Purpose::Reduced));
        assert!(!has(&edit, &Purpose::Viewport));
        let plain = Edit {
            adjustments: Vec::new(),
            ..edit.clone()
        };
        assert_eq!(
            stages(&edit, &Purpose::Viewport, &raw(0), None),
            stages(&plain, &Purpose::Viewport, &raw(0), None)
        );
        assert!(
            !stages(&edit, &export(), &raw(0), None)
                .steps
                .iter()
                .any(|s| matches!(s, Step::Tap(Tap::Planes { .. })))
        );
        // The refined masks' picture is tapped once, before the Detail
        // section, for a range shape and a gradient alike.
        let at = |e: &Edit, want: &dyn Fn(&Step) -> bool| {
            stages(e, &Purpose::Viewport, &raw(0), None)
                .steps
                .iter()
                .position(want)
                .expect("the step is there")
        };
        let picture = |s: &Step| matches!(s, Step::Tap(Tap::MaskPicture));
        let dehaze = |s: &Step| matches!(s, Step::Op(Stage::Dehaze(_)));
        edit.adjustments[0].mask.refine = 0.3;
        assert!(at(&edit, &picture) < at(&edit, &dehaze));
        let taps = |e: &Edit| {
            stages(e, &Purpose::Viewport, &raw(0), None)
                .steps
                .iter()
                .filter(|s| picture(s))
                .count()
        };
        assert_eq!(taps(&edit), 1);
        let mut gradient = edit.clone();
        gradient.adjustments[0].mask.components[0] = Component::default();
        assert!(at(&gradient, &picture) < at(&gradient, &dehaze));
        assert_eq!(taps(&gradient), 1);
        edit.detail.dehaze = 0.0;
        assert!(!has(&edit, &export()));
    }

    /// The demosaic goes to the device for the viewport, the prefetch,
    /// and the export when one is lent and its method has a device
    /// version; the CPU's without one. An X-Trans frame's resolves to
    /// its own.
    #[test]
    fn the_demosaic_is_on_the_device_for_every_walk_lent_one() {
        use crate::walk::{Place, plan};
        use greycard_edit::Demosaic;
        let place = |edit: &Edit, purpose: &Purpose, facts: &Facts, lent: bool| {
            let steps = stages(edit, purpose, facts, None);
            let i = steps
                .steps
                .iter()
                .position(|s| matches!(s, Step::Op(Stage::Demosaic(..))))?;
            Some((plan(&steps, purpose, lent)[i], steps.steps[i].clone()))
        };
        let with = |method| Edit {
            demosaic: method,
            ..Edit::default()
        };
        for method in [
            Demosaic::AmazeVng4,
            Demosaic::RcdVng4,
            Demosaic::Amaze,
            Demosaic::Rcd,
            Demosaic::Vng4,
        ] {
            let edit = with(method);
            for purpose in [Purpose::Viewport, Purpose::Prefetch, export()] {
                let (on, _) = place(&edit, &purpose, &raw(0), true).unwrap();
                assert_eq!(on, Place::Gpu, "{method:?} {purpose:?}");
                let (off, _) = place(&edit, &purpose, &raw(0), false).unwrap();
                assert_eq!(off, Place::Cpu, "{method:?} {purpose:?} with no device");
            }
            // The reduced pictures bin in its place.
            assert!(place(&edit, &Purpose::Reduced, &raw(0), true).is_none());
        }
        let (on, _) = place(&with(Demosaic::Bilinear), &Purpose::Viewport, &raw(0), true).unwrap();
        assert_eq!(on, Place::Cpu, "bilinear has no device version");
        let (on, stage) = place(&Edit::default(), &Purpose::Viewport, &xtrans(), true).unwrap();
        assert_eq!(on, Place::Cpu);
        assert!(matches!(
            stage,
            Step::Op(Stage::Demosaic(DemosaicMethod::Markesteijn3, _))
        ));
    }

    /// An export lent a device runs there every stage with a device
    /// version: its CA, demosaic, and lens, as the viewport does, and
    /// the Detail section, the dehaze, and the sharpen, whose run it
    /// reads back for its own finish on the CPU. Without a device
    /// (`--cpu`, or a machine with none) every step is the CPU's.
    #[test]
    fn an_export_lent_a_device_runs_every_device_stage_there() {
        use crate::walk::{Place, plan};
        let mut edit = everything();
        edit.lens.chromatic_aberration = true;
        edit.lens.manual = -0.05;
        let steps = stages(&edit, &export(), &raw(0), None);
        let lent = plan(&steps, &export(), true);
        let mut on = Vec::new();
        for (step, place) in steps.steps.iter().zip(&lent) {
            if let Step::Op(stage) = step {
                if *place == Place::Gpu {
                    on.push(stage.name());
                }
                if matches!(stage, Stage::Finish(_)) {
                    assert_eq!(*place, Place::Cpu, "{}", stage.name());
                }
            }
        }
        assert_eq!(
            on,
            [
                "CA",
                "demosaic",
                "lens",
                "local contrast",
                "dehaze",
                "sharpen"
            ]
        );
        // The viewport's own puts the same three there.
        let viewport = stages(&edit, &Purpose::Viewport, &raw(0), None);
        let in_memory = |s: &Steps, p: &[Place]| {
            s.steps
                .iter()
                .zip(p)
                .filter(|(s, p)| {
                    **p == Place::Gpu
                        && matches!(
                            s,
                            Step::Op(Stage::Ca(_) | Stage::Demosaic(..) | Stage::Lens(_))
                        )
                })
                .count()
        };
        assert_eq!(
            in_memory(&viewport, &plan(&viewport, &Purpose::Viewport, true)),
            3
        );
        assert!(
            plan(&steps, &export(), false)
                .iter()
                .all(|p| *p == Place::Cpu)
        );
    }

    #[test]
    fn a_raw_s_list_for_each_purpose() {
        let edit = everything();
        let full = [
            "prepare",
            "CA",
            "demosaic",
            "denoise",
            "into working",
            "defringe",
            "keep Base",
            "retouch",
            "guide",
            "keep Patched",
            "sharpen guide",
            "local contrast",
            "dehaze",
            "keep PreSharpen",
            "sharpen",
            "keep Developed",
        ];
        let viewport = stages(&edit, &Purpose::Viewport, &raw(0), None);
        assert_eq!(names(&viewport)[..full.len()], full);
        assert_eq!(names(&viewport)[full.len()..], ["shader"]);
        assert!(viewport.skipped.is_empty());

        let export = stages(&edit, &export(), &raw(0), None);
        assert_eq!(names(&export)[..full.len()], full);
        assert_eq!(names(&export)[full.len()..], ["finish LongEdge"]);

        let reduced = stages(&edit, &Purpose::Reduced, &raw(0), None);
        assert_eq!(
            names(&reduced),
            [
                "prepare",
                "CA",
                "bin 2",
                "into working",
                "defringe",
                "keep Base",
                "retouch, fills read",
                "guide",
                "keep Patched",
                "local contrast",
                "dehaze",
                "keep PreSharpen",
                "keep Developed",
                "finish Exact(2048)",
            ]
        );
        assert_eq!(
            reduced.skipped,
            [
                ("the denoise", Skip::Reduced),
                ("the capture sharpen", Skip::Reduced)
            ]
        );
        assert!(reduced.reduced_recipe());
        // The defringe's radius is in the binned picture's pixels.
        let radius = |s: &Steps| {
            s.ops()
                .find_map(|o| match o {
                    Stage::Defringe(d) => Some(d.radius),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(radius(&reduced), radius(&viewport) / 2.0);

        let models = stages(&edit, &Purpose::ModelsSee, &raw(0), None);
        assert_eq!(
            names(&models),
            [
                "prepare",
                "CA",
                "bin 2",
                "into working",
                "defringe",
                "keep Base",
                "keep Patched",
                "local contrast",
                "dehaze",
            ]
        );
        assert!(models.skipped.contains(&("the retouch", Skip::ModelsSee)));

        let prefetch = stages(&edit, &Purpose::Prefetch, &raw(0), None);
        assert_eq!(names(&prefetch), full[..7]);
        // The shelf's base is the viewport's: the same steps to it.
        let base = viewport.at(Keep::Base).unwrap();
        assert_eq!(prefetch.steps, viewport.steps[..=base]);
    }

    #[test]
    fn a_picture_s_list_turns_and_reduces_it() {
        let edit = everything();
        let viewport = stages(&edit, &Purpose::Viewport, &picture(1), None);
        assert_eq!(names(&viewport)[..3], ["orient", "defringe", "keep Base"]);
        let unturned = stages(&edit, &Purpose::Viewport, &picture(0), None);
        assert_eq!(names(&unturned)[..2], ["defringe", "keep Base"]);
        let reduced = stages(&edit, &Purpose::Reduced, &picture(1), None);
        assert_eq!(
            names(&reduced)[..4],
            ["orient", "reduce 2", "defringe", "keep Base"]
        );
        // A picture is not denoised, so nothing of that is skipped.
        assert_eq!(reduced.skipped, [("the capture sharpen", Skip::Reduced)]);
    }

    /// A picture's white balance is its first step after the turn and
    /// the reduce when it names a light other than the picture's own; As shot, an offset of none, and a
    /// kelvin light (a raw's) are no step, and a raw never takes one.
    #[test]
    fn a_pictures_white_balance_is_its_first_step() {
        use greycard_edit::WhiteBalance;
        let with = |wb: WhiteBalance, facts: &Facts| {
            let mut edit = Edit::for_picture();
            edit.white_balance = wb;
            stages(&edit, &Purpose::Viewport, facts, None)
        };
        let warm = WhiteBalance::Offset {
            mired: 25.0,
            tint: 0.0,
        };
        let steps = with(warm.clone(), &picture(1));
        assert_eq!(names(&steps)[..2], ["orient", "white balance"]);
        let mut edit = Edit::for_picture();
        edit.white_balance = warm.clone();
        let reduced = stages(&edit, &Purpose::Reduced, &picture(1), None);
        assert_eq!(
            names(&reduced)[..3],
            ["orient", "reduce 2", "white balance"]
        );
        let Some(Stage::White { own, light }) = steps.ops().nth(1) else {
            panic!()
        };
        assert!((1e6 / own.cct - 1e6 / light.cct - 25.0).abs() < 1e-9);
        for none in [
            WhiteBalance::AsShot,
            WhiteBalance::Offset {
                mired: 0.0,
                tint: 0.0,
            },
            WhiteBalance::Custom {
                temperature: 3200.0,
                tint: 0.0,
            },
        ] {
            let steps = with(none.clone(), &picture(0));
            assert!(
                !steps.ops().any(|s| matches!(s, Stage::White { .. })),
                "{none:?}"
            );
        }
        assert!(
            !with(warm, &raw(0))
                .ops()
                .any(|s| matches!(s, Stage::White { .. }))
        );
    }

    /// A raw whose file corrects its lens with a warp.
    fn phone() -> Facts {
        let mut facts = raw(0);
        let InputKind::Raw { warp, .. } = &mut facts.input else {
            unreachable!()
        };
        let mut lens = greycard_core::opcodes::WarpPlane::identity();
        lens.radial[2] = 0.03;
        *warp = Some(Box::new(WarpOptions {
            warps: vec![Warp {
                planes: vec![lens],
                center_x: 0.5,
                center_y: 0.5,
                reciprocal: false,
            }],
            area: MapPlacement {
                x: 8,
                y: 8,
                image_width: 6016,
                image_height: 4016,
            },
        }));
        facts
    }

    /// The file's warp runs on camera RGB: after the demosaic and the
    /// denoise, after the bin, and after the learned denoiser, each
    /// before the move into the working space, and on no other raw.
    #[test]
    fn a_files_warp_runs_before_the_move_into_working() {
        let edit = everything();
        let at = |s: &Steps| names(s).iter().position(|n| n == "the file's warp");
        let viewport = stages(&edit, &Purpose::Viewport, &phone(), None);
        assert_eq!(
            names(&viewport)[..6],
            [
                "prepare",
                "CA",
                "demosaic",
                "denoise",
                "the file's warp",
                "into working"
            ]
        );
        let reduced = stages(&edit, &Purpose::Reduced, &phone(), None);
        assert_eq!(
            names(&reduced)[2..5],
            ["bin 2", "the file's warp", "into working"]
        );
        let mut learned = Edit::default();
        learned.noise.learned = greycard_edit::Learned::Best;
        let pair = stages(&learned, &Purpose::Viewport, &phone(), None);
        assert_eq!(
            names(&pair)[2..5],
            ["learned denoiser", "the file's warp", "into working"]
        );
        // The list standing in for a learned tier not to be had keeps
        // it.
        assert_eq!(
            names(&pair.without_learned())[2..4],
            ["demosaic", "the file's warp"]
        );
        assert!(at(&stages(&edit, &Purpose::Viewport, &raw(0), None)).is_none());
        assert!(
            !Stage::Warp(Box::new(WarpOptions {
                warps: vec![],
                area: MapPlacement {
                    x: 0,
                    y: 0,
                    image_width: 1,
                    image_height: 1
                }
            }))
            .device()
        );
    }

    /// A database that knows the R5 and an RF 50mm with all three of a
    /// profile's corrections, and a shot on that lens.
    fn rf50() -> (greycard_lens::Database, Shot) {
        let mut db = greycard_lens::Database::default();
        db.read_xml(
            r#"<lensdatabase version="2">
            <camera><maker>Canon</maker><model>Canon EOS R5</model>
            <mount>Canon RF</mount><cropfactor>1</cropfactor></camera>
            <lens><maker>Canon</maker><model>Canon RF 50mm F1.8 STM</model>
            <mount>Canon RF</mount><cropfactor>1.0</cropfactor><calibration>
            <distortion model="poly3" focal="50" k1="0.02"/>
            <tca model="linear" focal="50" kr="1.0002" kb="0.9998"/>
            <vignetting model="pa" focal="50" aperture="2.8" distance="10" k1="-0.5" k2="0" k3="0"/>
            </calibration></lens></lensdatabase>"#,
        )
        .unwrap();
        let shot = Shot {
            lens_model: Some("Canon RF 50mm F1.8 STM".into()),
            focal_length: Some(50.0),
            f_number: Some(2.8),
            ..Shot::default()
        };
        (db, shot)
    }

    /// A picture starts with its lens profile's corrections off, the
    /// lens still matched: a fresh picture develops with no lens step,
    /// one switch puts the correction in, and a raw's fresh edit still
    /// corrects.
    #[test]
    fn a_fresh_picture_takes_no_lens_profile_and_a_raw_still_does() {
        let (db, shot) = rf50();
        let mut facts = picture(0);
        facts.identity.shot = shot.clone();
        let lens = |edit: &Edit, facts: &Facts| -> Option<LensCorrection> {
            stages(edit, &Purpose::Viewport, facts, Some(&db))
                .ops()
                .find_map(|o| match o {
                    Stage::Lens(c) => c.first().copied(),
                    _ => None,
                })
        };
        assert_eq!(lens(&Edit::for_picture(), &facts), None);
        // The lens is still the database's for the picture: one switch
        // and its correction runs.
        let mut on = Edit::for_picture();
        on.lens.distortion = true;
        let c = lens(&on, &facts).expect("the profile's distortion");
        assert!(c.distortion.is_some() && c.vignetting.is_none());
        // A picture's sidecar from before keeps its corrections on.
        let before = Edit {
            lens: greycard_edit::Lens::default(),
            ..Edit::for_picture()
        };
        let c = lens(&before, &facts).expect("the profile's corrections");
        assert!(c.distortion.is_some() && c.vignetting.is_some());
        // A raw's default is unchanged.
        let mut raw = raw(0);
        raw.identity.shot = shot;
        let c = lens(&Edit::default(), &raw).expect("a raw's profile");
        assert!(c.distortion.is_some() && c.vignetting.is_some());
    }

    /// A monochrome raw's three channels are one: its profile's lateral
    /// aberration and one set by hand stand down, and the rest of the
    /// profile runs.
    #[test]
    fn a_monochrome_raw_takes_no_lateral_aberration() {
        let (db, shot) = rf50();
        let mut facts = raw(0);
        facts.identity.shot = shot;
        if let InputKind::Raw {
            pattern,
            monochrome,
            ..
        } = &mut facts.input
        {
            *pattern = None;
            *monochrome = true;
        }
        let mut edit = Edit::default();
        edit.lens.ca_red = 0.001;
        let s = stages(&edit, &Purpose::Viewport, &facts, Some(&db));
        let lens = s
            .ops()
            .find_map(|o| match o {
                Stage::Lens(c) => c.first().copied(),
                _ => None,
            })
            .expect("the profile's correction");
        assert!(lens.distortion.is_some() && lens.vignetting.is_some());
        assert!(lens.chromatic_aberration.is_none());
        assert!(!s.ops().any(|o| matches!(o, Stage::Ca(_))));
    }

    /// A file that corrects its own lens has the last word on it: under
    /// the file's own lens, its profile gives up the distortion to a
    /// warp, and the lateral aberration (the profile's and the automatic
    /// one) to a warp that moves each color; the vignetting stays.
    #[test]
    fn a_files_warp_takes_the_profiles_distortion_and_the_ca() {
        let (db, shot) = rf50();
        let with_shot = |mut f: Facts| {
            f.identity.shot = shot.clone();
            f
        };
        let edit = Edit::default();
        let lens_of = |facts: &Facts| -> (Option<LensCorrection>, bool) {
            let s = stages(&edit, &Purpose::Viewport, facts, Some(&db));
            let lens = s.ops().find_map(|o| match o {
                Stage::Lens(c) => c.first().copied(),
                _ => None,
            });
            (lens, s.ops().any(|o| matches!(o, Stage::Ca(_))))
        };

        // The aberration measured on the mosaic stands the profile's
        // down; a linear raw has no mosaic to measure, so the profile's
        // runs.
        let (plain, ca) = lens_of(&with_shot(raw(0)));
        let plain = plain.expect("the profile's correction");
        assert!(plain.distortion.is_some() && plain.chromatic_aberration.is_none());
        assert!(ca, "the automatic CA");
        let mut linear = with_shot(raw(0));
        if let InputKind::Raw { pattern, .. } = &mut linear.input {
            *pattern = None;
        }
        let (linear, ca) = lens_of(&linear);
        assert!(
            linear
                .expect("the profile's correction")
                .chromatic_aberration
                .is_some()
        );
        assert!(!ca, "no mosaic to measure");

        let (warped, ca) = lens_of(&with_shot(phone()));
        let warped = warped.expect("the vignetting stays");
        assert!(warped.distortion.is_none());
        assert!(warped.chromatic_aberration.is_none() && warped.vignetting.is_some());
        assert!(ca, "a warp of one set leaves the CA to the measurement");

        let mut each = with_shot(phone());
        let InputKind::Raw { warp: Some(w), .. } = &mut each.input else {
            unreachable!()
        };
        let plane = w.warps[0].planes[0].clone();
        w.warps[0].planes = vec![plane.clone(), plane.clone(), plane];
        let (colors, ca) = lens_of(&each);
        let colors = colors.expect("the vignetting stays");
        assert!(colors.distortion.is_none() && colors.chromatic_aberration.is_none());
        assert!(colors.vignetting.is_some());
        assert!(!ca, "the file corrects each color");

        // A lens picked by hand keeps all it asks for.
        let mut picked = Edit::default();
        picked.lens.chosen = greycard_edit::LensChoice::Database("Canon RF 50mm F1.8 STM".into());
        let s = stages(&picked, &Purpose::Viewport, &each, Some(&db));
        assert!(
            s.ops()
                .any(|o| matches!(o, Stage::Lens(c) if c[0].distortion.is_some()))
        );

        // A panorama's lens was corrected when it was merged: no
        // profile, the file's own or one picked, and it is said; a
        // correction set by hand still runs.
        let pano = |mut f: Facts| {
            if let InputKind::Raw { lens_corrected, .. } = &mut f.input {
                *lens_corrected = true;
            }
            f
        };
        for edit in [&edit, &picked] {
            let s = stages(
                edit,
                &Purpose::Viewport,
                &pano(with_shot(raw(0))),
                Some(&db),
            );
            assert!(!s.ops().any(|o| matches!(o, Stage::Lens(_))), "{s:?}");
            assert!(
                s.skipped
                    .iter()
                    .any(|(what, _)| *what == "the lens profile")
            );
        }
        let mut by_hand = Edit::default();
        by_hand.lens.manual = -0.05;
        let s = stages(
            &by_hand,
            &Purpose::Viewport,
            &pano(with_shot(raw(0))),
            Some(&db),
        );
        assert!(s.ops().any(|o| matches!(
            o,
            Stage::Lens(c) if c.len() == 1 && c[0].vignetting.is_none()
        )));
    }

    /// An edit from before the chroma blend, its blend seeded low from
    /// the ISO: the chroma is brought up as that blend, so the mix is
    /// the one it always was; the same edit written now blends the
    /// chroma in full.
    #[test]
    fn an_edit_from_before_the_chroma_blend_blends_as_it_did() {
        let old = r#"{"version": 6, "noise": {"learned": "best", "learned_strength": 0.35}}"#;
        let edit = greycard_edit::Edit::from_json(old).unwrap();
        let viewport = stages(&edit, &Purpose::Viewport, &raw(0), None);
        assert!(viewport.ops().any(|o| *o
            == Stage::Blend {
                luma: 0.35,
                chroma: 0.35
            }));
        let now = r#"{"version": 7, "noise": {"learned": "best", "learned_strength": 0.35}}"#;
        let edit = greycard_edit::Edit::from_json(now).unwrap();
        let viewport = stages(&edit, &Purpose::Viewport, &raw(0), None);
        assert!(viewport.ops().any(|o| *o
            == Stage::Blend {
                luma: 0.35,
                chroma: 1.0
            }));
    }

    #[test]
    fn a_learned_tier_makes_a_pair_and_blends_it() {
        let mut edit = everything();
        edit.noise.learned = greycard_edit::Learned::Best;
        edit.noise.learned_strength = 0.6;
        edit.noise.learned_chroma_strength = 0.9;
        let viewport = stages(&edit, &Purpose::Viewport, &raw(0), None);
        assert_eq!(
            names(&viewport)[..7],
            [
                "prepare",
                "CA",
                "learned denoiser",
                "into working",
                "keep Pair",
                "blend",
                "defringe"
            ]
        );
        assert!(viewport.ops().any(|o| *o
            == Stage::Blend {
                luma: 0.6,
                chroma: 0.9
            }));
        // The learned tier measures the noise, and stands in for the
        // profiled denoise, which does not run.
        let Some(Stage::Prepare(p)) = viewport.ops().next() else {
            panic!("a raw's list begins at its prepare");
        };
        assert!(p.measure_noise);
        assert!(p.settings.denoise.is_none());
        assert_eq!(needs(&edit, &Purpose::Viewport).denoiser.map(|m| m.id), {
            greycard_ai::denoiser("best").map(|m| m.id)
        });

        // Without its model, the engine's own path stands in.
        let stand_in = viewport.without_learned();
        assert_eq!(
            names(&stand_in)[..5],
            ["prepare", "CA", "demosaic", "into working", "defringe"]
        );

        // The reduced develop has no learned denoiser, and says so.
        let reduced = stages(&edit, &Purpose::Reduced, &raw(0), None);
        assert!(!reduced.ops().any(|o| matches!(o, Stage::Learned(_))));
        assert!(reduced.skipped.contains(&(LEARNED_DENOISER, Skip::Reduced)));
        assert_eq!(needs(&edit, &Purpose::Reduced).denoiser, None);

        // Nor does an X-Trans mosaic, which it has no version for; the
        // full develop stands in, which is the edit's picture, so the
        // list is not the reduced recipe's.
        let x = stages(&edit, &Purpose::Viewport, &xtrans(), None);
        assert!(!x.ops().any(|o| matches!(o, Stage::Learned(_))));
        assert!(matches!(
            x.skipped[..],
            [(LEARNED_DENOISER, Skip::Unavailable(_))]
        ));
        assert!(!x.reduced_recipe());
        assert!(!stages(&edit, &export(), &xtrans(), None).reduced_recipe());
    }

    /// A merge's frame: the prepare with nothing clipped and its noise
    /// measured, the CA, the demosaic, and nothing after; the reference's
    /// white taken; the darkest frame's highlights reconstructed; the
    /// learned denoiser in the demosaic's place when asked, on a Bayer
    /// mosaic alone.
    #[test]
    fn a_merges_frame_ends_at_the_demosaic_with_nothing_clipped() {
        let merge = |white, reconstruct, denoiser| {
            Purpose::MergeInput(MergeInput {
                white,
                reconstruct,
                denoiser,
                ca: MergeCa::Own,
                lens: Vec::new(),
            })
        };
        let edit = everything();
        let frame = stages(
            &edit,
            &merge(Some([2.0, 1.0, 1.5]), false, None),
            &raw(0),
            None,
        );
        assert_eq!(names(&frame), ["prepare", "CA", "demosaic"]);
        let Some(Stage::Prepare(p)) = frame.ops().next() else {
            panic!("a raw's list begins at its prepare");
        };
        assert!(!p.clip && p.measure_noise);
        assert_eq!(p.settings.highlights, HighlightMode::Clip);
        assert_eq!(
            p.settings.white_point,
            WhitePoint::Coefficients([2.0, 1.0, 1.5])
        );
        assert!(p.settings.denoise.is_none());
        // On the CPU, a device lent or not.
        let purpose = merge(None, false, None);
        assert!(
            crate::walk::plan(&frame, &purpose, true)
                .iter()
                .all(|p| *p == crate::walk::Place::Cpu)
        );
        let darkest = stages(&edit, &merge(None, true, None), &raw(0), None);
        let Some(Stage::Prepare(p)) = darkest.ops().next() else {
            panic!("a raw's list begins at its prepare");
        };
        assert_eq!(p.settings.highlights, HighlightMode::InpaintOpposed);
        assert_eq!(p.settings.white_point, WhitePoint::AsShot);

        let model = greycard_ai::denoiser("balanced").unwrap();
        let learned = stages(&edit, &merge(None, false, Some(model)), &raw(0), None);
        assert_eq!(names(&learned), ["prepare", "CA", "learned denoiser"]);
        assert_eq!(
            needs(&edit, &merge(None, false, Some(model)))
                .denoiser
                .map(|m| m.id),
            Some(model.id)
        );
        let x = stages(&edit, &merge(None, false, Some(model)), &xtrans(), None);
        assert_eq!(names(&x), ["prepare", "CA", "demosaic"]);
        assert!(matches!(
            x.skipped[..],
            [(LEARNED_DENOISER, Skip::Unavailable(_))]
        ));

        // A panorama's frame: the lens's vignetting in camera space,
        // after the demosaic, before anything leaves camera RGB.
        let vignetting = LensCorrection {
            vignetting: Some(greycard_core::develop::lens::Vignetting {
                k: [-0.4, 0.0, 0.0],
            }),
            ..Default::default()
        };
        let pano = stages(
            &edit,
            &Purpose::MergeInput(MergeInput {
                white: None,
                reconstruct: true,
                denoiser: None,
                ca: MergeCa::Own,
                lens: vec![vignetting],
            }),
            &raw(0),
            None,
        );
        assert_eq!(names(&pano), ["prepare", "CA", "demosaic", "lens"]);
        assert!(!pano.ops().any(|o| matches!(o, Stage::IntoWorking { .. })));
    }

    /// A merge's frame fits its CA and hands the fit back (the
    /// reference), takes another frame's, fits its own, or has none,
    /// as the merge asks.
    #[test]
    fn a_merges_frame_takes_its_ca_fit_as_asked() {
        let edit = everything();
        let ca_of = |ca| {
            let purpose = Purpose::MergeInput(MergeInput {
                white: None,
                reconstruct: false,
                denoiser: None,
                ca,
                lens: Vec::new(),
            });
            let s = stages(&edit, &purpose, &raw(0), None);
            s.ops().nth(1).cloned()
        };
        let (_, made) =
            ca::fit_ca(&[0.0; 16], 4, 4, &CfaPattern::rggb(), &CaOptions::default()).unwrap();
        let fit = CaFitRef(Arc::new(made));
        assert!(matches!(ca_of(MergeCa::Own), Some(Stage::Ca(_))));
        assert!(matches!(ca_of(MergeCa::Make), Some(Stage::CaFit(_))));
        assert_eq!(ca_of(MergeCa::Take(fit.clone())), Some(Stage::CaApply(fit)));
        assert!(matches!(ca_of(MergeCa::Off), Some(Stage::Demosaic(..))));
    }

    /// A linear file has no mosaic for the learned denoiser; a merge of
    /// ours takes no profiled denoise either.
    #[test]
    fn a_linear_file_takes_no_learned_denoiser_and_a_merge_no_denoise() {
        let linear = |merged| Facts {
            input: InputKind::Raw {
                pattern: None,
                crop: (6000, 4000),
                tag: Orientation::Normal,
                warp: None,
                merged,
                headroom: 0.0,
                lens_corrected: false,
                monochrome: false,
            },
            ..raw(0)
        };
        let mut learned = everything();
        learned.noise.learned = greycard_edit::Learned::Best;
        let s = stages(&learned, &Purpose::Viewport, &linear(false), None);
        assert!(!s.ops().any(|o| matches!(o, Stage::Learned(_))));
        assert!(matches!(
            s.skipped[..],
            [(LEARNED_DENOISER, Skip::Unavailable(_))]
        ));
        let mut profiled = everything();
        profiled.noise.enabled = true;
        profiled.noise.profiled = true;
        profiled.noise.learned = greycard_edit::Learned::Off;
        let denoises = |f: &Facts| {
            stages(&profiled, &Purpose::Viewport, f, None)
                .ops()
                .any(|o| matches!(o, Stage::Denoise(_)))
        };
        assert!(denoises(&linear(false)));
        assert!(!denoises(&linear(true)));
    }

    #[test]
    fn the_bin_follows_the_mosaic_and_the_crop() {
        let edit = Edit::default();
        let x = stages(&edit, &Purpose::Reduced, &xtrans(), None);
        assert!(x.ops().any(|o| *o == Stage::Bin(3)));
        // A crop that leaves less than twice the 2048 develops in full.
        let mut cropped = Edit::default();
        cropped.geometry.crop = Some(greycard_edit::geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        let r = stages(&cropped, &Purpose::Reduced, &raw(0), None);
        assert!(r.ops().any(|o| matches!(o, Stage::Demosaic(..))));
        assert!(!r.ops().any(|o| matches!(o, Stage::Bin(_))));
    }

    #[test]
    fn a_learned_mask_is_tapped_at_the_base_as_the_purpose_has_it() {
        let mut edit = Edit::default();
        let mut a = Adjustment::default();
        a.mask.components.push(greycard_edit::mask::Component {
            shape: Shape::Subject {},
            ..Default::default()
        });
        edit.adjustments.push(a);
        let masks = |p: &Purpose| {
            stages(&edit, p, &raw(0), None)
                .steps
                .iter()
                .find_map(|s| match s {
                    Step::Tap(Tap::Masks(m)) => Some(*m),
                    _ => None,
                })
        };
        assert_eq!(masks(&Purpose::Viewport), Some(Masks::OnAsk));
        assert_eq!(masks(&export()), Some(Masks::Make));
        assert_eq!(masks(&Purpose::Reduced), Some(Masks::CacheOnly));
        assert_eq!(
            needs(&edit, &Purpose::Viewport).masks,
            [(edit.adjustments[0].id, 0)]
        );
        assert_eq!(masks(&Purpose::Viewport), masks(&Purpose::Prefetch));
    }

    #[test]
    fn a_list_equals_itself_and_not_one_option_moved() {
        let edit = everything();
        let a = stages(&edit, &Purpose::Viewport, &raw(0), None);
        assert_eq!(a, stages(&edit, &Purpose::Viewport, &raw(0), None));
        let moved = |change: &dyn Fn(&mut Edit)| {
            let mut e = edit.clone();
            change(&mut e);
            stages(&e, &Purpose::Viewport, &raw(0), None)
        };
        let sharper = moved(&|e| e.sharpen.iterations += 1);
        assert_ne!(a, sharper);
        // Everything up to the sharpen is the same, so the picture
        // before it can be kept.
        let pre = a.at(Keep::PreSharpen).unwrap();
        assert_eq!(a.steps[..=pre], sharper.steps[..=pre]);
        let hazier = moved(&|e| e.detail.dehaze = 0.5);
        let patched = a.at(Keep::Patched).unwrap();
        assert_eq!(a.steps[..=patched], hazier.steps[..=patched]);
        assert_ne!(a.steps[..=pre], hazier.steps[..=pre]);
        let warmer = moved(&|e| {
            e.white_balance = greycard_edit::WhiteBalance::Custom {
                temperature: 3200.0,
                tint: 0.0,
            }
        });
        assert_ne!(a.steps[0], warmer.steps[0]);
        // A turn moves the step into the working space and nothing
        // before it.
        let turned = stages(&edit, &Purpose::Viewport, &raw(1), None);
        assert_eq!(a.steps[..4], turned.steps[..4]);
        assert_ne!(a.steps[4], turned.steps[4]);
        // An exposure is the finish's, which reads it from the edit.
        assert_eq!(a, moved(&|e| e.light.exposure = 1.0));
    }

    /// Two edits develop the same picture when their lists to the
    /// finish are one: the light and a look are the finish's, and so is
    /// a mask's amount, but which bands the masks ask for is the
    /// develop's, as are the demosaic and the Detail section. A mask's
    /// refined edge is neither, nor a learned shape added to a mask.
    #[test]
    fn same_develop_reads_the_lists_to_the_finish() {
        let facts = raw(0);
        let same = |a: &Edit, b: &Edit| same_develop(a, b, &facts, None);
        let edit = Edit::default();
        let mut light = edit.clone();
        light.light.exposure = 2.0;
        assert!(same(&edit, &light));
        let mut demosaic = edit.clone();
        demosaic.demosaic = greycard_edit::Demosaic::Vng4;
        assert!(!same(&edit, &demosaic));
        let mut dehaze = edit.clone();
        dehaze.detail.dehaze = 0.5;
        assert!(!same(&edit, &dehaze));
        let mut structure = edit.clone();
        structure.detail.structure = 0.5;
        assert!(!same(&edit, &structure));
        let masked = |texture: f32, refine: f32| {
            let mut e = edit.clone();
            let mut a = greycard_edit::Adjustment {
                id: 1,
                ..Default::default()
            };
            a.mask
                .components
                .push(greycard_edit::mask::Component::default());
            a.mask.refine = refine;
            a.look.detail.texture = texture;
            e.adjustments.push(a);
            e
        };
        assert!(!same(&edit, &masked(-0.5, 0.0)), "the bands asked for");
        assert!(same(&masked(-0.5, 0.0), &masked(-0.8, 0.0)), "their amount");
        let mut wide = masked(-0.5, 0.0);
        wide.adjustments[0].look.detail.structure = 0.4;
        assert!(
            !same(&masked(-0.5, 0.0), &wide),
            "Structure's band asked for"
        );
        let mut wider = wide.clone();
        wider.adjustments[0].look.detail.structure = -0.9;
        assert!(same(&wide, &wider), "Structure's amount");
        assert!(same(&masked(0.0, 0.0), &masked(0.0, 0.3)), "a refined edge");
        let mut subject = masked(0.0, 0.0);
        subject.adjustments[0].mask.components[0].shape = greycard_edit::mask::Shape::Subject {};
        assert!(
            stages(&subject, &Purpose::Viewport, &facts, None)
                .steps
                .iter()
                .any(|s| matches!(s, Step::Tap(Tap::Masks(_)))),
            "the learned masks' tap in the list"
        );
        assert!(same(&masked(0.0, 0.0), &subject), "a learned shape");
    }
}
