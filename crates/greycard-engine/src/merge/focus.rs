//! The focus stack: every frame developed the short way to camera RGB,
//! registered onto a reference, merged band by band from whichever
//! frame holds each sharpest (the core's `stack_camera`), and written
//! as a linear DNG beside the first frame, `<first> stack.dng`.
//!
//! The command line's `greycard stack` and the window's Merge to Focus
//! stack both run [`frames`]; the window, and the command line when it
//! writes a DNG, write it with [`write_dng`]. Neither has a stack of its
//! own.
//!
//! No frame is held: each is read when the core's merge asks for it,
//! once to place it and once to blend it, so a stack's memory is one
//! frame's working set whatever the count ([`wants`]).

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use greycard_core::decode::RawMetadata;
use greycard_core::dng::{Compression, LinearDngOptions, write_linear_dng};
use greycard_core::output::OnExists;
use greycard_core::raw::{RawFrame, Samples};
use greycard_core::stack::{self, Reference, StackReport};
use greycard_core::{
    CameraImage, DemosaicMethod, DevelopSettings, HighlightMode, WhiteBalance, demosaic,
};
use greycard_edit::Edit;

use super::{Files, Progress, Step, hold, name_of};

/// What goes after the first frame's name.
pub const SUFFIX: &str = " stack";

/// What a stack is asked. The default is what the window asks: the
/// middle frame, the core's selectivity and residual, lossless
/// samples, beside the first frame, a name already there numbered.
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
        }
    }
}

/// What a stack holds a pixel of the frame at its peak, whatever the
/// number of frames: the core's accumulators and one frame's warp,
/// pyramids, and maps (87 bytes, counted by the allocator), and the
/// frame being decoded and developed beside them. The process's peak
/// resident set was 99 bytes a pixel at 24 megapixels, from 5 frames to
/// 100, and 87 at 45; this is that and a tenth.
pub const BYTES_A_PIXEL: u64 = 110;

/// What a stack keeps of each frame across the merge: its tags (the
/// reference's are the file's) and its line of the report. No pixels.
pub const BYTES_A_FRAME: u64 = 1 << 20;

/// What a stack of `frames` frames of `pixels` pixels wants at its
/// peak: [`BYTES_A_PIXEL`] of one frame, and [`BYTES_A_FRAME`] a frame.
pub fn wants(frames: usize, pixels: usize) -> u64 {
    (pixels as u64)
        .saturating_mul(BYTES_A_PIXEL)
        .saturating_add((frames as u64).saturating_mul(BYTES_A_FRAME))
}

/// Whether a stack of `frames` frames of `pixels` pixels ([`wants`])
/// fits in `memory` bytes: refused when it does not, since a stack past
/// the machine's memory would take the process down with it. The
/// caller asks the memory available now, not the machine's whole. A
/// machine that will not say its memory is not asked.
pub fn room(frames: usize, pixels: usize, memory: Option<u64>) -> Result<()> {
    let Some(memory) = memory else {
        return Ok(());
    };
    let wants = wants(frames, pixels);
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

/// The short develop each frame takes: bilinear, hot pixels repaired,
/// no CA correction, highlights clipped rather than reconstructed, as
/// `register` has it, stopped at the demosaic, which leaves camera RGB
/// with no white balance and no matrix on it. That is the space the
/// merge belongs in and the space a linear DNG stores.
pub fn develop_settings() -> DevelopSettings {
    DevelopSettings {
        demosaic: DemosaicMethod::Bilinear,
        highlights: HighlightMode::Clip,
        chromatic_aberration: None,
        ..Default::default()
    }
}

/// The frames merged, not yet written.
pub struct Stacked {
    /// Camera RGB, the white balance divided back out.
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
    /// Seconds reading and developing the frames, and merging them.
    pub developing: f64,
    pub merging: f64,
}

/// `files`, in focus order, each developed by [`develop_settings`] and
/// merged under `options`. The frames are read as the core's merge asks
/// for them ([`stack::stack_camera_from`]), not held: each is decoded
/// and developed once to be placed and again to be blended, and
/// nothing of it is kept between but its tags. Each first read is said
/// to `progress` as a frame, then the placing, then each blend. A
/// developed picture is refused: it is past the white balance and the
/// matrix. So is a stack the machine's available memory will not hold
/// ([`room`]), read off the first frame's size before any other frame
/// is decoded.
pub fn frames(files: &[PathBuf], options: &Options, progress: &dyn Progress) -> Result<Stacked> {
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
    let settings = develop_settings();
    let started = Instant::now();
    let of = files.len();
    // Each frame's tags, from its first read: the reference's are the
    // file's, and the reference is the merge's to find.
    let mut heads: Vec<Option<(RawFrame, RawMetadata, WhiteBalance)>> =
        (0..of).map(|_| None).collect();
    let mut developing = 0.0f64;
    let mut checked = false;
    let read = |k: usize| -> Result<CameraImage> {
        let t = Instant::now();
        let file = &files[k];
        let (mut frame, metadata) = greycard_core::decode::decode_path_with_metadata(file)
            .with_context(|| format!("decoding {}", file.display()))?;
        if !checked {
            let memory = options.memory.or_else(greycard_ai::ram::available);
            room(of, frame.width * frame.height, memory)?;
            checked = true;
        }
        let demosaiced = demosaic(&frame, &settings)
            .with_context(|| format!("developing {}", file.display()))?;
        if heads[k].is_none() {
            // The tags are read from the frame; its samples are let go.
            frame.samples = Samples::U16(Vec::new());
            heads[k] = Some((frame, metadata, demosaiced.white_balance));
        }
        developing += t.elapsed().as_secs_f64();
        Ok(demosaiced.image)
    };
    let said = |s: stack::Step| {
        progress.step(match s {
            stack::Step::Reading { index, of } => Step::Frame { index, of },
            stack::Step::Placing => Step::Placing,
            stack::Step::Blending { index, of } => Step::Blending { index, of },
        })
    };

    let core = stack::Options {
        reference: options.reference,
        selectivity: options.selectivity,
        max_residual: options.max_residual,
        ..Default::default()
    };
    let (merged, report) = stack::stack_camera_from(of, read, said, &core)?;
    let merging = started.elapsed().as_secs_f64() - developing;
    let (frame, metadata, balance) = heads
        .swap_remove(report.reference)
        .expect("every frame is read in the first pass");
    Ok(Stacked {
        merged,
        frame,
        metadata,
        balance,
        report,
        names: files.iter().map(|f| name_of(f)).collect(),
        developing,
        merging,
    })
}

/// `stacked` written as a linear DNG to `out`: the reference's color
/// tags and EXIF (the merge is in its geometry and at its white
/// balance), the samples as lossless JPEG unless `uncompressed`, and a
/// small sRGB preview.
pub fn write_dng<W: std::io::Write + std::io::Seek>(
    out: W,
    stacked: &Stacked,
    uncompressed: bool,
) -> Result<()> {
    let preview = greycard_core::merge::preview(
        &stacked.merged,
        &stacked.balance,
        greycard_core::merge::PREVIEW_SIDE,
    )?;
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
    write_linear_dng(
        out,
        &stacked.frame,
        &stacked.merged,
        stacked.balance.coefficients_f32(),
        &options,
    )?;
    Ok(())
}

/// What a stack written did.
pub struct Report {
    pub out: PathBuf,
    pub stack: StackReport,
    pub names: Vec<String>,
    /// The seconds each part took, in order.
    pub seconds: Vec<(&'static str, f64)>,
}

/// The focus stack `files` merged and written as a linear DNG, beside
/// the first frame unless `options` says where. The name is held
/// before a frame is read, and the file written under the hidden folder
/// that holds it and renamed at the end, as every merge's is: a stack
/// cut short leaves no half file, and a frame is never written over.
pub fn stack(files: &[PathBuf], options: &Options, progress: &dyn Progress) -> Result<Report> {
    if files.len() < 2 {
        bail!("a stack wants at least two frames; got {}", files.len());
    }
    let started = Instant::now();
    let out = match &options.out {
        Some(p) => p.clone(),
        None => default_output(&files[0]),
    };
    let reserved = hold(&Files(files), &out, options.on_exists)?;
    let stacked = frames(files, options, progress)?;
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
        ("develop", stacked.developing),
        ("merge", stacked.merging),
        ("write", t.elapsed().as_secs_f64()),
        ("total", started.elapsed().as_secs_f64()),
    ];
    Ok(Report {
        out,
        stack: stacked.report,
        names: stacked.names,
        seconds,
    })
}

/// The edit a stack's file starts with: the reference frame's, less
/// the learned denoiser, which a file with no mosaic cannot run. Its
/// exposure stays: a stack is at the reference's.
pub fn edit_for_stack(reference: &Edit) -> Edit {
    let mut edit = reference.clone();
    edit.noise.learned = greycard_edit::Learned::Off;
    edit
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::merge::Noise;
    use greycard_core::merge::testing::{mosaic, write_dng as write_raw};

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("greycard-focus-{name}-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Two frames of one scene, as a focus stack's are of one subject.
    fn two_frames(dir: &Path) -> Vec<PathBuf> {
        let (w, h) = (192, 128);
        (0..2)
            .map(|k| {
                let p = dir.join(format!("IMG_{k}.dng"));
                write_raw(
                    &p,
                    mosaic(w, h, 0.25, Noise::ASSUMED, 40 + k as u64),
                    w,
                    h,
                    (1, 100),
                    "Stack",
                );
                p
            })
            .collect()
    }

    /// The stack is its steps done by hand, byte for byte: each frame
    /// decoded and demosaiced the short way, merged by the core, and
    /// written with the reference's tags.
    #[test]
    fn the_stack_is_its_steps_by_hand() {
        let dir = scratch("same");
        let files = two_frames(&dir);
        let report = stack(&files, &Options::default(), &|_: Step| {}).expect("a stack");
        assert_eq!(report.out, dir.join("IMG_0 stack.dng"));
        assert_eq!(report.stack.used(), 2, "{:#?}", report.stack.frames);

        let settings = DevelopSettings {
            demosaic: DemosaicMethod::Bilinear,
            highlights: HighlightMode::Clip,
            chromatic_aberration: None,
            ..Default::default()
        };
        let mut sources = Vec::new();
        let mut images = Vec::new();
        for f in &files {
            let (frame, metadata) = greycard_core::decode::decode_path_with_metadata(f).unwrap();
            let d = demosaic(&frame, &settings).unwrap();
            images.push(d.image);
            sources.push((frame, metadata, d.white_balance));
        }
        let (merged, by_hand) = stack::stack_camera(&images, &stack::Options::default()).unwrap();
        assert_eq!(by_hand.reference, report.stack.reference);
        let (frame, metadata, balance) = &sources[by_hand.reference];
        let preview =
            greycard_core::merge::preview(&merged, balance, greycard_core::merge::PREVIEW_SIDE)
                .unwrap();
        let mut want = std::io::Cursor::new(Vec::new());
        write_linear_dng(
            &mut want,
            frame,
            &merged,
            balance.coefficients_f32(),
            &LinearDngOptions {
                compression: Compression::Lossless,
                metadata: Some(metadata),
                preview: Some(&preview),
                software: Some(concat!("greycard ", env!("CARGO_PKG_VERSION"))),
                headroom: None,
            },
        )
        .unwrap();
        let got = std::fs::read(&report.out).unwrap();
        assert!(
            got == want.into_inner(),
            "the stack is not its steps by hand"
        );
        // Nothing of the hidden folder is left beside it.
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 3, "{left:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Three frames, each read once to be placed and again to be
    /// blended, said as frames, the placing, and the blends; the merge
    /// is what the core makes of the three held at once, bit for bit.
    #[test]
    fn a_stack_reads_its_frames_twice_and_says_so() {
        let dir = scratch("passes");
        let (w, h) = (192, 128);
        let files: Vec<PathBuf> = (0..3)
            .map(|k| {
                let p = dir.join(format!("IMG_{k}.dng"));
                write_raw(
                    &p,
                    mosaic(w, h, 0.25, Noise::ASSUMED, 50 + k as u64),
                    w,
                    h,
                    (1, 100),
                    "Stack",
                );
                p
            })
            .collect();
        let heard = std::cell::RefCell::new(Vec::new());
        let stacked = frames(&files, &Options::default(), &|s: Step| {
            heard.borrow_mut().push(s)
        })
        .expect("a stack");
        let used = stacked.report.used();
        let mut want: Vec<Step> = (1..=3).map(|index| Step::Frame { index, of: 3 }).collect();
        want.push(Step::Placing);
        want.extend((1..=used).map(|index| Step::Blending { index, of: used }));
        assert_eq!(*heard.borrow(), want);

        let images: Vec<CameraImage> = files
            .iter()
            .map(|f| {
                let (frame, _) = greycard_core::decode::decode_path_with_metadata(f).unwrap();
                demosaic(&frame, &develop_settings()).unwrap().image
            })
            .collect();
        let (held, report) = stack::stack_camera(&images, &stack::Options::default()).unwrap();
        assert_eq!(report, stacked.report);
        assert!(
            held.data.iter().map(|v| v.to_bits()).eq(stacked
                .merged
                .data
                .iter()
                .map(|v| v.to_bits())),
            "read twice is not held once"
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A stack cut short between its passes (here a frame gone before
    /// it is read again) fails, and leaves neither a file nor its
    /// hidden folder: nothing is spilled, so nothing is left.
    #[test]
    fn a_stack_cut_short_between_its_passes_leaves_nothing() {
        let dir = scratch("cut");
        let files = two_frames(&dir);
        let gone = dir.join("gone.dng");
        let e = stack(&files, &Options::default(), &|s: Step| {
            if s == Step::Placing {
                std::fs::rename(&files[1], &gone).unwrap();
            }
        })
        .err()
        .expect("cut short");
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
        let files = two_frames(&dir);
        std::fs::write(dir.join("IMG_0 stack.dng"), b"there").unwrap();
        let report = stack(&files, &Options::default(), &|_: Step| {}).expect("a stack");
        assert_eq!(report.out, dir.join("IMG_0 stack-1.dng"));
        assert_eq!(
            std::fs::read(dir.join("IMG_0 stack.dng")).unwrap(),
            b"there"
        );
        let e = stack(
            &files,
            &Options {
                out: Some(files[1].clone()),
                on_exists: OnExists::Overwrite,
                ..Options::default()
            },
            &|_: Step| {},
        )
        .err()
        .expect("a frame refused");
        assert!(e.to_string().contains("one of the frames"), "{e}");
        let one = stack(&files[..1], &Options::default(), &|_: Step| {})
            .err()
            .expect("one frame");
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
        assert_eq!(wants(21, 24_000_000), 2_640_000_000 + 21 * (1 << 20));
        assert_eq!(wants(100, 24_000_000) - wants(2, 24_000_000), 98 << 20);
        assert!(room(100, 24_000_000, Some(16_000_000_000)).is_ok());
        assert!(room(100, 60_000_000, Some(16_000_000_000)).is_ok());
        assert!(room(2, 24_000_000, Some(2_000_000_000)).is_err());
        assert!(room(50, 45_000_000, None).is_ok());
        let e = room(21, 24_000_000, Some(1_000_000_000)).unwrap_err();
        assert_eq!(
            e.to_string(),
            "this stack needs about 2.7 GB of memory and 1.0 GB is free: close other \
             programs and try again"
        );
        let dir = scratch("memory");
        let files = two_frames(&dir);
        let heard = std::cell::RefCell::new(Vec::new());
        let e = stack(
            &files,
            &Options {
                memory: Some(1_000_000),
                ..Options::default()
            },
            &|s: Step| heard.borrow_mut().push(s),
        )
        .err()
        .expect("refused");
        assert!(e.to_string().contains("close other programs"), "{e}");
        assert_eq!(*heard.borrow(), [Step::Frame { index: 1, of: 2 }]);
        assert!(!dir.join("IMG_0 stack.dng").exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
