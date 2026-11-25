//! The focus stack: every frame read twice, once to place it and once
//! to blend it, merged band by band from whichever frame holds each
//! sharpest (the core's `stack_camera_from`), and written as a linear
//! DNG beside the first frame, `<first> stack.dng`.
//!
//! The placing reads take a short develop ([`register_settings`]): the
//! registration fits log luminance, and a fast demosaic with no CA
//! lands the frames where the walk's would at half its cost. The
//! blending reads walk each frame
//! through the stage list as the HDR merge's frames are walked
//! (`Purpose::MergeInput`): the edit's demosaic for the sensor, hot
//! pixels, the CA corrected by one fit made on the reference, the
//! highlights reconstructed as a bracket's darkest frame's are, and
//! the learned denoiser in the demosaic's place when asked. What the
//! merge keeps is what a frame developed by the editor holds.
//!
//! The file holds what is over 1 after the white balance under
//! headroom, as a bracket's and a panorama's do: the merge's brightest
//! sample after the white balance ([`core_merge::headroom`]), its
//! samples stored divided by it and its tag saying the stops, so the
//! develop's clip after the gains does not bite and the finish shows
//! the file at the reference's exposure. That is what the
//! reconstruction keeps over the sensor's clip, and also a saturated
//! red or blue that nothing clipped but that its gain lifts over 1:
//! such a color now renders as the frame's own develop renders it,
//! where the file before cut it at 1. A stack with nothing over 1
//! after the white balance has no headroom and is the file it was. A
//! stack whose recipe
//! does not say its highlights were reconstructed (one made before they
//! were) is made again as it was made, clipped.
//!
//! The command line's `greycard stack` and the window's Merge to Focus
//! stack both run [`frames`]; the window, and the command line when it
//! writes a DNG, write it with [`write_dng`]. Neither has a stack of its
//! own. The file keeps a recipe, so it is made again (`merge::again`)
//! as a bracket and a panorama are: each frame blended where the
//! recipe put it ([`stack::stack_camera_placed`]), nothing fitted, each
//! read the short way first as well when the stack's prior speaks.
//!
//! No frame is held: each is read when the core's merge asks for it, so
//! a stack's memory is one frame's working set whatever the count
//! ([`wants`]).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use greycard_core::decode::RawMetadata;
use greycard_core::develop::ca::CaFit;
use greycard_core::dng::{Compression, LinearDngOptions, Merged, write_linear_merged_dng};
use greycard_core::merge::{self as core_merge, Geometry, Noise, Recipe, Tag};
use greycard_core::output::OnExists;
use greycard_core::raw::{RawFrame, Samples, SensorLayout};
use greycard_core::register::Transform;
use greycard_core::stack::{self, Reference, StackReport};
use greycard_core::{
    CameraImage, DemosaicMethod, DevelopSettings, HighlightMode, WhiteBalance, demosaic,
};
use greycard_edit::Edit;

use super::{CaFits, Files, LearnedSkipped, Progress, Step, hashes, hold, name_of, walk_frame};
use crate::stages::{CaFitRef, MergeCa, MergeInput};
use crate::walk::Models;

/// What goes after the first frame's name.
pub const SUFFIX: &str = " stack";

/// What a stack is asked. The default is what the window asks: the
/// middle frame, the core's selectivity and residual, the CA fitted
/// once, no learned denoiser, lossless samples, beside the first frame,
/// a name already there numbered.
#[derive(Debug, Clone)]
pub struct Options {
    /// The frame the others are put onto.
    pub reference: Reference,
    /// How far each band is taken from the frame with the most to say
    /// in it rather than averaged.
    pub selectivity: f32,
    /// The residual a link between two neighboring frames may have and
    /// still be believed.
    pub max_residual: f32,
    /// The DNG's samples stored uncompressed rather than as lossless
    /// JPEG.
    pub uncompressed: bool,
    /// Where the file goes; none for beside the first frame
    /// ([`default_output`]).
    pub out: Option<PathBuf>,
    /// What to do when that file is there already.
    pub on_exists: OnExists,
    /// The machine's available memory in bytes, in place of asking the
    /// system: for a test of the refusal.
    pub memory: Option<u64>,
    /// The learned denoiser in each frame's demosaic's place, as a
    /// bracket's; none unless asked.
    pub denoiser: Option<&'static greycard_ai::Model>,
    /// How the frames' CA is fitted.
    pub ca: CaFits,
    /// Each frame's place onto the reference as a recipe keeps it, none
    /// for a frame left out, for a stack made again: no frame is
    /// placed or fitted, each is blended where it was put. The
    /// reference is then `Reference::Index`.
    pub placed: Option<Vec<Option<Transform>>>,
    /// Whether each frame's highlights are reconstructed and what is
    /// over the clip kept under headroom; off only for a stack made
    /// again whose recipe says they were clipped.
    pub reconstruct: bool,
}

impl Default for Options {
    fn default() -> Self {
        let core = stack::Options::default();
        Self {
            reference: Reference::Middle,
            selectivity: core.selectivity,
            max_residual: core.max_residual,
            uncompressed: false,
            out: None,
            on_exists: OnExists::default(),
            memory: None,
            denoiser: None,
            ca: CaFits::Once,
            placed: None,
            reconstruct: true,
        }
    }
}

/// The frame of `count` a stack under `reference` puts the others on,
/// when that is known before the frames are read: for a caller that
/// wants the reference's edit first. None for the sharpest, which the
/// merge finds.
pub fn reference_of(count: usize, reference: Reference) -> Option<usize> {
    match reference {
        Reference::Middle => Some(count / 2),
        Reference::Index(i) => Some(i),
        Reference::Sharpest => None,
    }
}

/// What a stack holds a pixel of the frame at its peak, whatever the
/// number of frames: the core's accumulators and one frame's warp,
/// pyramids, and maps (87 bytes, counted by the allocator), and the
/// frame being decoded and walked beside them. The process's peak
/// resident set was 103 bytes a pixel at 24 megapixels, at 5 frames and
/// at 20, once the frames were walked to be blended (99 when they were
/// not); this is that and a tenth.
pub const BYTES_A_PIXEL: u64 = 115;

/// [`BYTES_A_PIXEL`] when each frame is walked with the learned
/// denoiser: the network's picture and the plain demosaic beside it,
/// the runtime, and its tiles. A stack of 24-megapixel frames made
/// again at the fast tier peaked at 169 to 171 bytes a pixel on the
/// CPU provider, which holds the network's working memory in the
/// process where a graphics card holds it on the card; this is that
/// and a tenth.
pub const BYTES_A_PIXEL_LEARNED: u64 = 190;

/// What a stack keeps of each frame across the merge: its tags (the
/// reference's are the file's) and its line of the report. No pixels.
pub const BYTES_A_FRAME: u64 = 1 << 20;

/// What a stack of `frames` frames of `pixels` pixels wants at its
/// peak: [`BYTES_A_PIXEL`] of one frame, or [`BYTES_A_PIXEL_LEARNED`]
/// when the frames are `learned` denoised, and [`BYTES_A_FRAME`] a
/// frame.
pub fn wants(frames: usize, pixels: usize, learned: bool) -> u64 {
    let a_pixel = if learned {
        BYTES_A_PIXEL_LEARNED
    } else {
        BYTES_A_PIXEL
    };
    (pixels as u64)
        .saturating_mul(a_pixel)
        .saturating_add((frames as u64).saturating_mul(BYTES_A_FRAME))
}

/// Whether a stack of `frames` frames of `pixels` pixels ([`wants`])
/// fits in `memory` bytes: refused when it does not, since a stack past
/// the machine's memory would take the process down with it. The
/// caller asks the memory available now, not the machine's whole. A
/// machine that will not say its memory is not asked.
pub fn room(frames: usize, pixels: usize, memory: Option<u64>, learned: bool) -> Result<()> {
    let Some(memory) = memory else {
        return Ok(());
    };
    let wants = wants(frames, pixels, learned);
    if wants > memory {
        let gb = |b: u64| b as f64 / 1e9;
        bail!(
            "this stack needs about {:.1} GB of memory and {:.1} GB is free: close other \
             programs and try again",
            gb(wants),
            gb(memory)
        );
    }
    Ok(())
}

/// `<first frame> stack.dng`, beside the first frame.
pub fn default_output(first: &Path) -> PathBuf {
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "stack".into());
    let name = format!("{stem}{SUFFIX}.dng");
    match first.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// The short develop `frame` takes to be placed: RCD on a Bayer
/// mosaic, Markesteijn's one pass on X-Trans, hot pixels repaired, no
/// CA correction, highlights clipped, stopped at the demosaic, which
/// leaves camera RGB with no white balance and no matrix on it.
///
/// Measured on a real 24-megapixel stack, against the frames placed
/// from their walks: RCD lands each well-linked frame within 0.04 px
/// at the corners of where the walk's fit puts it, at a residual the
/// same to the fourth digit, for 0.35 s a frame against the walk's
/// 0.65. Bilinear, what a stack placed from before, costs 0.10 s and
/// lands a quarter of a pixel off on one link, its fit stopping short
/// on the softer picture.
pub fn register_settings(frame: &RawFrame) -> DevelopSettings {
    let xtrans = matches!(&frame.layout, SensorLayout::Cfa(p) if p.is_xtrans());
    DevelopSettings {
        demosaic: if xtrans {
            DemosaicMethod::Markesteijn1
        } else {
            DemosaicMethod::Rcd
        },
        highlights: HighlightMode::Clip,
        chromatic_aberration: None,
        ..Default::default()
    }
}

/// The frames merged, not yet written.
pub struct Stacked {
    /// Camera RGB, the white balance divided back out, held to the
    /// brightest the frames' walks made (the clip, with nothing
    /// reconstructed over it).
    pub merged: CameraImage,
    /// What the file's samples are stored divided by: the merge's
    /// brightest sample after the white balance, 1 when nothing is over
    /// 1 after it (a saturated color its gain lifts over 1 asks for
    /// headroom as a reconstructed highlight does).
    pub headroom: f32,
    /// The reference frame, its samples let go: its tags are the
    /// file's.
    pub frame: RawFrame,
    pub metadata: RawMetadata,
    /// The reference's white balance: the merge is at it.
    pub balance: WhiteBalance,
    pub report: StackReport,
    /// Each frame's name, in the order given.
    pub names: Vec<String>,
    /// Seconds reading the frames, both passes; of those, the blending
    /// pass's walks; and merging them.
    pub developing: f64,
    pub walking: f64,
    pub merging: f64,
    /// The file's `NoiseProfile`, in its stored levels.
    pub noise_profile: [f64; 2],
    /// Our tag, with the recipe the merge was made by.
    pub tag: Tag,
    /// What the stack went without or did that it should not need,
    /// each in words, for the log.
    pub warnings: Vec<String>,
    /// Why frames that asked for a learned tier went without it.
    pub learned_skipped: Option<LearnedSkipped>,
}

/// What the blending pass keeps between its reads: the reference's CA
/// fit, made once, its noise, and what the walks said.
struct Walks {
    /// The reference's fit: none until it is walked, then what it made
    /// (none on a frame the correction does not measure).
    fit: Option<Option<Arc<CaFit>>>,
    noise: Option<Noise>,
    denoised: bool,
    skipped: Vec<Option<LearnedSkipped>>,
    warnings: Vec<String>,
    seconds: f64,
    /// The brightest sample of every frame blended, and never under 1:
    /// what the merge is held to.
    top: f32,
}

/// `files`, in focus order, merged under `options`, each walked under
/// `edit`'s demosaic and CA to be blended, `models` running the learned
/// denoiser when `options` asks. The frames are read as the core's
/// merge asks for them ([`stack::stack_camera_from`]), not held: each
/// is decoded and developed the short way to be placed, and decoded and
/// walked again to be blended, and nothing of it is kept between but
/// its tags. Each first read is said to `progress` as a frame, then the
/// placing, then each blend. With the places given (`options.placed`)
/// only the blends are read and said, unless the stack's prior speaks
/// (a selectivity above 0 and eight frames or more placed): then the
/// frames used are read the short way first, said as frames and the
/// placing, to gather its evidence again. A developed picture is refused: it is past
/// the white balance and the matrix. So is a stack the machine's
/// available memory will not hold ([`room`]), read off the first
/// frame's size before any other frame is decoded.
pub fn frames(
    files: &[PathBuf],
    edit: &Edit,
    options: &Options,
    models: &mut dyn Models,
    progress: &dyn Progress,
) -> Result<Stacked> {
    if files.len() < 2 {
        bail!("a stack wants at least two frames; got {}", files.len());
    }
    if let Some(file) = files
        .iter()
        .find(|f| greycard_core::picture::is_picture_path(f))
    {
        bail!(
            "{} is not a RAW file: a stack is merged in camera space, before white \
             balance and the matrix, and a developed picture is past both",
            file.display()
        );
    }
    let started = Instant::now();
    let of = files.len();
    let sources = Files(files);
    // Each frame's tags, from its first read: the reference's are the
    // file's, and the reference is the merge's to find.
    type Head = (RawFrame, RawMetadata, WhiteBalance);
    let heads: RefCell<Vec<Option<Head>>> = RefCell::new((0..of).map(|_| None).collect());
    let mut developing = 0.0f64;
    let mut checked = false;
    // Where the core's merge is: its blends come after every placing
    // read. The sharpest reference is found by a scoring pass, said
    // done by its own placing step, and is the first frame the chain
    // reads after it; any other is known now.
    let blending = Cell::new(false);
    let scored = Cell::new(false);
    let known = reference_of(of, options.reference);
    let mut chained: Option<usize> = None;
    let mut walks = Walks {
        fit: None,
        noise: None,
        denoised: false,
        skipped: Vec::new(),
        warnings: Vec::new(),
        seconds: 0.0,
        top: 1.0,
    };
    let learned = options.denoiser.is_some();
    // Made again: the reference's tags and white read first, as its
    // placing read would take them. Every read is a blend unless the
    // stack's prior is gathered again, whose reads come first and are
    // placing reads, as the first merge's were.
    if let Some(placed) = &options.placed {
        let Some(r) = known.filter(|_| placed.len() == of) else {
            bail!(
                "the recipe places {} frames and {of} were given, or names no reference",
                placed.len()
            );
        };
        let t = Instant::now();
        let file = &files[r];
        let (mut frame, metadata) = greycard_core::decode::decode_path_with_metadata(file)
            .with_context(|| format!("decoding {}", file.display()))?;
        let memory = options.memory.or_else(greycard_ai::ram::available);
        room(of, frame.width * frame.height, memory, learned)?;
        checked = true;
        // The white is the frame's own, whatever the demosaic.
        let quick = DevelopSettings {
            demosaic: DemosaicMethod::Bilinear,
            ..register_settings(&frame)
        };
        let white = demosaic(&frame, &quick)
            .with_context(|| format!("developing {}", file.display()))?
            .white_balance;
        frame.samples = Samples::U16(Vec::new());
        heads.borrow_mut()[r] = Some((frame, metadata, white));
        developing += t.elapsed().as_secs_f64();
    }
    let read = |k: usize| -> Result<CameraImage> {
        let t = Instant::now();
        if !blending.get() {
            let file = &files[k];
            let (mut frame, metadata) = greycard_core::decode::decode_path_with_metadata(file)
                .with_context(|| format!("decoding {}", file.display()))?;
            if !checked {
                let memory = options.memory.or_else(greycard_ai::ram::available);
                room(of, frame.width * frame.height, memory, learned)?;
                checked = true;
            }
            if chained.is_none() && (known.is_some() || scored.get()) {
                chained = Some(k);
            }
            let demosaiced = demosaic(&frame, &register_settings(&frame))
                .with_context(|| format!("developing {}", file.display()))?;
            let mut heads = heads.borrow_mut();
            if heads[k].is_none() {
                // The tags are read from the frame; its samples are let go.
                frame.samples = Samples::U16(Vec::new());
                heads[k] = Some((frame, metadata, demosaiced.white_balance));
            }
            developing += t.elapsed().as_secs_f64();
            return Ok(demosaiced.image);
        }
        let reference = known
            .or(chained)
            .context("the stack's reference was not read first")?;
        // Every frame at the reference's white, as a bracket's: only
        // the demosaic sees it.
        let white = heads.borrow()[reference]
            .as_ref()
            .map(|(_, _, b)| b.coefficients_f32().map(f64::from))
            .context("the reference's tags were not read")?;
        let input = |k: usize, ca: MergeCa, denoiser| MergeInput {
            white: (k != reference).then_some(white),
            reconstruct: options.reconstruct,
            denoiser,
            ca,
            lens: Vec::new(),
        };
        // One CA fit, the reference's, for every frame: frames that
        // differ in focus fit corrections that differ, and a merge that
        // takes each band from another frame would show it. Made from
        // the reference's prepare alone when another frame is blended
        // first, and not at all when its list would make none.
        if options.ca == CaFits::Once && walks.fit.is_none() && k != reference {
            let head = heads.borrow()[reference]
                .as_ref()
                .map(|(f, _, _)| f.clone())
                .context("the reference's tags were not read")?;
            let made = reference_fit(
                &sources,
                reference,
                &head,
                edit,
                input(reference, MergeCa::Make, None),
                models,
            )?;
            walks.fit = Some(made.as_ref().and_then(|(fit, _)| fit.clone()));
            if let Some((_, noise)) = made {
                walks.noise = Some(noise);
            }
        }
        let ca = match (options.ca, &walks.fit) {
            (CaFits::Off, _) => MergeCa::Off,
            (CaFits::PerFrame, _) => MergeCa::Own,
            (CaFits::Once, None) => MergeCa::Make,
            (CaFits::Once, Some(Some(fit))) => MergeCa::Take(CaFitRef(fit.clone())),
            // The reference made none: each frame as the reference was
            // asked, as a bracket's.
            (CaFits::Once, Some(None)) => MergeCa::Own,
        };
        let (f, _, _) = walk_frame(
            &sources,
            k,
            edit,
            input(k, ca, options.denoiser),
            None,
            models,
        )?;
        if k == reference {
            if options.ca == CaFits::Once && walks.fit.is_none() {
                walks.fit = Some(f.ca_fit.clone());
            }
            walks.noise = Some(f.noise);
        }
        if let Some(why) = &f.stood_in {
            walks
                .warnings
                .push(format!("the learned denoiser stood down: {why}"));
        }
        walks.denoised |= f.denoised;
        walks.skipped.push(f.learned_skipped);
        if options.reconstruct {
            use rayon::prelude::*;
            let brightest = f
                .rgb
                .par_iter()
                .copied()
                .filter(|v| v.is_finite())
                .reduce(|| 1.0, f32::max);
            walks.top = walks.top.max(brightest);
        }
        let elapsed = t.elapsed().as_secs_f64();
        developing += elapsed;
        walks.seconds += elapsed;
        Ok(CameraImage::from_data(f.width, f.height, f.rgb)?)
    };
    let said = |s: stack::Step| {
        progress.step(match s {
            stack::Step::Reading { index, of } => Step::Frame { index, of },
            stack::Step::Placing => {
                scored.set(true);
                Step::Placing
            }
            stack::Step::Blending { index, of } => {
                blending.set(true);
                Step::Blending { index, of }
            }
        })
    };

    let core = stack::Options {
        reference: options.reference,
        selectivity: options.selectivity,
        max_residual: options.max_residual,
        ..Default::default()
    };
    let (mut merged, report) = match (&options.placed, known) {
        (Some(placed), Some(r)) => stack::stack_camera_placed(r, placed, read, said, &core)?,
        _ => stack::stack_camera_from(of, read, said, &core)?,
    };
    let merging = started.elapsed().as_secs_f64() - developing;
    if known.or(chained) != Some(report.reference) {
        bail!(
            "the frames were blended at the white of frame {:?} and the merge's reference \
             is frame {}",
            known.or(chained),
            report.reference
        );
    }
    let warnings = walks.warnings;
    let placed: Vec<Option<Transform>> = report
        .frames
        .iter()
        .map(|f| f.used().then_some(f.transform))
        .collect();
    let (frame, metadata, balance) = heads
        .into_inner()
        .swap_remove(report.reference)
        .expect("every frame is read in the first pass");
    // Held to what the frames held: the pyramid's blend overshoots an
    // edge, and rings under black. With nothing reconstructed over the
    // clip, or the highlights clipped, that is the clip, as the file
    // stored a stack before it kept its highlights.
    let top = walks.top;
    {
        use rayon::prelude::*;
        merged
            .data
            .par_iter_mut()
            .for_each(|v| *v = v.clamp(0.0, top));
    }
    // The headroom a bracket's and a panorama's file take: the
    // brightest sample after the white balance, so neither the file's
    // white nor the develop's clip after the gains bites. Nothing over
    // 1 after the white balance keeps none; a stack made again clipped
    // keeps none, as before.
    let headroom = if options.reconstruct {
        core_merge::headroom(&merged, balance.coefficients_f32())
    } else {
        1.0
    };
    // The reference's noise, not less: where the frames are equally
    // sharp the blend averages them and the noise is less, but where
    // the detail is, each band comes from one frame and so does its
    // noise. The file's readers (the denoisers, the grain, the
    // sharpen's floor) want it right in the detail; over the flat
    // regions it overstates, which smooths what has nothing to lose.
    let noise = walks.noise.unwrap_or(Noise::ASSUMED);
    let noise_profile = core_merge::noise_profile(&[(1.0, noise)], headroom);
    let names: Vec<String> = files.iter().map(|f| name_of(f)).collect();
    let (heads, wholes) = hashes(&sources);
    let denoised = walks.denoised;
    let tag = Tag {
        kind: "stack".into(),
        frames: names.clone(),
        reference: report.reference,
        denoised,
        headroom_stops: headroom.log2(),
        lens_corrected: false,
        heads,
        wholes,
        unread: None,
        recipe: Some(Recipe {
            ca: options.ca.name().into(),
            // The tier that ran: one asked that stood down did not.
            denoise: options
                .denoiser
                .filter(|_| denoised)
                .and_then(greycard_ai::tier_of)
                .map(Into::into),
            noise: None,
            deghost: None,
            kernel: None,
            geometry: Geometry::Stack(core_merge::Stacked {
                selectivity: options.selectivity,
                max_residual: options.max_residual,
                placed,
                uncompressed: options.uncompressed,
                reconstructed: options.reconstruct,
            }),
        }),
    };
    log::info!(
        "stack: {of} frames read in {developing:.1} s, {:.1} s of it the blends' walks; \
         merged in {merging:.1} s; {:.2} stops of headroom",
        walks.seconds,
        headroom.log2()
    );
    Ok(Stacked {
        merged,
        headroom,
        frame,
        metadata,
        balance,
        report,
        names,
        developing,
        walking: walks.seconds,
        merging,
        noise_profile,
        tag,
        warnings,
        learned_skipped: LearnedSkipped::of_frames(walks.skipped),
    })
}

/// The CA fit and the noise frame `k` (`head` its tags) makes under
/// `input`, which asks it to make the fit: its list run through the
/// prepare and the fit and no further, so the fit is the one its whole
/// walk would make. None when the list makes no fit: the edit's CA off,
/// a mosaic the correction does not measure, a DNG whose warps correct
/// its own; then nothing is decoded.
fn reference_fit(
    sources: &Files<'_>,
    k: usize,
    head: &RawFrame,
    edit: &Edit,
    input: MergeInput,
    models: &mut dyn Models,
) -> Result<Option<(Option<Arc<CaFit>>, Noise)>> {
    use crate::stages::{Purpose, Stage, Step as ListStep, stages};
    use crate::walk::{Buf, Input, Kept, NoHold, plan, walk};
    let purpose = Purpose::MergeInput(input);
    let facts = Input::Raw(Arc::new(head.clone())).facts(0);
    let mut steps = stages(edit, &purpose, &facts, None);
    let Some(at) = steps
        .steps
        .iter()
        .position(|s| matches!(s, ListStep::Op(Stage::CaFit(_))))
    else {
        return Ok(None);
    };
    steps.steps.truncate(at + 1);
    let t = Instant::now();
    let (frame, _) = super::Sources::load(sources, k)?;
    let input = Input::Raw(Arc::new(frame));
    let places = plan(&steps, &purpose, false);
    let walked = walk(
        &steps,
        &places,
        &input,
        &mut Kept::new(),
        models,
        &mut None,
        &NoHold,
    )
    .map_err(|e| anyhow::anyhow!("{}: {e}", super::Sources::name(sources, k)))?;
    let Buf::Mosaic(prepared) = &walked.buf else {
        bail!("the reference's fit did not end at its mosaic");
    };
    let noise = prepared
        .noise
        .map_or(Noise::ASSUMED, |e| Noise::of_model(&e.model));
    log::info!(
        "stack: the reference's CA fit in {:.2} s",
        t.elapsed().as_secs_f64()
    );
    Ok(Some((walked.ca_fit, noise)))
}

/// `stacked` written as a linear DNG to `out`: the reference's color
/// tags and EXIF (the merge is in its geometry and at its white
/// balance), the samples as lossless JPEG unless `uncompressed`, its
/// `NoiseProfile` and our tag with the recipe, and a small sRGB
/// preview.
pub fn write_dng<W: std::io::Write + std::io::Seek>(
    out: W,
    stacked: &Stacked,
    uncompressed: bool,
) -> Result<()> {
    let preview = core_merge::preview(&stacked.merged, &stacked.balance, core_merge::PREVIEW_SIDE)?;
    let options = LinearDngOptions {
        compression: if uncompressed {
            Compression::Uncompressed
        } else {
            Compression::Lossless
        },
        metadata: Some(&stacked.metadata),
        preview: Some(&preview),
        software: Some(concat!("greycard ", env!("CARGO_PKG_VERSION"))),
        headroom: (stacked.headroom > 1.0).then_some(stacked.headroom),
    };
    write_linear_merged_dng(
        out,
        &stacked.frame,
        &stacked.merged,
        stacked.balance.coefficients_f32(),
        &options,
        &Merged {
            noise_profile: stacked.noise_profile,
            tag: &stacked.tag,
        },
    )?;
    Ok(())
}

/// What a stack written did.
#[derive(Debug, Clone)]
pub struct Report {
    pub out: PathBuf,
    pub stack: StackReport,
    pub names: Vec<String>,
    pub noise_profile: [f64; 2],
    /// The stops the samples were stored under the reference's scale
    /// by: 0 with nothing over 1 after the white balance.
    pub headroom_stops: f32,
    /// Whether the frames were denoised.
    pub denoised: bool,
    /// The seconds each part took, in order.
    pub seconds: Vec<(&'static str, f64)>,
    /// What the stack went without or did that it should not need, each
    /// in words.
    pub warnings: Vec<String>,
    /// Why frames that asked for a learned tier went without it.
    pub learned_skipped: Option<LearnedSkipped>,
}

/// The focus stack `files` merged and written as a linear DNG, beside
/// the first frame unless `options` says where. The name is held
/// before a frame is read, and the file written under the hidden folder
/// that holds it and renamed at the end, as every merge's is: a stack
/// cut short leaves no half file, and a frame is never written over.
pub fn stack(
    files: &[PathBuf],
    edit: &Edit,
    options: &Options,
    models: &mut dyn Models,
    progress: &dyn Progress,
) -> Result<Report> {
    if files.len() < 2 {
        bail!("a stack wants at least two frames; got {}", files.len());
    }
    let started = Instant::now();
    let out = match &options.out {
        Some(p) => p.clone(),
        None => default_output(&files[0]),
    };
    let reserved = hold(&Files(files), &out, options.on_exists)?;
    let stacked = frames(files, edit, options, models, progress)?;
    progress.step(Step::Writing);
    let t = Instant::now();
    let part = reserved.part();
    let written = (|| -> Result<()> {
        let file = std::fs::File::create(&part)?;
        let mut writer = std::io::BufWriter::new(file);
        write_dng(&mut writer, &stacked, options.uncompressed)?;
        use std::io::Write as _;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        Ok(())
    })();
    let out = reserved.out().to_path_buf();
    if let Err(e) = written {
        return Err(e.context(format!("writing {}", out.display())));
    }
    reserved.promote(&part)?;
    drop(reserved);
    let seconds = vec![
        ("place", stacked.developing - stacked.walking),
        ("walk", stacked.walking),
        ("merge", stacked.merging),
        ("write", t.elapsed().as_secs_f64()),
        ("total", started.elapsed().as_secs_f64()),
    ];
    Ok(Report {
        out,
        stack: stacked.report,
        names: stacked.names,
        noise_profile: stacked.noise_profile,
        headroom_stops: stacked.headroom.log2(),
        denoised: stacked.tag.denoised,
        seconds,
        warnings: stacked.warnings,
        learned_skipped: stacked.learned_skipped,
    })
}

/// The edit a stack's file starts with: the reference frame's, less
/// the learned denoiser and the highlight fill, which a file with no
/// mosaic cannot run. Its exposure stays: a stack is at the
/// reference's.
pub fn edit_for_stack(reference: &Edit) -> Edit {
    let mut edit = reference.clone();
    edit.noise.learned = greycard_edit::Learned::Off;
    edit.highlight_fill = None;
    edit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::NoModels;
    use greycard_core::merge::testing::{mosaic, mosaic_of, write_dng as write_raw};

    /// A stack's file starts without the highlight fill, as without
    /// the learned denoiser, and at the reference's exposure.
    #[test]
    fn a_stack_starts_without_the_highlight_fill() {
        let mut reference = Edit::default();
        reference.light.exposure = 0.5;
        reference.highlight_fill = Some("hl-1".into());
        let edit = edit_for_stack(&reference);
        assert_eq!(edit.highlight_fill, None);
        assert_eq!(edit.light.exposure, 0.5);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("greycard-focus-{name}-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `n` frames of one scene, as a focus stack's are of one subject:
    /// its brightest eighth over the clip.
    fn frames_of(dir: &Path, n: usize, seed: u64) -> Vec<PathBuf> {
        frames_at(dir, n, seed, 0.25)
    }

    /// [`frames_of`] with the scene times `gain`.
    fn frames_at(dir: &Path, n: usize, seed: u64, gain: f32) -> Vec<PathBuf> {
        let (w, h) = (192, 128);
        (0..n)
            .map(|k| {
                let p = dir.join(format!("IMG_{k}.dng"));
                write_raw(
                    &p,
                    mosaic(w, h, gain, Noise::ASSUMED, seed + k as u64),
                    w,
                    h,
                    (1, 100),
                    "Stack",
                );
                p
            })
            .collect()
    }

    fn stacked(files: &[PathBuf], options: &Options) -> Report {
        stack(
            files,
            &Edit::default(),
            options,
            &mut NoModels,
            &|_: Step| {},
        )
        .expect("a stack")
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|v| v.to_bits()).collect()
    }

    /// The develop a stack read its frames with before it walked them,
    /// both passes: bilinear, no CA, clipped.
    fn bilinear() -> DevelopSettings {
        DevelopSettings {
            demosaic: DemosaicMethod::Bilinear,
            highlights: HighlightMode::Clip,
            chromatic_aberration: None,
            ..Default::default()
        }
    }

    /// The stack is its steps done by hand, bit for bit: each frame
    /// placed from its short develop, and blended as the HDR merge's
    /// walk develops it, at the reference's white and with the
    /// reference's CA fit, its highlights reconstructed, held to the
    /// brightest the walks made and stored under the headroom its
    /// brightest balanced sample asks.
    #[test]
    fn the_stack_places_the_short_develop_and_blends_the_walk() {
        let dir = scratch("by-hand");
        let files = frames_of(&dir, 3, 40);
        let edit = Edit::default();
        let got = frames(
            &files,
            &edit,
            &Options::default(),
            &mut NoModels,
            &|_: Step| {},
        )
        .expect("a stack");
        let r = got.report.reference;
        assert_eq!(r, 1);
        assert_eq!(got.report.used(), 3, "{:#?}", got.report.frames);

        let quick: Vec<CameraImage> = files
            .iter()
            .map(|f| {
                let (frame, _) = greycard_core::decode::decode_path_with_metadata(f).unwrap();
                demosaic(&frame, &register_settings(&frame)).unwrap().image
            })
            .collect();
        let sources = Files(&files);
        let input = |k: usize, ca: MergeCa, white| MergeInput {
            white: (k != r).then_some(white),
            reconstruct: true,
            denoiser: None,
            ca,
            lens: Vec::new(),
        };
        let walk = |k: usize, ca: MergeCa, white| {
            walk_frame(&sources, k, &edit, input(k, ca, white), None, &mut NoModels)
                .unwrap()
                .0
        };
        let first = walk(r, MergeCa::Make, [1.0; 3]);
        let white = first.gains.map(f64::from);
        let ca = first
            .ca_fit
            .clone()
            .map_or(MergeCa::Own, |f| MergeCa::Take(CaFitRef(f)));
        let walked: Vec<CameraImage> = (0..3)
            .map(|k| {
                let f = walk(k, ca.clone(), white);
                CameraImage::from_data(f.width, f.height, f.rgb).unwrap()
            })
            .collect();
        let blending = Cell::new(false);
        let (mut merged, report) = stack::stack_camera_from(
            3,
            |k| {
                Ok::<_, anyhow::Error>(if blending.get() {
                    walked[k].clone()
                } else {
                    quick[k].clone()
                })
            },
            |s| {
                if matches!(s, stack::Step::Blending { .. }) {
                    blending.set(true)
                }
            },
            &stack::Options::default(),
        )
        .unwrap();
        assert_eq!(report, got.report);
        let top = walked
            .iter()
            .flat_map(|f| f.data.iter().copied())
            .fold(1.0f32, f32::max);
        assert!(top > 1.0, "the frames reconstruct nothing over the clip");
        for v in &mut merged.data {
            *v = v.clamp(0.0, top);
        }
        assert!(
            bits(&merged.data) == bits(&got.merged.data),
            "the stack is not its steps by hand"
        );
        let headroom = core_merge::headroom(&merged, first.gains);
        assert!(headroom > 1.0);
        assert_eq!(got.headroom.to_bits(), headroom.to_bits());
        assert_eq!(got.tag.headroom_stops, headroom.log2());
        // The walk is not the short develop: the merge of the short
        // develops alone, what a stack was, is another picture.
        let (short, _) = stack::stack_camera(&quick, &stack::Options::default()).unwrap();
        assert!(bits(&short.data) != bits(&got.merged.data));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The RMS of each channel of camera RGB `rgb` (`w` wide) against
    /// `truth`, inside a margin.
    fn rms(
        rgb: &[f32],
        w: usize,
        h: usize,
        truth: impl Fn(usize, usize, usize) -> f32,
    ) -> [f64; 3] {
        let m = 24;
        let mut sum = [0.0f64; 3];
        let mut n = 0.0;
        for y in m..h - m {
            for x in m..w - m {
                for (c, s) in sum.iter_mut().enumerate() {
                    let d = f64::from(rgb[(y * w + x) * 3 + c] - truth(x, y, c));
                    *s += d * d;
                }
                n += 1.0;
            }
        }
        sum.map(|s| (s / n).sqrt())
    }

    /// A stack of three frames of `texture`, 400 by 320, tinted, under
    /// a lens that magnifies red by `red` and blue by `blue`: how far
    /// each channel of the stack under each run's edit and CA lands
    /// from the texture, and how far the merge of the short develops
    /// alone (what a stack was before it walked its frames) lands.
    fn stack_against(
        name: &str,
        texture: fn(f32, f32) -> f32,
        red: f32,
        blue: f32,
        runs: &[(Edit, CaFits)],
    ) -> (Vec<[f64; 3]>, [f64; 3]) {
        let dir = scratch(name);
        let (w, h) = (400, 320);
        let tint = [0.55f32, 1.0, 0.7];
        let (cx, cy) = ((w - 1) as f32 / 2.0, (h - 1) as f32 / 2.0);
        let light = |x: usize, y: usize, c: usize| {
            let scale = [red, 1.0, blue][c];
            let sx = cx + (x as f32 - cx) / scale;
            let sy = cy + (y as f32 - cy) / scale;
            texture(sx, sy) * tint[c]
        };
        let quiet = Noise {
            shot: 1e-6,
            read: 1e-9,
        };
        let files: Vec<PathBuf> = (0..3)
            .map(|k| {
                let p = dir.join(format!("IMG_{k}.dng"));
                write_raw(
                    &p,
                    mosaic_of(w, h, 1.0, quiet, 60 + k as u64, light),
                    w,
                    h,
                    (1, 100),
                    "Stack",
                );
                p
            })
            .collect();
        let truth = |x: usize, y: usize, c: usize| texture(x as f32, y as f32) * tint[c];
        let got = runs
            .iter()
            .map(|(edit, ca)| {
                let options = Options {
                    ca: *ca,
                    ..Options::default()
                };
                let s =
                    frames(&files, edit, &options, &mut NoModels, &|_: Step| {}).expect("a stack");
                rms(&s.merged.data, w, h, truth)
            })
            .collect();
        let quick: Vec<CameraImage> = files
            .iter()
            .map(|f| {
                let (frame, _) = greycard_core::decode::decode_path_with_metadata(f).unwrap();
                demosaic(&frame, &bilinear()).unwrap().image
            })
            .collect();
        let (short, _) = stack::stack_camera(&quick, &stack::Options::default()).unwrap();
        greycard_core::testing::remove_dir_retry(&dir);
        (got, rms(&short.data, w, h, truth))
    }

    /// A stack of frames whose lens moves red out and blue in, a
    /// lateral CA of half a percent and four tenths (a pixel and more
    /// at the corners), comes out with the fringes taken out by the
    /// reference's one fit: red and blue under seven tenths as far from the
    /// scene as the short develop a stack read before left them, and
    /// as the same walk with the fit asked off leaves them.
    #[test]
    fn a_stack_corrects_its_frames_lateral_ca() {
        let smooth = |x: f32, y: f32| {
            0.25 + 0.12 * (x * 0.3).sin() * (y * 0.25).cos() + 0.06 * (x * 0.13 + y * 0.11).sin()
        };
        let (got, short) = stack_against(
            "ca",
            smooth,
            1.005,
            0.996,
            &[
                (Edit::default(), CaFits::Once),
                (Edit::default(), CaFits::Off),
            ],
        );
        let (once, off) = (got[0], got[1]);
        println!("once {once:?}\n off {off:?}\nshort {short:?}");
        for c in [0, 2] {
            assert!(once[c] < 0.7 * short[c], "{c}: {once:?} against {short:?}");
            assert!(once[c] < 0.7 * off[c], "{c}: {once:?} against {off:?}");
        }
    }

    /// A stack of a texture with detail near the mosaic's own scale
    /// keeps it as the editor's demosaic for the sensor does: every
    /// channel nearer the scene than the short develop's bilinear left
    /// it, and nearer than the walk under an edit that asks bilinear,
    /// which lands where the short develop does.
    #[test]
    fn a_stack_demosaics_its_frames_as_the_editor() {
        let fine = |x: f32, y: f32| {
            0.25 + 0.08 * (x * 1.1).sin() * (y * 0.9).cos()
                + 0.06 * (x * 0.11 + y * 0.07).sin()
                + 0.04 * ((x - y) * 0.031).cos()
        };
        let bilinear = Edit {
            demosaic: greycard_edit::Demosaic::Bilinear,
            ..Edit::default()
        };
        let (got, short) = stack_against(
            "demosaic",
            fine,
            1.0,
            1.0,
            &[(Edit::default(), CaFits::Once), (bilinear, CaFits::Once)],
        );
        let (editor, asked) = (got[0], got[1]);
        println!("editor {editor:?}\n asked {asked:?}\n short {short:?}");
        for c in 0..3 {
            assert!(
                editor[c] < 0.8 * short[c],
                "{c}: {editor:?} against {short:?}"
            );
            assert!(
                editor[c] < 0.8 * asked[c],
                "{c}: {editor:?} against {asked:?}"
            );
            assert!(
                (asked[c] / short[c] - 1.0).abs() < 0.2,
                "{c}: {asked:?} against {short:?}"
            );
        }
    }

    /// The written stack is a merge of ours: its tag says a stack, its
    /// reference, and the recipe (the CA fitted once, no tier, the
    /// core's options, each frame's transform), and its `NoiseProfile`
    /// is the reference's own noise as its walk measured it, at the
    /// headroom the file is stored under. The file develops at that
    /// noise, and its tag and its baseline say the headroom.
    #[test]
    fn a_stacks_file_carries_its_recipe_and_the_references_noise() {
        let dir = scratch("tag");
        let files = frames_of(&dir, 3, 70);
        let report = stacked(&files, &Options::default());
        let back = greycard_core::decode::decode_path(&report.out).unwrap();
        let tag = back.dng.merged.clone().expect("our tag");
        assert_eq!((tag.kind.as_str(), tag.reference), ("stack", 1));
        assert_eq!(tag.frames, ["IMG_0.dng", "IMG_1.dng", "IMG_2.dng"]);
        assert!(!tag.denoised);
        assert!(report.headroom_stops > 0.0);
        assert!((tag.headroom_stops - report.headroom_stops).abs() < 1e-4);
        assert_eq!(greycard_edit::baseline::headroom(&back), tag.headroom_stops);
        let baseline = back.dng.baseline_exposure.expect("a baseline");
        assert!((baseline - report.headroom_stops).abs() < 1e-4);
        assert_eq!(
            tag.heads,
            files
                .iter()
                .map(|f| Some(greycard_library::hash_file(f).unwrap()))
                .collect::<Vec<_>>()
        );
        let recipe = tag.recipe.expect("a recipe");
        assert_eq!((recipe.ca.as_str(), recipe.denoise), ("once", None));
        let Geometry::Stack(s) = recipe.geometry else {
            panic!("a stack's geometry");
        };
        let core = stack::Options::default();
        assert_eq!(
            (s.selectivity, s.max_residual),
            (core.selectivity, core.max_residual)
        );
        assert_eq!(
            s.placed,
            report
                .stack
                .frames
                .iter()
                .map(|f| Some(f.transform))
                .collect::<Vec<_>>()
        );
        assert!(s.reconstructed);

        let (r, _, _) = walk_frame(
            &Files(&files),
            1,
            &Edit::default(),
            MergeInput {
                white: None,
                reconstruct: true,
                denoiser: None,
                ca: MergeCa::Make,
                lens: Vec::new(),
            },
            None,
            &mut NoModels,
        )
        .unwrap();
        let want = core_merge::noise_profile(&[(1.0, r.noise)], report.headroom_stops.exp2());
        for (got, want) in report.noise_profile.iter().zip(&want) {
            assert!((got / want - 1.0).abs() < 1e-5, "{got} against {want}");
        }
        let noise = back.dng.noise_profile.expect("a noise profile");
        for c in 0..3 {
            assert!(
                (f64::from(noise.a[c]) / want[0] - 1.0).abs() < 1e-5
                    && (f64::from(noise.b[c]) / want[1] - 1.0).abs() < 1e-5,
                "{noise:?} against {want:?}"
            );
        }
        assert!(want[0] > 0.0 && want[1] > 0.0);
        let prepared =
            greycard_core::develop::prepare(&back, &DevelopSettings::default(), true).unwrap();
        assert_eq!(prepared.noise.expect("the file's noise").model, noise);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A stack made again from its recipe: nothing placed or fitted,
    /// each frame read once and blended where the recipe put it, so with
    /// no tier the same file to the sample, its samples stored
    /// uncompressed as they were; with one, its frames walked with the
    /// learned denoiser and the file saying so, the frames still where
    /// they were. The sharpest frame as the reference is found as the
    /// merge finds it.
    #[test]
    fn a_stack_made_again_lands_its_frames_where_they_were() {
        let dir = scratch("again");
        let files = frames_of(&dir, 3, 80);
        let report = stacked(
            &files,
            &Options {
                reference: Reference::Sharpest,
                uncompressed: true,
                ..Options::default()
            },
        );
        let samples = |p: &Path| {
            greycard_core::decode::decode_path(p)
                .unwrap()
                .samples
                .to_f32()
        };
        let before = samples(&report.out);
        let tag = |p: &Path| greycard_core::decode::merge_tag_path(p).unwrap().unwrap();
        let first = tag(&report.out);
        assert_eq!(first.reference, report.stack.reference);
        // Its highlights reconstructed, under headroom: made again, the
        // same.
        assert!(first.headroom_stops > 0.0);
        let size = std::fs::metadata(&report.out).unwrap().len();

        let heard = std::cell::RefCell::new(Vec::new());
        let super::super::Again::Stack(again) = super::super::again(
            &report.out,
            &super::super::Overrides::default(),
            &Edit::default(),
            &mut NoModels,
            &|s: Step| heard.borrow_mut().push(s),
        )
        .unwrap() else {
            panic!("a stack");
        };
        let mut want: Vec<Step> = (1..=3)
            .map(|index| Step::Blending { index, of: 3 })
            .collect();
        want.push(Step::Writing);
        assert_eq!(heard.into_inner(), want, "only the blends read");
        assert!(again.stack.frames.iter().all(|f| f.fit.is_none()));
        assert_eq!(std::fs::metadata(&report.out).unwrap().len(), size);
        assert_eq!(again.out, report.out);
        assert!(again.warnings.is_empty(), "{:?}", again.warnings);
        assert_eq!(tag(&report.out), first);
        assert!(bits(&samples(&report.out)) == bits(&before));

        let super::super::Again::Stack(denoised) = super::super::again(
            &report.out,
            &super::super::Overrides {
                denoise: Some(Some("fast".into())),
                out: None,
            },
            &Edit::default(),
            &mut crate::testing::PlainDenoiser,
            &|_: Step| {},
        )
        .unwrap() else {
            panic!("a stack");
        };
        assert!(denoised.denoised);
        assert!(denoised.warnings.is_empty(), "{:?}", denoised.warnings);
        let made = tag(&report.out);
        assert!(made.denoised);
        let recipe = made.recipe.unwrap();
        assert_eq!(recipe.denoise.as_deref(), Some("fast"));
        assert_eq!(Some(recipe.geometry), first.recipe.map(|r| r.geometry));
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.') || n.ends_with("-1.dng"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Where the green clipped and red and blue did not, the stack
    /// keeps what the reconstruction recovers over the clip: green a
    /// quarter as far from the scene as the clip leaves it (0.05 against
    /// 0.18 in the camera's units). The file holds it
    /// under the headroom its tag and its baseline say, to the 16-bit
    /// step.
    #[test]
    fn a_stack_keeps_its_reconstructed_highlights_over_the_clip() {
        let dir = scratch("over");
        let files = frames_of(&dir, 3, 20);
        let got = frames(
            &files,
            &Edit::default(),
            &Options::default(),
            &mut NoModels,
            &|_: Step| {},
        )
        .expect("a stack");
        let (w, h) = (got.merged.width, got.merged.height);
        let truth = |x: usize, y: usize, c: usize| {
            greycard_core::merge::testing::scene(x, y, c, w, h) * 0.25
        };
        // Inside a margin, and away from the patches' edges, where the
        // reconstruction's neighbors are of another reflectance.
        let (mut kept, mut clipped, mut n) = (0.0f64, 0.0f64, 0usize);
        for y in 8..h - 8 {
            for x in 8..w - 8 {
                let g = truth(x, y, 1);
                let edge = (x % 23) < 2 || (x % 23) > 20 || (y % 19) < 2 || (y % 19) > 16;
                if edge || !(1.05..1.3).contains(&g) || truth(x, y, 2) > 0.9 {
                    continue;
                }
                let v = got.merged.data[(y * w + x) * 3 + 1];
                kept += f64::from(v - g).powi(2);
                clipped += f64::from(1.0 - g).powi(2);
                n += 1;
            }
        }
        assert!(n > 200, "{n} pixels");
        let (kept, clipped) = ((kept / n as f64).sqrt(), (clipped / n as f64).sqrt());
        println!("{n} px: kept {kept:.4}, clipped {clipped:.4}");
        assert!(
            kept < 0.5 * clipped,
            "the stack's green is {kept:.3} from the scene, the clip's {clipped:.3}"
        );
        assert!(got.merged.data.iter().any(|&v| v > 1.05));

        let out = dir.join("over.dng");
        {
            let mut file = std::fs::File::create(&out).unwrap();
            write_dng(&mut file, &got, true).unwrap();
        }
        let back = greycard_core::decode::decode_path(&out).unwrap();
        let stops = got.headroom.log2();
        assert!(stops > 0.0);
        let baseline = back.dng.baseline_exposure.expect("a baseline");
        assert!((baseline - stops).abs() < 1e-4, "{baseline} {stops}");
        let tag = back.dng.merged.as_ref().expect("our tag");
        assert!((tag.headroom_stops - stops).abs() < 1e-4);
        let step = got.headroom / 65535.0;
        for (s, v) in back.samples.to_f32().iter().zip(&got.merged.data) {
            assert!((s / 65535.0 * got.headroom - v).abs() <= step, "{s} {v}");
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A stack with nothing over 1 after the white balance (nothing near
    /// the sensor's clip, no color its gain lifts over 1) reconstructs
    /// nothing, keeps no headroom, and stores the samples it stored with
    /// its highlights clipped, to the bit.
    #[test]
    fn a_stack_with_nothing_over_white_is_the_file_it_was() {
        let dir = scratch("under");
        let files = frames_at(&dir, 3, 30, 1.0 / 64.0);
        let made = |reconstruct: bool, name: &str| {
            stacked(
                &files,
                &Options {
                    reconstruct,
                    uncompressed: true,
                    out: Some(dir.join(name)),
                    ..Options::default()
                },
            )
        };
        let kept = made(true, "kept.dng");
        let clipped = made(false, "clipped.dng");
        assert_eq!(kept.headroom_stops, 0.0);
        let read = |p: &Path| greycard_core::decode::decode_path(p).unwrap();
        let (a, b) = (read(&kept.out), read(&clipped.out));
        assert!(bits(&a.samples.to_f32()) == bits(&b.samples.to_f32()));
        assert_eq!(a.dng.baseline_exposure, None);
        assert_eq!(b.dng.baseline_exposure, None);
        let (ta, tb) = (a.dng.merged.unwrap(), b.dng.merged.unwrap());
        assert_eq!((ta.headroom_stops, tb.headroom_stops), (0.0, 0.0));
        assert_eq!(kept.noise_profile, clipped.noise_profile);
        let reconstructed =
            |t: &greycard_core::merge::Tag| match &t.recipe.as_ref().unwrap().geometry {
                Geometry::Stack(s) => s.reconstructed,
                _ => panic!("a stack"),
            };
        assert!(reconstructed(&ta) && !reconstructed(&tb));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A saturated red that nothing clipped but that its gain lifts over
    /// 1 asks for headroom: the frames reconstruct nothing (the merge is
    /// the clipped stack's, to the bit), and the file keeps the red,
    /// which the develop cut at 1 when the stack stored it without
    /// headroom.
    #[test]
    fn a_color_over_white_after_its_gain_keeps_its_headroom() {
        let dir = scratch("red");
        let (w, h) = (192, 128);
        // Red at 0.7 of the sensor's clip under a red gain of 2 (the
        // fixture's as-shot white): 1.4 after the white balance.
        let light = |x: usize, y: usize, c: usize| {
            let t = 0.9 + 0.1 * ((x as f32 * 0.6).sin() * (y as f32 * 0.4).cos());
            let red = (48..144).contains(&x) && (32..96).contains(&y);
            t * if red {
                [0.7, 0.2, 0.15][c]
            } else {
                [0.15, 0.3, 0.2][c]
            }
        };
        let files: Vec<PathBuf> = (0..3)
            .map(|k| {
                let p = dir.join(format!("IMG_{k}.dng"));
                write_raw(
                    &p,
                    mosaic_of(w, h, 1.0, Noise::ASSUMED, 10 + k as u64, light),
                    w,
                    h,
                    (1, 100),
                    "Stack",
                );
                p
            })
            .collect();
        let merged = |reconstruct: bool| {
            frames(
                &files,
                &Edit::default(),
                &Options {
                    reconstruct,
                    ..Options::default()
                },
                &mut NoModels,
                &|_: Step| {},
            )
            .expect("a stack")
        };
        let (kept, clipped) = (merged(true), merged(false));
        assert!(bits(&kept.merged.data) == bits(&clipped.merged.data));
        assert_eq!(clipped.headroom, 1.0);
        assert!(kept.headroom > 1.3, "{}", kept.headroom);
        // What the develop reads of each file, at the scale the finish
        // shows it: the brightest balanced sample.
        let developed = |s: &Stacked, name: &str| {
            let out = dir.join(name);
            {
                let mut file = std::fs::File::create(&out).unwrap();
                write_dng(&mut file, s, true).unwrap();
            }
            let back = greycard_core::decode::decode_path(&out).unwrap();
            let shown = greycard_edit::baseline::headroom(&back).exp2();
            let prepared =
                greycard_core::develop::prepare(&back, &DevelopSettings::default(), false).unwrap();
            prepared.samples.iter().copied().fold(0.0f32, f32::max) * shown
        };
        let (new, old) = (
            developed(&kept, "kept.dng"),
            developed(&clipped, "clipped.dng"),
        );
        assert!(old <= 1.0 + 1e-4, "the clipped file's red reads {old}");
        assert!(new > 1.3, "the file with headroom reads {new}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A stack whose recipe does not say its highlights were
    /// reconstructed, as one made before they were, is made again as it
    /// was made: clipped, no headroom, the same samples. Made fresh,
    /// the same frames keep their highlights.
    #[test]
    fn a_clipped_stack_made_again_stays_clipped() {
        let dir = scratch("again-clipped");
        let files = frames_of(&dir, 3, 85);
        let report = stacked(
            &files,
            &Options {
                reconstruct: false,
                uncompressed: true,
                ..Options::default()
            },
        );
        let read = |p: &Path| greycard_core::decode::decode_path(p).unwrap();
        let before = read(&report.out);
        let first = before.dng.merged.clone().unwrap();
        assert_eq!(first.headroom_stops, 0.0);
        assert!(
            !String::from_utf8_lossy(&first.to_bytes()).contains("highlights="),
            "a clipped stack's recipe says nothing of its highlights, as before"
        );
        let super::super::Again::Stack(again) = super::super::again(
            &report.out,
            &super::super::Overrides::default(),
            &Edit::default(),
            &mut NoModels,
            &|_: Step| {},
        )
        .unwrap() else {
            panic!("a stack");
        };
        assert_eq!(again.headroom_stops, 0.0);
        let after = read(&report.out);
        assert_eq!(after.dng.merged, Some(first));
        assert!(bits(&after.samples.to_f32()) == bits(&before.samples.to_f32()));
        let fresh = stacked(
            &files,
            &Options {
                out: Some(dir.join("fresh.dng")),
                ..Options::default()
            },
        );
        assert!(fresh.headroom_stops > 0.0);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A stack of nine, where the stack's prior speaks, made again from
    /// its recipe: the prior gathered again from the short reads, the
    /// frames blended with their highlights reconstructed, the same
    /// file to the sample, under the same headroom.
    #[test]
    fn a_stack_of_nine_made_again_under_headroom_is_the_same_file() {
        let dir = scratch("again-nine");
        let files = frames_of(&dir, 9, 120);
        let report = stacked(
            &files,
            &Options {
                uncompressed: true,
                ..Options::default()
            },
        );
        assert!(report.stack.used() >= 8, "{:#?}", report.stack.frames);
        assert!(report.headroom_stops > 0.0);
        let read = |p: &Path| greycard_core::decode::decode_path(p).unwrap();
        let before = read(&report.out);
        let heard = std::cell::RefCell::new(Vec::new());
        let super::super::Again::Stack(again) = super::super::again(
            &report.out,
            &super::super::Overrides::default(),
            &Edit::default(),
            &mut NoModels,
            &|s: Step| heard.borrow_mut().push(s),
        )
        .unwrap() else {
            panic!("a stack");
        };
        assert!(
            heard.borrow().contains(&Step::Placing),
            "the prior is gathered again"
        );
        assert_eq!(again.headroom_stops, report.headroom_stops);
        let after = read(&report.out);
        assert_eq!(after.dng.merged, before.dng.merged);
        assert!(bits(&after.samples.to_f32()) == bits(&before.samples.to_f32()));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The reference's CA fit is made from its prepare alone, the fit
    /// its whole walk makes; on a frame the correction does not
    /// measure (here one with no mosaic) there is none, and nothing is
    /// decoded.
    #[test]
    fn the_references_fit_is_its_walks_or_none() {
        let dir = scratch("fit");
        let files = frames_of(&dir, 1, 95);
        let (mut head, _) = greycard_core::decode::decode_path_with_metadata(&files[0]).unwrap();
        head.samples = Samples::U16(Vec::new());
        let input = || MergeInput {
            white: None,
            reconstruct: false,
            denoiser: None,
            ca: MergeCa::Make,
            lens: Vec::new(),
        };
        let sources = Files(&files);
        let (fit, noise) =
            reference_fit(&sources, 0, &head, &Edit::default(), input(), &mut NoModels)
                .unwrap()
                .expect("a Bayer mosaic is measured");
        let (whole, _, _) =
            walk_frame(&sources, 0, &Edit::default(), input(), None, &mut NoModels).unwrap();
        assert_eq!(format!("{fit:?}"), format!("{:?}", whole.ca_fit));
        assert_eq!(noise, whole.noise);
        head.layout = SensorLayout::Linear;
        let gone = dir.join("gone.dng");
        std::fs::rename(&files[0], &gone).unwrap();
        assert!(
            reference_fit(&sources, 0, &head, &Edit::default(), input(), &mut NoModels)
                .unwrap()
                .is_none()
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The placing develop is a fast demosaic each mosaic takes: RCD on
    /// Bayer, Markesteijn's one pass on X-Trans, which has no RCD.
    #[test]
    fn a_frame_is_placed_by_its_mosaics_fast_demosaic() {
        let dir = scratch("placing");
        let files = frames_of(&dir, 1, 90);
        let (mut frame, _) = greycard_core::decode::decode_path_with_metadata(&files[0]).unwrap();
        assert_eq!(register_settings(&frame).demosaic, DemosaicMethod::Rcd);
        frame.layout = SensorLayout::Cfa(greycard_core::raw::CfaPattern::xtrans());
        assert_eq!(
            register_settings(&frame).demosaic,
            DemosaicMethod::Markesteijn1
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Three frames, each read once to be placed and again to be
    /// blended, said as frames, the placing, and the blends.
    #[test]
    fn a_stack_reads_its_frames_twice_and_says_so() {
        let dir = scratch("passes");
        let files = frames_of(&dir, 3, 50);
        let heard = std::cell::RefCell::new(Vec::new());
        let stacked = frames(
            &files,
            &Edit::default(),
            &Options::default(),
            &mut NoModels,
            &|s: Step| heard.borrow_mut().push(s),
        )
        .expect("a stack");
        let used = stacked.report.used();
        let mut want: Vec<Step> = (1..=3).map(|index| Step::Frame { index, of: 3 }).collect();
        want.push(Step::Placing);
        want.extend((1..=used).map(|index| Step::Blending { index, of: used }));
        assert_eq!(*heard.borrow(), want);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A stack cut short between its passes (here a frame gone before
    /// it is read again) fails, and leaves neither a file nor its
    /// hidden folder: nothing is spilled, so nothing is left.
    #[test]
    fn a_stack_cut_short_between_its_passes_leaves_nothing() {
        let dir = scratch("cut");
        let files = frames_of(&dir, 2, 40);
        let gone = dir.join("gone.dng");
        let e = stack(
            &files,
            &Edit::default(),
            &Options::default(),
            &mut NoModels,
            &|s: Step| {
                if s == Step::Placing {
                    std::fs::rename(&files[1], &gone).unwrap();
                }
            },
        )
        .expect_err("cut short");
        assert!(format!("{e:#}").contains("IMG_1.dng"), "{e:#}");
        let mut left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["IMG_0.dng", "gone.dng"]);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A name there already is numbered, and a frame named as the
    /// output is refused before anything is read.
    #[test]
    fn a_stack_is_numbered_and_never_written_over_a_frame() {
        let dir = scratch("names");
        let files = frames_of(&dir, 2, 40);
        std::fs::write(dir.join("IMG_0 stack.dng"), b"there").unwrap();
        let report = stacked(&files, &Options::default());
        assert_eq!(report.out, dir.join("IMG_0 stack-1.dng"));
        assert_eq!(
            std::fs::read(dir.join("IMG_0 stack.dng")).unwrap(),
            b"there"
        );
        let e = stack(
            &files,
            &Edit::default(),
            &Options {
                out: Some(files[1].clone()),
                on_exists: OnExists::Overwrite,
                ..Options::default()
            },
            &mut NoModels,
            &|_: Step| {},
        )
        .expect_err("a frame refused");
        assert!(e.to_string().contains("one of the frames"), "{e}");
        let one = stack(
            &files[..1],
            &Edit::default(),
            &Options::default(),
            &mut NoModels,
            &|_: Step| {},
        )
        .expect_err("one frame");
        assert!(one.to_string().contains("at least two"), "{one}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A stack past the machine's memory is refused before a second
    /// frame is read, and nothing is written.
    #[test]
    fn a_stack_past_the_memory_is_refused() {
        // The frames read in passes: the count is a megabyte a frame of
        // tags, and the size is what decides. A hundred frames of 24
        // and of 60 megapixels fit in 16 GB; one frame's working set
        // past what is free does not.
        assert_eq!(wants(21, 24_000_000, false), 2_760_000_000 + 21 * (1 << 20));
        assert_eq!(
            wants(100, 24_000_000, false) - wants(2, 24_000_000, false),
            98 << 20
        );
        assert_eq!(
            wants(5, 24_000_000, true) - wants(5, 24_000_000, false),
            24_000_000 * (BYTES_A_PIXEL_LEARNED - BYTES_A_PIXEL)
        );
        assert!(room(100, 24_000_000, Some(16_000_000_000), false).is_ok());
        assert!(room(100, 60_000_000, Some(16_000_000_000), false).is_ok());
        assert!(room(2, 24_000_000, Some(2_000_000_000), false).is_err());
        assert!(room(50, 45_000_000, None, true).is_ok());
        let e = room(21, 24_000_000, Some(1_000_000_000), false).unwrap_err();
        assert_eq!(
            e.to_string(),
            "this stack needs about 2.8 GB of memory and 1.0 GB is free: close other \
             programs and try again"
        );
        let dir = scratch("memory");
        let files = frames_of(&dir, 2, 40);
        let heard = std::cell::RefCell::new(Vec::new());
        let e = stack(
            &files,
            &Edit::default(),
            &Options {
                memory: Some(1_000_000),
                ..Options::default()
            },
            &mut NoModels,
            &|s: Step| heard.borrow_mut().push(s),
        )
        .expect_err("refused");
        assert!(e.to_string().contains("close other programs"), "{e}");
        assert_eq!(*heard.borrow(), [Step::Frame { index: 1, of: 2 }]);
        assert!(!dir.join("IMG_0 stack.dng").exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The area two discs of radii `r1` and `r2` share, `d` apart.
    fn lens_area(d: f64, r1: f64, r2: f64) -> f64 {
        use std::f64::consts::PI;
        if d >= r1 + r2 {
            return 0.0;
        }
        if d <= (r1 - r2).abs() {
            return PI * r1.min(r2).powi(2);
        }
        let a = ((d * d + r1 * r1 - r2 * r2) / (2.0 * d * r1)).clamp(-1.0, 1.0);
        let b = ((d * d + r2 * r2 - r1 * r1) / (2.0 * d * r2)).clamp(-1.0, 1.0);
        let k = ((-d + r1 + r2) * (d + r1 - r2) * (d - r1 + r2) * (d + r1 + r2)).max(0.0);
        r1 * r1 * a.acos() + r2 * r2 * b.acos() - 0.5 * k.sqrt()
    }

    /// A focus stack of `n` frames at one exposure: a textured ground
    /// whose depth runs across the frame, and in the middle a blown
    /// specular, a disc of `r0` px at `peak` times the clip, at the
    /// depth frame `n / 2` is focused on. Frame `k` sees the ground
    /// under a Gaussian of 0.7 px a step of focus from it, and the
    /// specular through a pillbox of 2.5 px a step: each frame's blown
    /// area is larger and its reconstruction its own. The light of frame
    /// `k` at a pixel and channel, in the sensor's units.
    fn specular_scene(
        k: usize,
        n: usize,
        (w, h): (usize, usize),
        r0: f64,
        peak: f64,
    ) -> impl Fn(usize, usize, usize) -> f32 {
        let focus = (n / 2) as f64;
        let steps = (k as f64 - focus).abs();
        let b = (2.5 * steps).max(0.35);
        let (cx, cy) = ((w / 2) as f64, (h / 2) as f64);
        // Periods of 3 to 21 px, each its own way.
        let waves: [(f64, f64, f64); 5] = [
            (3.1, 0.3, 0.0),
            (5.2, 1.9, 1.1),
            (8.3, 0.9, 2.3),
            (13.0, 2.6, 0.7),
            (21.0, 1.4, 1.9),
        ];
        move |x, y, c| {
            let (fx, fy) = (x as f64, y as f64);
            let depth = (n - 1) as f64 * fx / (w - 1) as f64;
            let sigma = 0.7 * (k as f64 - depth).abs();
            let mut t = 0.5;
            for (period, angle, phase) in waves {
                let f = 1.0 / period;
                let fade = (-2.0 * std::f64::consts::PI.powi(2) * sigma * sigma * f * f).exp();
                let u = fx * angle.cos() + fy * angle.sin();
                t += 0.08 * fade * (std::f64::consts::TAU * f * u + phase).sin();
            }
            let ground = 0.3 * t * [0.55, 1.0, 0.7][c];
            let r = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let spot = peak * lens_area(r, r0, b) / (std::f64::consts::PI * b * b);
            (ground + spot * [0.9, 1.0, 0.8][c]) as f32
        }
    }

    /// Balanced luminance of camera RGB `rgb` at a pixel.
    fn balanced_luma(rgb: &[f32], i: usize, gains: [f32; 3]) -> f64 {
        let p = &rgb[i * 3..i * 3 + 3];
        f64::from(0.25 * p[0] * gains[0] + 0.5 * p[1] * gains[1] + 0.25 * p[2] * gains[2])
    }

    /// The mean of `rgb`'s balanced luminance over rings 1 px wide
    /// about `(cx, cy)`, out to `to`.
    fn radial(rgb: &[f32], w: usize, h: usize, gains: [f32; 3], to: usize) -> Vec<f64> {
        let (cx, cy) = ((w / 2) as f64, (h / 2) as f64);
        let mut sum = vec![0.0f64; to];
        let mut n = vec![0usize; to];
        for y in 0..h {
            for x in 0..w {
                let r = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt() as usize;
                if r < to {
                    sum[r] += balanced_luma(rgb, y * w + x, gains);
                    n[r] += 1;
                }
            }
        }
        sum.iter()
            .zip(&n)
            .map(|(s, &n)| s / n.max(1) as f64)
            .collect()
    }

    /// Where a radial profile leaves 1 after the white balance: the
    /// blown disc's radius.
    fn blown_radius(profile: &[f64]) -> usize {
        profile.iter().rposition(|&v| v >= 1.0).map_or(0, |r| r + 1)
    }

    /// The radii, inside `to`, at which the log of a profile falls
    /// fastest: its edges, the steepest first, each a local maximum of
    /// the slope at least a tenth of a stop a pixel.
    fn edges(profile: &[f64], to: usize) -> Vec<(usize, f64)> {
        let slope: Vec<f64> = profile
            .windows(2)
            .map(|p| (p[0].max(1e-6) / p[1].max(1e-6)).log2())
            .collect();
        let mut out: Vec<(usize, f64)> = (1..slope.len().min(to) - 1)
            .filter(|&r| slope[r] >= 0.1 && slope[r] >= slope[r - 1] && slope[r] >= slope[r + 1])
            .map(|r| (r, slope[r]))
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    /// A crop of camera RGB `rgb` about the middle, balanced, at
    /// `stops` of exposure, clipped and gamma'd for a look.
    fn crop_png(
        rgb: &[f32],
        w: usize,
        h: usize,
        gains: [f32; 3],
        side: usize,
        stops: f32,
    ) -> image::RgbImage {
        let (x0, y0) = (w / 2 - side / 2, h / 2 - side / 2);
        let scale = stops.exp2();
        image::RgbImage::from_fn(side as u32, side as u32, |x, y| {
            let i = (y0 + y as usize) * w + x0 + x as usize;
            let px = |c: usize| {
                let v = (rgb[i * 3 + c] * gains[c] * scale).clamp(0.0, 1.0);
                (v.powf(1.0 / 2.2) * 255.0).round() as u8
            };
            image::Rgb([px(0), px(1), px(2)])
        })
    }

    /// A measurement, not a check: a constant-exposure stack of nine
    /// with a blown specular whose defocus grows away from its focused
    /// frame, merged clipped (as before), with its highlights
    /// reconstructed (now), and at a selectivity of 0, against the
    /// focused frame's own walk. Prints the blown disc's radius, the
    /// edges of each radial profile, the dark halo outside the disc,
    /// and which frame each ring of the merge is nearest; with
    /// `GREYCARD_SYNTHETIC_OUT` set, writes crops there.
    #[test]
    #[ignore = "a measurement: a blown specular through the stack"]
    fn synthetic_blown_specular_stack() {
        let dir = scratch("specular");
        let (w, h, n) = (320usize, 240usize, 9usize);
        let (r0, peak) = (8.0, 6.0);
        let files: Vec<PathBuf> = (0..n)
            .map(|k| {
                let p = dir.join(format!("IMG_{k}.dng"));
                let light = specular_scene(k, n, (w, h), r0, peak);
                write_raw(
                    &p,
                    mosaic_of(w, h, 1.0, Noise::ASSUMED, 300 + k as u64, light),
                    w,
                    h,
                    (1, 100),
                    "Stack",
                );
                p
            })
            .collect();
        let run = |reconstruct: bool, selectivity: f32| {
            frames(
                &files,
                &Edit::default(),
                &Options {
                    reconstruct,
                    selectivity,
                    ..Options::default()
                },
                &mut NoModels,
                &|_: Step| {},
            )
            .expect("a stack")
        };
        let selectivity = Options::default().selectivity;
        let old = run(false, selectivity);
        let new = run(true, selectivity);
        let flat = run(true, 0.0);
        assert_eq!(new.report.used(), n, "{:#?}", new.report.frames);
        let gains = new.balance.coefficients_f32();
        let reference = new.report.reference;
        let white = gains.map(f64::from);
        let sources = Files(&files);
        let walk = |k: usize, reconstruct: bool| {
            walk_frame(
                &sources,
                k,
                &Edit::default(),
                MergeInput {
                    white: (k != reference).then_some(white),
                    reconstruct,
                    denoiser: None,
                    ca: MergeCa::Off,
                    lens: Vec::new(),
                },
                None,
                &mut NoModels,
            )
            .unwrap()
            .0
            .rgb
        };
        let focused = walk(n / 2, true);
        let focused_clipped: Vec<f32> = walk(n / 2, false).iter().map(|v| v.min(1.0)).collect();
        let each: Vec<Vec<f32>> = (0..n).map(|k| walk(k, true)).collect();
        let to = 60;
        let profile = |rgb: &[f32]| radial(rgb, w, h, gains, to);
        let rows: Vec<(&str, Vec<f64>)> = vec![
            ("clipped merge (before)", profile(&old.merged.data)),
            ("reconstructed merge (now)", profile(&new.merged.data)),
            ("reconstructed, selectivity 0", profile(&flat.merged.data)),
            ("focused frame, reconstructed", profile(&focused)),
            ("focused frame, clipped", profile(&focused_clipped)),
        ];
        let frames_profiles: Vec<Vec<f64>> = each.iter().map(|f| profile(f)).collect();
        let ground = |p: &[f64]| p[45..to].iter().sum::<f64>() / (to - 45) as f64;
        println!(
            "headroom: before {:.2} stops, now {:.2} stops",
            old.headroom.log2(),
            new.headroom.log2()
        );
        println!(
            "| result | blown radius px | peak | edges (radius: stops/px) | darkest ring / ground |"
        );
        println!("|---|---|---|---|---|");
        for (name, p) in &rows {
            let r = blown_radius(p);
            let g = ground(p);
            let dark = p[r.max(1)..40]
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            let e: Vec<String> = edges(p, 40)
                .iter()
                .take(4)
                .map(|(r, s)| format!("{r}: {s:.2}"))
                .collect();
            println!(
                "| {name} | {r} | {:.2} | {} | {:.3} |",
                p[0],
                e.join(", "),
                dark / g
            );
        }
        println!(
            "frames' blown radii: {:?}",
            frames_profiles
                .iter()
                .map(|p| blown_radius(p))
                .collect::<Vec<_>>()
        );
        let nearest = |p: &[f64], r: usize| {
            (0..n)
                .min_by(|&a, &b| {
                    (frames_profiles[a][r] - p[r])
                        .abs()
                        .total_cmp(&(frames_profiles[b][r] - p[r]).abs())
                })
                .unwrap()
        };
        for (name, p) in rows.iter().take(3) {
            let line: Vec<String> = (0..30)
                .step_by(2)
                .map(|r| format!("{r}:{}", nearest(p, r)))
                .collect();
            println!("nearest frame by radius, {name}: {}", line.join(" "));
        }
        for (name, p) in rows.iter().take(3) {
            let diff: Vec<String> = (0..30)
                .step_by(2)
                .map(|r| format!("{r}:{:+.2}", p[r] - rows[3].1[r]))
                .collect();
            println!("against the focused frame, {name}: {}", diff.join(" "));
        }
        if let Ok(out) = std::env::var("GREYCARD_SYNTHETIC_OUT") {
            let out = PathBuf::from(out);
            std::fs::create_dir_all(&out).unwrap();
            let side = 112;
            let shots: [(&str, &[f32]); 4] = [
                ("clipped-merge", &old.merged.data),
                ("reconstructed-merge", &new.merged.data),
                ("reconstructed-selectivity-0", &flat.merged.data),
                ("focused-frame", &focused),
            ];
            for stops in [0.0f32, -3.0] {
                let mut sheet = image::RgbImage::new(side as u32 * 4 + 6, side as u32);
                for (i, (_, rgb)) in shots.iter().enumerate() {
                    let c = crop_png(rgb, w, h, gains, side, stops);
                    image::imageops::replace(&mut sheet, &c, (i * (side + 2)) as i64, 0);
                }
                let name = format!(
                    "synthetic-clipped-reconstructed-sel0-focused-{}ev.png",
                    stops as i32
                );
                sheet.save(out.join(name)).unwrap();
            }
            for k in [0, 2, n / 2] {
                crop_png(&each[k], w, h, gains, side, -3.0)
                    .save(out.join(format!("synthetic-frame-{k}--3ev.png")))
                    .unwrap();
            }
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
