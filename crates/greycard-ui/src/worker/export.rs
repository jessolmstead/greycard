//! The export path: opening a frame for it, and writing its picture.

use super::*;

/// The open file's picture under `edit` at `turn`: the last develop's
/// when it was of the same develop and turn, else developed afresh.
/// The export's picture is the reference's, on the CPU, whatever the
/// viewport ran on. If the last develop had its CA or its lens on the
/// GPU, redevelop.
#[allow(clippy::too_many_arguments)]
pub(super) fn open_picture(
    edit: &Edit,
    turn: u8,
    input: Option<&Input>,
    last: &mut Option<Last>,
    base: &mut Option<Base>,
    learned: &mut Option<LearnedBase>,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    lenses: Option<&greycard_lens::Database>,
    deliver: &Deliver,
) -> Result<(Arc<WorkingImage>, Vec<String>), String> {
    if let Some(l) = &*last
        && l.turn == turn
        && l.edit.same_develop(edit)
        && !l.on_gpu
    {
        return Ok((l.image.clone(), l.left_out.clone()));
    }
    let Some(f) = input else {
        return Err("no file is open".into());
    };
    match develop_job(
        f, edit, turn, 0, base, learned, ai, cache, lenses, &mut None, deliver,
    ) {
        (outcome, Some(image)) => {
            *last = base
                .as_ref()
                .map(|b| Last::made(edit.clone(), b, image.clone(), &outcome));
            Ok((image, left_out_of(&outcome, base.as_ref())))
        }
        (Outcome::Failed { message, .. }, None) => Err(message),
        _ => Err("the develop made no picture".into()),
    }
}

/// A set frame's edit with its learned-denoiser blend seeded from the
/// raw's ISO when it has no sidecar yet, as its first open would.
pub(super) fn seeded(edit: &Edit, seed: bool, input: Option<&Input>) -> Edit {
    let mut edit = edit.clone();
    if seed && let Some(Input::Raw(f)) = input {
        edit.noise.learned_strength = Noise::blend_for_iso(f.shot.iso);
    }
    edit
}

/// A frame of a set that is not the open file: opened and developed
/// here under its own edit, with a base and a learned pair of its
/// own so the open file's are there as they were for the next
/// develop, then written to `path`. The edit it was written under,
/// and what of it was left out ([`left_out_of`]).
pub(super) fn export_other(
    frame: &crate::queue::Frame,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    lenses: Option<&greycard_lens::Database>,
    deliver: &Deliver,
    settings: &crate::export::Settings,
    path: &std::path::Path,
) -> Result<(Edit, Vec<String>), String> {
    let (input, metadata) = timed_open(&frame.source).map_err(|e| format!("{e:#}"))?;
    let edit = seeded(&frame.edit, frame.seed_blend, Some(&input));
    ai.forget(Some(frame.source.clone()));
    let (mut base, mut learned) = (None, None);
    let (image, mut left_out) = match develop_job(
        &input,
        &edit,
        frame.turn % 4,
        0,
        &mut base,
        &mut learned,
        ai,
        cache,
        lenses,
        &mut None,
        deliver,
    ) {
        (outcome, Some(image)) => (image, left_out_of(&outcome, base.as_ref())),
        (Outcome::Failed { message, .. }, None) => return Err(message),
        _ => return Err("the develop made no picture".into()),
    };
    left_out.extend(write_export(
        image,
        &edit,
        base.as_ref(),
        ai,
        Some(&frame.source),
        Some(&metadata),
        frame.time_shift,
        settings,
        path,
    )?);
    Ok((edit, left_out))
}

/// Finish `image`, the develop `base` was made for, under `edit` and
/// the sheet's `settings`, and write it to `path`: the learned masks
/// made now if they are not yet, the edit's geometry, the finish, the
/// mark, and the file with the source's EXIF, its capture times moved
/// by the frame's `time_shift`. What of the edit's masks could not be
/// made, and so was left out.
#[allow(clippy::too_many_arguments)]
pub(super) fn write_export(
    image: Arc<WorkingImage>,
    edit: &Edit,
    base: Option<&Base>,
    ai: &mut Ai,
    source_path: Option<&std::path::Path>,
    metadata: Option<&greycard_core::decode::RawMetadata>,
    time_shift: i64,
    settings: &crate::export::Settings,
    path: &std::path::Path,
) -> Result<Vec<String>, String> {
    let shifted = metadata.map(|m| greycard_core::exif::with_capture_shifted(m, time_shift));
    let metadata = shifted.as_ref();
    let (mut rendered, left_out) = finish_export(image, edit, base, ai, settings);
    let origin = crate::export::Origin {
        source_name: source_path
            .and_then(|s| s.file_name())
            .map(|n| n.to_string_lossy().into_owned()),
        edit: Some(edit.to_json()),
    };
    // The border and then the mark, on the export alone; a mark that
    // cannot be drawn fails the export rather than let an unmarked
    // picture out.
    crate::export::border(&mut rendered, settings);
    crate::export::mark(&mut rendered, settings)
        .and_then(|()| crate::export::write(&rendered, settings, path, metadata, &origin))
        .map_err(|e| format!("{e:#}"))?;
    Ok(left_out)
}

/// What [`finish_export`] would hand the finish for `image` under
/// `edit` and `settings`, before any pixel is finished
/// ([`crate::export::Scene`]): the edit's geometry, the resize, the
/// guide plane and the vignette, as the export has them. No masks: the
/// measurement this serves carries none.
pub(super) fn scene_export(
    image: &WorkingImage,
    edit: &Edit,
    base: Option<&Base>,
    settings: &crate::export::Settings,
) -> crate::export::Scene {
    let source = (image.width as u32, image.height as u32);
    let framed;
    let image: &WorkingImage = if edit.geometry.is_identity() {
        image
    } else {
        framed = crate::geometry::apply(image, &edit.geometry);
        &framed
    };
    crate::export::scene_framed(image, edit, source, settings, base.map(|b| &*b.guide))
}

/// `image`, the develop `base` was made for, finished under `edit` and
/// `settings` as an export is: the learned masks made now if they are
/// not yet, the edit's geometry, then the finish. No mark and no file.
/// Beside it, what the edit names that could not be had, which the
/// picture is finished without, as the window draws it without: a
/// look not in the look directory, and each learned mask whose model
/// is not downloaded or failed.
pub(super) fn finish_export(
    image: Arc<WorkingImage>,
    edit: &Edit,
    base: Option<&Base>,
    ai: &mut Ai,
    settings: &crate::export::Settings,
) -> (crate::export::Rendered, Vec<String>) {
    let source = (image.width as u32, image.height as u32);
    let (rasters, left_out) = learned_masks(edit, base, ai);
    let framed;
    let image: &WorkingImage = if edit.geometry.is_identity() {
        &image
    } else {
        framed = crate::geometry::apply(&image, &edit.geometry);
        &framed
    };
    let clip_level = base.map(|b| b.clip_level).unwrap_or(f32::INFINITY);
    // The masks' bands through the same geometry as the picture, when
    // they were made for this edit and this picture's size.
    let planes = base
        .and_then(|b| b.planes.as_ref())
        .filter(|k| k.serves_edit(edit))
        .map(|k| &k.planes)
        .filter(|p| (p.width as u32, p.height as u32) == source);
    let detail = planes.map(|p| {
        let bands = p.to_image();
        if edit.geometry.is_identity() {
            bands
        } else {
            crate::geometry::apply(&bands, &edit.geometry)
        }
    });
    let rendered = crate::export::render(
        image,
        edit,
        source,
        settings,
        &rasters,
        clip_level,
        base.map(|b| &*b.guide),
        base.map(|b| b.source).unwrap_or_default(),
        base.and_then(Base::white_shift).as_ref(),
        detail.as_ref().zip(planes.map(|p| &p.meta)),
    );
    (rendered, left_out)
}
