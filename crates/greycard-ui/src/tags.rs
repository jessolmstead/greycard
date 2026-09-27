//! The session's rating, flag and label changes, for culling's undo.
//!
//! A develop edit has its history in the sidecar; the culling keys'
//! changes have none there, since a rating is a fact about the frame
//! and not a step in developing it (§117). So culling's Ctrl+Z steps
//! back through these instead: every change the keys and the frame
//! menu made this session, newest first, and nothing else.
//!
//! Each step names its frames by path, not by row or index: the list
//! of files changes under it (a rejects move, a filter, a root added),
//! and a step whose frame has gone is simply passed over. A frame is
//! put back only while it still carries what the step left on it, so
//! a rating that arrived since from another tool's XMP, or a paste,
//! is not undone by a step that never made it.

use std::path::{Path, PathBuf};

use greycard_edit::meta::{Change, Flag, Label, Meta};

/// The three fields the culling keys write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tags {
    pub(crate) rating: u8,
    pub(crate) flag: Flag,
    pub(crate) label: Label,
}

impl Tags {
    pub(crate) fn of(meta: &Meta) -> Self {
        Self {
            rating: meta.rating,
            flag: meta.flag,
            label: meta.label,
        }
    }

    pub(crate) fn put(self, meta: &mut Meta) {
        meta.rating = self.rating;
        meta.flag = self.flag;
        meta.label = self.label;
    }
}

/// The word over the picture for a key's change, once settled
/// against the frames it went on: what the frame carries now, not
/// the key that was pressed, so a label key that cleared the label
/// says so. The flag's past tense is the one a culler says aloud.
pub(crate) fn notice(change: Change) -> String {
    match change {
        Change::Rating(0) => "No stars".into(),
        Change::Rating(1) => "1 star".into(),
        Change::Rating(n) => format!("{n} stars"),
        Change::Flag(Flag::Pick) => "Pick".into(),
        Change::Flag(Flag::Reject) => "Rejected".into(),
        Change::Flag(Flag::None) => "Unflagged".into(),
        Change::Label(Label::None) => "No label".into(),
        Change::Label(label) => label.name().into(),
    }
}

/// The one field that differs between `from` and `to`, as the change
/// that would take a frame from the one to the other: what an undo
/// or a redo did to it, in the key's own terms, for the word over
/// the picture. None when nothing differs; a step moves one field,
/// so a difference in more than one names the first found.
pub(crate) fn between(from: Tags, to: Tags) -> Option<Change> {
    if from.rating != to.rating {
        Some(Change::Rating(to.rating))
    } else if from.flag != to.flag {
        Some(Change::Flag(to.flag))
    } else if from.label != to.label {
        Some(Change::Label(to.label))
    } else {
        None
    }
}

/// The word for an undo (`back`) or a redo that put `landed` frames
/// back: "Undo: No stars" for one frame, what it carries now after
/// the key's own word, and a bare "Undone" for a step over several.
pub(crate) fn step_notice(back: bool, landed: &[Option<Change>]) -> String {
    let what = if back { "Undo" } else { "Redo" };
    match landed {
        [Some(change)] => format!("{what}: {}", notice(*change)),
        _ if back => "Undone".into(),
        _ => "Redone".into(),
    }
}

/// One frame's part in a step.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Moved {
    pub(crate) path: PathBuf,
    pub(crate) before: Tags,
    pub(crate) after: Tags,
}

/// One key's change, over every frame it moved.
pub(crate) type Step = Vec<Moved>;

#[derive(Debug, Default)]
pub(crate) struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

/// As many steps as a long culling pass takes and then some; the
/// oldest go past it.
const KEPT: usize = 1000;

impl History {
    /// A change just made. What was undone is gone, as in any undo.
    pub(crate) fn record(&mut self, step: Step) {
        if step.is_empty() {
            return;
        }
        self.redo.clear();
        self.undo.push(step);
        if self.undo.len() > KEPT {
            self.undo.remove(0);
        }
    }

    /// The step to take back, with each frame going to its `before`;
    /// it moves to the redo side.
    pub(crate) fn undo(&mut self) -> Option<Step> {
        let step = self.undo.pop()?;
        self.redo.push(step.clone());
        Some(step)
    }

    /// The step to make again, each frame going back to its `after`,
    /// given as an undo is: `before` the state it leaves, `after` the
    /// state it lands on.
    pub(crate) fn redo(&mut self) -> Option<Step> {
        let step = self.redo.pop()?;
        self.undo.push(step.clone());
        Some(step.into_iter().map(Moved::reversed).collect())
    }
}

impl Moved {
    /// The same change the other way: undone becomes redone.
    fn reversed(self) -> Self {
        Self {
            path: self.path,
            before: self.after,
            after: self.before,
        }
    }
}

/// Where `step` would put each frame it still finds as it left it,
/// as (file index, tags to put); `find` looks a path up among the
/// files now listed. For an undo, `step` is as [`History::undo`] gave
/// it; a redo's is already turned round.
pub(crate) fn landing(
    step: &[Moved],
    find: impl Fn(&Path) -> Option<usize>,
    now: impl Fn(usize) -> Tags,
) -> Vec<(usize, Tags)> {
    step.iter()
        .filter_map(|m| {
            let i = find(&m.path)?;
            (now(i) == m.after).then_some((i, m.before))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(rating: u8, flag: Flag) -> Tags {
        Tags {
            rating,
            flag,
            label: Label::None,
        }
    }

    fn moved(path: &str, before: Tags, after: Tags) -> Moved {
        Moved {
            path: path.into(),
            before,
            after,
        }
    }

    #[test]
    fn undo_and_redo_walk_the_steps_and_a_new_change_drops_the_redo() {
        let mut h = History::default();
        let (none, three, pick) = (
            tags(0, Flag::None),
            tags(3, Flag::None),
            tags(3, Flag::Pick),
        );
        h.record(vec![moved("a", none, three)]);
        h.record(vec![moved("a", three, pick)]);

        // The last first, each frame going to what it was.
        let back = h.undo().unwrap();
        assert_eq!((back[0].before, back[0].after), (three, pick));
        let back = h.undo().unwrap();
        assert_eq!((back[0].before, back[0].after), (none, three));
        assert!(h.undo().is_none());

        // A redo is given the way round it will be laid.
        let again = h.redo().unwrap();
        assert_eq!((again[0].before, again[0].after), (three, none));

        // A new change: the other undone step is gone for good.
        h.record(vec![moved("b", none, pick)]);
        assert!(h.redo().is_none());
        assert_eq!(h.undo().unwrap()[0].path, PathBuf::from("b"));
        assert_eq!(h.undo().unwrap()[0].path, PathBuf::from("a"));
        assert!(h.undo().is_none());
    }

    #[test]
    fn a_frame_changed_since_or_gone_is_left_alone() {
        let (none, three, five) = (
            tags(0, Flag::None),
            tags(3, Flag::None),
            tags(5, Flag::None),
        );
        let step = vec![
            moved("kept", none, three),
            moved("rated-since", none, three),
            moved("gone", none, three),
        ];
        let files = ["kept", "rated-since"];
        let find = |p: &Path| files.iter().position(|f| Path::new(f) == p);
        // "rated-since" has five stars now, from somewhere else.
        let now = |i: usize| if i == 0 { three } else { five };
        assert_eq!(landing(&step, find, now), vec![(0, none)]);
    }

    #[test]
    fn the_notice_says_what_the_frame_carries_now() {
        assert_eq!(notice(Change::Rating(0)), "No stars");
        assert_eq!(notice(Change::Rating(1)), "1 star");
        assert_eq!(notice(Change::Rating(3)), "3 stars");
        assert_eq!(notice(Change::Flag(Flag::Pick)), "Pick");
        assert_eq!(notice(Change::Flag(Flag::Reject)), "Rejected");
        assert_eq!(notice(Change::Flag(Flag::None)), "Unflagged");
        assert_eq!(notice(Change::Label(Label::Red)), "Red");
        assert_eq!(notice(Change::Label(Label::Blue)), "Blue");
        assert_eq!(notice(Change::Label(Label::None)), "No label");
    }

    #[test]
    fn the_word_for_a_step_names_what_one_frame_carries_now() {
        let (none, three, pick) = (
            tags(0, Flag::None),
            tags(3, Flag::None),
            tags(3, Flag::Pick),
        );
        assert_eq!(between(three, none), Some(Change::Rating(0)));
        assert_eq!(between(three, pick), Some(Change::Flag(Flag::Pick)));
        assert_eq!(between(pick, pick), None);
        assert_eq!(step_notice(true, &[between(three, none)]), "Undo: No stars");
        assert_eq!(step_notice(false, &[between(none, three)]), "Redo: 3 stars");
        assert_eq!(
            step_notice(true, &[Some(Change::Rating(0)), Some(Change::Rating(0))]),
            "Undone"
        );
        assert_eq!(step_notice(false, &[None, None]), "Redone");
    }

    #[test]
    fn an_empty_change_is_not_a_step() {
        let mut h = History::default();
        h.record(Vec::new());
        assert!(h.undo().is_none());
    }
}
