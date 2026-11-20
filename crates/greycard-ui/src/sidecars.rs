//! Writing a frame's sidecar, and its meta to an XMP beside it, from
//! the window's thread: one file at a time, or handed to the batch
//! when there are two or more, which is `panel::saves`'s.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use greycard_edit_io::xmp;

use crate::panel::browser::{camera_tag, file_name};
use crate::{State, tr};

/// Write cell `c`'s file's sidecar, if sidecars are written, and its
/// meta to an XMP beside it when that is asked for: [`write_file`] of
/// the cell's file. The sidecar is the file's whole, every line of it,
/// written once whichever of its cells moved.
pub(crate) fn write_sidecar(st: &mut State, c: usize) {
    if let Some(file) = st.file_at(c) {
        write_file(st, file);
    }
}

/// Write file `file`'s sidecar, if sidecars are written, and its meta
/// to an XMP beside it when that is asked for.
///
/// The XMP goes first so that the mark for what it now holds is in
/// the sidecar before the sidecar is written; otherwise the next
/// load would read back what this build just wrote. An XMP that
/// will not parse costs the interop and nothing else: the `.gcd` is
/// written either way.
pub(crate) fn write_file(st: &mut State, file: usize) {
    if !file_writable(st, file) {
        return;
    }
    // Saved here, it is no longer a batch's to save.
    crate::panel::saves::take_back(st, file);
    // A frame followed to an archive's copy has its XMP written there
    // by the archive write, from the sidecar, off the window's thread.
    if st.window.xmp_sidecars && !crate::sync::followed(st, file) {
        // `tiff:Orientation` is the camera's tag and the frame's
        // quarter turns composed; with no tag to compose onto,
        // whatever the file says about the orientation is left
        // alone and the rest of the meta still goes.
        let turn = st
            .file_cell(file)
            .and_then(|c| camera_tag(st, c))
            .map(|o| xmp::Turn::new(o, st.sidecars[file].turn));
        // The capture time's dates, from the camera's own time, asked
        // only when a shift moves them.
        let (path, meta) = (st.files[file].clone(), st.sidecars[file].meta.clone());
        let taken = || crate::panel::time_shift::camera_time(st, file);
        // Said in the log only, whatever the failure: the edit is in
        // the `.gcd`, which says for itself when it is not saved
        // ([`Unsaved`]), and the XMP goes again at the next save.
        match xmp::save_dated(&path, &meta, turn, taken) {
            Ok(Some(mark)) => st.sidecars[file].xmp = Some(mark),
            Ok(None) => {}
            Err(e) => tracing::warn!("{}: xmp not written: {e}", file_name(&path)),
        }
    }
    save_file(st, file);
}

/// [`write_file`] of each of `files`: one alone on the window's
/// thread, two or more as a batch off it ([`crate::panel::saves`]),
/// marked saved when the batch lands.
pub(crate) fn write_files(st: &mut State, files: &[usize]) {
    match files {
        [] => {}
        [file] => write_file(st, *file),
        _ => crate::panel::saves::save(st, files),
    }
}

/// [`save_file`] of cell `c`'s file.
pub(crate) fn save_sidecar(st: &mut State, c: usize) -> bool {
    st.file_at(c).is_some_and(|file| save_file(st, file))
}

/// Write file `file`'s sidecar where it lives: for a frame the window
/// follows on an archive's copy (§233), the save is made in memory and
/// the file written off the window's thread by the archive write, with
/// its check against the copy; for every other frame, written here
/// beside it, the index told and the archive's copy queued. One save,
/// one revision, however many of its lines moved. True when the save
/// was made.
pub(crate) fn save_file(st: &mut State, file: usize) -> bool {
    crate::panel::saves::take_back(st, file);
    if crate::sync::followed(st, file) {
        st.sidecars[file].advance(&greycard_edit::sync::Stamp::now());
        crate::sync::after_save(st, file);
        for c in st.cells_of_file(file) {
            crate::edited::saved(st, c);
        }
        // The archive write has it now, and says for itself what waits.
        let path = st.files[file].clone();
        landed(st, &path);
        return true;
    }
    let (path, placement) = (st.files[file].clone(), st.window.placement);
    match st.sidecars[file].save_in(&path, placement) {
        Ok(_) => {
            crate::library::sidecar_written(st, file);
            landed(st, &path);
            true
        }
        Err(e) => {
            tracing::warn!("{}: sidecar not saved: {e}", file_name(&path));
            failed(st, &path, Why::of(&e));
            false
        }
    }
}

/// Whether cell `c`'s sidecar may be written now: sidecars are on,
/// the frame is not being deleted, and the sidecar in memory is the
/// frame's own and not its row standing in (`rows`), which would put
/// the default edit over whatever the file holds.
pub(crate) fn writable(st: &State, c: usize) -> bool {
    st.file_at(c).is_some_and(|file| file_writable(st, file))
}

/// [`writable`] by the file.
pub(crate) fn file_writable(st: &State, file: usize) -> bool {
    st.window.write_sidecars
        && !crate::panel::delete::held_file(st, file)
        && st.from_row.get(file).is_none_or(|r| r.read)
}

/// Why a sidecar's write failed, in the few words the status line has
/// for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Why {
    Full,
    ReadOnly,
    Denied,
    /// Anything else, whose whole is in the log.
    Other,
}

impl Why {
    pub(crate) fn of_io(kind: std::io::ErrorKind) -> Self {
        use std::io::ErrorKind as K;
        match kind {
            K::StorageFull | K::QuotaExceeded => Self::Full,
            K::ReadOnlyFilesystem => Self::ReadOnly,
            // On Windows access denied is also a file another program
            // holds open, which no folder's rights explain: the log
            // has which.
            K::PermissionDenied if !cfg!(windows) => Self::Denied,
            _ => Self::Other,
        }
    }

    pub(crate) fn of(e: &greycard_edit::Error) -> Self {
        match e {
            greycard_edit::Error::Io(e) => Self::of_io(e.kind()),
            _ => Self::Other,
        }
    }

    /// The reason in words for a save of `raw`'s, none when the log has
    /// to say it.
    fn words(self, raw: &Path) -> Option<String> {
        match self {
            Self::Full => Some(tr!("the disk is full")),
            Self::ReadOnly => Some(tr!("the disk is read-only")),
            Self::Denied => Some(tr!(
                "{folder} cannot be written to",
                folder = raw.parent().unwrap_or(raw).display()
            )),
            Self::Other => None,
        }
    }
}

/// The frames whose edit is in memory only: their sidecar's last save
/// failed (a full disk, a folder that refuses it), and nothing has
/// landed since. The status bar's note says how many while any are,
/// beside the archive's (`sync::refresh_note`); a click on it saves
/// them again, as leaving the folder and the quit do. The first
/// failure of a frame also says on the status line which and why,
/// once: a slider dragged on a full disk saves at every step.
#[derive(Default)]
pub(crate) struct Unsaved {
    /// Each frame by its path.
    pub(crate) edits: HashSet<PathBuf>,
    /// The status line's word last said of them, taken off when the
    /// last of them lands while it still stands; never when it says
    /// that edits were let go (true), which no landing undoes.
    said: Option<(String, bool)>,
    /// How many words have been said: a caller that writes the status
    /// line after its saves puts a new one back ([`said_since`]).
    said_count: u64,
}

impl Unsaved {
    /// A frame moved from `was` to `to`: its edit's state follows it,
    /// and whatever was kept at `to` was another file's and goes.
    pub(crate) fn file_moved(&mut self, was: &Path, to: &Path) {
        if self.edits.remove(was) {
            self.edits.insert(to.to_path_buf());
        } else {
            self.edits.remove(to);
        }
    }

    pub(crate) fn said_count(&self) -> u64 {
        self.said_count
    }
}

/// The status bar's clause for the frames whose edit is not saved:
/// "2 edits not saved", none while every edit is.
pub(crate) fn note(st: &State) -> Option<String> {
    let n = st.work.unsaved.edits.len();
    (n > 0).then(|| tr!("{n} edit not saved" | "{n} edits not saved" % n))
}

/// A save of `raw`'s sidecar failed for `why`: [`failed_all`] of the
/// one.
pub(crate) fn failed(st: &mut State, raw: &Path, why: Why) {
    failed_all(st, &[(raw.to_path_buf(), why)]);
}

/// Saves that failed together, a batch's: the frames added to those
/// not saved and the note brought up to them, and when any of them
/// had not failed already, which and why said once on the status line.
/// Nothing is shown in a run nobody is at (a snapshot, a screenshot,
/// an export), nor with no window.
pub(crate) fn failed_all(st: &mut State, failures: &[(PathBuf, Why)]) {
    let unsaved = &mut st.work.unsaved;
    let mut new = false;
    for (raw, _) in failures {
        new |= unsaved.edits.insert(raw.clone());
    }
    if new {
        say(st, detail(failures), false);
    }
}

/// `line` said on the status line, remembered as the word of what is
/// not saved (of edits let go, with `lost`), and the note brought up
/// to the set.
fn say(st: &mut State, line: String, lost: bool) {
    if st.window.batch {
        return;
    }
    st.work.unsaved.said = Some((line.clone(), lost));
    st.work.unsaved.said_count += 1;
    if let Some(app) = st.window.app.upgrade() {
        app.set_status(line.into());
        crate::sync::refresh_note(st, &app);
    }
}

/// A save of `raw`'s sidecar landed: its edit is on disk, and the note
/// says one fewer; with none left, the status line's word of them
/// comes off if it still stands.
pub(crate) fn landed(st: &mut State, raw: &Path) {
    if !st.work.unsaved.edits.remove(raw) {
        return;
    }
    shown(st);
}

/// The note and the status line brought up to what is not saved, after
/// the set changed.
fn shown(st: &mut State) {
    if st.window.batch {
        return;
    }
    let Some(app) = st.window.app.upgrade() else {
        return;
    };
    if st.work.unsaved.edits.is_empty()
        && let Some((line, _)) = st.work.unsaved.said.take_if(|(_, lost)| !*lost)
        && app.get_status() == line.as_str()
    {
        app.set_status("".into());
    }
    crate::sync::refresh_note(st, &app);
}

/// How long a folder change waits for the edits not saved to save
/// again while their batch moves no file ([`retry_leaving`]): long
/// enough for a slow share, short enough that a hung one does not hold
/// the change for more than a moment.
pub(crate) const LEAVE_WRITE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Every listed frame whose edit is not saved and the window may write
/// now (not held for a delete, nor standing in from its row).
fn retryable(st: &State) -> Vec<usize> {
    st.work
        .unsaved
        .edits
        .iter()
        .filter_map(|p| st.file_by_path(p))
        .filter(|&f| file_writable(st, f))
        .collect()
}

/// Save again every frame whose edit is not saved: a click on the
/// note, which waits for the saves as any save does.
pub(crate) fn retry(st: &mut State) {
    let files = retryable(st);
    if !files.is_empty() {
        tracing::info!("sidecars: {} edit(s) not saved, saved again", files.len());
        write_files(st, &files);
    }
}

/// [`retry`] as the list is left or the window closes: always as a
/// batch off the window's thread, one frame or many, given up when it
/// has moved no file for `wait` (a share that stopped answering), so
/// the folder change or the quit is never held longer. What has not
/// landed by then stays not saved, and is let go with the list.
pub(crate) fn retry_leaving(st: &mut State, wait: std::time::Duration) {
    let files = retryable(st);
    if files.is_empty() {
        return;
    }
    tracing::info!(
        "sidecars: {} edit(s) not saved, saved again as the list goes",
        files.len()
    );
    crate::panel::saves::save_bounded(st, &files, wait);
}

/// A frame's sidecar in memory taken from disk again (an import into
/// it, a look renamed on a thread): an edit not saved there is gone,
/// and the note says one fewer.
pub(crate) fn reread(st: &mut State, raw: &Path) {
    if st.work.unsaved.edits.remove(raw) {
        tracing::warn!(
            "{}: its edit was never saved, and the file's is read over it",
            file_name(raw)
        );
        shown(st);
    }
}

/// The sidecars in memory of the frames whose edit is not saved, by
/// path, before a list read from disk replaces them: each still in the
/// new list is put back over the disk's copy ([`put_back`]).
pub(crate) fn carried(st: &State) -> Vec<(PathBuf, greycard_edit::Sidecar)> {
    st.work
        .unsaved
        .edits
        .iter()
        .filter_map(|p| {
            let f = st.files.iter().position(|f| f == p)?;
            Some((p.clone(), st.sidecars.get(f)?.clone()))
        })
        .collect()
}

/// The new list's sidecar of each frame in `carried` put back to the
/// one in memory that was not saved, read and its own: it stays not
/// saved, and is saved again at the next retry.
pub(crate) fn put_back(st: &mut State, carried: Vec<(PathBuf, greycard_edit::Sidecar)>) {
    for (path, sidecar) in carried {
        if let Some(f) = st.files.iter().position(|f| *f == path) {
            st.sidecars[f] = sidecar;
            st.from_row[f] = crate::rows::FromRow::read();
        }
    }
}

/// The list replaced: a frame no longer in it (deleted, moved out, its
/// folder left) takes its edit not saved with it. Each lost is said in
/// the log, and on the status line as not saved, which it is: the last
/// word there is never that all is well.
pub(crate) fn listed(st: &mut State) {
    let listed: HashSet<&PathBuf> = st.files.iter().collect();
    let mut lost: Vec<PathBuf> = st
        .work
        .unsaved
        .edits
        .iter()
        .filter(|p| !listed.contains(p))
        .cloned()
        .collect();
    if lost.is_empty() {
        return;
    }
    lost.sort();
    for p in &lost {
        tracing::warn!("{}: its edit was never saved, and is let go", file_name(p));
        st.work.unsaved.edits.remove(p);
    }
    let failures: Vec<(PathBuf, Why)> = lost.into_iter().map(|p| (p, Why::Other)).collect();
    say(st, detail(&failures), true);
}

/// The word said since `before` ([`Unsaved::said_count`]), for a
/// caller to put back over what it writes on the status line after
/// its saves: Move back's, whose frames leave the list, and with it
/// the note, at once.
pub(crate) fn said_since(st: &State, before: u64) -> Option<String> {
    (st.work.unsaved.said_count != before)
        .then(|| st.work.unsaved.said.as_ref().map(|(line, _)| line.clone()))
        .flatten()
}

/// The status line's word for `failures`: the frame named when there
/// is one, a count when there are more, and the reason when they share
/// one.
fn detail(failures: &[(PathBuf, Why)]) -> String {
    let Some((first, _)) = failures.first() else {
        return String::new();
    };
    let why = match failures {
        [(_, w), rest @ ..] if rest.iter().all(|(_, r)| r == w) => *w,
        _ => Why::Other,
    }
    .words(first);
    let name = file_name(first);
    match (failures.len(), why) {
        (1, Some(why)) => tr!(
            "the edit of {name} is not saved: {why}",
            name = name,
            why = why
        ),
        (1, None) => tr!("the edit of {name} is not saved; see the log", name = name),
        (n, why) => {
            // The first frame and a count, not the note's count again.
            let others = n - 1;
            let names = tr!("{} and {n} other" | "{} and {n} others" % others, name);
            match why {
                Some(why) => tr!(
                    "the edits of {names} are not saved: {why}",
                    names = names,
                    why = why
                ),
                None => tr!(
                    "the edits of {names} are not saved; see the log",
                    names = names
                ),
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::testing::{remove_scratch, state_for, window};
    use crate::{App, Shared};
    use greycard_edit::{Edit, Placement};

    /// `count` frames in a folder `missing` under a scratch folder of
    /// their own, which is not there yet: every save there fails, on
    /// every platform, until the test makes it. Sidecars are written,
    /// beside each frame, and each has an edit in memory to write.
    pub(crate) fn unwritable(name: &str, count: usize) -> (App, Rc<Shared>, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("greycard-{name}-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let missing = dir.join("missing");
        let files: Vec<PathBuf> = (0..count)
            .map(|i| missing.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(count);
        let (state, _worker) = state_for(&app, files);
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = Placement::Beside;
            let mut edit = Edit::default();
            edit.light.exposure = 0.5;
            for s in &mut st.sidecars[..count] {
                s.record(edit.clone());
            }
        }
        (app, state, dir, missing)
    }

    /// The note stays while the edit is not saved, whatever writes the
    /// status line after; the frame and the reason are said there once.
    #[test]
    fn the_note_outlasts_the_status_line_and_goes_when_the_edit_lands() {
        let (app, state, dir, missing) = unwritable("unsaved-note", 1);
        let mut st = state.borrow_mut();
        write_sidecar(&mut st, 0);
        assert_eq!(
            app.get_status(),
            "the edit of IMG_0000.CR3 is not saved; see the log"
        );
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        // A develop's word over it, and a drag's saves failing on: the
        // note stands, and nothing more is said.
        app.set_status("developing...".into());
        for _ in 0..5 {
            write_sidecar(&mut st, 0);
        }
        assert_eq!(app.get_status(), "developing...");
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        assert_eq!(st.work.unsaved.said_count(), 1);
        // Landed: the note goes; a line written over the word stays.
        std::fs::create_dir_all(&missing).expect("the folder");
        write_sidecar(&mut st, 0);
        assert_eq!(app.get_sync_note(), "");
        assert_eq!(app.get_status(), "developing...");
        assert!(st.work.unsaved.edits.is_empty());
        drop(st);
        remove_scratch(state, &dir);
    }

    /// The word still on the status line comes off with the note once
    /// the edit lands; failing again after is said again.
    #[test]
    fn the_word_comes_off_when_the_edit_lands() {
        let (app, state, dir, missing) = unwritable("unsaved-word", 1);
        let mut st = state.borrow_mut();
        write_sidecar(&mut st, 0);
        assert_ne!(app.get_status(), "");
        std::fs::create_dir_all(&missing).expect("the folder");
        write_sidecar(&mut st, 0);
        assert_eq!(app.get_status(), "");
        assert_eq!(app.get_sync_note(), "");
        crate::testing::remove_dir_retry(&missing);
        write_sidecar(&mut st, 0);
        assert_eq!(st.work.unsaved.said_count(), 2);
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A batch failing on three is one word with their count; one of
    /// them landing leaves the note at two and the word standing, and
    /// the rest landing takes both off. A click on the note saves them
    /// again.
    #[test]
    fn a_batch_counts_and_the_note_follows_each_landing() {
        let (app, state, dir, missing) = unwritable("unsaved-batch", 3);
        let mut st = state.borrow_mut();
        write_files(&mut st, &[0, 1, 2]);
        assert!(!crate::panel::saves::out(&st));
        assert_eq!(
            app.get_status(),
            "the edits of IMG_0000.CR3 and 2 others are not saved; see the log"
        );
        assert_eq!(app.get_sync_note(), "3 edits not saved");
        // Sent again, still failing: nothing more said.
        app.set_status("".into());
        write_files(&mut st, &[0, 1, 2]);
        assert_eq!(app.get_status(), "");
        assert_eq!(st.work.unsaved.said_count(), 1);
        app.set_status("the edits of IMG_0000.CR3 and 2 others are not saved; see the log".into());
        std::fs::create_dir_all(&missing).expect("the folder");
        write_sidecar(&mut st, 0);
        assert_eq!(app.get_sync_note(), "2 edits not saved");
        assert_eq!(
            app.get_status(),
            "the edits of IMG_0000.CR3 and 2 others are not saved; see the log"
        );
        // The note's click: the two saved again.
        crate::sync::retry(&mut st, &app);
        assert_eq!(app.get_sync_note(), "");
        assert_eq!(app.get_status(), "");
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A session that writes no sidecars writes none, and a folder it
    /// cannot write is nothing to say.
    #[test]
    fn a_session_that_writes_no_sidecars_says_nothing() {
        let (app, state, dir, _missing) = unwritable("unsaved-off", 2);
        let mut st = state.borrow_mut();
        st.window.write_sidecars = false;
        write_sidecar(&mut st, 0);
        write_files(&mut st, &[0, 1]);
        assert_eq!(app.get_status(), "");
        assert_eq!(app.get_sync_note(), "");
        assert!(st.work.unsaved.edits.is_empty());
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A run nobody is at keeps count and says nothing.
    #[test]
    fn a_batch_run_says_nothing() {
        let (app, state, dir, _missing) = unwritable("unsaved-run", 1);
        let mut st = state.borrow_mut();
        st.window.batch = true;
        write_sidecar(&mut st, 0);
        assert_eq!(app.get_status(), "");
        assert_eq!(app.get_sync_note(), "");
        assert_eq!(st.work.unsaved.edits.len(), 1);
        drop(st);
        remove_scratch(state, &dir);
    }

    /// XMPs beside the frames that will not parse are a refusal by
    /// design, which costs the interop and nothing else: the edits are
    /// saved, alone or as a batch, and nothing is said.
    #[test]
    fn foreign_xmps_that_will_not_parse_say_nothing() {
        let dir = std::env::temp_dir().join(format!("greycard-unsaved-xmp-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..4)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        for f in &files {
            std::fs::write(f.with_extension("xmp"), b"<x:xmpmeta><rdf:RDF").unwrap();
        }
        let app = window(4);
        let (state, _worker) = state_for(&app, files.clone());
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.xmp_sidecars = true;
        st.window.placement = Placement::Beside;
        for s in &mut st.sidecars[..4] {
            s.meta.rating = 3;
        }
        write_sidecar(&mut st, 0);
        write_files(&mut st, &[1, 2, 3]);
        assert_eq!(app.get_status(), "");
        assert_eq!(app.get_sync_note(), "");
        assert!(st.work.unsaved.edits.is_empty());
        for f in &files {
            assert!(greycard_edit::Sidecar::load(f).unwrap().is_some());
        }
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A merge whose own save failed hands its edit to the window,
    /// which saves it once the file is listed, and notes it when that
    /// fails too.
    #[test]
    fn a_merged_edit_not_saved_is_saved_from_the_window() {
        let (app, state, dir, missing) = unwritable("unsaved-merge", 1);
        let mut st = state.borrow_mut();
        let path = st.files[0].clone();
        let mut edit = Edit::default();
        edit.light.exposure = 1.25;
        st.work.merge_edits.insert(path.clone(), edit.clone());
        crate::panel::merge::held(&mut st);
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        std::fs::create_dir_all(&missing).expect("the folder");
        retry(&mut st);
        assert_eq!(app.get_sync_note(), "");
        let back = greycard_edit::Sidecar::load(&path)
            .unwrap()
            .expect("written");
        assert_eq!(back.current.light.exposure, 1.25);
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A frame moved takes its state with it; one moved onto a path
    /// that had one takes that path's away.
    #[test]
    fn a_moved_frame_takes_its_unsaved_edit_along() {
        let mut u = Unsaved::default();
        let (a, b, c) = (Path::new("a.CR3"), Path::new("b.CR3"), Path::new("c.CR3"));
        u.edits.insert(a.into());
        u.edits.insert(c.into());
        u.file_moved(a, b);
        assert!(!u.edits.contains(a) && u.edits.contains(b));
        u.file_moved(a, c);
        assert!(!u.edits.contains(c));
    }

    /// A frame that leaves the list takes its edit not saved with it,
    /// and the status line says that edit is not saved: the word is
    /// never wiped as if it had landed, nor by a later landing.
    #[test]
    fn a_list_replaced_lets_go_of_what_it_no_longer_holds() {
        let (app, state, dir, missing) = unwritable("unsaved-listed", 3);
        let mut st = state.borrow_mut();
        write_files(&mut st, &[0, 1, 2]);
        assert_eq!(app.get_sync_note(), "3 edits not saved");
        app.set_status("".into());
        let gone = st.files.pop().unwrap();
        listed(&mut st);
        assert!(!st.work.unsaved.edits.contains(&gone));
        assert_eq!(app.get_sync_note(), "2 edits not saved");
        assert_eq!(
            app.get_status(),
            "the edit of IMG_0002.CR3 is not saved; see the log"
        );
        // The others land: the word of the one let go stays.
        std::fs::create_dir_all(&missing).expect("the folder");
        retry(&mut st);
        assert_eq!(app.get_sync_note(), "");
        assert_eq!(
            app.get_status(),
            "the edit of IMG_0002.CR3 is not saved; see the log"
        );
        drop(st);
        remove_scratch(state, &dir);
    }

    /// The same view listed again from disk (a collection's frames
    /// changed, an archive action) keeps an edit not saved over the
    /// disk's copy, and its note; a list of other frames lets it go,
    /// and says so.
    #[test]
    fn a_relist_keeps_an_unsaved_edit_and_another_folder_lets_it_go() {
        let dir =
            std::env::temp_dir().join(format!("greycard-unsaved-relist-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files = vec![dir.join("missing").join("IMG_0000.CR3")];
        let app = window(1);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = Placement::Beside;
            let mut edit = Edit::default();
            edit.light.exposure = 0.5;
            st.sidecars[0].record(edit);
            write_sidecar(&mut st, 0);
        }
        let held = |files: &[PathBuf]| -> Vec<crate::rows::Held> {
            files
                .iter()
                .map(|f| crate::rows::from_disk(f, Some(Placement::Beside), Default::default()))
                .collect()
        };
        crate::panel::browser::open_loaded(
            &state,
            &app,
            &worker,
            files.clone(),
            held(&files),
            true,
            0,
        );
        {
            let st = state.borrow();
            assert_eq!(st.sidecars[0].current.light.exposure, 0.5);
            assert_eq!(st.work.unsaved.edits.len(), 1);
        }
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        let other = vec![dir.join("other").join("IMG_0100.CR3")];
        crate::panel::browser::open_loaded(
            &state,
            &app,
            &worker,
            other.clone(),
            held(&other),
            true,
            0,
        );
        assert_eq!(app.get_sync_note(), "");
        assert_eq!(
            app.get_status(),
            "the edit of IMG_0000.CR3 is not saved; see the log"
        );
        remove_scratch(state, &dir);
    }

    /// Leaving the folder saves the edits not saved once more, before
    /// the list goes.
    #[test]
    fn leaving_the_folder_saves_again() {
        let dir =
            std::env::temp_dir().join(format!("greycard-unsaved-leave-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let missing = dir.join("missing");
        let files = vec![missing.join("IMG_0000.CR3")];
        let app = window(1);
        let (state, worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = Placement::Beside;
            st.sidecars[0].meta.rating = 4;
            write_sidecar(&mut st, 0);
            assert_eq!(st.work.unsaved.edits.len(), 1);
        }
        std::fs::create_dir_all(&missing).expect("the folder");
        let elsewhere = vec![dir.join("other").join("IMG_0100.CR3")];
        crate::roots::open_listing(
            &state,
            &app,
            &worker,
            crate::roots::Listing::Files(elsewhere),
            crate::roots::Select::Row(0),
        );
        assert!(state.borrow().work.unsaved.edits.is_empty());
        let back = greycard_edit::Sidecar::load(&files[0])
            .unwrap()
            .expect("written");
        assert_eq!(back.meta.rating, 4);
        remove_scratch(state, &dir);
    }

    /// The quit saves the edits not saved once more.
    #[test]
    fn the_quit_saves_again() {
        let (_app, state, dir, missing) = unwritable("unsaved-quit", 1);
        let mut st = state.borrow_mut();
        write_sidecar(&mut st, 0);
        std::fs::create_dir_all(&missing).expect("the folder");
        crate::panel::startup::save_at_quit(&mut st, Edit::default(), crate::sync::QUIT_WRITE_WAIT);
        assert!(st.work.unsaved.edits.is_empty());
        let back = greycard_edit::Sidecar::load(&st.files[0])
            .unwrap()
            .expect("written");
        assert_eq!(back.current.light.exposure, 0.5);
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A frame's sidecar read again from disk over the one not saved:
    /// the note does not count it.
    #[test]
    fn a_sidecar_read_again_is_not_counted() {
        let (app, state, dir, _missing) = unwritable("unsaved-reread", 1);
        let mut st = state.borrow_mut();
        write_sidecar(&mut st, 0);
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        assert!(crate::rows::reread_file(&mut st, &app, 0) || st.work.unsaved.edits.is_empty());
        assert_eq!(app.get_sync_note(), "");
        drop(st);
        remove_scratch(state, &dir);
    }

    #[test]
    fn the_reasons_are_said_plainly() {
        use std::io::ErrorKind as K;
        assert_eq!(Why::of_io(K::StorageFull), Why::Full);
        assert_eq!(Why::of_io(K::QuotaExceeded), Why::Full);
        assert_eq!(Why::of_io(K::ReadOnlyFilesystem), Why::ReadOnly);
        let denied = if cfg!(windows) {
            Why::Other
        } else {
            Why::Denied
        };
        assert_eq!(Why::of_io(K::PermissionDenied), denied);
        assert_eq!(Why::of_io(K::NotFound), Why::Other);
        assert_eq!(
            Why::of(&greycard_edit::Error::Xmp("refused".into())),
            Why::Other
        );
        let raw = |n: &str| Path::new("shoot").join(format!("IMG_{n}.CR3"));
        assert_eq!(
            detail(&[(raw("1"), Why::Full)]),
            "the edit of IMG_1.CR3 is not saved: the disk is full"
        );
        assert_eq!(
            detail(&[(raw("1"), Why::Other)]),
            "the edit of IMG_1.CR3 is not saved; see the log"
        );
        assert_eq!(
            detail(&[(raw("1"), Why::ReadOnly), (raw("2"), Why::ReadOnly)]),
            "the edits of IMG_1.CR3 and 1 other are not saved: the disk is read-only"
        );
        assert_eq!(
            detail(&[
                (raw("1"), Why::Denied),
                (raw("2"), Why::Full),
                (raw("3"), Why::Full)
            ]),
            "the edits of IMG_1.CR3 and 2 others are not saved; see the log"
        );
        assert_eq!(
            detail(&[(raw("1"), Why::Denied)]),
            "the edit of IMG_1.CR3 is not saved: shoot cannot be written to"
        );
    }
}
