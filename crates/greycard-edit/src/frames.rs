//! Changes laid over a set of frames at once: a turn, the meta, a
//! preset, and a sync.

use crate::{Edit, LineMut, Preset, Section, Sidecar, meta};

/// Turn every frame in `frames` by `quarters` quarter turns
/// clockwise; the frames it moved.
///
/// No history state is recorded and none is changed — a turn is no
/// part of the edit's line — and every state a sidecar holds is shown
/// under the new turn, so that what undo, redo and a snapshot hand back
/// means the pixels the current state means (see
/// [`Sidecar::turn_by`]). `aspect` is each frame's shape as the caller
/// measured it, by its index, when it can; a frame nothing can measure
/// is not turned. A frame past the end of the list is passed over
/// rather than a panic; a selection can outlive a folder change.
pub fn turn_into(
    sidecars: &mut [Sidecar],
    frames: &[usize],
    quarters: i32,
    mut aspect: impl FnMut(usize) -> Option<f32>,
) -> Vec<usize> {
    if quarters.rem_euclid(4) == 0 {
        return Vec::new();
    }
    frames
        .iter()
        .copied()
        .filter(|&i| match sidecars.get_mut(i) {
            Some(sidecar) => sidecar.turn_by(quarters, aspect(i)).is_some(),
            None => false,
        })
        .collect()
}

/// Put `change` into the meta of every frame in `frames`, settled
/// across the set first; the settled change and the frames it moved.
///
/// Nothing here touches `current`, `history` or the snapshots: the
/// sidecar holds the meta beside the edit, and this writes only the
/// one field. A frame past the end of the list is passed over rather
/// than a panic; a selection can outlive a folder change.
pub fn meta_into(
    sidecars: &mut [Sidecar],
    frames: &[usize],
    change: meta::Change,
) -> (meta::Change, Vec<usize>) {
    let change = change.settled(
        frames
            .iter()
            .filter_map(|&i| sidecars.get(i))
            .map(|s| &s.meta),
    );
    let moved = frames
        .iter()
        .copied()
        .filter(|&i| {
            sidecars
                .get_mut(i)
                .is_some_and(|sidecar| change.apply(&mut sidecar.meta))
        })
        .collect();
    (change, moved)
}

/// Lay `preset`'s carried sections over the current edit of every
/// frame in `frames`, each as one step of its history; the frames it
/// moved. `learned_from` is a sync's own difference from a plain
/// preset apply: `Some` source edit carries its learned denoiser's
/// tier and blend along with Noise ([`sync_learned`]), which is not
/// something `Preset::apply` ever does (a preset leaves those to each
/// file's own ISO, so they are not in `preset.edit` even when the
/// preset came from [`Preset::from_edit`] and Noise was chosen; the
/// source edit itself is needed). A frame that already has every
/// carried section as `preset` has it (and, when asked, the source's
/// learned tier) records nothing and is left out of what comes back,
/// so its sidecar is not rewritten. Nothing but `current` and
/// `history` is touched: the meta, the turn and the snapshots are the
/// frame's own. A frame past the end of the list is passed over
/// rather than a panic, as [`meta_into`] does. Each step is recorded
/// with `label` as its words ([`Sidecar::record_as`]).
///
/// This is what both `panel::sync::lay_over_targets` (a sync, and a
/// preset click over two or more selected frames) build on, so a
/// section means the same thing however it reaches a frame.
pub fn apply_preset_into(
    sidecars: &mut [Sidecar],
    preset: &Preset,
    frames: &[usize],
    learned_from: Option<&Edit>,
    label: Option<&str>,
) -> Vec<usize> {
    if preset.is_empty() {
        return Vec::new();
    }
    frames
        .iter()
        .copied()
        .filter(|&i| {
            sidecars
                .get_mut(i)
                .and_then(|sidecar| sidecar.line_mut(None))
                .is_some_and(|mut line| {
                    apply_preset_to_line(&mut line, preset, learned_from, label)
                })
        })
        .collect()
}

/// [`apply_preset_into`] over one line: `preset`'s carried sections
/// over the line's current edit as one step of its history, with
/// `learned_from`'s learned denoiser when a sync brings one; whether
/// the line moved. What a version of a frame takes as its own, as the
/// frame's own line does.
pub fn apply_preset_to_line(
    line: &mut LineMut<'_>,
    preset: &Preset,
    learned_from: Option<&Edit>,
    label: Option<&str>,
) -> bool {
    if preset.is_empty() {
        return false;
    }
    let mut applied = preset.applied(&line.current);
    if let Some(from) = learned_from {
        sync_learned(from, &mut applied);
    }
    line.record_as(applied, label.map(str::to_string))
}

/// Lay `sections` of `from` over the current edit of every frame in
/// `frames`, each as one step of its history; the frames it moved.
///
/// This is a preset that is never written: [`Preset::from_edit`]
/// keeps the chosen sections of `from` and [`apply_preset_into`] lays
/// them over each frame, so a sync and a preset cannot come to mean
/// two different things by a section, but for one thing on top: a
/// sync's Noise brings the learned denoiser and its blend as well
/// ([`sync_learned`]). A preset leaves those to each file's ISO; a
/// sync is between frames of one shoot, and a frame taken to the
/// learned tier is the one the others are meant to look like.
/// `label` is each step's words, [`history::sync_label`] of the
/// frame synced from as the editor has it.
///
/// [`history::sync_label`]: crate::history::sync_label
pub fn sync_into(
    sidecars: &mut [Sidecar],
    from: &Edit,
    frames: &[usize],
    sections: &[Section],
    label: Option<&str>,
) -> Vec<usize> {
    let carried = Preset::from_edit("", from, sections);
    let learned = carried.sections.contains(&Section::Noise);
    apply_preset_into(sidecars, &carried, frames, learned.then_some(from), label)
}

/// What a sync's Noise carries beyond a preset's: the learned
/// denoiser's tier and its blends.
pub fn sync_learned(from: &Edit, onto: &mut Edit) {
    onto.noise.learned = from.noise.learned;
    onto.noise.learned_strength = from.noise.learned_strength;
    onto.noise.learned_chroma_strength = from.noise.learned_chroma_strength;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Adjustment, Learned, WhiteBalance, geometry, retouch};

    #[test]
    fn a_culling_key_moves_a_whole_set_and_leaves_their_edits_alone() {
        use crate::meta::{Change, Flag, Label};
        let mut sidecars = vec![Sidecar::default(); 3];
        let mut edit = Edit::default();
        edit.light.exposure = 1.0;
        assert!(sidecars[0].record(edit));
        let before = sidecars[0].clone();

        // Two of the three frames: the third is nobody's business.
        let (settled, moved) = meta_into(&mut sidecars, &[0, 1], Change::Rating(4));
        assert_eq!(settled, Change::Rating(4));
        assert_eq!(moved, [0, 1]);
        assert_eq!(sidecars[0].meta.rating, 4);
        assert_eq!(sidecars[1].meta.rating, 4);
        assert!(sidecars[2].meta.is_empty());
        // The edit, its history and its snapshots are where they were.
        assert_eq!(sidecars[0].current, before.current);
        assert_eq!(sidecars[0].history, before.history);
        assert_eq!(sidecars[0].snapshots, before.snapshots);
        // The same key again writes nothing, so no sidecar is
        // rewritten for a key that said what was there already.
        let (_, moved) = meta_into(&mut sidecars, &[0, 1], Change::Rating(4));
        assert!(moved.is_empty());

        // A label key sets it, and the same key over a set that all
        // carries it takes it off.
        meta_into(&mut sidecars, &[0, 1], Change::Label(Label::Red));
        assert_eq!(sidecars[0].meta.label, Label::Red);
        let (settled, moved) = meta_into(&mut sidecars, &[0, 1], Change::Label(Label::Red));
        assert_eq!(settled, Change::Label(Label::None));
        assert_eq!(moved, [0, 1]);
        assert_eq!(sidecars[1].meta.label, Label::None);
        // The rating is none of the label's business, nor the flag's.
        meta_into(&mut sidecars, &[0], Change::Flag(Flag::Reject));
        assert_eq!(sidecars[0].meta.rating, 4);
        assert_eq!(sidecars[0].meta.flag, Flag::Reject);

        // A frame that is not there any more is passed over.
        let (_, moved) = meta_into(&mut sidecars, &[9], Change::Rating(1));
        assert!(moved.is_empty());
    }

    #[test]
    fn a_sync_lays_the_chosen_sections_over_the_set_as_one_step_each() {
        use crate::meta::Change;
        // The frame synced from: a look, a white balance, a crop, a
        // mask and a patch of its own.
        let mut from = Edit::default();
        from.light.exposure = 0.7;
        from.light.tone.contrast = 1.3;
        from.color.vibrance = 0.4;
        from.white_balance = WhiteBalance::Custom {
            temperature: 3200.0,
            tint: 0.002,
        };
        from.geometry.crop = Some(geometry::Crop {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        });
        from.adjustments.push(Adjustment {
            id: 9,
            name: "Sky".into(),
            ..Adjustment::default()
        });
        from.retouch.patches.push(retouch::Patch::default());

        // Three others: one with an edit and a history of its own,
        // one fresh, one with a star.
        let mut sidecars = vec![Sidecar::default(); 4];
        let mut own = Edit::default();
        own.light.exposure = -1.0;
        own.sharpen.radius = 2.0;
        own.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.9,
            h: 0.9,
        });
        assert!(sidecars[1].record(own.clone()));
        meta_into(&mut sidecars, &[3], Change::Rating(3));
        sidecars[3].turn = 1;
        let before: Vec<Sidecar> = sidecars.clone();

        let sections = [Section::Light, Section::Color, Section::WhiteBalance];
        let moved = sync_into(&mut sidecars, &from, &[1, 2, 3, 9], &sections, None);
        assert_eq!(moved, [1, 2, 3], "the frame past the end is passed over");
        for &i in &moved {
            let s = &sidecars[i];
            // The chosen sections are the source's.
            assert_eq!(s.current.light, from.light, "frame {i}");
            assert_eq!(s.current.color, from.color, "frame {i}");
            assert_eq!(s.current.white_balance, from.white_balance, "frame {i}");
            // The rest is the frame's own: its sharpen, its crop,
            // and no masks or patches of anybody else's.
            let was = &before[i].current;
            assert_eq!(s.current.sharpen, was.sharpen, "frame {i}");
            assert_eq!(s.current.geometry, was.geometry, "frame {i}");
            assert_eq!(s.current.adjustments, was.adjustments, "frame {i}");
            assert_eq!(s.current.retouch, was.retouch, "frame {i}");
            // One step on the history, the state before it kept.
            assert_eq!(s.history.len(), before[i].history.len() + 1, "frame {i}");
            assert_eq!(s.history.last().map(|s| &s.edit), Some(was), "frame {i}");
            // The meta and the turn are not the edit's.
            assert_eq!(s.meta, before[i].meta, "frame {i}");
            assert_eq!(s.turn, before[i].turn, "frame {i}");
        }
        // The frame not in the set is untouched.
        assert_eq!(sidecars[0], before[0]);
        // Undo on a synced frame takes the sync back whole.
        let mut undone = sidecars[1].clone();
        assert!(undone.undo());
        assert_eq!(undone.current, own);

        // Once more: every frame has it already, so nothing moves
        // and no sidecar needs writing.
        let again = sync_into(&mut sidecars, &from, &[1, 2, 3], &sections, None);
        assert!(again.is_empty());
        assert_eq!(sidecars[1].history.len(), before[1].history.len() + 1);

        // The masks, asked for, come across with ids of their own,
        // exactly as a preset's do.
        let moved = sync_into(&mut sidecars, &from, &[2], &[Section::Adjustments], None);
        assert_eq!(moved, [2]);
        assert_eq!(sidecars[2].current.adjustments.len(), 1);
        assert_eq!(sidecars[2].current.adjustments[0].name, "Sky");
        assert_eq!(sidecars[2].current.adjustments[0].id, 1);

        // Noise carries the learned denoiser in a sync, which a
        // preset of the same section does not.
        let mut learned = from.clone();
        learned.noise.strength = 0.8;
        learned.noise.learned = Learned::Balanced;
        learned.noise.learned_strength = 0.6;
        learned.noise.learned_chroma_strength = 0.9;
        let moved = sync_into(&mut sidecars, &learned, &[3], &[Section::Noise], None);
        assert_eq!(moved, [3]);
        assert_eq!(sidecars[3].current.noise, learned.noise);
        let preset = Preset::from_edit("", &learned, &[Section::Noise]);
        let laid = preset.applied(&Edit::default());
        assert_eq!(laid.noise.strength, 0.8);
        assert_eq!(laid.noise.learned, Learned::Off, "a preset leaves it");
        // Without Noise chosen, the learned tier stays the frame's.
        let moved = sync_into(&mut sidecars, &from, &[3], &[Section::Light], None);
        assert!(moved.is_empty(), "light was the source's already");
        assert_eq!(sidecars[3].current.noise.learned, Learned::Balanced);

        // Nothing chosen is nothing done.
        assert!(sync_into(&mut sidecars, &from, &[0], &[], None).is_empty());
        assert_eq!(sidecars[0], before[0]);
    }

    #[test]
    fn a_turn_key_turns_every_frame_in_the_set_and_leaves_the_rest() {
        let mut sidecars = vec![Sidecar::default(); 3];
        sidecars[1].current.geometry.crop = Some(geometry::Crop {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        sidecars[1].current.light.exposure = 1.0;
        let moved = turn_into(&mut sidecars, &[0, 1], 1, |_| Some(1.5));
        assert_eq!(moved, vec![0, 1]);
        assert_eq!(sidecars[0].turn, 1);
        assert_eq!(sidecars[1].turn, 1);
        assert_eq!(sidecars[2].turn, 0, "a frame not in the set is not turned");
        // The crop went round with the picture; the exposure did not
        // move, and no history state was made for any of it.
        assert_eq!(
            sidecars[1].current.geometry.crop,
            Some(geometry::Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 0.5
            })
        );
        assert_eq!(sidecars[1].current.light.exposure, 1.0);
        assert!(sidecars[1].history.is_empty());
        // Left again is where it started.
        turn_into(&mut sidecars, &[0, 1], -1, |_| None);
        assert_eq!(sidecars[1].turn, 0);
        assert_eq!(
            sidecars[1].current.geometry.crop,
            Some(geometry::Crop {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 0.5
            })
        );
        // Four quarters are none, and a frame off the end is passed
        // over rather than a panic.
        assert!(turn_into(&mut sidecars, &[0], 4, |_| Some(1.5)).is_empty());
        assert_eq!(sidecars[0].turn, 0);
        assert!(turn_into(&mut sidecars, &[9], 1, |_| Some(1.5)).is_empty());
        // A frame nothing can measure is not turned when it has work
        // placed on the picture: the one turned before knows its shape,
        // the one never turned does not.
        sidecars[2].current.retouch.patches.push(Default::default());
        assert_eq!(turn_into(&mut sidecars, &[1, 2], 1, |_| None), vec![1]);
        assert_eq!(sidecars[2].turn, 0);
        // With nothing placed, it turns all the same: no state of it
        // wants the shape.
        sidecars[2].current.retouch.patches.clear();
        assert_eq!(turn_into(&mut sidecars, &[2], 1, |_| None), vec![2]);
    }
}
