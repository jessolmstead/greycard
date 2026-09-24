//! The browser's selection: a set of frames, and one of them current.
//!
//! The current frame is the one the viewport shows, the panel edits
//! and a sync copies from; it is also the anchor a Shift+click
//! measures a range from. It is always in the set, and the set is
//! never empty while there is a current frame. Everything here is
//! in files, not rows: a row is only a file's place under the
//! filter, and the filter can move it.
//!
//! Pure, so the arithmetic is tested here rather than through a
//! window; `panel::browser` does the opening and the drawing.

/// What a click on a frame asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Click {
    /// Open the frame and make it the whole set: a plain click, or a
    /// modified one with nothing current to measure from.
    Open(usize),
    /// The set becomes this; the current frame stays where it is.
    Set(Vec<usize>),
}

/// A click on `file` with Ctrl and Shift as they were held.
///
/// Ctrl toggles the frame in or out of the set; Shift makes the set
/// the run of rows from the current frame to it; both add that run
/// to the set. Neither moves the current frame, which is the frame a
/// sync copies from: the frame edited is clicked first, the rest are
/// added to it, and what is on screen does not change under the
/// hand. The current frame cannot be toggled out; a plain click on
/// another is how the current frame moves.
pub fn click(
    set: &[usize],
    shown: &[usize],
    current: Option<usize>,
    file: usize,
    ctrl: bool,
    shift: bool,
) -> Click {
    let Some(current) = current else {
        return Click::Open(file);
    };
    let set = frames(set, Some(current));
    match (ctrl, shift) {
        (false, false) => Click::Open(file),
        (true, false) => Click::Set(toggle(&set, current, file)),
        (false, true) => Click::Set(range(shown, current, file)),
        (true, true) => Click::Set(union(&set, &range(shown, current, file))),
    }
}

/// `file` in or out of the set, the current frame always in.
pub fn toggle(set: &[usize], current: usize, file: usize) -> Vec<usize> {
    let mut out = frames(set, Some(current));
    if file == current {
        return out;
    }
    match out.binary_search(&file) {
        Ok(at) => {
            out.remove(at);
        }
        Err(at) => out.insert(at, file),
    }
    out
}

/// The frames on the rows from `from`'s to `to`'s, both ends in,
/// whichever way round they are. A frame the filter hides has no
/// row: from a hidden anchor the run is `to` alone.
pub fn range(shown: &[usize], from: usize, to: usize) -> Vec<usize> {
    let row = |f: usize| shown.iter().position(|&s| s == f);
    let (Some(a), Some(b)) = (row(from), row(to)) else {
        return vec![to];
    };
    let (lo, hi) = (a.min(b), a.max(b));
    let mut out = shown[lo..=hi].to_vec();
    out.sort_unstable();
    out
}

/// Two sets as one.
pub fn union(a: &[usize], b: &[usize]) -> Vec<usize> {
    let mut out: Vec<usize> = a.iter().chain(b).copied().collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The set as it is acted on: sorted, each frame once, and the
/// current frame in it. No current frame is no set.
pub fn frames(set: &[usize], current: Option<usize>) -> Vec<usize> {
    let Some(current) = current else {
        return Vec::new();
    };
    union(set, &[current])
}

/// The set after the list changed under it: what the filter no
/// longer shows leaves it, since a key or a sync must not reach a
/// frame nobody can see is chosen. The current frame stays when it
/// is shown; a current frame the filter hid is the caller's to move.
pub fn prune(set: &[usize], shown: &[usize], current: Option<usize>) -> Vec<usize> {
    let shows = |f: &usize| shown.binary_search(f).is_ok();
    let mut out: Vec<usize> = frames(set, current).into_iter().filter(shows).collect();
    if out.is_empty()
        && let Some(c) = current
    {
        out.push(c);
    }
    out
}

/// The other frames of the set: what a sync lays the current frame
/// over.
pub fn others(set: &[usize], current: Option<usize>) -> Vec<usize> {
    frames(set, current)
        .into_iter()
        .filter(|&f| Some(f) != current)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_click_opens_and_a_modified_one_changes_the_set() {
        let shown: Vec<usize> = (0..8).collect();
        assert_eq!(
            click(&[2], &shown, Some(2), 5, false, false),
            Click::Open(5)
        );
        // Nothing current: a modifier has nothing to measure from.
        assert_eq!(click(&[], &shown, None, 5, true, false), Click::Open(5));
        assert_eq!(click(&[], &shown, None, 5, false, true), Click::Open(5));
        assert_eq!(
            click(&[2], &shown, Some(2), 5, true, false),
            Click::Set(vec![2, 5])
        );
        assert_eq!(
            click(&[2], &shown, Some(2), 5, false, true),
            Click::Set(vec![2, 3, 4, 5])
        );
        // Shift replaces the set with the run; Ctrl+Shift adds it.
        assert_eq!(
            click(&[2, 7], &shown, Some(2), 0, false, true),
            Click::Set(vec![0, 1, 2])
        );
        assert_eq!(
            click(&[2, 7], &shown, Some(2), 0, true, true),
            Click::Set(vec![0, 1, 2, 7])
        );
    }

    #[test]
    fn ctrl_toggles_a_frame_and_never_the_current_one() {
        assert_eq!(toggle(&[3], 3, 1), vec![1, 3]);
        assert_eq!(toggle(&[1, 3], 3, 1), vec![3]);
        assert_eq!(toggle(&[1, 3, 6], 3, 3), vec![1, 3, 6]);
        // A set that lost its current frame gets it back.
        assert_eq!(toggle(&[1], 3, 6), vec![1, 3, 6]);
    }

    #[test]
    fn a_range_runs_along_the_rows_either_way_under_a_filter() {
        // Rows 0.. are files 1, 4, 5, 9: the filter hides the rest,
        // and a run from 4 to 9 is the three rows, not six files.
        let shown = [1, 4, 5, 9];
        assert_eq!(range(&shown, 4, 9), vec![4, 5, 9]);
        assert_eq!(range(&shown, 9, 1), vec![1, 4, 5, 9]);
        assert_eq!(range(&shown, 5, 5), vec![5]);
        // An anchor the filter hid: the frame clicked alone.
        assert_eq!(range(&shown, 2, 5), vec![5]);
    }

    #[test]
    fn a_set_collapses_and_prunes_to_what_is_shown() {
        assert_eq!(frames(&[5, 1, 5], Some(3)), vec![1, 3, 5]);
        assert!(frames(&[1, 2], None).is_empty());
        assert_eq!(others(&[1, 3, 5], Some(3)), vec![1, 5]);
        assert!(others(&[3], Some(3)).is_empty());
        // A filter that hides two of the set keeps the rest.
        assert_eq!(prune(&[1, 3, 5, 7], &[0, 3, 5, 6], Some(3)), vec![3, 5]);
        // One that hides the current frame leaves the caller to move
        // it, but never an empty set with a frame current.
        assert_eq!(prune(&[1, 3], &[0, 5], Some(3)), vec![3]);
        assert!(prune(&[1, 3], &[0, 5], None).is_empty());
    }
}
