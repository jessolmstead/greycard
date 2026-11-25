//! Keeping the open frame's pictures when its edit is saved.

use super::*;

/// A save of the open frame (`Job::Keep`): its edit's pictures made
/// from the develop the worker holds when that is of this edit, on the
/// keeping thread; a reset edit's taken out of the cache; a frame not
/// open, or `leaving` with no develop of its edit in hand, held as
/// edited and its key read again, so it shows the camera's until a
/// picture of its edit is kept. Answers the save back when its develop
/// is still to land.
pub(super) fn keep_open(
    keep: Keep,
    leaving: bool,
    hand: InHand<'_>,
    ai: &mut Ai,
    edited: &Arc<crate::edited_pictures::Edited>,
    deliver: &Deliver,
) -> Option<Keep> {
    let frame = keep.frame.clone();
    let path = frame.path.clone();
    let name = frame.said();
    let said = |frame: Frame| deliver(Outcome::EditedKept { frame });
    let is_edited = crate::edited_pictures::is_edited(&path, &keep.edit, keep.iso);
    if !is_edited {
        let hash = greycard_library::hash_file(&path).ok();
        if edited.reset(&frame, hash.as_deref()) {
            said(frame);
        }
        return None;
    }
    let left = |edited: &crate::edited_pictures::Edited| {
        edited.note(
            &frame,
            true,
            keep.iso,
            Some(edited.key_of(&path, &keep.edit)),
        );
        said(frame.clone());
    };
    // The develop in hand is the open file's under the edit saved: a
    // version of the open file saved from its own cell is the cell on
    // screen, whose edit the develop is of.
    if hand.opened != Some(path.as_path()) || !edited.cache_on() {
        left(edited);
        return None;
    }
    let Ok(hash) = greycard_library::hash_file(&path) else {
        left(edited);
        return None;
    };
    let key = edited.key_of(&path, &keep.edit);
    // Kept already from a develop of the frame (a save that changed
    // nothing of the picture, or a develop of an edit kept before): the
    // cells are asked again only when they were showing something else.
    // The reduced develop's pictures of the key do not count: this
    // develop's take their place.
    if edited.all_kept_by(&hash, key, crate::edited_pictures::EDITED_RECIPE) {
        if edited.settle_key(&frame, &hash, key, keep.iso) {
            said(frame);
        }
        return None;
    }
    // Being made already, from a save or a develop just before: that
    // making says when it is kept, and nothing waits here.
    let marked = edited.mark(&hash, key, crate::edited_pictures::EDITED_RECIPE)?;
    // The develop in hand of this edit at this turn: the frame's turn is
    // not in the key, but the picture in hand is turned by it, and the
    // making takes it back out.
    let Some(base) = hand.base else {
        if leaving {
            left(edited);
            return None;
        }
        return Some(keep);
    };
    // Its fills are still being made: the pictures wait for the last,
    // as for a develop still to land.
    if base.filling {
        if leaving {
            left(edited);
            return None;
        }
        return Some(keep);
    }
    let lenses = hand.lenses;
    let of_keep = |edit: &Edit, turn: u8| {
        turn == keep.turn
            && crate::stages::same_develop(edit, &keep.edit, &base.facts_at(turn), lenses)
    };
    // On the CPU the window's walk keeps its developed picture; on the
    // device it is the texture on screen.
    let cpu = base
        .developed_as
        .as_ref()
        .filter(|(edit, turn)| of_keep(edit, *turn))
        .and_then(|_| base.kept.picture(crate::stages::Keep::Developed).cloned())
        .map(|image| (image, base.left_out.clone()));
    // A texture sharpened only as far as the window showed it is not
    // the edit's picture: the save waits for the settle.
    let gpu = hand.shown.filter(|s| !s.quick && of_keep(&s.edit, s.turn));
    let (cpu, gpu) = match (cpu, gpu, hand.gpu) {
        (Some(l), _, _) => (Some(l), None),
        (None, Some(s), Some(ctx)) => (None, Some((s, ctx))),
        _ => {
            if leaving {
                left(edited);
                return None;
            }
            return Some(keep);
        }
    };
    // A picture without something its edit asks for (a learned model
    // not downloaded or failed, a look not in the directory) is not the
    // edit's, and would be kept under the edit's key for good: none is
    // kept, and the frame shows the camera's until one can be.
    let (rasters, mut left_out) = learned_masks(&keep.edit, Some(&*base), ai);
    left_out.extend(
        cpu.as_ref()
            .map(|(_, l)| l)
            .or(gpu.map(|(s, _)| &s.left_out))
            .into_iter()
            .flatten()
            .cloned(),
    );
    if !left_out.is_empty() {
        for why in keep_lines(&left_out) {
            tracing::info!("the edit's pictures of {name}: not kept, {why}");
        }
        left(edited);
        return None;
    }
    let camera = base.facts_at(keep.turn).camera();
    let factor = |source: (u32, u32)| {
        crate::edited_pictures::factor_for(source, &keep.edit.geometry, camera)
    };
    let (developed, source, factor, read_back) = match (cpu, gpu) {
        (Some((image, _)), _) => {
            let source = (image.width as u32, image.height as u32);
            (
                crate::edited_pictures::Developed::Full(image),
                source,
                factor(source),
                None,
            )
        }
        (None, Some((s, ctx))) => {
            let source = (s.texture.width(), s.texture.height());
            let k = factor(source);
            let started = Instant::now();
            match crate::edited_pictures::read_back_reduced(ctx, &s.texture, k) {
                Ok(reduced) => (
                    crate::edited_pictures::Developed::Reduced(reduced),
                    source,
                    k,
                    Some(started.elapsed().as_secs_f64()),
                ),
                Err(e) => {
                    tracing::warn!("the edit's pictures of {name}: the read back failed: {e}");
                    left(edited);
                    return None;
                }
            }
        }
        (None, None) => unreachable!("one picture or the other, matched above"),
    };
    // The refined masks' picture, made here when the edit refines and
    // none is kept: a Refine edge moved is saved without a develop, and
    // its ask for the picture may not have run yet.
    let picture = super::masks::picture_for(base, &keep.edit);
    let job = crate::edited_pictures::Keeping {
        frame,
        hash,
        key,
        developed,
        factor,
        read_back,
        iso: keep.iso,
        marked,
        finish: crate::edited_pictures::Finish {
            turn: keep.turn,
            source,
            full: source,
            rasters,
            clip_level: base.meta.clip_level,
            guide: Some(super::masks::guide_with(base, picture)),
            kind: base.meta.source,
            white: base.white_shift(),
            detail: greycard_render::detail::DetailPlanes::with_sample(
                base.planes_for(&keep.edit),
                keep.edit
                    .samples_before_detail()
                    .then(|| base.patched_picture()),
                base.meta.clip_level,
            ),
            edit: keep.edit,
            camera,
        },
    };
    let (on, deliver) = (edited.clone(), deliver.clone());
    edited.keep_later(Box::new(move || {
        crate::edited_pictures::keep(&on, job, |frame| deliver(Outcome::EditedKept { frame }))
    }));
    None
}

/// What a keep's log says of `left_out`: each item once, and the People
/// masks with no person picked once between them, however many shapes
/// ask.
fn keep_lines(left_out: &[String]) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut asked = false;
    for why in left_out {
        if why.contains(crate::ai::PART_ASKS) {
            if !asked {
                lines.push(crate::ai::PART_ASKS);
            }
            asked = true;
        } else if !lines.contains(&why.as_str()) {
            lines.push(why.as_str());
        }
    }
    lines
}

#[cfg(test)]
mod keep_line_tests {
    use super::keep_lines;
    use crate::ai::PART_ASKS;

    /// A People mask with no person picked is said once a keep, however
    /// many of its shapes ask; anything else left out is said as it is.
    #[test]
    fn a_people_mask_asking_is_said_once() {
        let left_out = vec![
            format!("Lips 1's Lips shape: {PART_ASKS}; its mask written empty"),
            "the look Film".to_string(),
            format!("Eyes 1's Iris shape: {PART_ASKS}; its mask written empty"),
            format!("Skin 1's Skin shape: {PART_ASKS}; its mask written empty"),
        ];
        assert_eq!(keep_lines(&left_out), vec![PART_ASKS, "the look Film"]);
        assert!(keep_lines(&[]).is_empty());
    }
}
