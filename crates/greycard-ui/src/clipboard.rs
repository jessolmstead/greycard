//! The settings clipboard: one frame's edit, copied whole, waiting to
//! be pasted over the selection.
//!
//! A paste is a sync with the clipboard between: the sections, parts
//! and masks chosen on the sync sheet are laid over each frame of the
//! selection by the same `panel::sync::lay_over_targets` a sync runs,
//! geometry and retouch left out as a sync leaves them, each frame's
//! step named "Paste from" the frame copied. The copy is the whole
//! edit, not the sections: what goes is asked at the paste, when the
//! frames it goes onto are known.
//!
//! It lives in the editor's state, not on the system's clipboard. An
//! edit is not text, nothing else on the desktop reads it, and a
//! paste reads the frame it came from by its path, which the system's
//! clipboard would not keep. It is the session's: a quit forgets it.
//!
//! Pure, so what a copy holds and what a paste lays over a frame are
//! tested here without a window.

use greycard_edit::history::paste_label;
use greycard_edit::preset::{Choice, Section};
use greycard_edit::{Edit, Preset};

/// A frame's edit, copied, and the file it was copied from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Clipboard {
    /// The edit as it was at the copy: the panel's, recorded or not,
    /// outside culling; the sidecar's current state in it.
    pub(crate) edit: Edit,
    /// The frame it came from: its path and its version, not a cell's
    /// number: moving the rejects or opening another folder renumbers
    /// the cells, and the clipboard outlives both.
    pub(crate) from: crate::versions::Key,
    /// The frame's name as the status line and the history row say it:
    /// the file's, or a version's stem and name, "IMG_0001 (Warm)". Kept
    /// from the copy, since a version's name may change after.
    name: String,
}

impl Clipboard {
    /// `edit` copied from the frame `from`, whole, named `name`.
    pub(crate) fn copy(edit: Edit, from: crate::versions::Key, name: String) -> Self {
        Self { edit, from, name }
    }

    /// [`Clipboard::copy`] from a file's own frame, named for the file.
    #[cfg(test)]
    pub(crate) fn copy_file(edit: Edit, path: impl Into<std::path::PathBuf>) -> Self {
        let from = crate::versions::Key::file(path);
        let name = crate::panel::browser::file_name(&from.path);
        Self::copy(edit, from, name)
    }

    /// The name of the frame copied from, as a status line and a
    /// history row say it: the file name, with the extension, since a
    /// RAW and its JPEG can share a stem in one folder.
    pub(crate) fn name(&self) -> String {
        self.name.clone()
    }

    /// The words each pasted frame's history step is recorded with.
    pub(crate) fn label(&self) -> String {
        paste_label(&self.name())
    }

    /// The cells of `set` a paste goes onto: all of them but the frame
    /// the clipboard came from, which has its own settings already (a
    /// version of that file is another frame, and takes the paste).
    pub(crate) fn targets(&self, st: &crate::State, set: &[usize]) -> Vec<usize> {
        self.targets_by(set, |c| st.key_of(c))
    }

    /// [`Clipboard::targets`] with the cells named by `key_of`: the
    /// cells of `set` that are a frame and not the one copied from.
    pub(crate) fn targets_by(
        &self,
        set: &[usize],
        key_of: impl Fn(usize) -> Option<crate::versions::Key>,
    ) -> Vec<usize> {
        set.iter()
            .copied()
            .filter(|&c| key_of(c).is_some_and(|k| k != self.from))
            .collect()
    }

    /// What `choice` picks of the copied edit, as the preset a paste
    /// lays over each frame: `Preset::from_choice`, as a sync builds
    /// its own, so a section, a part or a mask means the same thing
    /// either way.
    pub(crate) fn preset(&self, choice: &Choice) -> Preset {
        Preset::from_choice("", &self.edit, choice)
    }

    /// The source a paste's Noise carries the learned denoiser from,
    /// as a sync's does (`greycard_edit::sync_learned`): the copied
    /// edit when Noise is chosen, else none.
    pub(crate) fn learned_from(&self, preset: &Preset) -> Option<&Edit> {
        preset
            .sections
            .contains(&Section::Noise)
            .then_some(&self.edit)
    }

    /// What a paste of `sections` makes of one frame's edit: exactly
    /// what `greycard_edit::apply_preset_into` records on a target,
    /// for the frame on screen, whose edit is the panel's and not the
    /// sidecar's.
    pub(crate) fn applied(&self, preset: &Preset, onto: &Edit) -> Edit {
        let mut out = preset.applied(onto);
        if let Some(from) = self.learned_from(preset) {
            greycard_edit::sync_learned(from, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::versions::Key;
    use greycard_edit::{Learned, Sidecar};
    use std::path::PathBuf;

    fn files() -> Vec<PathBuf> {
        (0..5)
            .map(|i| PathBuf::from("/shoot").join(format!("IMG_{i:04}.CR3")))
            .collect()
    }

    fn source() -> Edit {
        let mut e = Edit::default();
        e.light.exposure = 0.7;
        e.color.saturation = -0.3;
        e.noise.learned = Learned::Balanced;
        e.noise.learned_strength = 0.6;
        e.geometry.angle = 2.0;
        e
    }

    #[test]
    fn a_copy_holds_the_whole_edit_and_the_frame_it_came_from() {
        let files = files();
        let clip = Clipboard::copy_file(source(), &files[1]);
        assert_eq!(clip.edit, source());
        assert_eq!(clip.from.path, files[1]);
        assert_eq!(clip.name(), "IMG_0001.CR3");
        assert_eq!(clip.label(), "Paste from IMG_0001.CR3");
        // From a version: named for it, as its cell is.
        let warm = Clipboard::copy(
            source(),
            Key::of(&files[1], Some("abcd")),
            "IMG_0001 (Warm)".into(),
        );
        assert_eq!(warm.name(), "IMG_0001 (Warm)");
        assert_eq!(warm.label(), "Paste from IMG_0001 (Warm)");
    }

    #[test]
    fn a_paste_leaves_out_the_frame_it_was_copied_from() {
        let files = files();
        let clip = Clipboard::copy_file(source(), &files[1]);
        let by = |files: &[PathBuf]| {
            let files = files.to_vec();
            move |i: usize| files.get(i).map(Key::file)
        };
        assert_eq!(clip.targets_by(&[0, 1, 3], by(&files)), vec![0, 3]);
        // Onto the source alone: nothing to do.
        assert!(clip.targets_by(&[1], by(&files)).is_empty());
        // An index past the list is nobody's.
        assert_eq!(clip.targets_by(&[3, 9], by(&files)), vec![3]);
        // The folder renumbered under the clipboard: the frame, not
        // the index, is what is left out.
        let moved: Vec<PathBuf> = files.iter().rev().cloned().collect();
        assert_eq!(
            clip.targets_by(&[0, 1, 2, 3, 4], by(&moved)),
            vec![0, 1, 2, 4]
        );
        // A version of the file copied from is another frame, and
        // takes the paste.
        let version = Key::of(&files[1], Some("abcd"));
        let one = move |i: usize| (i == 1).then(|| version.clone());
        assert_eq!(clip.targets_by(&[1], one), vec![1]);
    }

    #[test]
    fn a_paste_lays_the_chosen_sections_and_no_geometry() {
        let clip = Clipboard::copy_file(source(), "/shoot/IMG_0001.CR3");
        let mut onto = Edit::default();
        onto.light.exposure = -1.0;
        onto.geometry.angle = -4.0;
        // Light alone: the exposure goes, the color and the crop's
        // angle stay the frame's own.
        let light = clip.preset(&Choice::whole(&[Section::Light]));
        let out = clip.applied(&light, &onto);
        assert_eq!(out.light.exposure, 0.7);
        assert_eq!(out.color.saturation, onto.color.saturation);
        assert_eq!(out.geometry.angle, -4.0);
        assert!(clip.learned_from(&light).is_none());
        assert_eq!(out.noise.learned, onto.noise.learned);
        // Everything a sync offers: still no geometry, and Noise
        // brings the learned denoiser as a sync's does.
        let all: Vec<Section> = Section::ALL.to_vec();
        let every = clip.preset(&Choice::whole(&all));
        let out = clip.applied(&every, &onto);
        assert_eq!(out.color.saturation, -0.3);
        assert_eq!(out.geometry.angle, -4.0);
        assert_eq!(out.noise.learned, Learned::Balanced);
        assert_eq!(out.noise.learned_strength, 0.6);
        // What the frame on screen gets is what a target's sidecar
        // records, through the same bottom layer.
        let mut sidecars = vec![Sidecar::default()];
        sidecars[0].current = onto.clone();
        let label = clip.label();
        let moved = greycard_edit::apply_preset_into(
            &mut sidecars,
            &every,
            &[0],
            clip.learned_from(&every),
            Some(&label),
        );
        assert_eq!(moved, vec![0]);
        assert_eq!(sidecars[0].current, out);
        assert_eq!(sidecars[0].current_label.as_deref(), Some(label.as_str()));
        // A second paste of the same records nothing.
        assert!(
            greycard_edit::apply_preset_into(
                &mut sidecars,
                &every,
                &[0],
                clip.learned_from(&every),
                Some(&label),
            )
            .is_empty()
        );
    }
}
