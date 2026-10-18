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
    base: Option<&Base>,
    ai: &mut Ai,
) -> (Rasters, Vec<String>) {
    let mut rasters = std::collections::HashMap::new();
    let mut left_out = Vec::new();
    // A look the edit names that the export cannot have, not in the
    // look directory or with no table for the picture's display curve:
    // the picture is finished without it, as the window draws it
    // without, and said in the Look section's own words.
    if let greycard_edit::look::LutChoice::Named(name) = &edit.look_lut.lut
        && edit.look_lut.strength > 0.0
        && edit.look_lut.look_under(edit.display_curve).is_none()
    {
        let why = if edit.look_lut.is_there() {
            greycard_edit::look::mismatch(&greycard_edit::look::list(), name, edit.display_curve)
                .map(|(why, _)| why)
                .unwrap_or_else(|| "its table would not read".into())
        } else {
            "not in the look directory".into()
        };
        left_out.push(format!(
            "the look {name}: {}; written without it",
            why.trim_end_matches(['.', ' '])
        ));
    }
    if let Some(b) = base {
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
                match ai.raster(
                    b.stamp,
                    &b.image,
                    &b.edit,
                    b.source,
                    b.turn,
                    (a.id, i),
                    &c.shape,
                    None,
                ) {
                    Ok(made) => {
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
    }
    (rasters, left_out)
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
/// the cache the open frame's masks are read from, made from the
/// reduced develop's picture, which the models see at their own 2048
/// anyway.
pub(super) fn masks_for(
    frame: &Frame,
    ai: &mut Ai,
    lenses: Option<&greycard_lens::Database>,
    waiting: &dyn Fn() -> bool,
) -> MasksMade {
    let path = frame.path.as_path();
    let name = frame.said();
    let started = Instant::now();
    let (mut sidecar, trouble) = crate::panel::browser::load_sidecar_said(path, true);
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
    let developed = match crate::reduced::develop(path, &edit, turn, lenses) {
        Ok(developed) => developed,
        Err(why) => {
            tracing::info!("the masks of {name}: not made: {why}");
            return MasksMade::Short;
        }
    };
    ai.forget(Some(path.to_path_buf()));
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
            match ai.raster(
                1,
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
    let mut edit = job.edit.clone();
    edit.retouch = Default::default();
    let developed = crate::reduced::develop(&job.frame.path, &edit, job.turn, lenses)
        .map_err(|why| why.to_string())?;
    // Another file: its hash is the one a person found here records.
    ai.forget(Some(job.frame.path.clone()));
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
