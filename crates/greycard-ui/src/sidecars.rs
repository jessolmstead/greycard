//! Writing a frame's sidecar, and its meta to an XMP beside it, from
//! the window's thread: one file at a time, or handed to the batch
//! when there are two or more, which is `panel::saves`'s.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use greycard_edit::Checked;
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
    write_xmp(st, file);
    // The save took or joined another writer's, whose meta the XMP
    // written above did not have: it goes again, and the sidecar with
    // its mark.
    if save_file_checked(st, file).is_some_and(|c| matches!(c, Checked::Took | Checked::Joined))
        && st.window.xmp_sidecars
    {
        write_xmp(st, file);
        save_file(st, file);
    }
}

/// [`write_file`]'s XMP: file `file`'s meta to the XMP beside it when
/// that is asked for, its mark kept in the sidecar in memory.
fn write_xmp(st: &mut State, file: usize) {
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
            .map(|o| xmp::Turn::new(o, st.sidecars[file].xmp_turn()));
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
    save_file_checked(st, file).is_some()
}

/// [`save_file`], saying what the save found on disk: none when it
/// failed. The file is asked first whether it is still the one the
/// window read or last wrote (§233's check, on the local copy:
/// [`Sidecar::save_checked`]); when another writer saved it since, its
/// file is taken, its meta joined, or it is kept beside the frame and
/// ours written, and the window holds and shows what was saved, as a
/// read of the file would.
///
/// [`Sidecar::save_checked`]: greycard_edit::Sidecar::save_checked
fn save_file_checked(st: &mut State, file: usize) -> Option<Checked> {
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
        return Some(Checked::Written);
    }
    let (path, placement) = (st.files[file].clone(), st.window.placement);
    let stamp = greycard_edit::sync::Stamp::now();
    match st.sidecars[file].save_checked_said(&path, placement, &stamp) {
        Ok(greycard_edit::Saved { checked, asides }) => {
            let of = |l: Vec<PathBuf>| -> Vec<(PathBuf, PathBuf)> {
                l.into_iter().map(|to| (path.clone(), to)).collect()
            };
            said_asides(st, &of(asides.bad), &of(asides.kept));
            if checked != Checked::Written {
                tracing::info!(
                    "{}: its sidecar was saved by another writer since it was read; {}",
                    file_name(&path),
                    match checked {
                        Checked::Joined => "the meta joined with it",
                        Checked::Took => "behind it, and taken from it",
                        _ => "that file kept beside the frame, and the edit here written",
                    }
                );
            }
            if matches!(checked, Checked::Took | Checked::Joined) {
                taken_from_disk(st, file);
            }
            crate::library::sidecar_written(st, file);
            landed(st, &path);
            Some(checked)
        }
        Err(greycard_edit::NotSaved { error, asides }) => {
            tracing::warn!("{}: sidecar not saved: {error}", file_name(&path));
            log_asides(&path, &asides);
            failed(st, &path, Why::of(&error));
            None
        }
    }
}

/// File `file`'s sidecar in memory replaced by what its file holds (a
/// join with another writer's save, or that save taken): shown as a
/// read of the file shows it, in its cells' marks and pictures, the
/// list following its versions, and on the frame on screen in the
/// panel and the history, at the next turn of the event loop so that
/// whatever saved is done with the panel first.
pub(crate) fn taken_from_disk(st: &mut State, file: usize) {
    let Some(app) = st.window.app.upgrade() else {
        return;
    };
    if crate::rows::versions_differ(st, file) {
        crate::versions::settle_later(&app);
    }
    crate::panel::browser::show_badges_of_file(st, &app, file);
    crate::edited::refresh_file(st, file);
    if st.current.is_some_and(|c| st.file_at(c) == Some(file)) {
        crate::panel::history::show_settled_later(&app);
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
        && st.sidecars.get(file).is_none_or(|s| !s.read_only())
}

/// Why a sidecar's write failed, in the few words the status line has
/// for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Why {
    Full,
    ReadOnly,
    Denied,
    /// The sidecar on disk is from a newer build, which this one does
    /// not write over.
    Newer,
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
            greycard_edit::Error::Newer(_) | greycard_edit::Error::ReadOnly(_) => Self::Newer,
            _ => Self::Other,
        }
    }

    /// The reason in words for a save of `raw`'s, or of `n` frames' the
    /// first of which is `raw`, none when the log has to say it.
    fn words(self, raw: &Path, n: usize) -> Option<String> {
        match self {
            Self::Newer => Some(tr!("its sidecar is from a newer greycard; update to save"
                | "their sidecars are from a newer greycard; update to save" % n)),
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
    /// The reason each frame's last save failed, for the word said when
    /// its edit is let go.
    why: std::collections::HashMap<PathBuf, Why>,
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
            if let Some(why) = self.why.remove(was) {
                self.why.insert(to.to_path_buf(), why);
            }
        } else {
            self.edits.remove(to);
            self.why.remove(to);
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
    for (raw, why) in failures {
        new |= unsaved.edits.insert(raw.clone());
        unsaved.why.insert(raw.clone(), *why);
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

/// Sidecars put aside by a save that then landed (each frame and where
/// its file went): those that would not parse (`bad`), said on the
/// status line, or else other writers' files kept beside their frames
/// (`kept`); the frame and the file named when there is one, a count
/// when there are more. Each is said once, being a file of its own. A
/// save that put one aside and then failed says its failure, and the
/// file in the log ([`log_asides`]).
pub(crate) fn said_asides(st: &mut State, bad: &[(PathBuf, PathBuf)], kept: &[(PathBuf, PathBuf)]) {
    let line = match (bad, kept) {
        ([], []) => return,
        ([], [(raw, to)]) => tr!(
            "{name} was edited elsewhere; other copy kept as {file}",
            name = file_name(raw),
            file = file_name(to)
        ),
        ([], more) => tr!("{n} frame was edited elsewhere; other copy kept beside it"
            | "{n} frames were edited elsewhere; other copies kept beside them" % more.len()),
        (set_aside, _) => said_bad(set_aside),
    };
    if st.window.batch {
        return;
    }
    if let Some(app) = st.window.app.upgrade() {
        app.set_status(line.into());
    }
}

/// The status line's word for sidecars set aside that would not parse.
fn said_bad(set_aside: &[(PathBuf, PathBuf)]) -> String {
    match set_aside {
        [(raw, to)] => tr!(
            "{name}: unreadable sidecar kept as {file}; edit saved",
            name = file_name(raw),
            file = file_name(to)
        ),
        more => tr!("{n} unreadable sidecar kept beside its frame; edit saved"
            | "{n} unreadable sidecars kept beside their frames; edits saved" % more.len()),
    }
}

/// The sidecars a save of `raw`'s put aside before it failed, in the
/// log beside the failure: kept where they went, the edit not saved.
pub(crate) fn log_asides(raw: &Path, asides: &greycard_edit::Asides) {
    for to in asides.bad.iter().chain(&asides.kept) {
        tracing::warn!(
            "{}: its sidecar was put aside as {}; the edit here is not saved",
            file_name(raw),
            to.display()
        );
    }
}

/// A save of `raw`'s sidecar landed: its edit is on disk, and the note
/// says one fewer; with none left, the status line's word of them
/// comes off if it still stands.
pub(crate) fn landed(st: &mut State, raw: &Path) {
    st.work.unsaved.why.remove(raw);
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
    st.work.unsaved.why.remove(raw);
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
    let failures: Vec<(PathBuf, Why)> = lost
        .into_iter()
        .map(|p| {
            let why = st.work.unsaved.why.remove(&p).unwrap_or(Why::Other);
            (p, why)
        })
        .collect();
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
    .words(first, failures.len());
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
            assert!(greycard_edit::Sidecar::load_as_file(f).unwrap().is_some());
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
        let back = greycard_edit::Sidecar::load_as_file(&path)
            .unwrap()
            .expect("written");
        assert_eq!(back.current().light.exposure, 1.25);
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
            assert_eq!(st.sidecars[0].current().light.exposure, 0.5);
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
        let back = greycard_edit::Sidecar::load_as_file(&files[0])
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
        let back = greycard_edit::Sidecar::load_as_file(&st.files[0])
            .unwrap()
            .expect("written");
        assert_eq!(back.current().light.exposure, 0.5);
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

    /// `count` frames in a scratch folder of their own, each with a
    /// sidecar on disk at an exposure of 0.25 and read by the window as
    /// a frame is opened, sidecars written beside them.
    fn held(name: &str, count: usize) -> (App, Rc<Shared>, PathBuf, Vec<PathBuf>) {
        let dir = std::env::temp_dir().join(format!("greycard-{name}-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..count)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        for f in &files {
            let mut s = greycard_edit::Sidecar::default();
            s.record(exposed(0.25));
            s.save_in(f, Placement::Beside).expect("written");
        }
        let app = window(count);
        let (state, _worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = Placement::Beside;
            for f in 0..count {
                assert!(crate::rows::reread_file(&mut st, &app, f));
            }
        }
        (app, state, dir, files)
    }

    fn exposed(ev: f32) -> Edit {
        let mut e = Edit::default();
        e.light.exposure = ev;
        e
    }

    /// What `greycard presets apply` does to `raw`'s sidecar: read from
    /// disk, a preset laid over its current state under the preset's
    /// label, the rating set by another hand beside it, and saved back
    /// where it was found.
    fn second_writer(raw: &Path) {
        let mut other = greycard_edit::Sidecar::load_as_file(raw).unwrap().unwrap();
        let mut grain = Edit::default();
        grain.grain.amount = 0.4;
        let preset = greycard_edit::Preset {
            name: "Fine Grain".into(),
            sections: vec![greycard_edit::Section::Grain],
            edit: grain,
            ..Default::default()
        };
        let edit = preset.applied(other.current());
        let label = greycard_edit::history::preset_label(&preset.name);
        assert!(other.record_as(edit, Some(label)));
        other.meta.rating = 4;
        // A minute before the window's save, so which of the two is the
        // later is not left to a tie within one second.
        let mut stamp = greycard_edit::sync::Stamp::now();
        stamp.at -= 60;
        other
            .save_in_stamped(raw, Placement::of(raw), &stamp)
            .expect("written");
    }

    /// The other writer's file kept beside `raw`'s sidecar, read.
    fn kept_copy(raw: &Path) -> greycard_edit::Sidecar {
        let path = greycard_edit::Sidecar::path_for(raw);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        greycard_edit::Sidecar::read_as_file(&path.with_file_name(format!("{name}.other"))).unwrap()
    }

    /// Whether `s` holds a state, in its history or as its current, with
    /// `pick` of its edit.
    fn has_state(s: &greycard_edit::Sidecar, pick: impl Fn(&Edit) -> bool) -> bool {
        s.history.iter().any(|st| pick(st.edit())) || pick(s.current())
    }

    /// A second writer between two saves of a frame the window holds
    /// (here the CLI's preset over the file), and a slider here: both
    /// changed the lines, so the other's file is kept beside the frame,
    /// said once on the status line, and the window's written as it is.
    #[test]
    fn a_second_writer_between_saves_is_kept_not_undone() {
        let (app, state, dir, files) = held("second-writer", 1);
        let raw = &files[0];
        second_writer(raw);
        let mut st = state.borrow_mut();
        st.sidecars[0].record(exposed(1.0));
        write_sidecar(&mut st, 0);
        let back = greycard_edit::Sidecar::load_as_file(raw).unwrap().unwrap();
        assert_eq!(back.current().light.exposure, 1.0, "the window's own");
        assert!(!has_state(&back, |e| e.grain.amount == 0.4));
        assert_eq!(st.sidecars[0], back, "the window holds what is on disk");
        let kept = kept_copy(raw);
        assert_eq!(kept.meta.rating, 4, "the other writer's rating");
        assert_eq!(
            kept.current().grain.amount,
            0.4,
            "the other writer's preset"
        );
        assert_eq!(
            app.get_status(),
            "IMG_0000.CR3 was edited elsewhere; other copy kept as IMG_0000.CR3.gcd.other"
        );
        assert!(st.work.unsaved.edits.is_empty());
        drop(st);
        remove_scratch(state, &dir);
    }

    /// Only a rating given in the window while the CLI laid its preset:
    /// the preset stays current, on disk and in the window, and the
    /// window's rating (the later) is the frame's.
    #[test]
    fn a_rating_in_the_window_keeps_the_presets_look() {
        let (_app, state, dir, files) = held("second-writer-rating", 1);
        second_writer(&files[0]);
        let mut st = state.borrow_mut();
        st.sidecars[0].meta.rating = 2;
        write_sidecar(&mut st, 0);
        let back = greycard_edit::Sidecar::load_as_file(&files[0])
            .unwrap()
            .unwrap();
        assert_eq!(back.current().grain.amount, 0.4);
        assert_eq!(back.current().light.exposure, 0.25);
        assert_eq!(back.meta.rating, 2);
        assert_eq!(st.sidecars[0], back);
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A batch that keeps another writer's file, landing after a newer
    /// save of its frame on the window's thread: the newer save stands,
    /// in the window and on disk, and the other's file is kept.
    #[test]
    fn a_batch_landing_late_does_not_undo_a_newer_save() {
        let (_app, state, dir, files) = held("second-writer-late", 2);
        second_writer(&files[1]);
        let mut st = state.borrow_mut();
        for s in &mut st.sidecars[..2] {
            s.record(exposed(1.0));
        }
        crate::panel::saves::tests::HOLD.set(true);
        write_files(&mut st, &[0, 1]);
        crate::panel::saves::tests::HOLD.set(false);
        crate::panel::saves::tests::wait_moved(&st);
        st.sidecars[1].record(exposed(1.5));
        write_sidecar(&mut st, 1);
        assert!(crate::panel::saves::finish(&mut st, None));
        let back = greycard_edit::Sidecar::load_as_file(&files[1])
            .unwrap()
            .unwrap();
        assert_eq!(st.sidecars[1].current().light.exposure, 1.5);
        assert_eq!(st.sidecars[1], back);
        assert_eq!(kept_copy(&files[1]).current().grain.amount, 0.4);
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A sidecar that will not parse is set aside and the window's edit
    /// written: nothing waits on a hand to delete it.
    #[test]
    fn a_torn_sidecar_is_set_aside_and_the_edit_saved() {
        let (app, state, dir, files) = held("second-writer-torn", 2);
        for f in &files {
            let path = greycard_edit::Sidecar::path_for(f);
            std::fs::remove_file(&path).unwrap();
            std::fs::write(&path, "{\"current\": ").unwrap();
        }
        let mut st = state.borrow_mut();
        for s in &mut st.sidecars[..2] {
            s.record(exposed(1.0));
        }
        write_sidecar(&mut st, 0);
        assert_eq!(
            app.get_status(),
            "IMG_0000.CR3: unreadable sidecar kept as IMG_0000.CR3.gcd.bad; edit saved"
        );
        crate::panel::saves::save(&mut st, &[1]);
        assert_eq!(
            app.get_status(),
            "IMG_0001.CR3: unreadable sidecar kept as IMG_0001.CR3.gcd.bad; edit saved"
        );
        assert_eq!(app.get_sync_note(), "");
        for (f, raw) in files.iter().enumerate() {
            let back = greycard_edit::Sidecar::load_as_file(raw).unwrap().unwrap();
            assert_eq!(st.sidecars[f], back, "{f}");
            let bad = greycard_edit::Sidecar::path_for(raw).with_extension("gcd.bad");
            assert_eq!(std::fs::read_to_string(bad).unwrap(), "{\"current\": ");
        }
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A batch with nothing of its own on a frame another writer laid a
    /// preset and a rating on takes the file, and the XMP beside it
    /// follows the taken meta at once.
    #[test]
    fn a_batch_that_takes_the_file_writes_its_xmp() {
        let (_app, state, dir, files) = held("second-writer-took-xmp", 1);
        second_writer(&files[0]);
        let mut st = state.borrow_mut();
        st.window.xmp_sidecars = true;
        crate::panel::saves::save(&mut st, &[0]);
        let back = greycard_edit::Sidecar::load_as_file(&files[0])
            .unwrap()
            .unwrap();
        assert_eq!(back.current().grain.amount, 0.4, "the preset current");
        assert_eq!(st.sidecars[0], back);
        let text = std::fs::read_to_string(files[0].with_extension("xmp")).expect("the XMP");
        assert!(
            text.contains("Rating=\"4\"") || text.contains("Rating>4<"),
            "{text}"
        );
        drop(st);
        remove_scratch(state, &dir);
    }

    /// An edit let go with its list, its save refused by a newer build's
    /// sidecar, is said with that reason.
    #[test]
    fn an_edit_let_go_says_why_it_was_not_saved() {
        let (app, state, dir, files) = held("second-writer-let-go", 1);
        let path = greycard_edit::Sidecar::path_for(&files[0]);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "{\"current\": {\"version\": 99}}").unwrap();
        let mut st = state.borrow_mut();
        st.sidecars[0].record(exposed(1.0));
        write_sidecar(&mut st, 0);
        app.set_status("".into());
        st.files.pop();
        listed(&mut st);
        assert_eq!(
            app.get_status(),
            "the edit of IMG_0000.CR3 is not saved: its sidecar is from a newer greycard; \
             update to save"
        );
        drop(st);
        remove_scratch(state, &dir);
    }

    fn history_names(app: &App) -> Vec<String> {
        use slint::Model;
        let rows = app.get_history_rows();
        (0..rows.row_count())
            .map(|r| rows.row_data(r).unwrap().name.to_string())
            .collect()
    }

    /// The frame on screen, rated here while another writer laid its
    /// preset: the file is taken with the rating joined into it, and
    /// shown as a read of the file would show it, its history and its
    /// panel the file's, at the next turn of the event loop.
    #[test]
    fn the_frame_on_screen_shows_what_was_taken() {
        let (app, state, dir, files) = held("second-writer-shown", 1);
        let worker = Rc::new(crate::worker::Worker::new(|_| {}));
        second_writer(&files[0]);
        let mut st = state.borrow_mut();
        st.current = Some(0);
        crate::panel::history::show_settled_pending(&mut st, &app, &worker);
        st.sidecars[0].meta.rating = 2;
        write_sidecar(&mut st, 0);
        assert!(crate::panel::history::show_settled_pending(&mut st, &app, &worker) >= 1);
        let names = history_names(&app);
        assert!(names.iter().any(|n| n.contains("Fine Grain")), "{names:?}");
        assert_eq!(st.develop.edit, st.sidecars[0].current().clone());
        assert_eq!(st.sidecars[0].meta.rating, 2);
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A batch (a paste, a preset over a selection) meets the second
    /// writer's file at its rename: kept there beside the frame, the
    /// batch's written, the other frames saved as they were.
    #[test]
    fn a_batch_keeps_a_second_writers_save() {
        let (app, state, dir, files) = held("second-writer-batch", 3);
        second_writer(&files[1]);
        let mut st = state.borrow_mut();
        for s in &mut st.sidecars[..3] {
            s.record(exposed(1.0));
        }
        write_files(&mut st, &[0, 1, 2]);
        assert!(!crate::panel::saves::out(&st));
        for (f, raw) in files.iter().enumerate() {
            let back = greycard_edit::Sidecar::load_as_file(raw).unwrap().unwrap();
            assert_eq!(st.sidecars[f], back, "{f}");
            assert_eq!(back.current().light.exposure, 1.0, "{f}");
            assert!(!has_state(&back, |e| e.grain.amount == 0.4), "{f}");
        }
        assert_eq!(kept_copy(&files[1]).current().grain.amount, 0.4);
        assert_eq!(
            app.get_status(),
            "IMG_0001.CR3 was edited elsewhere; other copy kept as IMG_0001.CR3.gcd.other"
        );
        drop(st);
        remove_scratch(state, &dir);
    }

    /// Our own batch's rename, landed on disk before the window's next
    /// save and taken in after it, is not another writer: the save
    /// writes as it would, and nothing is joined.
    #[test]
    fn our_own_batch_is_not_a_second_writer() {
        let (_app, state, dir, files) = held("second-writer-own", 2);
        let mut st = state.borrow_mut();
        for s in &mut st.sidecars[..2] {
            s.record(exposed(1.0));
        }
        crate::panel::saves::tests::HOLD.set(true);
        write_files(&mut st, &[0, 1]);
        crate::panel::saves::tests::HOLD.set(false);
        // The batch's renames done, its landing not yet taken in.
        crate::panel::saves::tests::wait_moved(&st);
        let states = st.sidecars[1].history.len();
        st.sidecars[1].record(exposed(1.5));
        write_sidecar(&mut st, 1);
        assert!(crate::panel::saves::finish(&mut st, None));
        let back = greycard_edit::Sidecar::load_as_file(&files[1])
            .unwrap()
            .unwrap();
        assert_eq!(st.sidecars[1], back);
        assert_eq!(back.current().light.exposure, 1.5);
        assert_eq!(back.history.len(), states + 1, "nothing joined");
        assert!(
            back.history
                .iter()
                .all(|s| !s.label().unwrap_or("").contains("Reconciled"))
        );
        drop(st);
        remove_scratch(state, &dir);
    }

    /// An edit whose save failed, kept in memory and saved again: the
    /// retry asks the disk too, and keeps a sidecar another writer made
    /// meanwhile beside the frame rather than writing over it unseen.
    #[test]
    fn a_retried_save_keeps_what_was_written_meanwhile() {
        let (app, state, dir, missing) = unwritable("second-writer-retry", 1);
        let mut st = state.borrow_mut();
        write_sidecar(&mut st, 0);
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        std::fs::create_dir_all(&missing).expect("the folder");
        let raw = st.files[0].clone();
        let mut other = greycard_edit::Sidecar::default();
        let mut grain = Edit::default();
        grain.grain.amount = 0.4;
        other.record(grain);
        other.meta.rating = 4;
        other.save_in(&raw, Placement::Beside).unwrap();
        retry(&mut st);
        assert_eq!(app.get_sync_note(), "");
        let back = greycard_edit::Sidecar::load_as_file(&raw).unwrap().unwrap();
        assert!(has_state(&back, |e| e.light.exposure == 0.5));
        assert_eq!(st.sidecars[0], back);
        let kept = kept_copy(&raw);
        assert_eq!((kept.meta.rating, kept.current().grain.amount), (4, 0.4));
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A sidecar replaced by one from a newer build, or one that will
    /// not read, is never written over: the edit stays in memory and is
    /// noted not saved, as a save that failed is.
    #[test]
    fn a_newer_sidecar_on_disk_is_not_written_over() {
        let (app, state, dir, files) = held("second-writer-newer", 1);
        let path = greycard_edit::Sidecar::path_for(&files[0]);
        let newer = "{\"current\": {\"version\": 99, \"light\": {\"exposure\": 2.0}}}";
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, newer).unwrap();
        let mut st = state.borrow_mut();
        st.sidecars[0].record(exposed(1.0));
        write_sidecar(&mut st, 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        assert_eq!(st.sidecars[0].current().light.exposure, 1.0);
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        assert_eq!(
            app.get_status(),
            "the edit of IMG_0000.CR3 is not saved: its sidecar is from a newer greycard; \
             update to save"
        );
        // The batch holds to it as well.
        st.sidecars[0].record(exposed(1.25));
        crate::panel::saves::save(&mut st, &[0]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        drop(st);
        remove_scratch(state, &dir);
    }

    /// The XMP beside the frame follows what the save takes: rated here
    /// while the other writer laid its preset and rating, the file is
    /// taken with the window's rating joined into it, and the XMP says
    /// the joined rating, by hand and in a batch.
    #[test]
    fn the_xmp_follows_what_was_taken() {
        let (_app, state, dir, files) = held("second-writer-xmp", 2);
        for f in &files {
            second_writer(f);
        }
        let mut st = state.borrow_mut();
        st.window.xmp_sidecars = true;
        st.sidecars[0].meta.flag = greycard_edit::meta::Flag::Pick;
        write_sidecar(&mut st, 0);
        st.sidecars[1].meta.flag = greycard_edit::meta::Flag::Pick;
        crate::panel::saves::save(&mut st, &[1]);
        for (f, raw) in files.iter().enumerate() {
            let text = std::fs::read_to_string(raw.with_extension("xmp")).expect("the XMP");
            assert!(
                text.contains("Rating=\"4\"") || text.contains("Rating>4<"),
                "{f}: {text}"
            );
            let back = greycard_edit::Sidecar::load_as_file(raw).unwrap().unwrap();
            assert_eq!(back.meta.rating, 4);
            assert_eq!(back.meta.flag, greycard_edit::meta::Flag::Pick);
            assert_eq!(st.sidecars[f], back, "{f}");
        }
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
        assert_eq!(Why::of(&greycard_edit::Error::Newer(99)), Why::Newer);
        let raw = |n: &str| Path::new("shoot").join(format!("IMG_{n}.CR3"));
        assert_eq!(
            detail(&[(raw("1"), Why::Newer), (raw("2"), Why::Newer)]),
            "the edits of IMG_1.CR3 and 1 other are not saved: their sidecars are from a newer \
             greycard; update to save"
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

    /// A batch's own write is the file the window's sidecar knows after
    /// it lands: the next save, by hand or by a batch, finds it as known
    /// by a stat and reads nothing.
    #[test]
    fn a_batchs_own_write_is_known_at_the_next_save() {
        let (_app, state, dir, files) = held("batch-known", 1);
        let mut st = state.borrow_mut();
        st.sidecars[0].record(exposed(1.0));
        crate::panel::saves::save(&mut st, &[0]);
        let reads = greycard_edit::changed_reads();
        st.sidecars[0].record(exposed(1.5));
        assert_eq!(save_file_checked(&mut st, 0), Some(Checked::Written));
        assert_eq!(greycard_edit::changed_reads(), reads, "nothing read");
        let back = greycard_edit::Sidecar::load_as_file(&files[0])
            .unwrap()
            .unwrap();
        assert_eq!(st.sidecars[0], back);
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A batch that keeps another writer's file, and a slider moved in
    /// the window after the batch started: the landing leaves the
    /// window's sidecar, slider and all, and its next save writes it.
    #[test]
    fn a_change_made_while_a_batch_saves_is_kept() {
        let (_app, state, dir, files) = held("batch-keep-change", 1);
        second_writer(&files[0]);
        let mut st = state.borrow_mut();
        st.sidecars[0].record(exposed(1.0));
        crate::panel::saves::tests::HOLD.set(true);
        crate::panel::saves::save(&mut st, &[0]);
        crate::panel::saves::tests::HOLD.set(false);
        crate::panel::saves::tests::wait_moved(&st);
        st.sidecars[0].record(exposed(1.5));
        assert!(crate::panel::saves::finish(&mut st, None));
        assert_eq!(
            st.sidecars[0].current().light.exposure,
            1.5,
            "the slider kept"
        );
        save_file(&mut st, 0);
        let back = greycard_edit::Sidecar::load_as_file(&files[0])
            .unwrap()
            .unwrap();
        assert_eq!(back.current().light.exposure, 1.5);
        assert!(has_state(&back, |e| e.light.exposure == 1.0));
        assert_eq!(st.sidecars[0], back);
        assert_eq!(
            kept_copy(&files[0]).current().grain.amount,
            0.4,
            "the preset"
        );
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A save that sets a torn sidecar aside and then fails on a newer
    /// build's in the other place says the failure, not that the edit is
    /// saved, by hand and in a batch; the torn file is kept all the same.
    #[test]
    fn a_save_that_sets_one_aside_and_fails_says_the_failure() {
        let (app, state, dir, files) = held("set-aside-failed", 1);
        let beside = greycard_edit::Sidecar::path_for(&files[0]);
        let folder = greycard_edit::Sidecar::path_in(&files[0], Placement::Folder);
        std::fs::create_dir_all(folder.parent().unwrap()).unwrap();
        let newer = "{\"current\": {\"version\": 99, \"light\": {\"exposure\": 2.0}}}";
        std::fs::write(&folder, newer).unwrap();
        let failure = "the edit of IMG_0000.CR3 is not saved: its sidecar is from a newer \
                       greycard; update to save";
        let mut st = state.borrow_mut();
        st.sidecars[0].record(exposed(1.0));
        for batch in [false, true] {
            let _ = std::fs::remove_file(&beside);
            std::fs::write(&beside, "{\"current\": ").unwrap();
            app.set_status("".into());
            if batch {
                crate::panel::saves::save(&mut st, &[0]);
            } else {
                assert!(!save_file(&mut st, 0));
            }
            // Said once, by the first failure; the batch's adds nothing.
            let said = if batch { "" } else { failure };
            assert_eq!(app.get_status(), said, "batch {batch}");
            assert_eq!(app.get_sync_note(), "1 edit not saved");
            assert!(!beside.exists(), "batch {batch}");
            assert_eq!(std::fs::read_to_string(&folder).unwrap(), newer);
        }
        let bad = beside.with_file_name("IMG_0000.CR3.gcd.bad");
        let bad2 = beside.with_file_name("IMG_0000.CR3.gcd.2.bad");
        for b in [bad, bad2] {
            assert_eq!(std::fs::read_to_string(b).unwrap(), "{\"current\": ");
        }
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A batch that sets one frame's torn sidecar aside and saves it, and
    /// fails on another's from a newer build: the failure is what the
    /// status line says, not that the edits are saved.
    #[test]
    fn a_batchs_failure_is_not_said_over_by_a_set_aside() {
        let (app, state, dir, files) = held("set-aside-and-failure", 2);
        let torn = greycard_edit::Sidecar::path_for(&files[0]);
        std::fs::remove_file(&torn).unwrap();
        std::fs::write(&torn, "{\"current\": ").unwrap();
        let newer = greycard_edit::Sidecar::path_for(&files[1]);
        std::fs::remove_file(&newer).unwrap();
        std::fs::write(&newer, "{\"current\": {\"version\": 99}}").unwrap();
        let mut st = state.borrow_mut();
        for s in &mut st.sidecars[..2] {
            s.record(exposed(1.0));
        }
        let failure = "the edit of IMG_0001.CR3 is not saved: its sidecar is from a newer \
                       greycard; update to save";
        crate::panel::saves::save(&mut st, &[0, 1]);
        assert_eq!(app.get_status(), failure);
        // Failing again says nothing new, and nothing over the failure.
        std::fs::remove_file(&torn).unwrap();
        std::fs::write(&torn, "{\"current\": ").unwrap();
        app.set_status("".into());
        crate::panel::saves::save(&mut st, &[0, 1]);
        assert_eq!(app.get_status(), "");
        let back = greycard_edit::Sidecar::load_as_file(&files[0])
            .unwrap()
            .unwrap();
        assert_eq!(st.sidecars[0], back);
        assert!(torn.with_file_name("IMG_0000.CR3.gcd.2.bad").exists());
        drop(st);
        remove_scratch(state, &dir);
    }

    /// A frame noted not saved, whose batch join lands after the window
    /// changed it again: the landing leaves the window's sidecar, so the
    /// note stays until a save of what the window holds lands.
    #[test]
    fn a_refused_batch_landing_keeps_the_unsaved_note() {
        let (app, state, dir, files) = held("batch-refused-note", 1);
        let path = greycard_edit::Sidecar::path_for(&files[0]);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "{\"current\": {\"version\": 99}}").unwrap();
        let mut st = state.borrow_mut();
        st.sidecars[0].record(exposed(1.0));
        assert!(!save_file(&mut st, 0));
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        std::fs::remove_file(&path).unwrap();
        let mut other = greycard_edit::Sidecar::default();
        other.record(exposed(-1.0));
        other.save_in(&files[0], Placement::Beside).unwrap();
        crate::panel::saves::tests::HOLD.set(true);
        crate::panel::saves::save(&mut st, &[0]);
        crate::panel::saves::tests::HOLD.set(false);
        crate::panel::saves::tests::wait_moved(&st);
        st.sidecars[0].record(exposed(1.5));
        assert!(crate::panel::saves::finish(&mut st, None));
        assert_eq!(st.sidecars[0].current().light.exposure, 1.5);
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        assert!(save_file(&mut st, 0));
        assert_eq!(app.get_sync_note(), "");
        drop(st);
        remove_scratch(state, &dir);
    }
}
