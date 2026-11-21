//! The export path: opening a frame for it, and writing its picture.

use super::*;
use crate::walk::NoLearned;

/// The open file written to `path` under `edit` at `turn`: walked
/// through its stage list for the export from what the window's walks
/// kept, `held`, and finished under the sheet's `settings`. The walk
/// resumes from a copy of the window's points, sharing their pictures:
/// what it keeps and lets go of in resuming is the copy's, so the
/// window's points (on the device too) are as they were for its next
/// develop. With `gpu` the export's device stages run there as the
/// viewport's do, so the viewport's points serve it; without one it
/// resumes only from a point the CPU made. What of the edit was left
/// out.
#[allow(clippy::too_many_arguments)]
pub(super) fn export_open(
    edit: &Edit,
    turn: u8,
    input: Option<&Input>,
    held: Option<&FrameKept>,
    models: &mut WorkerModels<'_>,
    lenses: Option<&greycard_lens::Database>,
    gpu: &mut Option<greycard_gpu::Context>,
    written: Written<'_>,
) -> Result<Vec<String>, String> {
    let Some(f) = input else {
        return Err("no file is open".into());
    };
    let (rendered, left_out) =
        render_open(edit, turn, f, held, models, lenses, gpu, written.settings)?;
    write_rendered(&rendered, edit, written)?;
    Ok(left_out)
}

/// [`export_open`]'s pixels, with what of the edit was left out.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_open(
    edit: &Edit,
    turn: u8,
    input: &Input,
    held: Option<&FrameKept>,
    models: &mut WorkerModels<'_>,
    lenses: Option<&greycard_lens::Database>,
    gpu: &mut Option<greycard_gpu::Context>,
    settings: &greycard_render::export::Settings,
) -> Result<(greycard_render::export::Rendered, Vec<String>), String> {
    let mut kept = held.map(|h| h.kept.clone()).unwrap_or_default();
    let (image, finish, mut left_out) = walk_for_export(
        input.walked(),
        edit,
        turn,
        models,
        lenses,
        settings,
        &mut kept,
        gpu,
    )?;
    drop(kept);
    let rendered = crate::finish::finish_file(&image, edit, &finish, settings)
        .map_err(|e| format!("{e:#}"))?;
    left_out.extend(lens_left_out(input, edit, lenses));
    Ok((rendered, left_out))
}

/// Where and how an export's file is written: the sheet's `settings`,
/// at `path`, with the source's EXIF from `metadata`, its capture times
/// moved by the frame's `time_shift`.
pub(super) struct Written<'a> {
    pub settings: &'a greycard_render::export::Settings,
    pub path: &'a std::path::Path,
    pub source: Option<&'a std::path::Path>,
    pub metadata: Option<&'a greycard_core::decode::RawMetadata>,
    pub time_shift: i64,
}

/// The lens picked by hand that the export goes without, when it does
/// ([`crate::lens::left_out`]).
fn lens_left_out(
    input: &Input,
    edit: &Edit,
    lenses: Option<&greycard_lens::Database>,
) -> Option<String> {
    let (make, model, shot) = input.identity();
    crate::lens::left_out((make, model, shot, input.is_raw()), &edit.lens, lenses)
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

/// A frame of a set that is not the open file: opened, walked through
/// its stage list for the export ([`walk_export`]), and written to
/// `path`, the open file's base and learned pair left as they were for
/// the next develop. The edit it was written under, and what of it
/// was left out. With `gpu`, its device stages run there.
#[allow(clippy::too_many_arguments)]
pub(super) fn export_other(
    frame: &crate::queue::Frame,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    lenses: Option<&greycard_lens::Database>,
    gpu: &mut Option<greycard_gpu::Context>,
    deliver: &Deliver,
    settings: &greycard_render::export::Settings,
    path: &std::path::Path,
) -> Result<(Edit, Vec<String>), String> {
    let (input, metadata) = timed_open(&frame.source).map_err(|e| format!("{e:#}"))?;
    let edit = seeded(&frame.edit, frame.seed_blend, Some(&input));
    ai.forget(Some(frame.source.clone()));
    let lens_words = lens_left_out(&input, &edit, lenses);
    let (rendered, mut left_out) = walk_export(
        input,
        &edit,
        frame.turn % 4,
        // A set frame is no develop of the window's: its fills are
        // said under the generation no develop has.
        &mut WorkerModels::new(ai, cache, deliver, 0),
        lenses,
        gpu,
        settings,
    )?;
    left_out.extend(lens_words);
    write_rendered(
        &rendered,
        &edit,
        Written {
            settings,
            path,
            source: Some(&frame.source),
            metadata: Some(&metadata),
            time_shift: frame.time_shift,
        },
    )?;
    Ok((edit, left_out))
}

/// `input` walked under `edit` at `turn` through its stage list for
/// the export, from nothing kept, with `models`'s models and on `gpu`
/// where the plan puts a stage there, and finished under the sheet's
/// `settings` with its border and mark: the file's pixels. Beside
/// them, what of the edit was left out, in [`left_out_of`]'s words and
/// then [`learned_masks`]'.
pub(super) fn walk_export(
    input: Input,
    edit: &Edit,
    turn: u8,
    models: &mut WorkerModels<'_>,
    lenses: Option<&greycard_lens::Database>,
    gpu: &mut Option<greycard_gpu::Context>,
    settings: &greycard_render::export::Settings,
) -> Result<(greycard_render::export::Rendered, Vec<String>), String> {
    let mut kept = crate::walk::Kept::new();
    let (image, finish, left_out) = walk_for_export(
        input.walked(),
        edit,
        turn,
        models,
        lenses,
        settings,
        &mut kept,
        gpu,
    )?;
    drop(kept);
    let rendered = crate::finish::finish_file(&image, edit, &finish, settings)
        .map_err(|e| format!("{e:#}"))?;
    Ok((rendered, left_out))
}

/// `input` walked under `edit` at `turn` through its stage list for
/// the export, from what `kept` holds, with `models`'s models, its
/// device stages on `gpu` where one is lent (a declined or failed
/// op runs on the CPU, as the viewport's does): the picture before the
/// finish and what the finish reads beside it. Beside them, what of the edit was left out, in
/// [`left_out_of`]'s words and then [`learned_masks`]'. The walk keeps
/// nothing new in `kept` but the patched picture a learned mask beside
/// a Fill is made on.
#[allow(clippy::too_many_arguments)]
pub(super) fn walk_for_export(
    input: crate::walk::Input,
    edit: &Edit,
    turn: u8,
    models: &mut WorkerModels<'_>,
    lenses: Option<&greycard_lens::Database>,
    settings: &greycard_render::export::Settings,
    kept: &mut crate::walk::Kept,
    gpu: &mut Option<greycard_gpu::Context>,
) -> Result<(Arc<WorkingImage>, crate::finish::FinishInput, Vec<String>), String> {
    use crate::stages::{Keep, LEARNED_DENOISER, Purpose, Skip, Stage, Step, Tap, stages};
    use crate::walk::{Buf, ExportHold, plan, walk};

    let started = Instant::now();
    models.edit = edit.clone();
    models.turn = turn;
    models.run_unmade = true;
    models.patient = true;
    models.short = None;
    let purpose = Purpose::Export(Box::new(settings.clone()));
    let facts = input.facts(turn);
    let mut steps = stages(edit, &purpose, &facts, lenses);
    // Nothing is kept for a later walk: each kept point holds a picture
    // the size of the frame, and the stage after it would copy it
    // rather than change it. The patched picture alone stays when a
    // learned mask is made beside a Fill, which it is made on where the
    // Fill meets it; no other patch changes what a mask is made on.
    let on_patched = steps
        .steps
        .iter()
        .any(|s| matches!(s, Step::Tap(Tap::Masks(_))))
        && steps.ops().any(|s| {
            matches!(s, Stage::Retouch(r, _)
                if r.patches.iter().any(|p| p.method == greycard_edit::retouch::Method::Fill))
        });
    steps.steps.retain(|s| match s {
        Step::Keep(Keep::Patched) => on_patched,
        Step::Keep(_) => false,
        _ => true,
    });
    let plan = plan(&steps, &purpose, gpu.is_some());
    let walked =
        walk(&steps, &plan, &input, kept, models, gpu, &ExportHold).map_err(|e| e.to_string())?;
    drop(input);
    debug_assert!(walked.without.is_empty(), "an export runs its fills");
    // A model the card stayed short of fails the item, rather than
    // write a picture without what the edit asks for.
    if let Some(why) = models.short.take() {
        // A removal's said as the window says it; the reason is the
        // log's.
        if let Some(&(_, need, have)) = models.fills.short.first() {
            tracing::info!("{why}; not written");
            return Err(crate::ai::FillNotHere::Card { need, have }.words());
        }
        return Err(format!("{why}; not written"));
    }
    if let Some(NoLearned::Short(why)) = &walked.meta.stood_in {
        return Err(format!("{why}; not written"));
    }
    let fills = std::mem::take(&mut models.fills);
    let masks = std::mem::take(&mut models.masks);
    let learned = std::mem::replace(&mut models.learned, LearnedReport::Off);
    // The learned denoiser as the worker's develop reports it: what
    // stood in for it, or the mosaic it has no version for.
    let unavailable = steps.skipped.iter().find_map(|(name, why)| match why {
        Skip::Unavailable(why) if *name == LEARNED_DENOISER => Some(*why),
        _ => None,
    });
    for (name, why) in super::develop::stood_down(&steps) {
        tracing::info!("{name} stood down: {why}");
    }
    let learned = match (&walked.meta.stood_in, unavailable) {
        (Some(no), _) => super::develop::stood_in_report(no),
        (None, Some(why)) => LearnedReport::Unavailable(why),
        (None, None) => learned,
    };
    let mut left_out = left_out_words(&learned, walked.meta.gave_way.as_ref(), &fills);
    left_out.extend(super::masks::profile_left_out(edit, facts.is_raw()));
    left_out.extend(super::masks::look_left_out(edit));
    left_out.extend(masks);
    let mut walked = walked;
    // Neither is read by the finish: the base the masks were made on,
    // and the sharpen's blend, which the viewport alone paints.
    walked.taps.masks = None;
    walked.sharpen_mask = None;
    let mut finish = walked.finish_input();
    finish.camera = facts.camera();
    let Buf::Working(image) = walked.buf else {
        unreachable!("an export's walk ends at a picture in memory")
    };
    // The develop's line, its denoiser and fills in its words.
    super::develop::warn_failed_fills(&fills);
    let parts: Vec<String> = super::develop::learned_part(&learned)
        .into_iter()
        .chain(
            walked
                .seconds
                .iter()
                .map(|(stage, s)| format!("{stage} {s:.2} s")),
        )
        .chain(super::develop::fill_parts(&fills))
        .collect();
    tracing::info!(
        "developed {}x{} for the export: {}, total {:.2} s",
        image.width,
        image.height,
        parts.join(", "),
        started.elapsed().as_secs_f64()
    );
    Ok((image, finish, left_out))
}

/// The file of `rendered`, finished under `edit`, as `written` says:
/// the source's EXIF with its capture times moved by the frame's time
/// shift, and its name and edit as where it came from.
fn write_rendered(
    rendered: &greycard_render::export::Rendered,
    edit: &Edit,
    written: Written<'_>,
) -> Result<(), String> {
    let shifted = written
        .metadata
        .map(|m| greycard_core::exif::with_capture_shifted(m, written.time_shift));
    let origin = greycard_render::export::Origin {
        source_name: written
            .source
            .and_then(|s| s.file_name())
            .map(|n| n.to_string_lossy().into_owned()),
        edit: Some(edit.to_json()),
    };
    greycard_render::export::write(
        rendered,
        written.settings,
        written.path,
        shifted.as_ref(),
        &origin,
    )
    .map_err(|e| format!("{e:#}"))
}
