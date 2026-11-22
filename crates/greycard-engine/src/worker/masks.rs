//! The learned masks' rasters, for an export, a save and the reduced
//! develop.

use super::*;

/// Learned masks' rasters by adjustment id and component.
type Rasters = std::collections::HashMap<(u64, usize), Arc<Raster>>;

/// The learned masks `edit` draws, made now if they are not yet, for
/// the picture `base` was made for; and what the edit names that could
/// not be had: a look not in the look directory, and each learned mask
/// whose model is not downloaded or failed.
pub(super) fn learned_masks(
    edit: &Edit,
    base: Option<&FrameKept>,
    ai: &mut Ai,
) -> (Rasters, Vec<String>) {
    let mut left_out: Vec<String> = look_left_out(edit).into_iter().collect();
    let Some(b) = base else {
        return (Rasters::new(), left_out);
    };
    let (rasters, masks) = learned_rasters(
        edit,
        ai,
        &On {
            base: b.base(),
            image: &b.image,
            retouched: retouched_of(b, Some(edit)),
            edit: &b.edit,
            source: b.meta.source,
            turn: b.turn,
        },
    );
    left_out.extend(masks);
    (rasters, left_out)
}

/// A look the edit names that the export cannot have, not in the
/// look directory or with no table for the picture's display curve:
/// the picture is finished without it, as the window draws it
/// without, and said in the Look section's own words.
pub(super) fn look_left_out(edit: &Edit) -> Option<String> {
    let greycard_edit::look::LutChoice::Named(name) = &edit.look_lut.lut else {
        return None;
    };
    let missed = crate::reduced::look_missing(edit, &greycard_edit_io::Stores::user());
    missed.then(|| {
        let why = if greycard_edit_io::looks::is_there(&edit.look_lut) {
            greycard_edit::look::mismatch(
                &greycard_edit_io::looks::list(),
                name,
                edit.display_curve,
            )
            .map(|(why, _)| why)
            .unwrap_or_else(|| "its table would not read".into())
        } else {
            "not in the look directory".into()
        };
        format!(
            "the look {name}: {}; written without it",
            why.trim_end_matches(['.', ' '])
        )
    })
}

/// A camera profile the edit names that the develop goes without, for
/// a raw, which alone a profile is read for: the picture is developed
/// under the file's own matrices, as the window draws it. Decided by
/// the develop's own (cached) read, so the two cannot disagree. One
/// not in the directory (a folder named like one included) is said as
/// a missing look is, and so is one there that will not read (cut
/// short, not a profile, not ours to read); the read logs why.
pub(super) fn profile_left_out(edit: &Edit, raw: bool) -> Option<String> {
    use greycard_edit::Loader;
    let greycard_edit::camera::ProfileChoice::Named(name) = &edit.camera.profile else {
        return None;
    };
    let stores = crate::stages::stores();
    if !raw || stores.profile(name).is_some() {
        return None;
    }
    let there = greycard_edit_io::profiles::path_in(stores.profiles.as_deref(), name)
        .is_some_and(|path| path.is_file());
    let why = if there {
        "it would not read"
    } else {
        "not in the profile directory"
    };
    Some(format!(
        "the camera profile {name}: {why}; written without it"
    ))
}

/// The picture the learned masks are made on, and what the models'
/// cache knows it by.
pub(super) struct On<'a> {
    /// Which picture it is, and the print its rasters are kept by.
    pub base: crate::ai::Base,
    pub image: &'a WorkingImage,
    /// The picture with its retouch applied, when a patch is on it.
    pub retouched: Option<crate::ai::Retouched<'a>>,
    /// The edit the picture was developed under.
    pub edit: &'a Edit,
    pub source: greycard_render::finish::Source,
    pub turn: u8,
}

/// Each learned mask `edit` draws, made now on `on` if it is not yet;
/// and each one whose model is not downloaded or failed, in words.
pub(super) fn learned_rasters(edit: &Edit, ai: &mut Ai, on: &On<'_>) -> (Rasters, Vec<String>) {
    let mut rasters = Rasters::new();
    let mut left_out = Vec::new();
    for a in &edit.adjustments {
        // An adjustment switched off, or with nothing live in its
        // mask, is not drawn (`finish::Local::of`): nothing of it
        // is made, and nothing of it is missed.
        if !a.enabled || a.mask.is_empty() {
            continue;
        }
        for (i, c) in a.mask.live() {
            // An Object with nothing picked yet asks for nothing.
            if !c.shape.is_learned() || !crate::ai::prompted(&c.shape) {
                continue;
            }
            match ai.raster_retouched(
                on.base,
                on.image,
                on.retouched,
                on.edit,
                on.source,
                on.turn,
                (a.id, i),
                &c.shape,
                None,
            ) {
                Ok((made, _)) => {
                    // A Part whose person is not settled here goes
                    // out empty, and the log says so.
                    if let Some(part) = &made.part {
                        let why = part.left_out();
                        left_out.push(format!(
                            "{}'s {} shape: {why}; its mask written empty",
                            a.name,
                            c.shape.name()
                        ));
                    }
                    rasters.insert((a.id, i), made.raster);
                }
                Err(why) => left_out.push(format!(
                    "{}'s {} shape: {why}; its mask written without it",
                    a.name,
                    c.shape.name()
                )),
            }
        }
    }
    (rasters, left_out)
}

/// The open frame's learned masks made already, read on `b` for
/// `edit` from the mask cache, no model run: each answered to the
/// window as the open's first picture goes, so the frame is drawn
/// with them the first time rather than without and then with, and
/// recorded as given, so a base moved after asks for it again. Only
/// the shapes in `learned`, those the window would ask for at once,
/// each read from the slot of the model it names. One a Fill patch
/// lies over is read as made again on the retouched picture under
/// those patches, from that picture's own slot. Left to the window's
/// ask after the picture, as it always was: a shape with no raster in
/// the cache (nor, under a fill, one made again under the same
/// patches); and one with a word for the status line (a Sky with no
/// sky), which the picture's own line would cover, and whose empty
/// raster draws as no raster does. The log says which went which way.
pub(super) fn kept_for_open(
    edit: &Edit,
    learned: &[(Key, &'static greycard_ai::Model)],
    b: &FrameKept,
    ai: &mut Ai,
    file: Option<&PathBuf>,
) -> Vec<Outcome> {
    let started = Instant::now();
    let frame = (b.image.width, b.image.height);
    let retouched = retouched_of(b, None);
    let mut kept = Vec::new();
    // What was left to the ask after the picture, and why; and how many
    // of those read were made again under fills.
    let (mut uncached, mut unremade, mut noted, mut remade) = (0, 0, 0, 0);
    for &(key, model) in learned {
        let Some(c) = edit
            .adjustments
            .iter()
            .find(|a| a.id == key.0)
            .and_then(|a| a.mask.components.get(key.1))
            .filter(|c| c.enabled && c.shape.is_learned() && crate::ai::prompted(&c.shape))
        else {
            continue;
        };
        let Some(made) = ai.raster_cached(frame, b.base(), b.turn, key, &c.shape, Some(model))
        else {
            uncached += 1;
            continue;
        };
        // The patches over the base's raster, as `raster_retouched`
        // measures them, name the raster made again under them.
        let under = retouched.and_then(|r| r.print_over(&made.raster));
        let made = match under {
            None => made,
            Some((print, _)) => {
                let again =
                    ai.remade_cached(frame, b.base(), b.turn, key, &c.shape, Some(model), print);
                let Some(again) = again else {
                    unremade += 1;
                    continue;
                };
                again
            }
        };
        if made.note.is_some() {
            noted += 1;
            continue;
        }
        remade += usize::from(under.is_some());
        ai.given(key, &c.shape, Some(model), under.map(|u| u.0), b.print);
        kept.push(Outcome::Mask {
            key,
            shape: c.shape.clone(),
            file: file.cloned(),
            raster: made.raster,
            provider: None,
            seconds: 0.0,
            note: None,
            part: None,
        });
    }
    if !kept.is_empty() {
        tracing::info!(
            "the open frame's masks: {} read from the mask cache in {:.3} s, before its picture{}",
            kept.len(),
            started.elapsed().as_secs_f64(),
            if remade > 0 {
                format!(" ({remade} made again under fills)")
            } else {
                String::new()
            }
        );
    }
    if uncached + unremade + noted > 0 {
        tracing::info!(
            "the open frame's masks: {} left to the ask after its picture ({uncached} not in the mask cache, {unremade} not made again under its fills yet, {noted} with a word for the status line)",
            uncached + unremade + noted
        );
    }
    kept
}

/// The base's picture with its retouch applied, when that is `edit`'s
/// retouch (any edit for `None`): the picture a learned shape a patch
/// meets is made on ([`Ai::raster_retouched`]).
pub(super) fn retouched_of<'a>(
    b: &'a FrameKept,
    edit: Option<&Edit>,
) -> Option<crate::ai::Retouched<'a>> {
    let patched = b.kept.patched()?;
    if edit.is_some_and(|e| !same_retouch(&e.retouch, patched.retouch)) {
        return None;
    }
    Some(crate::ai::Retouched {
        retouch: patched.retouch,
        image: patched.image,
        unmade: patched.unmade,
    })
}

/// Whether `applied`, a retouch as a develop applied it, is `edit`'s:
/// the same patches, a source the develop chose standing for one the
/// edit has not been given yet.
pub(super) fn same_retouch(edit: &Retouch, applied: &Retouch) -> bool {
    edit.enabled == applied.enabled
        && edit.patches.len() == applied.patches.len()
        && edit.patches.iter().zip(&applied.patches).all(|(p, q)| {
            p == q
                || (p.source.is_none()
                    && *p
                        == greycard_edit::retouch::Patch {
                            source: None,
                            ..q.clone()
                        })
        })
}

/// A learned component to be asked for again: its key, its shape, and
/// the model it was asked with.
pub(super) type Ask = (Key, Shape, Option<&'static greycard_ai::Model>);

/// The learned components the panel was given a raster of that the
/// develop of `edit` on `base` asks for again: those whose Fill patches
/// it changed, at once, and those it gave another base (a white
/// balance, a lens, a noise moved), to wait for the develops to settle
/// ([`Settling`]).
pub(super) fn to_ask_again(edit: &Edit, base: Option<&FrameKept>, ai: &Ai) -> (Vec<Ask>, Vec<Ask>) {
    let Some(b) = base else {
        return (Vec::new(), Vec::new());
    };
    let live = |key: Key, shape: &Shape| {
        edit.adjustments
            .iter()
            .find(|a| a.id == key.0)
            .and_then(|a| a.mask.components.get(key.1))
            .is_some_and(|c| c.enabled && c.shape == *shape)
    };
    (
        ai.to_ask_again(retouched_of(b, None), b.print, b.turn, live),
        ai.of_another_base(b.print, live),
    )
}

/// How long the develops must have been quiet before the masks a new
/// base asks for are made: a mask's model runs for seconds and cannot
/// be stopped, so one begun between two develops of a drag would hold
/// the next behind it.
pub(super) const SETTLE: std::time::Duration = std::time::Duration::from_secs(1);

/// The masks a new base asks for, held until the develops settle: each
/// develop puts its own in place of the last's, and the time they are
/// due moves with it, so a drag asks once, after its last develop.
pub(super) struct Settling {
    pub due: Instant,
    pub asks: Vec<Ask>,
}

/// After a develop at `now`: its asks of a new base held in `slot` until
/// [`SETTLE`] from now, in place of any held before (none when it asks
/// none: the base is the one the masks were made from again).
pub(super) fn settle(slot: &mut Option<Settling>, asks: Vec<Ask>, now: Instant) {
    *slot = (!asks.is_empty()).then(|| Settling {
        due: now + SETTLE,
        asks,
    });
}

/// The asks held in `slot`, taken once they are due at `now`.
pub(super) fn settled(slot: &mut Option<Settling>, now: Instant) -> Option<Vec<Ask>> {
    if slot.as_ref().is_some_and(|s| s.due <= now) {
        return slot.take().map(|s| s.asks);
    }
    None
}

/// How long until the asks held in `slot` are due, if any are held.
pub(super) fn settling_left(slot: &Option<Settling>, now: Instant) -> Option<std::time::Duration> {
    slot.as_ref().map(|s| s.due.saturating_duration_since(now))
}

/// How [`masks_for`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MasksMade {
    /// Every learned shape's raster is in the cache (or the edit draws
    /// none any more): the frame is asked for again.
    All,
    /// Not every one could be made (a model missing or failing, the
    /// develop refused): the frame waits for a model to be fetched.
    Short,
    /// It gave way between two shapes to a develop, a save, or a frame
    /// opened in the editor; the shapes made so far are in the cache.
    GaveWay,
}

/// The learned masks of `frame` under its line's edit, made and put in
/// the mask cache (`Job::MasksFor`): the frame is decoded and developed
/// reduced for the picture the models see, and each learned shape the
/// edit draws is made as an export of the frame would make it,
/// `waiting` asked between two whether to give way. The rasters go in
/// the cache under the reduced base's print, where the reduced develop
/// reads them; the open frame makes its own from the full develop.
pub(super) fn masks_for(
    frame: &Frame,
    ai: &mut Ai,
    lenses: Option<&greycard_lens::Database>,
    waiting: &dyn Fn() -> bool,
) -> MasksMade {
    let path = frame.path.as_path();
    let name = frame.said();
    let started = Instant::now();
    // A short met on the frame before is not this one's.
    ai.clear_shorts();
    let (mut sidecar, trouble) = greycard_edit_io::on_disk::read_sidecar_said(path);
    if trouble.is_some() {
        tracing::info!("the masks of {name}: its sidecar could not be read");
        return MasksMade::Short;
    }
    let Some(line) = sidecar.line_mut(frame.version()) else {
        tracing::info!("the masks of {name}: no such version in its sidecar");
        return MasksMade::Short;
    };
    let edit = std::mem::take(&mut line.current);
    let turn = sidecar.turn;
    if !crate::reduced::draws_learned(&edit) {
        return MasksMade::All;
    }
    // The models see the reduced develop without the retouch, as they
    // do the open frame's base, so the rasters go in the base's slots,
    // and a Fill, which the reduced develop refuses, does not stop them.
    let developed = match crate::reduced::develop_as(
        path,
        &edit,
        turn,
        lenses,
        &crate::stages::Purpose::ModelsSee,
    ) {
        Ok(developed) => developed,
        Err(why) => {
            tracing::info!("the masks of {name}: not made: {why}");
            return MasksMade::Short;
        }
    };
    ai.forget(Some(path.to_path_buf()));
    // Kept by the reduced base's print, where the reduced develop reads
    // them: the open frame and an export make their own from the full
    // develop.
    let base = crate::ai::Base {
        stamp: 1,
        print: ai.base_print(&edit, turn, crate::mask_base::Size::Reduced),
    };
    let (mut made, mut wanted) = (0, 0);
    for a in &edit.adjustments {
        if !a.enabled || a.mask.is_empty() {
            continue;
        }
        for (i, c) in a.mask.live() {
            if !c.shape.is_learned() || !crate::ai::prompted(&c.shape) {
                continue;
            }
            if wanted > 0 && waiting() {
                tracing::info!(
                    "the masks of {name}: {made} made, giving way to the editor; the rest after"
                );
                return MasksMade::GaveWay;
            }
            wanted += 1;
            // The raster's width is the frame's, not the half-size
            // picture's: the open frame makes the same one.
            let full = (developed.full.0 as usize, developed.full.1 as usize);
            match ai.raster_for(
                full,
                base,
                &developed.image,
                &edit,
                developed.kind,
                turn,
                (a.id, i),
                &c.shape,
                None,
            ) {
                Ok(_) => made += 1,
                Err(why) => tracing::info!(
                    "the masks of {name}: {}'s {} shape: {why}",
                    a.name,
                    c.shape.name()
                ),
            }
        }
    }
    tracing::info!(
        "the masks of {name}: {made} of {wanted} made for the reduced develop in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    if made == wanted {
        MasksMade::All
    } else {
        MasksMade::Short
    }
}

/// A person picked on one frame, looked for on another frame of the
/// set ([`Job::PersonOn`]): the frame decoded and developed reduced
/// under its line's edit, as [`masks_for`] does, and its people found
/// on the People model's look-neutral view of that.
pub(super) fn person_on(
    job: &PersonJob,
    ai: &mut Ai,
    lenses: Option<&greycard_lens::Database>,
) -> Result<crate::ai::PersonOn, String> {
    let started = Instant::now();
    // A Fill's patch is the open frame's alone, and refuses the reduced
    // develop; the people are found on the base, which it is not in.
    let developed = crate::reduced::develop_as(
        &job.frame.path,
        &job.edit,
        job.turn,
        lenses,
        &crate::stages::Purpose::ModelsSee,
    )
    .map_err(|why| why.to_string())?;
    // Another file: its hash is the one a person found here records,
    // and a short met on the frame before is not this one's.
    ai.forget(Some(job.frame.path.clone()));
    ai.clear_shorts();
    let found = ai.person_on(
        &developed.image,
        developed.kind,
        job.turn,
        job.carried.as_ref(),
        &job.wanted,
    );
    let what = match &found {
        Ok(crate::ai::PersonOn::Found(_)) => "found".to_string(),
        Ok(crate::ai::PersonOn::Settled) => "its own person decided already".to_string(),
        Ok(crate::ai::PersonOn::NotFound) => "not found".to_string(),
        Err(why) => format!("not looked for ({why})"),
    };
    tracing::info!(
        "the person picked: {what} on {} in {:.1} s",
        job.frame.said(),
        started.elapsed().as_secs_f64()
    );
    found
}

/// The picture the refined masks are made against
/// ([`greycard_render::refine::mask_picture`]): of `patched`, the
/// picture before the Detail section and the sharpen, which the range
/// shapes read while that section does something; so a refined edge
/// is the picture's own, with no Dehaze or Clarity halo in it, and no
/// mask's refined edge moves when another mask's shapes change. Kept
/// on the base for the next develop of the same picture.
pub(super) fn mask_picture(
    b: &mut FrameKept,
    patched: &Arc<WorkingImage>,
) -> Arc<greycard_render::refine::MaskPicture> {
    if let Some(k) = &b.mask_picture
        && Arc::ptr_eq(&k.patched, patched)
    {
        return k.picture.clone();
    }
    let started = Instant::now();
    let picture = Arc::new(greycard_render::refine::mask_picture(patched));
    tracing::info!(
        "the refined masks' picture: {:.3} s",
        started.elapsed().as_secs_f64()
    );
    b.mask_picture = Some(KeptMaskPicture {
        patched: patched.clone(),
        picture: picture.clone(),
    });
    picture
}

/// The picture `edit`'s refined masks are made against on `b`, kept
/// or made now from the base's patched picture: None when the edit
/// refines no mask.
pub(super) fn picture_for(
    b: &mut FrameKept,
    edit: &Edit,
) -> Option<Arc<greycard_render::refine::MaskPicture>> {
    if !greycard_render::refine::refines(edit) {
        return None;
    }
    let patched = b.patched_picture();
    Some(mask_picture(b, &patched))
}

/// `b`'s guide plane with the refined masks' `picture` beside it, when
/// there is one: a plane of its own, so the base's stays without.
pub(super) fn guide_with(
    b: &FrameKept,
    picture: Option<Arc<greycard_render::refine::MaskPicture>>,
) -> Arc<greycard_render::finish::Guide> {
    match picture {
        Some(p) => Arc::new(b.guide.with_picture(Some(p))),
        None => b.guide.clone(),
    }
}
