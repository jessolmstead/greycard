//! Focus stacking: `greycard stack` develops every frame the short
//! way, merges them in camera space, and writes the merge as a linear
//! DNG or a TIFF, beside the sources or where `-o` says.

use std::path::PathBuf;

use anyhow::{Context, Result};
use greycard_core::decode::RawlerDecoder;
use greycard_core::dng::{Compression, LinearDngOptions, write_linear_dng};
use greycard_core::output::OnExists;
use greycard_core::stack::{self, Reference};
use greycard_core::{
    CameraImage, DemosaicMethod, DevelopSettings, HighlightMode, RawFrame, WorkingImage, demosaic,
};

use crate::common::{SOFTWARE, srgb_preview, write_linear_tiff};

/// `--reference`, as the command line spells it.
pub(crate) fn stack_reference(name: &str) -> Result<Reference, String> {
    match name.trim() {
        "middle" => Ok(Reference::Middle),
        "sharpest" => Ok(Reference::Sharpest),
        other => other.parse::<usize>().map(Reference::Index).map_err(|_| {
            format!("want middle, sharpest or a frame number counting from 0, not {other}")
        }),
    }
}

/// What a merge is written as, from the name asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StackFormat {
    /// A linear DNG: camera-space samples with the reference frame's
    /// color tags and EXIF, which is what the merge already is.
    Dng,
    /// A 16-bit linear Rec.2020 TIFF, for a consumer that wants the
    /// picture rather than a file to develop.
    Tiff,
}

fn stack_format(path: &std::path::Path) -> Result<StackFormat> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("dng") => Ok(StackFormat::Dng),
        Some("tif" | "tiff") => Ok(StackFormat::Tiff),
        _ => anyhow::bail!(
            "a merged stack is written as a linear DNG (.dng) or a 16-bit linear TIFF \
             (.tif); {} is neither",
            path.display()
        ),
    }
}

/// `<first frame> stack.dng`, beside the first frame.
fn stack_default_output(first: &std::path::Path) -> PathBuf {
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "stack".into());
    let name = format!("{stem} stack.dng");
    match first.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// Merge a focus stack and write it beside the sources.
///
/// The frames go through the same short develop `register` uses —
/// bilinear, hot pixel repair on, no chromatic aberration correction,
/// highlights clipped rather than reconstructed (§119) — but stop one
/// step earlier, at [`demosaic`], which leaves camera-space samples
/// with no white balance and no matrix on them. That is the space the
/// merge belongs in and the space a linear DNG stores, so the result
/// is a file every editor treats as it would the originals, which is
/// what §71 asked the output of every stack to be. A merge taken all
/// the way to the working space could not be written as one: the DNG's
/// `ColorMatrix` and `AsShotNeutral` describe samples the matrix has
/// not been applied to, and a reader would apply it a second time.
///
/// The color tags and the EXIF come from the frame the merge was
/// registered onto, since the result is in its geometry and at its
/// white balance.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stack_files(
    files: &[PathBuf],
    output: Option<&std::path::Path>,
    reference: Reference,
    selectivity: f32,
    max_residual: f32,
    uncompressed: bool,
    on_exists: OnExists,
) -> Result<()> {
    if files.len() < 2 {
        anyhow::bail!("a stack wants at least two frames; got {}", files.len());
    }
    // What is already there decides the name before anything is
    // decoded, so a run that would write nothing does nothing.
    let wanted = match output {
        Some(path) => path.to_path_buf(),
        None => stack_default_output(&files[0]),
    };
    let format = stack_format(&wanted)?;
    let resolved = on_exists.resolve(&wanted);
    if let Some(note) = resolved.note() {
        tracing::warn!("{note}");
    }
    let Some(out_path) = resolved.path().map(std::path::Path::to_path_buf) else {
        return Ok(());
    };

    let settings = DevelopSettings {
        demosaic: DemosaicMethod::Bilinear,
        highlights: HighlightMode::Clip,
        chromatic_aberration: None,
        ..Default::default()
    };

    let developing = std::time::Instant::now();
    let mut sources = Vec::with_capacity(files.len());
    let mut images = Vec::with_capacity(files.len());
    for file in files {
        if greycard_core::picture::is_picture_path(file) {
            anyhow::bail!(
                "{} is not a RAW file: a stack is merged in camera space, before white \
                 balance and the matrix, and a developed picture is past both",
                file.display()
            );
        }
        let (frame, metadata) = RawlerDecoder
            .decode_path_with_metadata(file)
            .with_context(|| format!("decoding {}", file.display()))?;
        let demosaiced = demosaic(&frame, &settings)
            .with_context(|| format!("developing {}", file.display()))?;
        images.push(demosaiced.image);
        sources.push((frame, metadata, demosaiced.white_balance));
    }
    let developing = developing.elapsed();

    let options = stack::Options {
        reference,
        selectivity,
        max_residual,
        ..Default::default()
    };
    let merging = std::time::Instant::now();
    let (merged, report) = stack::stack_camera(&images, &options)?;
    let merging = merging.elapsed();
    drop(images);

    let name = |i: usize| {
        files[i]
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| files[i].display().to_string())
    };
    println!(
        "frames        {} at {}x{}, developed in {:.1} s",
        files.len(),
        report.width,
        report.height,
        developing.as_secs_f64()
    );
    println!(
        "reference     frame {} ({})",
        report.reference,
        name(report.reference)
    );
    println!(
        "merge         {:.1} s, {} pyramid levels, {} of {} frames used",
        merging.as_secs_f64(),
        report.levels,
        report.used(),
        files.len()
    );
    println!(
        "coverage      every frame reached {:.1}% of the picture",
        report.full_coverage_fraction() * 100.0
    );
    let (mx, my) = (
        (report.width - 1) as f64 / 2.0,
        (report.height - 1) as f64 / 2.0,
    );
    for f in &report.frames {
        print!(
            "  {:>3}  {:<24}  sharpness {:.4}",
            f.index,
            name(f.index),
            f.sharpness
        );
        match (&f.against, &f.fit) {
            (Some(against), Some(fit)) => {
                // A dropped frame has no transform onto the reference —
                // it never got one — so what is worth showing is the
                // link that was refused, and it has to say so.
                let (shown, what) = match f.dropped {
                    None => (f.transform, "the middle moves"),
                    Some(_) => (fit.transform, "the refused link moves the middle"),
                };
                let (dx, dy) = shown.displacement(mx, my);
                print!(
                    "  onto {against}, residual {:.4} over {:.1}%; \
                     {what} {dx:+.2}, {dy:+.2} px, scale {:.6}, rotation {:.4}\u{00b0}",
                    fit.residual,
                    fit.overlap * 100.0,
                    shown.scale(),
                    shown.rotation().to_degrees(),
                );
            }
            _ => print!("  the reference"),
        }
        match &f.dropped {
            Some(why) => println!("\n       DROPPED: {why}"),
            None => println!(),
        }
    }
    if report.used() < files.len() {
        println!(
            "left out      {} frame(s); the merge is of the rest",
            files.len() - report.used()
        );
    }

    let (frame, metadata, balance) = &sources[report.reference];
    let source_name = files[report.reference]
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    write_stack(
        merged,
        frame,
        metadata,
        balance,
        format,
        &out_path,
        source_name.as_deref(),
        uncompressed,
    )?;
    println!("wrote         {}", out_path.display());
    Ok(())
}

/// The merge taken into the working space: the white balance gains
/// first, then the camera matrix.
///
/// The gains are the half that is easy to forget. [`demosaic`] applies
/// them before the demosaic and divides them back out afterwards,
/// because a linear DNG stores camera-native samples and says what
/// neutralizes them in `AsShotNeutral` rather than baking it in. So
/// the merge, which is those samples, still has no white balance on
/// it, and the matrix alone gives a green picture — the matrix maps
/// *balanced* camera RGB into the working space. `develop` does gains
/// then matrix; so does this.
fn stack_to_working(
    merged: CameraImage,
    balance: &greycard_core::WhiteBalance,
) -> Result<WorkingImage> {
    let mut working = WorkingImage::from_data(merged.width, merged.height, merged.data)?;
    greycard_core::develop::apply_gains_rgb(&mut working.data, balance.coefficients_f32());
    greycard_core::develop::apply_matrix(&mut working.data, balance.matrix_f32());
    Ok(working)
}

/// A small sRGB rendering of the merge for the DNG's embedded preview,
/// in sensor orientation as the merge is, which is the orientation a
/// DNG's preview wants.
///
/// The samples are averaged down to at most `max_width` *before* the
/// color, so a 45-megapixel merge never allocates a second
/// full-resolution float image and a full-resolution 8-bit one on top
/// of it just to end up 1024 pixels wide.
fn stack_preview(
    merged: &CameraImage,
    balance: &greycard_core::WhiteBalance,
    max_width: usize,
) -> Result<image::RgbImage> {
    let step = merged.width.div_ceil(max_width.max(1)).max(1);
    let (w, h) = (merged.width.div_ceil(step), merged.height.div_ceil(step));
    let mut small = Vec::with_capacity(w * h * CameraImage::CHANNELS);
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0.0f64; CameraImage::CHANNELS];
            let mut n = 0u32;
            for sy in y * step..((y + 1) * step).min(merged.height) {
                for sx in x * step..((x + 1) * step).min(merged.width) {
                    let px = merged.pixel(sx, sy);
                    for (a, v) in sum.iter_mut().zip(px) {
                        *a += v as f64;
                    }
                    n += 1;
                }
            }
            let n = n.max(1) as f64;
            small.extend(sum.iter().map(|v| (v / n) as f32));
        }
    }
    let small = CameraImage::from_data(w, h, small)?;
    srgb_preview(&stack_to_working(small, balance)?, 0.0)
}

/// Write a merged stack, as a linear DNG or as a 16-bit linear TIFF.
///
/// Split out of [`stack_files`] so that a test can drive it without a
/// RAW file on disk, since what it does — the gains, the matrix, the
/// tags — is the part with something to get wrong.
#[allow(clippy::too_many_arguments)]
fn write_stack(
    merged: CameraImage,
    frame: &RawFrame,
    metadata: &greycard_core::decode::rawler_backend::RawMetadata,
    balance: &greycard_core::WhiteBalance,
    format: StackFormat,
    out_path: &std::path::Path,
    source_name: Option<&str>,
    uncompressed: bool,
) -> Result<()> {
    match format {
        StackFormat::Dng => {
            let preview = stack_preview(&merged, balance, 1024)?;
            let options = LinearDngOptions {
                compression: if uncompressed {
                    Compression::Uncompressed
                } else {
                    Compression::Lossless
                },
                metadata: Some(metadata),
                preview: Some(&preview),
                software: Some(SOFTWARE),
                headroom: None,
            };
            // Nothing is created until there is something to write
            // into it: a run that fails at the color must not leave an
            // empty file where the merge was asked for.
            let file = std::fs::File::create(out_path)
                .with_context(|| format!("creating {}", out_path.display()))?;
            let mut writer = std::io::BufWriter::new(file);
            write_linear_dng(
                &mut writer,
                frame,
                &merged,
                balance.coefficients_f32(),
                &options,
            )
            .with_context(|| format!("writing {}", out_path.display()))?;
            std::io::Write::flush(&mut writer)
                .with_context(|| format!("writing {}", out_path.display()))?;
        }
        StackFormat::Tiff => {
            let working = stack_to_working(merged, balance)?;
            let working = greycard_core::develop::orient(working, frame.orientation);
            let (w, h) = (working.width as u32, working.height as u32);
            let provenance = greycard_core::exif::Provenance {
                metadata,
                software: SOFTWARE,
                width: w,
                height: h,
                srgb: false,
                written: Some(std::time::SystemTime::now()),
                source_name,
                output: None,
                edit: None,
            };
            write_linear_tiff(&working, out_path, &provenance)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::color::WORKING_SPACE;
    use greycard_core::raw::Orientation;
    use greycard_core::{decode, develop};

    #[test]
    fn the_output_name_says_the_format() {
        assert_eq!(
            stack_format(std::path::Path::new("a/b.dng")).expect("a dng"),
            StackFormat::Dng
        );
        assert_eq!(
            stack_format(std::path::Path::new("a/b.TIFF")).expect("a tiff"),
            StackFormat::Tiff
        );
        assert!(stack_format(std::path::Path::new("a/b.jpg")).is_err());
        assert!(stack_format(std::path::Path::new("a/b")).is_err());
    }

    #[test]
    fn without_a_name_the_merge_goes_beside_the_first_frame() {
        assert_eq!(
            stack_default_output(std::path::Path::new("/photos/DSC_0001.NEF")),
            PathBuf::from("/photos/DSC_0001 stack.dng")
        );
        assert_eq!(
            stack_default_output(std::path::Path::new("DSC_0001.NEF")),
            PathBuf::from("DSC_0001 stack.dng")
        );
    }

    #[test]
    fn the_reference_is_named_or_numbered() {
        assert_eq!(stack_reference("middle"), Ok(Reference::Middle));
        assert_eq!(stack_reference(" sharpest "), Ok(Reference::Sharpest));
        assert_eq!(stack_reference("4"), Ok(Reference::Index(4)));
        assert!(stack_reference("last").is_err());
    }

    /// A textured RGGB frame whose camera is Rec.2020 itself, so the
    /// matrix is near enough the identity and a channel that comes
    /// back wrong has nowhere to hide. The as-shot gains are far from
    /// one in red and blue, which is what makes a merge written
    /// without them unmistakably green.
    fn textured_frame(width: usize, height: usize) -> RawFrame {
        use greycard_core::raw::{
            Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Samples,
        };
        let pattern = CfaPattern::rggb();
        // A hash lattice, smoothed: structure at a few scales and no
        // period, so the registration between two frames of it has
        // something to hold and nothing to confuse it with.
        let lattice = |ix: i64, iy: i64| {
            let mut h = (ix as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add((iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
            h ^= h >> 29;
            h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 32;
            (h >> 11) as f64 / (1u64 << 53) as f64
        };
        let value = |x: f64, y: f64| {
            let mut v = 0.0;
            let mut amplitude = 0.5;
            let mut cell = 32.0;
            for _ in 0..3 {
                let (gx, gy) = (x / cell, y / cell);
                let (i, j) = (gx.floor(), gy.floor());
                let (fx, fy) = (gx - i, gy - j);
                let ease = |t: f64| t * t * (3.0 - 2.0 * t);
                let (sx, sy) = (ease(fx), ease(fy));
                let (i, j) = (i as i64, j as i64);
                let a = lattice(i, j) + sx * (lattice(i + 1, j) - lattice(i, j));
                let b = lattice(i, j + 1) + sx * (lattice(i + 1, j + 1) - lattice(i, j + 1));
                v += amplitude * (a + sy * (b - a) - 0.5);
                amplitude *= 0.7;
                cell /= 2.0;
            }
            v
        };
        // Black 100, white 1100: a thousand counts of range. The three
        // channels sit at different levels so a swap or a missing gain
        // shows up in the means.
        let (black, range) = (100.0f64, 1000.0f64);
        let mut samples = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let base = match pattern.color_at(y, x) {
                    CfaColor::Red => 0.30,
                    CfaColor::Green => 0.55,
                    CfaColor::Blue => 0.18,
                    CfaColor::Other(_) => unreachable!(),
                };
                let v = (base + 0.10 * value(x as f64, y as f64)).clamp(0.02, 0.95);
                samples.push((black + v * range).round() as u16);
            }
        }
        let matrix = {
            let m = WORKING_SPACE.from_xyz_matrix().expect("Rec.2020 from XYZ");
            let mut flat = Vec::with_capacity(9);
            for row in m.rows {
                for v in row {
                    flat.push(v as f32);
                }
            }
            flat
        };
        RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            other_names: Vec::new(),
            width,
            height,
            channels: 1,
            layout: greycard_core::raw::SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(black as f32, 1),
                white: LevelPattern::uniform((black + range) as f32, 1),
            },
            as_shot_coefficients: Some([2.0, 1.0, 4.0]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: matrix,
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
        }
    }

    fn channel_means(data: impl Iterator<Item = f32>) -> [f64; 3] {
        let mut sum = [0.0f64; 3];
        let mut n = 0u64;
        for (i, v) in data.enumerate() {
            sum[i % 3] += v as f64;
            if i % 3 == 0 {
                n += 1;
            }
        }
        sum.map(|s| s / n.max(1) as f64)
    }

    /// The merge is camera-native samples with the white balance
    /// divided back out, so everything that turns it into a picture
    /// has to put the gains back on before the matrix. This drives the
    /// whole write path — `stack_camera`, the DNG, the TIFF and the
    /// embedded preview — and compares what comes back out of each
    /// against a plain `develop` of the same frame.
    ///
    /// It is the test that would have caught the bug it was written
    /// for: without the gains the preview's channel means came out
    /// R 0.20 G 0.40 B 0.19 against a develop's 0.36 / 0.36 / 0.35, a
    /// green picture that nothing else in the suite looked at.
    #[test]
    fn the_written_stack_carries_the_white_balance() {
        let dir = std::env::temp_dir().join(format!("greycard-stack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let frame = textured_frame(96, 72);
        let settings = DevelopSettings {
            demosaic: DemosaicMethod::Bilinear,
            highlights: HighlightMode::Clip,
            chromatic_aberration: None,
            ..Default::default()
        };
        // What the merge should look like once it is a picture again.
        let want = develop(&frame, &settings).expect("a develop");
        let want_means = channel_means(want.image.data.iter().copied());

        // Two copies of one frame: the merge is that frame, so
        // anything the write path does to it is the only difference.
        let demosaiced = demosaic(&frame, &settings).expect("a demosaic");
        let (merged, report) = stack::stack_camera(
            &[demosaiced.image.clone(), demosaiced.image.clone()],
            &stack::Options::default(),
        )
        .expect("a camera stack");
        assert_eq!(report.used(), 2, "{:#?}", report.frames);
        let camera_means = channel_means(merged.data.iter().copied());
        let straight = channel_means(demosaiced.image.data.iter().copied());
        for (a, b) in camera_means.iter().zip(&straight) {
            assert!(
                (a - b).abs() < 1e-3,
                "the merge of one frame twice is not that frame: {camera_means:?} against \
                 {straight:?}"
            );
        }

        let balance = &demosaiced.white_balance;
        let metadata = greycard_core::decode::rawler_backend::RawMetadata::default();

        // The DNG: written, decoded again, developed, and its means
        // compared with the develop of the original.
        let dng = dir.join("stack.dng");
        write_stack(
            merged.clone(),
            &frame,
            &metadata,
            balance,
            StackFormat::Dng,
            &dng,
            None,
            true,
        )
        .expect("writing the dng");
        let back = decode::decode_path(&dng).expect("decoding the dng");
        let developed = develop(&back, &settings).expect("developing the dng");
        let dng_means = channel_means(developed.image.data.iter().copied());
        println!("develop {want_means:?}\n    dng {dng_means:?}");
        for (a, b) in dng_means.iter().zip(&want_means) {
            assert!(
                (a - b).abs() < 0.01,
                "the DNG develops to {dng_means:?}, the source to {want_means:?}"
            );
        }

        // The TIFF: the same picture, written straight.
        let tiff = dir.join("stack.tif");
        write_stack(
            merged.clone(),
            &frame,
            &metadata,
            balance,
            StackFormat::Tiff,
            &tiff,
            None,
            false,
        )
        .expect("writing the tiff");
        let got = image::open(&tiff).expect("reading the tiff").into_rgb16();
        let tiff_means = channel_means(got.as_raw().iter().map(|v| *v as f32 / 65535.0));
        println!("   tiff {tiff_means:?}");
        for (a, b) in tiff_means.iter().zip(&want_means) {
            assert!(
                (a - b).abs() < 0.01,
                "the TIFF holds {tiff_means:?}, the source develops to {want_means:?}"
            );
        }

        // And the embedded preview, which is its own path into the
        // working space and was the one that was wrong.
        let preview = stack_preview(&merged, balance, 32).expect("a preview");
        assert!(preview.width() <= 32 && preview.width() > 8, "{preview:?}");
        let preview_means = channel_means(preview.as_raw().iter().map(|v| {
            let v = *v as f32 / 255.0;
            // Undo the sRGB curve the preview writes.
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }));
        println!("preview {preview_means:?}");
        for (a, b) in preview_means.iter().zip(&want_means) {
            assert!(
                (a - b).abs() < 0.03,
                "the preview renders {preview_means:?}, the source develops to {want_means:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A format the merge is not written in is refused before anything
    /// is created, so a bad name cannot truncate a file that was there.
    #[test]
    fn a_refused_format_creates_nothing() {
        let dir = std::env::temp_dir().join(format!("greycard-stack-fmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let there = dir.join("out.jpg");
        std::fs::write(&there, b"not to be touched").expect("a file to leave alone");
        let files = [dir.join("a.cr3"), dir.join("b.cr3")];
        let e = stack_files(
            &files,
            Some(&there),
            Reference::Middle,
            2.0,
            0.6,
            false,
            OnExists::Overwrite,
        )
        .expect_err("a jpg");
        assert!(e.to_string().contains("neither"), "{e}");
        assert_eq!(
            std::fs::read(&there).expect("the file is still there"),
            b"not to be touched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_frame_is_not_a_stack() {
        let one = [PathBuf::from("a.cr3")];
        let e = stack_files(
            &one,
            Some(std::path::Path::new("out.dng")),
            Reference::Middle,
            2.0,
            0.6,
            false,
            OnExists::Skip,
        )
        .expect_err("one frame");
        assert!(e.to_string().contains("at least two"), "{e}");
    }
}
