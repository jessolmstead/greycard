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
    edited: &Arc<crate::edited::Edited>,
    deliver: &Deliver,
) -> Option<Keep> {
    let frame = keep.frame.clone();
    let path = frame.path.clone();
    let name = frame.said();
    let said = |frame: Frame| deliver(Outcome::EditedKept { frame });
    let is_edited = crate::edited::is_edited(&path, &keep.edit, keep.iso);
    if !is_edited {
        let hash = greycard_library::hash_file(&path).ok();
        if edited.reset(&frame, hash.as_deref()) {
            said(frame);
        }
        return None;
    }
    let left = |edited: &crate::edited::Edited| {
        edited.note(&frame, true, keep.iso, Some(edited.key_of(&keep.edit)));
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
    let key = edited.key_of(&keep.edit);
    // Kept already from a develop of the frame (a save that changed
    // nothing of the picture, or a develop of an edit kept before): the
    // cells are asked again only when they were showing something else.
    // The reduced develop's pictures of the key do not count: this
    // develop's take their place.
    if edited.all_kept_by(&hash, key, crate::edited::EDITED_RECIPE) {
        if edited.settle_key(&frame, &hash, key, keep.iso) {
            said(frame);
        }
        return None;
    }
    // Being made already, from a save or a develop just before: that
    // making says when it is kept, and nothing waits here.
    let marked = edited.mark(&hash, key, crate::edited::EDITED_RECIPE)?;
    // The develop in hand of this edit at this turn: the frame's turn is
    // not in the key, but the picture in hand is turned by it, and the
    // making takes it back out.
    let cpu = hand
        .last
        .filter(|l| l.turn == keep.turn && l.edit.same_develop(&keep.edit));
    let gpu = hand
        .shown
        .filter(|s| s.turn == keep.turn && s.edit.same_develop(&keep.edit));
    let (cpu, gpu, base) = match (cpu, gpu, hand.gpu, hand.base) {
        (Some(l), _, _, Some(b)) => (Some(l), None, b),
        (None, Some(s), Some(ctx), Some(b)) => (None, Some((s, ctx)), b),
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
    let (rasters, mut left_out) = learned_masks(&keep.edit, Some(base), ai);
    left_out.extend(
        cpu.map(|l| &l.left_out)
            .or(gpu.map(|(s, _)| &s.left_out))
            .into_iter()
            .flatten()
            .cloned(),
    );
    if !left_out.is_empty() {
        for why in &left_out {
            tracing::info!("the edit's pictures of {name}: not kept, {why}");
        }
        left(edited);
        return None;
    }
    let factor = |source: (u32, u32)| crate::edited::factor_for(source, &keep.edit.geometry);
    let (developed, source, factor, read_back) = match (cpu, gpu) {
        (Some(l), _) => {
            let source = (l.image.width as u32, l.image.height as u32);
            (
                crate::edited::Developed::Full(l.image.clone()),
                source,
                factor(source),
                None,
            )
        }
        (None, Some((s, ctx))) => {
            let source = (s.texture.width(), s.texture.height());
            let k = factor(source);
            let started = Instant::now();
            match crate::edited::read_back_reduced(ctx, &s.texture, k) {
                Ok(reduced) => (
                    crate::edited::Developed::Reduced(reduced),
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
    let job = crate::edited::Keeping {
        frame,
        hash,
        key,
        developed,
        factor,
        read_back,
        iso: keep.iso,
        marked,
        finish: crate::edited::Finish {
            turn: keep.turn,
            source,
            full: source,
            rasters,
            clip_level: base.clip_level,
            guide: Some(base.guide.clone()),
            kind: base.source,
            white: base.white_shift(),
            detail: base
                .planes
                .as_ref()
                .filter(|k| k.serves_edit(&keep.edit))
                .map(|k| k.planes.clone()),
            edit: keep.edit,
        },
    };
    let (on, deliver) = (edited.clone(), deliver.clone());
    edited.keep_later(Box::new(move || {
        crate::edited::keep(&on, job, |frame| deliver(Outcome::EditedKept { frame }))
    }));
    None
}
