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
//! highlights clipped as a bracket's frames but its darkest are, and
//! the learned denoiser in the demosaic's place when asked. What the
//! merge keeps is what a frame developed by the editor holds, up to the
//! reference's clip.
//!
//! The highlights are not reconstructed: what a reconstruction keeps
//! over the clip a file can hold only under headroom, its samples
//! stored that many stops darker, and the capture sharpen reads a file
//! with headroom as darker than it is (its threshold and its pedestal
//! are on absolute levels): on a real stack it then took a quarter to a
//! third of the fine texture the walk gained. Once the sharpen reads
//! such a file at its scene's levels, the stack can keep its
//! reconstructed highlights.
//!
//! The command line's `greycard stack` and the window's Merge to Focus
//! stack both run [`frames`]; the window, and the command line when it
//! writes a DNG, write it with [`write_dng`]. Neither has a stack of its
//! own. The file keeps a recipe, so it is made again (`merge::again`)
//! as a bracket and a panorama are: each frame blended where the
//! recipe put it ([`stack::stack_camera_placed`]), nothing fitted.
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
    /// reference's clip.
    pub merged: CameraImage,
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
#[derive(Default)]
struct Walks {
    /// The reference's fit: none until it is walked, then what it made
    /// (none on a frame the correction does not measure).
    fit: Option<Option<Arc<CaFit>>>,
    noise: Option<Noise>,
    denoised: bool,
    skipped: Vec<Option<LearnedSkipped>>,
    warnings: Vec<String>,
    seconds: f64,
}

/// `files`, in focus order, merged under `options`, each walked under
/// `edit`'s demosaic and CA to be blended, `models` running the learned
/// denoiser when `options` asks. The frames are read as the core's
/// merge asks for them ([`stack::stack_camera_from`]), not held: each
/// is decoded and developed the short way to be placed, and decoded and
/// walked again to be blended, and nothing of it is kept between but
/// its tags. Each first read is said to `progress` as a frame, then the
/// placing, then each blend. With the places given (`options.placed`)
/// only the blends are read and said. A developed picture is refused: it is past
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
    let mut walks = Walks::default();
    let learned = options.denoiser.is_some();
    // Made again: the reference's tags and white read first, as its
    // placing read would take them, and every read a blend.
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
        blending.set(true);
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
            reconstruct: false,
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
    // Held to the reference's clip, as the file stores it: a finer
    // demosaic overshoots a clipped edge, and rings under black.
    {
        use rayon::prelude::*;
        merged
            .data
            .par_iter_mut()
            .for_each(|v| *v = v.clamp(0.0, 1.0));
    }
    // The reference's noise, not less: where the frames are equally
    // sharp the blend averages them and the noise is less, but where
    // the detail is, each band comes from one frame and so does its
    // noise. The file's readers (the denoisers, the grain, the
    // sharpen's floor) want it right in the detail; over the flat
    // regions it overstates, which smooths what has nothing to lose.
    let noise = walks.noise.unwrap_or(Noise::ASSUMED);
    let noise_profile = core_merge::noise_profile(&[(1.0, noise)], 1.0);
    let names: Vec<String> = files.iter().map(|f| name_of(f)).collect();
    let (heads, wholes) = hashes(&sources);
    let denoised = walks.denoised;
    let tag = Tag {
        kind: "stack".into(),
        frames: names.clone(),
        reference: report.reference,
        denoised,
        headroom_stops: 0.0,
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
            }),
        }),
    };
    log::info!(
        "stack: {of} frames read in {developing:.1} s, {:.1} s of it the blends' walks; \
         merged in {merging:.1} s",
        walks.seconds
    );
    Ok(Stacked {
        merged,
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
        headroom: None,
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

    /// `n` frames of one scene, as a focus stack's are of one subject.
    fn frames_of(dir: &Path, n: usize, seed: u64) -> Vec<PathBuf> {
        let (w, h) = (192, 128);
        (0..n)
            .map(|k| {
                let p = dir.join(format!("IMG_{k}.dng"));
                write_raw(
                    &p,
                    mosaic(w, h, 0.25, Noise::ASSUMED, seed + k as u64),
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
    /// reference's CA fit, held to its clip.
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
            reconstruct: false,
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
        for v in &mut merged.data {
            *v = v.clamp(0.0, 1.0);
        }
        assert!(
            bits(&merged.data) == bits(&got.merged.data),
            "the stack is not its steps by hand"
        );
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
    /// is the reference's own noise as its walk measured it, the file
    /// holding no headroom. The file develops at that noise.
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
        assert_eq!(tag.headroom_stops, 0.0);
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

        let (r, _, _) = walk_frame(
            &Files(&files),
            1,
            &Edit::default(),
            MergeInput {
                white: None,
                reconstruct: false,
                denoiser: None,
                ca: MergeCa::Make,
                lens: Vec::new(),
            },
            None,
            &mut NoModels,
        )
        .unwrap();
        let want = core_merge::noise_profile(&[(1.0, r.noise)], 1.0);
        assert_eq!(report.noise_profile, want);
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
}
