//! One frame in two places, the editor's side (§233; the file's side
//! is §243): the sidecar written to the archive's copy behind every
//! save, kept to the latest per frame and never on the window's
//! thread; the copy checked first against what we last wrote there,
//! so a copy another machine saved since is joined and not written
//! over; the writes that could not go kept in the library's index, so
//! a quit does not lose them, and taken up when the archive answers;
//! the status line's word for what waits; and the frame on screen
//! following to the archive's copy when its own root goes.
//!
//! Every write, every comparison and every read of a sidecar that is
//! not the window's own runs off the window's thread
//! ([`archive::supervise`]), beating as it goes; what it finds lands
//! on the window's thread, where the window's own sidecar and its
//! save are. A copy that is behind the window's is written; one that
//! is not is brought back whole, compared again with what the window
//! holds *now*, and taken or joined from that: a save made while the
//! job was out is never undone by what the job saw before it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use greycard_edit::sync::{Compared, Side, compare, join};
use greycard_edit::{Placement, Sidecar, xmp};
use greycard_library::{ArchiveWrite, Library};
use slint::ComponentHandle;

use crate::archive::{self, Beat, Heard};
use crate::panel::browser::file_name;
use crate::roots::ROOT_WAIT;
use crate::worker::Worker;
use crate::{App, State};

/// How long a write to an archive may say nothing before it is given
/// up (§233): a share that answered its look and then went quiet.
pub(crate) const WRITE_WAIT: Duration = Duration::from_secs(10);

/// What the window keeps of the writes behind its saves.
#[derive(Default)]
pub(crate) struct Sync {
    /// The latest write queued for each frame whose job has not gone
    /// out yet: ten saves in a burst are one write.
    queued: HashMap<PathBuf, Write>,
    /// The frames whose job is out, until it is done or lost: a job
    /// set aside (quiet past its wait) is still out, and goes on
    /// alone; a second for the frame does not start beside it.
    in_flight: HashSet<PathBuf>,
    /// The archives whose waiting writes have been taken up since
    /// they last did not answer: once an answer, not at every pass.
    caught_up: HashSet<PathBuf>,
    /// The frames the window follows on an archive's copy (§233), by
    /// the copy's path: what the frame was, and which archive.
    followed: HashMap<PathBuf, Followed>,
    /// The frames whose follow check is out.
    checking: HashSet<PathBuf>,
    /// Saves made before the index reader was open (a launch preset,
    /// the first save of a window whose index is still opening): paired
    /// and queued when it is (`reader_opened`), never dropped.
    deferred: Vec<(PathBuf, Source)>,
}

/// A frame followed to its archive copy: where it was, and where.
#[derive(Debug, Clone)]
pub(crate) struct Followed {
    pub(crate) local: PathBuf,
    pub(crate) archive: PathBuf,
    /// Whether the local copy's sidecar has been brought up to the
    /// copy's since its root came back (`heard_over`); once a return.
    pub(crate) homed: bool,
}

/// Where a write's sidecar comes from.
#[derive(Debug, Clone)]
pub(crate) enum Source {
    /// The window's own, saved just now.
    Memory(Box<Sidecar>),
    /// The frame's file, read on the job's thread: a frame the window
    /// does not hold, or one whose save was made on disk.
    Disk,
    /// The frame's file, to be settled against this, the copy's
    /// sidecar a write found not behind it, on the job's thread: taken
    /// whole when the file is behind, joined and saved when the two
    /// went their own ways; then written to the copy.
    Settle(Box<Sidecar>),
    /// Not a write to the copy at all: a followed frame's local root is
    /// back, and its sidecar at `local`, left behind while the saves
    /// went to the copy, is brought up to `ours` (the window's, the
    /// copy's) when it is behind. Through the frame's own queue, so it
    /// never runs beside a write of the frame.
    Home { local: PathBuf, ours: Box<Sidecar> },
}

/// One frame's sidecar to go to its copy on an archive.
#[derive(Debug, Clone)]
pub(crate) struct Write {
    /// The frame whose sidecar this is: the local frame, or for a frame
    /// the window follows, the archive's copy itself.
    pub(crate) frame: PathBuf,
    /// The frame's content hash, the key the index knows both copies
    /// by.
    pub(crate) hash: String,
    pub(crate) source: Source,
    /// Where a copy with no sidecar yet gets one: the local placement.
    pub(crate) placement: Placement,
    /// Whether the XMP beside the copy is written too.
    pub(crate) xmp: bool,
    /// The copies: each archive root and this frame's copy under it,
    /// one per archive ([`pair`]).
    pub(crate) copies: Vec<Copy>,
}

/// A frame's copy on one archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Copy {
    pub(crate) archive: PathBuf,
    pub(crate) copy: PathBuf,
}

/// What a write to one archive copy came to.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// Written, the XMP beside it regenerated, the write recorded.
    Written { archive: PathBuf, copy: PathBuf },
    /// The copy already held this: nothing written, the write
    /// recorded.
    Same { archive: PathBuf, copy: PathBuf },
    /// Not written; it waits in the index for the archive to answer
    /// (or the next try), for this reason.
    Waiting { archive: PathBuf, reason: String },
    /// The copy is not what we wrote and not behind what the job
    /// carried: the window compares it with what it holds now, and
    /// takes it or joins it.
    Take {
        archive: PathBuf,
        copy: PathBuf,
        frame: PathBuf,
        theirs: Box<Sidecar>,
    },
    /// The frame is gone from a folder that is there: deleted. Its
    /// row is dropped.
    Gone { archive: PathBuf, frame: PathBuf },
    /// A followed frame's local sidecar was brought up to the copy's
    /// (`Source::Home`): the indexer reads the frame again.
    Homed { local: PathBuf },
}

/// What a stat of the archive's sidecar says against the write we
/// recorded there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Check {
    /// No file: a copy with no sidecar yet.
    Absent,
    /// The file is the one we wrote: its size and mtime are the
    /// recorded ones.
    Ours,
    /// Something else wrote it since, or we never did: it is read and
    /// compared before anything is written.
    Changed,
    /// The stat itself failed, other than by the file not being
    /// there: a share that is not well. Nothing is written.
    Failed(String),
}

/// The archive's sidecar at `dest` against `last`, what we last wrote
/// there, by a stat alone: §233's check, which answers without
/// reading a file of megabytes over the wire. Equal size and mtime are
/// taken as the same file; on a filesystem whose times are coarse a
/// save another machine made within the same tick and to the same
/// length would pass as ours, which is accepted.
pub(crate) fn check(dest: &Path, last: Option<&ArchiveWrite>) -> Check {
    match std::fs::metadata(dest) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Check::Absent,
        Err(e) => Check::Failed(e.to_string()),
        Ok(meta) => match last {
            Some(w) if w.size == meta.len() && w.mtime == greycard_library::mtime_of(&meta) => {
                Check::Ours
            }
            _ => Check::Changed,
        },
    }
}

/// The stat check's `Ours`, held to one more thing: the revision we
/// recorded there is in `ours`'s own lineage (its revisions or the
/// ones it absorbed). The stat says the file is what we last wrote;
/// the lineage says what we last wrote came from this sidecar. A
/// sidecar that does not know it (the window's, read before a job
/// settled the file and the copy behind its back; or a copy rewritten
/// within the same second at the same size, which a stat cannot tell)
/// is compared, not written over. Nothing is read.
pub(crate) fn known(check: Check, last: Option<&ArchiveWrite>, ours: &Sidecar) -> Check {
    match (check, last) {
        (Check::Ours, Some(last))
            if !ours.revisions.iter().any(|r| r.hash == last.revision)
                && !ours.absorbed.contains(&last.revision) =>
        {
            Check::Changed
        }
        (check, _) => check,
    }
}

/// Where a copy's sidecar is, or goes: where [`Sidecar::find`] finds
/// one, else beside or under `.greycard/` as the local copy's is.
pub(crate) fn copy_sidecar(copy: &Path, placement: Placement) -> PathBuf {
    Sidecar::find(copy).unwrap_or_else(|| Sidecar::path_in(copy, placement))
}

/// A frame's copies on the archives, by the index alone: its content
/// hash, exactly one copy per archive that has one, and the archives
/// where more than one candidate stands and none is known to be this
/// frame's. One content hash can stand at two paths on one archive (a
/// shoot and its selects, both backed up); the copy of *this* frame is
/// the one under the folder its own folder was backed up to, by the
/// roots' pairings, and failing that the only candidate. Nothing is
/// asked of the disk.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Paired {
    pub(crate) hash: String,
    pub(crate) copies: Vec<Copy>,
    /// The archives with more than one candidate, and how many.
    pub(crate) ambiguous: Vec<(PathBuf, usize)>,
}

pub(crate) fn pair(lib: &Library, roots: &greycard_library::Roots, frame: &Path) -> Option<Paired> {
    let archives = roots.archives();
    if archives.is_empty() {
        return None;
    }
    let row = lib.by_path(frame).ok().flatten()?;
    let rows = lib.by_hash_under(&row.hash, archives).ok()?;
    // The frame as the index spells it: the window's path may run
    // through a symlink (a folder opened by one), the index's is
    // canonical, and a twin is any other row of the hash.
    let this = row.path;
    let mut paired = Paired {
        hash: row.hash,
        ..Paired::default()
    };
    for archive in archives {
        let candidates: Vec<&Path> = rows
            .iter()
            .filter(|e| e.path != frame && e.path.starts_with(archive))
            .map(|e| e.path.as_path())
            .collect();
        if candidates.is_empty() {
            continue;
        }
        // The folder this frame's folder was backed up to under this
        // archive names its copy; the oldest pairing first, as the
        // roots keep them.
        let pairings: Vec<&greycard_library::Pairing> = roots
            .backups()
            .iter()
            .filter(|p| p.dest.starts_with(archive))
            .collect();
        let expected: Vec<PathBuf> = pairings
            .iter()
            .filter_map(|p| {
                let rest = this.strip_prefix(&p.source).ok()?;
                Some(p.dest.join(rest))
            })
            .collect();
        // A sole hit is this frame's only when nothing says otherwise:
        // no pairing under this archive claims it for a folder this
        // frame is not in, and the hash stands at one path among the
        // local roots, this frame's. One raw in a shoot and its
        // selects, the shoot alone backed up, leaves the selects frame
        // with the shoot's copy as its sole hit; it is not its own.
        let sole = |c: &Path| {
            let claimed = pairings
                .iter()
                .any(|p| c.starts_with(&p.dest) && !this.starts_with(&p.source));
            let local_twins = lib
                .by_hash_under(&paired.hash, &roots.locals())
                .map(|rows| rows.iter().filter(|e| e.path != this).count())
                .unwrap_or(1);
            !claimed && local_twins == 0
        };
        let chosen = expected
            .iter()
            .find_map(|e| candidates.iter().find(|c| **c == e.as_path()).copied())
            .or_else(|| (candidates.len() == 1 && sole(candidates[0])).then(|| candidates[0]));
        match chosen {
            Some(copy) => paired.copies.push(Copy {
                archive: archive.to_path_buf(),
                copy: copy.to_path_buf(),
            }),
            None => paired
                .ambiguous
                .push((archive.to_path_buf(), candidates.len())),
        }
    }
    Some(paired)
}

/// Whether frame `c` is one the window follows on an archive's copy.
pub(crate) fn followed(st: &State, c: usize) -> bool {
    st.files
        .get(c)
        .is_some_and(|f| st.sync.followed.contains_key(f))
}

/// A frame's sidecar was saved by the window: its write to the
/// archive's copy is queued, the latest per frame, and sent unless one
/// for the frame is out (which sends this one when it lands). A frame
/// the window holds as its row standing in has its file's sidecar
/// written (the un-reject), so the file is what goes, not the stand-in.
/// A frame under an archive root queues nothing unless the window
/// follows it there: the editor works on the local copy, and the
/// archive's is written from it (§233).
pub(crate) fn after_save(st: &mut State, c: usize) {
    if !st.write_sidecars {
        return;
    }
    let Some(frame) = st.files.get(c).cloned() else {
        return;
    };
    if let Some(followed) = st.sync.followed.get(&frame).cloned() {
        // The frame is the archive's copy: its own write, checked
        // against the copy as any is.
        let Some(lib) = st.index_reader.as_ref() else {
            return;
        };
        let Some(row) = lib.by_path(&frame).ok().flatten() else {
            return;
        };
        if st.sidecars[c].revisions.is_empty() {
            return;
        }
        tracing::debug!(
            "sync: {} saved as {} (followed)",
            followed.local.display(),
            frame.display()
        );
        let write = Write {
            frame: frame.clone(),
            hash: row.hash,
            source: Source::Memory(Box::new(st.sidecars[c].clone())),
            placement: st.placement,
            xmp: st.xmp_sidecars,
            copies: vec![Copy {
                archive: followed.archive,
                copy: frame.clone(),
            }],
        };
        queue(st, write);
        return;
    }
    if st.library.roots.archive_of(&frame).is_some() {
        return;
    }
    let stand_in = st.from_row.get(c).is_some_and(|r| !r.read);
    let source = if stand_in {
        Source::Disk
    } else {
        if st.sidecars[c].revisions.is_empty() {
            return;
        }
        Source::Memory(Box::new(st.sidecars[c].clone()))
    };
    queue_for(st, &frame, source);
}

/// A frame's sidecar was saved on disk, by the window or a tool of its
/// (an export recorded on a frame it let go of, a stand-in un-rejected):
/// the archive write goes from the file.
pub(crate) fn after_disk_save(st: &mut State, frame: &Path) {
    if !st.write_sidecars || st.library.roots.archive_of(frame).is_some() {
        return;
    }
    queue_for(st, frame, Source::Disk);
}

/// A save made with no window open (a headless export's record,
/// §236): the frame's copies are not written now, but each is noted as
/// waiting in `index`, so the next window's catch-up writes them. The
/// index is opened for writing here, on the caller's thread; nothing
/// is created when there is no index. How many rows were noted.
pub(crate) fn note_disk_save(index: &Path, roots: &greycard_library::Roots, frame: &Path) -> usize {
    if !index.is_file() || roots.archive_of(frame).is_some() {
        return 0;
    }
    // Opened only at this build's schema, never made, brought up or
    // rebuilt from here: the window does that, with the user watching.
    // A lock held by a window is not waited on for long: the rows are
    // skipped, and said; the frame's next save in a window queues them.
    let mut lib = match Library::open_current(index, Duration::from_millis(250)) {
        Ok(lib) => lib,
        Err(e) => {
            tracing::warn!(
                "sync: {}: the index not written to: {e}; the archive copy waits for a window's save",
                file_name(frame)
            );
            return 0;
        }
    };
    let Some(paired) = pair(&lib, roots, frame) else {
        return 0;
    };
    let mut noted = 0;
    let rows = paired
        .copies
        .iter()
        .map(|c| {
            (
                c.archive.clone(),
                c.copy.clone(),
                "saved with no window open",
            )
        })
        .chain(paired.ambiguous.iter().map(|(a, _)| {
            (
                a.clone(),
                PathBuf::new(),
                "more than one copy of this frame on the archive: which is its own is not known",
            )
        }));
    for (archive, copy, reason) in rows {
        match lib.add_pending(&paired.hash, &archive, &copy, frame, reason) {
            Ok(()) => noted += 1,
            Err(e) => tracing::warn!("sync: {}: pending not noted: {e}", file_name(frame)),
        }
    }
    noted
}

/// The index reader is open now (`library::open_reader`): the saves
/// made before it was are paired and queued.
pub(crate) fn reader_opened(st: &mut State) {
    let deferred = std::mem::take(&mut st.sync.deferred);
    for (frame, source) in deferred {
        if st.write_sidecars && st.library.roots.archive_of(&frame).is_none() {
            queue_for(st, &frame, source);
        }
    }
}

/// Queue a write of `frame`'s sidecar from `source` to each of its
/// copies, and note in memory the archives where which copy is this
/// frame's is not known; the job notes the rest in the index.
fn queue_for(st: &mut State, frame: &Path, source: Source) {
    let Some(lib) = st.index_reader.as_ref() else {
        // The index is still opening (a launch preset's save comes
        // before it): kept, and queued when it is.
        st.sync.deferred.push((frame.to_path_buf(), source));
        return;
    };
    let Some(paired) = pair(lib, &st.library.roots, frame) else {
        return;
    };
    if paired.copies.is_empty() && paired.ambiguous.is_empty() {
        return;
    }
    let write = Write {
        frame: frame.to_path_buf(),
        hash: paired.hash,
        source,
        placement: st.placement,
        xmp: st.xmp_sidecars,
        copies: paired.copies,
    };
    if !paired.ambiguous.is_empty() {
        // Noted in the index by the job, which has the writing hand.
        let ambiguous = paired.ambiguous;
        let mut write = write;
        write.copies.extend(ambiguous.iter().map(|(a, _)| Copy {
            archive: a.clone(),
            copy: PathBuf::new(),
        }));
        queue(st, write);
        return;
    }
    queue(st, write);
}

/// Queue `write`, the latest for its frame, and send it unless one
/// for the frame is out.
fn queue(st: &mut State, write: Write) {
    let frame = write.frame.clone();
    st.sync.queued.insert(frame.clone(), write);
    send_next(st, &frame);
}

/// Send the queued write for `frame`, if there is one and none is out.
fn send_next(st: &mut State, frame: &Path) {
    if st.sync.in_flight.contains(frame) {
        return;
    }
    let Some(write) = st.sync.queued.remove(frame) else {
        return;
    };
    let Some(index) = st.index_path.clone() else {
        return;
    };
    let Some(app) = st.app.upgrade() else {
        return;
    };
    st.sync.in_flight.insert(frame.to_path_buf());
    let frame = frame.to_path_buf();
    let landing_frame = frame.clone();
    let sent = send(
        &app,
        "archive write",
        WRITE_WAIT,
        move |beat| write_through(&write, &index, beat),
        move |state, app, worker, heard| {
            landed(state, app, worker, &landing_frame, heard);
        },
    );
    if !sent {
        st.sync.in_flight.remove(&frame);
    }
}

/// The write to every copy of one frame, off the window's thread.
/// For each archive: the frame is noted pending first, so a quit, a
/// crash or a job that never answers leaves its row (§233); the
/// archive is looked at (§215's look, three seconds); the sidecar is
/// read from the file when it did not come from the window; the
/// copy's sidecar is checked by its stat, and read and compared only
/// when it is not what we wrote; then the XMP beside the copy is
/// regenerated and the sidecar written, through a temporary file of
/// this write's own, and the write recorded, which clears the row. A
/// failure of any kind leaves the row with its reason.
pub(crate) fn write_through(write: &Write, index: &Path, beat: &Beat) -> Vec<Outcome> {
    if let Source::Home { local, ours } = &write.source {
        return vec![bring_home(write, local, ours)];
    }
    let mut out = Vec::with_capacity(write.copies.len());
    for copy in &write.copies {
        beat();
        out.push(write_one(write, copy, index, beat));
    }
    out
}

/// A followed frame's local sidecar brought up to the copy's, which
/// the window holds, now that its root is back: written whole when it
/// is behind; left when it is the same, or ahead or apart (something
/// else edited it while the root was away: Back up or Bring back
/// joins the two). Nothing is recorded or noted: the archive is not
/// in it. Said as `Same`, since there is nothing for the landing to do.
fn bring_home(write: &Write, local: &Path, ours: &Sidecar) -> Outcome {
    let archive = write
        .copies
        .first()
        .map(|c| c.archive.clone())
        .unwrap_or_default();
    let same = Outcome::Same {
        archive,
        copy: write.frame.clone(),
    };
    let dest = copy_sidecar(local, write.placement);
    match Sidecar::load(local) {
        Ok(None) => {}
        Ok(Some(there)) => match compare(&there, ours) {
            Compared::Behind(Side::Local) => {}
            Compared::Same => return same,
            other => {
                tracing::info!(
                    "sync: {}: the sidecar here is {other:?} the copy's; left for Back up or \
                     Bring back to join",
                    local.display()
                );
                return same;
            }
        },
        Err(e) => {
            tracing::warn!("sync: {}: not brought home: {e}", local.display());
            return same;
        }
    }
    match ours.write_to(&dest) {
        Ok(()) => {
            tracing::info!(
                "sync: {}: its root is back; the sidecar brought up to the copy's",
                local.display()
            );
            Outcome::Homed {
                local: local.to_path_buf(),
            }
        }
        Err(e) => {
            tracing::warn!("sync: {}: not brought home: {e}", local.display());
            same
        }
    }
}

/// One copy: see [`write_through`].
fn write_one(write: &Write, target: &Copy, index: &Path, beat: &Beat) -> Outcome {
    let (archive, copy) = (&target.archive, &target.copy);
    let mut lib = archive::open_index(index, true, beat);
    let note = |lib: Option<&mut Library>, reason: &str| {
        if let Some(lib) = lib
            && let Err(e) = lib.note_pending(&write.hash, archive, copy, &write.frame, reason)
        {
            tracing::warn!("sync: {}: pending not noted: {e}", file_name(&write.frame));
        }
        Outcome::Waiting {
            archive: archive.to_path_buf(),
            reason: reason.to_string(),
        }
    };
    // Pending before the attempt.
    if let Some(lib) = lib.as_mut()
        && let Err(e) = lib.add_pending(&write.hash, archive, copy, &write.frame, "writing")
    {
        tracing::warn!("sync: {}: pending not noted: {e}", file_name(&write.frame));
    }
    if copy.as_os_str().is_empty() {
        return note(
            lib.as_mut(),
            "more than one copy of this frame on the archive: which is its own is not known",
        );
    }
    if crate::roots::answer_of(archive, ROOT_WAIT) != Some(true) {
        return note(lib.as_mut(), "offline");
    }
    beat();
    // The sidecar to write.
    let from_disk = |lib: &mut Option<Library>| -> Result<Sidecar, Outcome> {
        match Sidecar::load(&write.frame) {
            Ok(Some(s)) => Ok(s),
            Ok(None) => Err(match gone_or_moved(lib.as_mut(), write, archive, copy) {
                Some(outcome) => outcome,
                None => note(lib.as_mut(), "no sidecar here to write"),
            }),
            Err(e) => Err(note(lib.as_mut(), &format!("unreadable here: {e}"))),
        }
    };
    let sidecar = match &write.source {
        Source::Memory(s) | Source::Home { ours: s, .. } => (**s).clone(),
        Source::Disk => match from_disk(&mut lib) {
            Ok(s) => s,
            Err(outcome) => return outcome,
        },
        Source::Settle(theirs) => {
            // The file against the copy's sidecar, as the window does
            // for a frame it holds: taken whole, joined and saved, or
            // written as it is.
            let ours = match from_disk(&mut lib) {
                Ok(s) => s,
                Err(outcome) => return outcome,
            };
            let local = Sidecar::find(&write.frame)
                .unwrap_or_else(|| Sidecar::path_in(&write.frame, write.placement));
            match compare(&ours, theirs) {
                Compared::Same => {
                    record(
                        lib.as_mut(),
                        write,
                        target,
                        &copy_sidecar(copy, write.placement),
                        theirs,
                    );
                    return Outcome::Same {
                        archive: archive.to_path_buf(),
                        copy: copy.clone(),
                    };
                }
                Compared::Behind(Side::Local) => {
                    if let Err(e) = theirs.write_to(&local) {
                        return note(lib.as_mut(), &format!("not taken here: {e}"));
                    }
                    // The copy is what stands there already; it is ours
                    // now, by its stat.
                    record(
                        lib.as_mut(),
                        write,
                        target,
                        &copy_sidecar(copy, write.placement),
                        theirs,
                    );
                    return Outcome::Same {
                        archive: archive.to_path_buf(),
                        copy: copy.clone(),
                    };
                }
                Compared::Behind(Side::Other) => ours,
                Compared::SameEdit | Compared::Diverged => {
                    let mut joined = join(&ours, theirs);
                    if let Err(e) = joined.save_in(&write.frame, write.placement) {
                        return note(lib.as_mut(), &format!("the join not saved here: {e}"));
                    }
                    joined
                }
            }
        }
    };
    if sidecar.revisions.is_empty() {
        // Saved by a build from before revisions: the next save here
        // makes one, and the write goes then.
        return note(lib.as_mut(), "no revision here yet");
    }
    beat();
    let dest = copy_sidecar(copy, write.placement);
    let last = lib
        .as_ref()
        .and_then(|l| l.archive_write(&write.hash, archive, copy).ok().flatten());
    match known(check(&dest, last.as_ref()), last.as_ref(), &sidecar) {
        Check::Absent | Check::Ours => {}
        Check::Failed(e) => return note(lib.as_mut(), &format!("the copy: {e}")),
        Check::Changed => {
            beat();
            let theirs = match Sidecar::read(&dest) {
                Ok(s) => s,
                Err(e) => return note(lib.as_mut(), &format!("the copy: unreadable: {e}")),
            };
            match compare(&sidecar, &theirs) {
                Compared::Same => {
                    record(lib.as_mut(), write, target, &dest, &sidecar);
                    return Outcome::Same {
                        archive: archive.to_path_buf(),
                        copy: copy.clone(),
                    };
                }
                Compared::Behind(Side::Other) => {}
                _ => {
                    // Not ours and not behind: the window's to settle,
                    // against what it holds now.
                    if let Some(lib) = lib.as_mut()
                        && let Err(e) =
                            lib.note_pending(&write.hash, archive, copy, &write.frame, "to join")
                    {
                        tracing::warn!("sync: {}: pending not noted: {e}", file_name(&write.frame));
                    }
                    return Outcome::Take {
                        archive: archive.to_path_buf(),
                        copy: copy.clone(),
                        frame: write.frame.clone(),
                        theirs: Box::new(theirs),
                    };
                }
            }
        }
    }
    beat();
    if write.xmp {
        // The XMP beside the copy says what the sidecar says, never
        // merged (§233): the camera's tag and the frame's turns
        // composed, or no orientation when the copy will not say its
        // tag.
        let turn = greycard_core::decode::stance_path(copy)
            .ok()
            .map(|s| xmp::Turn::new(s.orientation, sidecar.turn));
        if let Err(e) = xmp::save(copy, &sidecar.meta, turn) {
            tracing::warn!("sync: {}: xmp not written: {e}", file_name(copy));
        }
    }
    beat();
    if let Err(e) = sidecar.write_to(&dest) {
        return note(lib.as_mut(), &format!("not written: {e}"));
    }
    record(lib.as_mut(), write, target, &dest, &sidecar);
    Outcome::Written {
        archive: archive.to_path_buf(),
        copy: copy.clone(),
    }
}

/// A frame whose file is not where its row said: moved, if the index
/// knows its hash at another local path that is there (the row follows
/// it, for the next try); otherwise away, not gone: its disk may be
/// unplugged, and the row is kept for when it is back. A frame deleted
/// on purpose keeps its row too, which the next pass over its folder
/// does not clear; the row says why it waits.
fn gone_or_moved(
    lib: Option<&mut Library>,
    write: &Write,
    archive: &Path,
    copy: &Path,
) -> Option<Outcome> {
    let lib = lib?;
    let elsewhere = lib
        .by_hash(&write.hash)
        .ok()?
        .into_iter()
        .filter(|e| !e.missing && e.path != write.frame && !e.path.starts_with(archive))
        .filter(|e| e.path.exists())
        .find(|e| {
            lib.by_path(&e.path)
                .ok()
                .flatten()
                .is_some_and(|r| r.path == e.path)
        });
    match elsewhere {
        Some(e) => {
            if let Err(err) = lib.move_pending(&write.hash, archive, copy, &write.frame, &e.path) {
                tracing::warn!("sync: pending not moved: {err}");
            }
            let _ = lib.note_pending(
                &write.hash,
                archive,
                copy,
                &e.path,
                "the frame moved; next try",
            );
            Some(Outcome::Waiting {
                archive: archive.to_path_buf(),
                reason: "the frame moved".into(),
            })
        }
        None => {
            // Its folder there and readable, the file not in it: deleted
            // (the editor's own Delete clears the row itself; this is
            // another tool's, or another machine's). The folder gone
            // too is a disk away or a share unmounted, which takes the
            // folder with it; an empty mount point still answers, which
            // is why the folder and not the root is asked.
            // A folder that lists empty is not trusted: a fixed mount
            // point with its disk unplugged lists empty too. So a
            // folder whose last frame was deleted keeps the row (open,
            // with a whole folder deleted).
            let folder_there = write
                .frame
                .parent()
                .and_then(|d| std::fs::read_dir(d).ok())
                .is_some_and(|mut entries| entries.next().is_some());
            if folder_there && std::fs::symlink_metadata(&write.frame).is_err() {
                tracing::info!(
                    "sync: {} is gone from its folder; its write to {} dropped",
                    write.frame.display(),
                    archive.display()
                );
                let _ = lib.clear_pending(&write.hash, archive, copy, &write.frame);
                return Some(Outcome::Gone {
                    archive: archive.to_path_buf(),
                    frame: write.frame.clone(),
                });
            }
            tracing::info!(
                "sync: {} is not here and nowhere else the library knows; its write to {} waits",
                write.frame.display(),
                archive.display()
            );
            let reason = "the frame is not here; its disk may be away";
            let _ = lib.note_pending(&write.hash, archive, copy, &write.frame, reason);
            Some(Outcome::Waiting {
                archive: archive.to_path_buf(),
                reason: reason.into(),
            })
        }
    }
}

/// Note in the index what now stands at `dest`: the revision written
/// and the file's size and mtime, and that nothing waits.
fn record(lib: Option<&mut Library>, write: &Write, target: &Copy, dest: &Path, sidecar: &Sidecar) {
    let Some(lib) = lib else {
        return;
    };
    let Some(revision) = sidecar.revisions.last() else {
        return;
    };
    let Ok(meta) = std::fs::metadata(dest) else {
        return;
    };
    if let Err(e) = lib.record_archive_write(
        &write.hash,
        &target.archive,
        &target.copy,
        &ArchiveWrite {
            path: dest.to_path_buf(),
            revision: revision.hash.clone(),
            size: meta.len(),
            mtime: greycard_library::mtime_of(&meta),
            written: 0,
        },
    ) {
        tracing::warn!("sync: {}: write not recorded: {e}", file_name(&write.frame));
    }
    if let Err(e) = lib.clear_pending(&write.hash, &target.archive, &target.copy, &write.frame) {
        tracing::warn!(
            "sync: {}: pending not cleared: {e}",
            file_name(&write.frame)
        );
    }
    // This frame's row made while which copy was its own was not known
    // is answered by this write; another frame's of the same hash is
    // not.
    if let Err(e) = lib.clear_pending(&write.hash, &target.archive, Path::new(""), &write.frame) {
        tracing::warn!(
            "sync: {}: pending not cleared: {e}",
            file_name(&write.frame)
        );
    }
}

/// Whether `archive` answered a look made for something else (a
/// view's read, a pass over it, the poll): when it did and its waiting
/// writes have not been taken up since it last did not, they are; when
/// it did not, they are due again at the next answer. A landing that
/// leaves writes waiting (a copy to join, one that would not read)
/// makes them due again too, so the next answering look tries them.
pub(crate) fn heard_from(st: &mut State, app: &App, archive: &Path, answered: bool) {
    if !st.write_sidecars || !st.library.roots.is_archive(archive) {
        return;
    }
    if !answered {
        st.sync.caught_up.remove(archive);
        return;
    }
    if st.sync.caught_up.contains(archive) {
        return;
    }
    st.sync.caught_up.insert(archive.to_path_buf());
    catch_up(st, app, archive);
}

/// The archives the roots' look found, one way or the other: those in
/// `offline` did not answer, the rest did.
pub(crate) fn heard_over(st: &mut State, app: &App, offline: &HashSet<PathBuf>) {
    let archives = st.library.roots.archives().to_vec();
    for a in archives {
        heard_from(st, app, &a, !offline.contains(&a));
    }
    bring_home_followed(st, offline);
}

/// A followed frame whose local root answers again: its sidecar there
/// is behind the copy's by every save made since, so a new window would
/// show the old look until the first save joined them. The copy's (the
/// window's) is brought home through the frame's queue, once a return
/// (`Followed::homed`); the frame goes on following.
fn bring_home_followed(st: &mut State, offline: &HashSet<PathBuf>) {
    if !st.write_sidecars {
        return;
    }
    let due: Vec<(PathBuf, Followed)> = st
        .sync
        .followed
        .iter()
        .filter(|(_, f)| !f.homed && !offline.iter().any(|o| f.local.starts_with(o)))
        .map(|(copy, f)| (copy.clone(), f.clone()))
        .collect();
    for (copy, followed) in due {
        let Some(c) = st.files.iter().position(|f| *f == copy) else {
            continue;
        };
        if !crate::panel::edit::writable(st, c) || st.sidecars[c].revisions.is_empty() {
            continue;
        }
        let Some(hash) = st
            .index_reader
            .as_ref()
            .and_then(|l| l.by_path(&copy).ok().flatten())
            .map(|r| r.hash)
        else {
            continue;
        };
        if let Some(f) = st.sync.followed.get_mut(&copy) {
            f.homed = true;
        }
        let write = Write {
            frame: copy.clone(),
            hash,
            source: Source::Home {
                local: followed.local.clone(),
                ours: Box::new(st.sidecars[c].clone()),
            },
            placement: st.placement,
            xmp: false,
            copies: vec![Copy {
                archive: followed.archive.clone(),
                copy: copy.clone(),
            }],
        };
        // Behind a save's write when one is queued: the latest wins the
        // queue, so a home never replaces a save. It goes when nothing
        // is queued or out.
        if !st.sync.queued.contains_key(&copy) && !st.sync.in_flight.contains(&copy) {
            queue(st, write);
        } else if let Some(f) = st.sync.followed.get_mut(&copy) {
            f.homed = false;
        }
    }
}

/// Take up the writes waiting for `archive` now: each goes through
/// the frame's own queue, from the file, so a catch-up and a save's
/// write never run at once for one frame.
pub(crate) fn catch_up(st: &mut State, app: &App, archive: &Path) {
    if !st.write_sidecars {
        return;
    }
    let pending = st
        .index_reader
        .as_ref()
        .and_then(|l| l.pending_for(archive).ok())
        .unwrap_or_default();
    if pending.is_empty() {
        return;
    }
    let (placement, xmp) = (st.placement, st.xmp_sidecars);
    let mut by_frame: HashMap<PathBuf, Write> = HashMap::new();
    for p in pending {
        let entry = by_frame.entry(p.frame.clone()).or_insert_with(|| Write {
            frame: p.frame.clone(),
            hash: p.hash.clone(),
            source: Source::Disk,
            placement,
            xmp,
            copies: Vec::new(),
        });
        let copy = if p.copy.as_os_str().is_empty() {
            // Which copy is this frame's was not known: paired again
            // from the index, in case the roots say now.
            match st
                .index_reader
                .as_ref()
                .and_then(|l| pair(l, &st.library.roots, &p.frame))
            {
                Some(paired) => match paired.copies.into_iter().find(|c| c.archive == p.archive) {
                    Some(c) => c,
                    None => Copy {
                        archive: p.archive.clone(),
                        copy: PathBuf::new(),
                    },
                },
                None => Copy {
                    archive: p.archive.clone(),
                    copy: PathBuf::new(),
                },
            }
        } else {
            Copy {
                archive: p.archive.clone(),
                copy: p.copy.clone(),
            }
        };
        if !entry.copies.contains(&copy) {
            entry.copies.push(copy);
        }
    }
    for (frame, write) in by_frame {
        // A frame the window holds with a sidecar of its own saves from
        // memory; one it has let go of, or stands in for, from disk. A
        // write already queued for the frame is the latest and stands.
        if st.sync.queued.contains_key(&frame) || st.sync.in_flight.contains(&frame) {
            continue;
        }
        let held = st
            .files
            .iter()
            .position(|f| *f == frame)
            .filter(|c| st.from_row.get(*c).is_none_or(|r| r.read));
        let write = match held {
            Some(c) if !st.sidecars[c].revisions.is_empty() => Write {
                source: Source::Memory(Box::new(st.sidecars[c].clone())),
                ..write
            },
            _ => write,
        };
        queue(st, write);
    }
    let _ = app;
}

/// The status line's click: every archive with writes waiting is
/// taken up now.
pub(crate) fn retry(st: &mut State, app: &App) {
    let counts = st
        .index_reader
        .as_ref()
        .and_then(|l| l.pending_counts().ok())
        .unwrap_or_default();
    for count in counts {
        if count.unknown > 0
            && let Some(lib) = st.index_reader.as_ref()
            && let Ok(rows) = lib.pending_for(&count.archive)
        {
            // Which frames cannot tell their copy, for the log: the
            // answer is a backup pairing for their folder.
            for row in rows.iter().filter(|r| r.copy.as_os_str().is_empty()) {
                tracing::info!(
                    "sync: {}: more than one copy on {}; back up its folder so the pairing \
                     says which is its own",
                    row.frame.display(),
                    count.archive.display()
                );
            }
        }
        st.sync.caught_up.remove(&count.archive);
        catch_up(st, app, &count.archive);
    }
}
/// Where a write's end lands on the window's thread.
fn landed(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    frame: &Path,
    heard: Heard<Vec<Outcome>>,
) {
    let outcomes = match heard {
        Heard::Aside => {
            // Quiet for the wait: it goes on alone and stays out, so
            // nothing for the frame runs beside it; the frame is
            // pending in the index already, and the catch-up takes it
            // when the archive answers. What it says later is taken in.
            refresh_note(&state.borrow(), app);
            return;
        }
        Heard::Lost => {
            state.borrow_mut().sync.in_flight.remove(frame);
            refresh_note(&state.borrow(), app);
            return;
        }
        Heard::Done { result, .. } => {
            state.borrow_mut().sync.in_flight.remove(frame);
            result
        }
    };
    let mut waiting = Vec::new();
    for outcome in outcomes {
        match outcome {
            Outcome::Written { archive, copy } => {
                tracing::debug!("sync: {} written to {}", copy.display(), archive.display());
            }
            Outcome::Same { archive, copy } => {
                tracing::debug!("sync: {} on {} as it is", copy.display(), archive.display());
            }
            Outcome::Waiting { archive, reason } => {
                tracing::info!("sync: waiting for {}: {reason}", archive.display());
                waiting.push(archive);
            }
            Outcome::Gone { archive, frame } => {
                tracing::info!(
                    "sync: {}: deleted; nothing owed to {} now",
                    frame.display(),
                    archive.display()
                );
            }
            Outcome::Homed { local } => {
                // As a save tells it: the row reads the sidecar again.
                if let Some(indexer) = &state.borrow().index {
                    indexer.file(local);
                }
            }
            Outcome::Take {
                archive,
                copy,
                frame,
                theirs,
            } => {
                tracing::info!(
                    "sync: {}: the copy on {} is not behind; settling",
                    file_name(&frame),
                    archive.display()
                );
                settle(state, app, worker, &frame, &archive, &copy, *theirs);
            }
        }
    }
    {
        let mut st = state.borrow_mut();
        // Writes left waiting while the archive answers are due again
        // at its next answer, and at the frame's next save.
        for a in waiting {
            st.sync.caught_up.remove(&a);
        }
        send_next(&mut st, frame);
    }
    refresh_note(&state.borrow(), app);
    // A frame whose root went while its writes were out follows now
    // that they are settled.
    follow_offline(&mut state.borrow_mut(), app);
}

/// The archive's copy of `frame`'s sidecar came back not behind what
/// the job carried. Compared again with what the window holds *now*:
/// when that is behind the copy's, the copy's is taken whole, its
/// bytes written over the local sidecar and no new revision made; when
/// the two went their own ways, or hold the same edit with meta to
/// join, they are joined and the join saved; when ours is ahead after
/// all, the write simply goes again. On the frame on screen the
/// sidecar is the panel's, shown and developed again as an undo is
/// (culling, when on, goes on); on a frame the window holds, saved in
/// place; on a frame the window does not hold, or holds only as a row
/// standing in, the file's sidecar is settled on a job's thread, since
/// the local root may be a share too. Each save queues the archive
/// write again, which then finds the copy behind and goes.
fn settle(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    frame: &Path,
    archive: &Path,
    copy: &Path,
    theirs: Sidecar,
) {
    if !settle_held(state, app, worker, frame, archive, copy, theirs) {
        return;
    }
    // Settled, the frame on screen may follow its copy once the write
    // above has landed; with nothing queued (nothing to write), it is
    // asked now.
    follow_offline(&mut state.borrow_mut(), app);
}

/// [`settle`]'s work on the window's sidecar; false when the frame is
/// not the window's to settle. The window's borrow ends in every arm.
fn settle_held(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    frame: &Path,
    archive: &Path,
    copy: &Path,
    theirs: Sidecar,
) -> bool {
    let mut st = state.borrow_mut();
    if !st.write_sidecars {
        return false;
    }
    let held = st
        .files
        .iter()
        .position(|f| f == frame)
        .filter(|c| crate::panel::edit::writable(&st, *c));
    let Some(c) = held else {
        // A frame the window does not hold, or holds as its row
        // standing in, or may not write (held for a delete): the
        // file's, settled on a job.
        if st.files.iter().any(|f| f == frame) {
            // Held but not to be written now: the row waits, the
            // frame's next save or the next catch-up takes it.
            return false;
        }
        settle_on_disk(&mut st, frame, archive, copy, theirs);
        return false;
    };
    match compare(&st.sidecars[c], &theirs) {
        Compared::Same | Compared::Behind(Side::Other) => {
            // Ours is the copy's, or went on past what the job saw: it
            // goes (again), which records it and clears the row.
            after_save(&mut st, c);
        }
        Compared::Behind(Side::Local) => {
            // Taken whole: the copy's bytes over the local sidecar, no
            // new revision, and nothing to write back.
            if !followed(&st, c) {
                let (path, placement) = (st.files[c].clone(), st.placement);
                let dest =
                    Sidecar::find(&path).unwrap_or_else(|| Sidecar::path_in(&path, placement));
                if let Err(e) = theirs.write_to(&dest) {
                    // The local root may be gone (the frame is about to
                    // follow its copy): the window takes the copy's
                    // anyway, and the file here stays behind it, as
                    // any copy away from a save does.
                    tracing::warn!("{}: sidecar not taken here: {e}", file_name(frame));
                }
            }
            st.sidecars[c] = theirs;
            if let Some(indexer) = &st.index {
                indexer.file(st.files[c].clone());
            }
            // The copy and the window are the same now, which the
            // index does not know yet: a write goes, finds the copy the
            // same, records it and clears the frame's row. Without it
            // the "to join" row would stand for good.
            after_save(&mut st, c);
            if st.current == Some(c) {
                drop(st);
                show_settled(state, app, worker);
                return true;
            }
        }
        Compared::SameEdit | Compared::Diverged => {
            let joined = join(&st.sidecars[c], &theirs);
            st.sidecars[c] = joined;
            if st.current == Some(c) {
                drop(st);
                crate::panel::history::take_current_settled(&mut state.borrow_mut(), app, worker);
                st = state.borrow_mut();
            } else {
                crate::panel::edit::save_sidecar(&mut st, c);
            }
            // The save failed (the disk gone under the frame, about to
            // follow its copy): the join still goes to the copy from
            // memory, so the copy has both edits and the row clears.
            if !st.sync.queued.contains_key(frame) && !st.sync.in_flight.contains(frame) {
                after_save(&mut st, c);
            }
        }
    }
    true
}

/// The frame on screen holds a sidecar that changed under it without
/// a save of its own: the panel shows it, developed again.
fn show_settled(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) {
    let mut st = state.borrow_mut();
    if st.cull.is_some() {
        return;
    }
    crate::panel::history::take_current_settled(&mut st, app, worker);
}

/// A frame the window does not hold, whose archive copy is not behind
/// its file: read, compared and settled on a job, then written to the
/// copy. The frame goes through its own queue, so nothing else for it
/// runs beside this.
fn settle_on_disk(st: &mut State, frame: &Path, archive: &Path, copy: &Path, theirs: Sidecar) {
    let Some(lib) = st.index_reader.as_ref() else {
        return;
    };
    let Some(row) = lib.by_path(frame).ok().flatten() else {
        return;
    };
    let write = Write {
        frame: frame.to_path_buf(),
        hash: row.hash,
        source: Source::Settle(Box::new(theirs)),
        placement: Placement::of(frame),
        xmp: st.xmp_sidecars,
        copies: vec![Copy {
            archive: archive.to_path_buf(),
            copy: copy.to_path_buf(),
        }],
    };
    queue(st, write);
}

/// The status line's word for what waits: "3 edits waiting for
/// Archive", one clause an archive, nothing while nothing waits. A
/// click on it is [`retry`].
pub(crate) fn refresh_note(st: &State, app: &App) {
    let counts = st
        .index_reader
        .as_ref()
        .and_then(|l| l.pending_counts().ok())
        .unwrap_or_default();
    let note = counts
        .iter()
        .map(|count| {
            let name = crate::roots::root_name(&st.library.roots, &count.archive);
            let n = count.total;
            let mut clause = format!(
                "{n} edit{} waiting for {name}",
                if n == 1 { "" } else { "s" }
            );
            if count.unknown > 0 {
                // Waiting on the user, not the archive: the log names
                // the frames when the note is clicked.
                let u = count.unknown;
                clause.push_str(&format!(
                    "; {u} can't tell which copy {} own",
                    if u == 1 { "is its" } else { "are their" }
                ));
            }
            clause
        })
        .collect::<Vec<_>>()
        .join("; ");
    app.set_sync_note(note.into());
}

/// The frame on screen, when its root is offline and the archive's
/// copy of it was not behind: its path becomes the archive copy's, so
/// the next save goes there, and the status line says so once (§233).
/// Before it follows, the copy is checked on a job against what we
/// last wrote there, as a write would check it: a copy another machine
/// saved since is settled first (taken or joined, saved, written), and
/// the frame follows when that has landed and nothing waits for it.
/// With a write waiting, queued or out, the frame stays on its local
/// path and its saves queue, as the copy is behind it. Once followed,
/// a frame does not follow back when its root returns: its saves go to
/// the copy until it is opened again.
pub(crate) fn follow_offline(st: &mut State, app: &App) {
    if !st.write_sidecars {
        return;
    }
    let Some(c) = st.current else {
        return;
    };
    let Some(frame) = st.files.get(c).cloned() else {
        return;
    };
    if !crate::rows::is_offline(st, c)
        || st.sync.followed.contains_key(&frame)
        || st.sync.checking.contains(&frame)
        || st.sync.queued.contains_key(&frame)
        || st.sync.in_flight.contains(&frame)
        || !crate::panel::edit::writable(st, c)
        || st.library.roots.archive_of(&frame).is_some()
    {
        return;
    }
    let Some(lib) = st.index_reader.as_ref() else {
        return;
    };
    let Some(paired) = pair(lib, &st.library.roots, &frame) else {
        return;
    };
    if lib.is_pending(&paired.hash).unwrap_or(true) {
        return;
    }
    let Some(target) = paired
        .copies
        .into_iter()
        .find(|c| !st.library.offline.contains(&c.archive))
    else {
        return;
    };
    if st.files.contains(&target.copy) {
        return;
    }
    let Some(index) = st.index_path.clone() else {
        return;
    };
    st.sync.checking.insert(frame.clone());
    let hash = paired.hash;
    let sidecar = st.sidecars[c].clone();
    let placement = st.placement;
    let job_frame = frame.clone();
    let job_target = target.clone();
    let land_target = target;
    let land_frame = frame.clone();
    let sent = send(
        app,
        "archive follow",
        WRITE_WAIT,
        move |beat| {
            follow_check(
                &job_frame,
                &hash,
                &sidecar,
                placement,
                &job_target,
                &index,
                beat,
            )
        },
        move |state, app, worker, heard| {
            follow_landed(state, app, worker, &land_frame, &land_target, heard);
        },
    );
    if !sent {
        st.sync.checking.remove(&frame);
    }
}

/// What a follow check found.
#[derive(Debug)]
pub(crate) enum Followable {
    /// The copy is ours, or has no sidecar: the frame may follow.
    Ready,
    /// The copy is another machine's and not behind ours: settled
    /// first.
    Take(Box<Sidecar>),
    /// The copy is behind ours: written, and then the frame may follow.
    Written,
    /// The archive did not answer, or the copy would not read.
    Not(String),
}

/// The follow's check, off the window's thread: the copy's sidecar
/// against what we last wrote there, read and compared when it is not
/// ours, and written when it is behind.
fn follow_check(
    frame: &Path,
    hash: &str,
    sidecar: &Sidecar,
    placement: Placement,
    target: &Copy,
    index: &Path,
    beat: &Beat,
) -> Followable {
    beat();
    if crate::roots::answer_of(&target.archive, ROOT_WAIT) != Some(true) {
        return Followable::Not("offline".into());
    }
    let lib = archive::open_index(index, true, beat);
    let dest = copy_sidecar(&target.copy, placement);
    let last = lib.as_ref().and_then(|l| {
        l.archive_write(hash, &target.archive, &target.copy)
            .ok()
            .flatten()
    });
    drop(lib);
    match known(check(&dest, last.as_ref()), last.as_ref(), sidecar) {
        Check::Absent | Check::Ours => Followable::Ready,
        Check::Failed(e) => Followable::Not(e),
        Check::Changed => {
            let theirs = match Sidecar::read(&dest) {
                Ok(s) => s,
                Err(e) => return Followable::Not(format!("the copy: unreadable: {e}")),
            };
            match compare(sidecar, &theirs) {
                Compared::Same => Followable::Ready,
                Compared::Behind(Side::Other) => {
                    let write = Write {
                        frame: frame.to_path_buf(),
                        hash: hash.to_string(),
                        source: Source::Memory(Box::new(sidecar.clone())),
                        placement,
                        xmp: false,
                        copies: vec![target.clone()],
                    };
                    match write_one(&write, target, index, beat) {
                        Outcome::Written { .. } | Outcome::Same { .. } => Followable::Written,
                        Outcome::Take { theirs, .. } => Followable::Take(theirs),
                        Outcome::Waiting { reason, .. } => Followable::Not(reason),
                        Outcome::Gone { .. } | Outcome::Homed { .. } => {
                            Followable::Not("gone".into())
                        }
                    }
                }
                _ => Followable::Take(Box::new(theirs)),
            }
        }
    }
}

/// Where the follow check lands: the frame follows when the copy is
/// ready, is settled when it is another's, stays otherwise.
fn follow_landed(
    state: &Rc<RefCell<State>>,
    app: &App,
    worker: &Rc<Worker>,
    frame: &Path,
    target: &Copy,
    heard: Heard<Followable>,
) {
    let found = match heard {
        Heard::Aside => return,
        Heard::Lost => {
            state.borrow_mut().sync.checking.remove(frame);
            return;
        }
        Heard::Done { result, .. } => {
            state.borrow_mut().sync.checking.remove(frame);
            result
        }
    };
    match found {
        Followable::Ready | Followable::Written => {
            follow(&mut state.borrow_mut(), app, frame, target)
        }
        Followable::Take(theirs) => {
            settle(
                state,
                app,
                worker,
                frame,
                &target.archive,
                &target.copy,
                *theirs,
            );
        }
        Followable::Not(reason) => {
            tracing::info!("sync: {} not followed: {reason}", file_name(frame));
        }
    }
}

/// The frame on screen becomes the archive's copy.
fn follow(st: &mut State, app: &App, frame: &Path, target: &Copy) {
    let Some(c) = st.current else {
        return;
    };
    if st.files.get(c).map(Path::new) != Some(frame)
        || !crate::rows::is_offline(st, c)
        || st.files.contains(&target.copy)
        || st.sync.queued.contains_key(frame)
        || st.sync.in_flight.contains(frame)
    {
        return;
    }
    let local_root = crate::rows::offline_root(st, c);
    if st.panel_stand_in.as_ref() == Some(&frame.to_path_buf()) {
        st.panel_stand_in = Some(target.copy.clone());
    }
    if let Some(id) = st
        .index_reader
        .as_ref()
        .and_then(|l| l.by_path(&target.copy).ok().flatten())
        .map(|e| e.id)
        && let Some(slot) = st.index_ids.get_mut(c)
    {
        *slot = Some(id);
    }
    tracing::info!(
        "{}: its root is offline; following to {}",
        frame.display(),
        target.copy.display()
    );
    st.files[c] = target.copy.clone();
    st.sync.followed.insert(
        target.copy.clone(),
        Followed {
            local: frame.to_path_buf(),
            archive: target.archive.clone(),
            homed: false,
        },
    );
    let where_ = crate::roots::root_name(&st.library.roots, &target.archive);
    let which = local_root
        .map(|r| crate::roots::root_name(&st.library.roots, &r))
        .unwrap_or_else(|| "its root".to_string());
    app.set_status(
        format!(
            "{which} is offline: {} is open from its copy on {where_}",
            file_name(frame)
        )
        .into(),
    );
}

/// The window's callbacks.
pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    let st = Rc::clone(state);
    let app_weak = app.as_weak();
    app.on_sync_retry(move || {
        if let Some(app) = app_weak.upgrade() {
            retry(&mut st.borrow_mut(), &app);
        }
    });
}

/// Where a job's end lands on the window's thread.
type Landing<R> =
    Arc<dyn Fn(&Rc<RefCell<State>>, &App, &Rc<Worker>, Heard<R>) + Send + std::marker::Sync>;

/// Run `job` off the window's thread with its beats watched for
/// `wait`, and land what it says on the window's thread. False when no
/// thread could be had.
#[cfg(not(test))]
fn send<R: Send + 'static>(
    app: &App,
    name: &str,
    wait: Duration,
    job: impl FnOnce(&Beat) -> R + Send + 'static,
    land: impl Fn(&Rc<RefCell<State>>, &App, &Rc<Worker>, Heard<R>) + Send + std::marker::Sync + 'static,
) -> bool {
    let land: Landing<R> = Arc::new(land);
    let app_weak = app.as_weak();
    let watched = archive::supervise(name, wait, job, move |heard| {
        let (app_weak, land) = (app_weak.clone(), Arc::clone(&land));
        let _ = slint::invoke_from_event_loop(move || {
            let (Some(app), Some(state), Some(worker)) = (
                app_weak.upgrade(),
                crate::STATE.with(|s| s.borrow().clone()),
                crate::WORKER.with(|w| w.borrow().clone()),
            ) else {
                return;
            };
            land(&state, &app, &worker, heard);
        });
    });
    match watched {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("sync: {e}");
            false
        }
    }
}

/// In a test the job is queued, and the test runs and lands it in
/// place (`tests::land_sent`): there is no event loop to land it on. A
/// test that asks for it (`tests::ASIDE_NEXT`) has the next job heard
/// as set aside first, and its end land later, late.
#[cfg(test)]
fn send<R: Send + 'static>(
    _app: &App,
    _name: &str,
    _wait: Duration,
    job: impl FnOnce(&Beat) -> R + Send + 'static,
    land: impl Fn(&Rc<RefCell<State>>, &App, &Rc<Worker>, Heard<R>) + Send + std::marker::Sync + 'static,
) -> bool {
    let land: Landing<R> = Arc::new(land);
    let aside = tests::ASIDE_NEXT.with(|a| a.replace(false));
    tests::SENT.with(|s| {
        s.borrow_mut().push(Box::new(move |state, app, worker| {
            if aside {
                land(state, app, worker, Heard::Aside);
                tests::SENT.with(|s| {
                    s.borrow_mut().push(Box::new(move |state, app, worker| {
                        let result = job(&archive::no_beat());
                        land(state, app, worker, Heard::Done { result, late: true });
                    }));
                });
            } else {
                let result = job(&archive::no_beat());
                land(
                    state,
                    app,
                    worker,
                    Heard::Done {
                        result,
                        late: false,
                    },
                );
            }
        }));
    });
    true
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::{state_for, window};
    use greycard_library::fixture::{A7, R5, write_frame};

    type Sent = Box<dyn FnOnce(&Rc<RefCell<State>>, &App, &Rc<Worker>)>;

    thread_local! {
        pub(crate) static SENT: RefCell<Vec<Sent>> = RefCell::new(Vec::new());
        /// The next job sent is heard as set aside first, then late.
        pub(crate) static ASIDE_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Run the jobs sent off and land them, as the threads and the
    /// event loop would, until none are left; how many there were.
    pub(crate) fn land_sent(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
        let mut n = 0;
        loop {
            let jobs: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
            if jobs.is_empty() {
                return n;
            }
            for job in jobs {
                job(state, app, worker);
                n += 1;
            }
        }
    }

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-sync-ui-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    /// A local root and an archive root, each with the shoot's two
    /// frames, the archive's a byte-for-byte copy, and the index over
    /// both at `db`.
    fn two_places(dir: &Path) -> (PathBuf, PathBuf, Vec<PathBuf>, PathBuf) {
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot = local.join("shoot");
        std::fs::create_dir_all(&shoot).unwrap();
        let a = shoot.join("a.tif");
        let b = shoot.join("b.tif");
        write_frame(&a, &R5, 1);
        write_frame(&b, &A7, 2);
        let there = nas.join("shoot");
        std::fs::create_dir_all(&there).unwrap();
        std::fs::copy(&a, there.join("a.tif")).unwrap();
        std::fs::copy(&b, there.join("b.tif")).unwrap();
        let db = dir.join("library.sqlite");
        let mut lib = Library::open(&db).unwrap();
        lib.index_tree(&local, &mut |_| {}).unwrap();
        lib.index_tree(&nas, &mut |_| {}).unwrap();
        (local, nas, vec![a, b], db)
    }

    /// A window over `files` with the two roots, `nas` an archive, the
    /// index opened as the window opens it (which says what waits),
    /// sidecars written.
    fn opened(
        app: &App,
        dir: &Path,
        files: Vec<PathBuf>,
        local: &Path,
        nas: &Path,
        db: &Path,
    ) -> (Rc<RefCell<State>>, Rc<Worker>) {
        let (state, worker) = state_for(app, files);
        {
            let mut st = state.borrow_mut();
            st.current = Some(0);
            st.write_sidecars = true;
            st.index_path = Some(db.to_path_buf());
            let file = dir.join("roots.json");
            let mut roots = greycard_library::Roots::default();
            roots.add(local).unwrap();
            roots.add(nas).unwrap();
            roots.set_archive(nas, true);
            roots.set_name(nas, "Archive");
            roots.save(&file).unwrap();
            st.library.roots = roots;
            st.library.file = Some(file);
            for f in st.files.clone() {
                if let Ok(Some(s)) = Sidecar::load(&f) {
                    let i = st.files.iter().position(|x| *x == f).unwrap();
                    st.sidecars[i] = s;
                }
            }
            crate::library::open_reader(&mut st, app);
        }
        (state, worker)
    }

    fn exposed(stops: f32) -> greycard_edit::Edit {
        let mut e = greycard_edit::Edit::default();
        e.light.exposure = stops;
        e
    }

    fn exposures_of(s: &Sidecar) -> Vec<f32> {
        s.history
            .iter()
            .map(|h| h.edit.light.exposure)
            .chain(std::iter::once(s.current.light.exposure))
            .collect()
    }

    fn no_beat() -> Beat {
        archive::no_beat()
    }

    fn pending_for(db: &Path, archive: &Path) -> Vec<greycard_library::Pending> {
        Library::open_read_only(db)
            .unwrap()
            .pending_for(archive)
            .unwrap()
    }

    fn save(state: &Rc<RefCell<State>>, stops: f32) {
        let mut st = state.borrow_mut();
        crate::panel::edit::save_edit(&mut st, exposed(stops));
    }

    /// The laptop saved the copy: on from what is there, at a time of
    /// its own.
    fn laptop_saves(copy: &Path, stops: f32) -> Sidecar {
        let mut theirs = Sidecar::load(copy).unwrap().unwrap_or_default();
        theirs.record(exposed(stops));
        theirs
            .save_in_stamped(
                copy,
                Placement::Beside,
                &greycard_edit::sync::Stamp {
                    at: 1 << 40,
                    host: "laptop".into(),
                },
            )
            .unwrap();
        theirs
    }

    #[test]
    fn the_check_tells_our_write_from_another_machines_and_a_failed_stat() {
        let dir = scratch("check");
        let dest = dir.join("a.tif.gcd");
        assert_eq!(check(&dest, None), Check::Absent);
        std::fs::write(&dest, b"{}").unwrap();
        let meta = std::fs::metadata(&dest).unwrap();
        let ours = ArchiveWrite {
            path: dest.clone(),
            revision: "r".into(),
            size: meta.len(),
            mtime: greycard_library::mtime_of(&meta),
            written: 1,
        };
        assert_eq!(check(&dest, Some(&ours)), Check::Ours);
        assert_eq!(check(&dest, None), Check::Changed, "a file we never wrote");
        let other = ArchiveWrite {
            size: meta.len() + 1,
            ..ours.clone()
        };
        assert_eq!(check(&dest, Some(&other)), Check::Changed);
        let later = ArchiveWrite {
            mtime: ours.mtime + 1,
            ..ours
        };
        assert_eq!(check(&dest, Some(&later)), Check::Changed);
        // A stat that fails other than by the file being away is a
        // failure, not an absence: a file where a folder should be.
        let under_file = dest.join("x.gcd");
        assert!(matches!(check(&under_file, None), Check::Failed(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Which copy on an archive is this frame's: the one under the
    /// folder its own folder was backed up to; with no pairing, the
    /// only one; with two and no pairing, not known.
    #[test]
    fn a_frame_is_paired_with_exactly_one_copy_or_none() {
        let dir = scratch("pair");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot = local.join("shoot");
        let selects = local.join("selects");
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&selects).unwrap();
        let a = shoot.join("a.tif");
        write_frame(&a, &R5, 1);
        let a2 = selects.join("a.tif");
        std::fs::copy(&a, &a2).unwrap();
        for sub in ["shoot", "selects"] {
            std::fs::create_dir_all(nas.join(sub)).unwrap();
            std::fs::copy(&a, nas.join(sub).join("a.tif")).unwrap();
        }
        let db = dir.join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_tree(&local, &mut |_| {}).unwrap();
            lib.index_tree(&nas, &mut |_| {}).unwrap();
        }
        let lib = Library::open_read_only(&db).unwrap();
        let mut roots = greycard_library::Roots::default();
        roots.add(&local).unwrap();
        roots.add(&nas).unwrap();
        roots.set_archive(&nas, true);
        // No pairing: two candidates, none known to be this frame's.
        let p = pair(&lib, &roots, &a).unwrap();
        assert!(p.copies.is_empty());
        assert_eq!(p.ambiguous, vec![(nas.clone(), 2)]);
        // The pairings say: each frame to its own.
        roots.set_backup_folder(&shoot, &nas, &nas.join("shoot"));
        roots.set_backup_folder(&selects, &nas, &nas.join("selects"));
        let p = pair(&lib, &roots, &a).unwrap();
        assert_eq!(
            p.copies,
            vec![Copy {
                archive: nas.clone(),
                copy: nas.join("shoot").join("a.tif")
            }]
        );
        let p2 = pair(&lib, &roots, &a2).unwrap();
        assert_eq!(p2.copies[0].copy, nas.join("selects").join("a.tif"));
        assert!(p2.ambiguous.is_empty());
        // One candidate needs no pairing.
        std::fs::remove_file(nas.join("selects").join("a.tif")).unwrap();
        let mut w = Library::open(&db).unwrap();
        w.forget(&[nas.join("selects").join("a.tif")]).unwrap();
        drop(w);
        let lib = Library::open_read_only(&db).unwrap();
        let bare = {
            let mut r = greycard_library::Roots::default();
            r.add(&local).unwrap();
            r.add(&nas).unwrap();
            r.set_archive(&nas, true);
            r
        };
        // Not while the shoot's frame, the same hash, is here too: the
        // sole hit may be its copy.
        let p = pair(&lib, &bare, &a2).unwrap();
        assert!(p.copies.is_empty(), "{p:?}");
        assert_eq!(p.ambiguous, vec![(nas.clone(), 1)]);
        // Nor when a pairing claims the hit for the shoot's folder.
        let p = pair(&lib, &roots, &a2).unwrap();
        assert!(p.copies.is_empty(), "{p:?}");
        // The shoot's frame gone, the selects frame is the hash's one
        // local frame, and the sole hit is its own.
        let mut w = Library::open(&db).unwrap();
        w.forget(std::slice::from_ref(&a)).unwrap();
        drop(w);
        let lib = Library::open_read_only(&db).unwrap();
        let p = pair(&lib, &bare, &a2).unwrap();
        assert_eq!(p.copies.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The write itself, off any window: a copy with no sidecar gets
    /// ours and the write is recorded; the same write again is seen as
    /// ours by its stat and goes; a copy another machine saved since
    /// is read and compared, and comes back to settle when it is not
    /// behind; an archive that does not answer leaves the frame waiting
    /// in the index with its reason; the row is there before any
    /// attempt.
    #[test]
    fn a_write_goes_to_the_copy_checks_what_stands_there_and_waits_when_it_cannot() {
        let dir = scratch("write-through");
        let (local, nas, files, db) = two_places(&dir);
        let frame = files[0].clone();
        let copy = nas.join("shoot").join("a.tif");
        let mut sidecar = Sidecar::default();
        sidecar.record(exposed(0.5));
        sidecar.save(&frame).unwrap();
        let hash = Library::open_read_only(&db)
            .unwrap()
            .by_path(&frame)
            .unwrap()
            .unwrap()
            .hash;
        let write = |s: &Sidecar| Write {
            frame: frame.clone(),
            hash: hash.clone(),
            source: Source::Memory(Box::new(s.clone())),
            placement: Placement::Beside,
            xmp: false,
            copies: vec![Copy {
                archive: nas.clone(),
                copy: copy.clone(),
            }],
        };
        let out = write_through(&write(&sidecar), &db, &no_beat());
        assert!(matches!(out[..], [Outcome::Written { .. }]), "{out:?}");
        let there = Sidecar::load(&copy).unwrap().unwrap();
        assert_eq!(compare(&sidecar, &there), Compared::Same);
        let lib = Library::open_read_only(&db).unwrap();
        let w = lib.archive_write(&hash, &nas, &copy).unwrap().unwrap();
        assert_eq!(w.revision, sidecar.revisions.last().unwrap().hash);
        assert!(lib.pending_for(&nas).unwrap().is_empty());
        drop(lib);
        sidecar.record(exposed(1.0));
        sidecar.save(&frame).unwrap();
        let out = write_through(&write(&sidecar), &db, &no_beat());
        assert!(matches!(out[..], [Outcome::Written { .. }]), "{out:?}");
        assert_eq!(
            Sidecar::load(&copy)
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            1.0
        );
        // Another machine saved the copy since: handed back to settle.
        laptop_saves(&copy, 2.0);
        let out = write_through(&write(&sidecar), &db, &no_beat());
        match &out[..] {
            [Outcome::Take { theirs, .. }] => assert_eq!(theirs.current.light.exposure, 2.0),
            other => panic!("{other:?}"),
        }
        let p = pending_for(&db, &nas);
        assert_eq!((p.len(), p[0].reason.as_str()), (1, "to join"));
        assert_eq!(
            Sidecar::load(&copy)
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            2.0
        );
        // The archive away: waiting, with the reason, the row counted.
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
        let out = write_through(&write(&sidecar), &db, &no_beat());
        assert!(
            matches!(&out[..], [Outcome::Waiting { reason, .. }] if reason == "offline"),
            "{out:?}"
        );
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        let p = pending_for(&db, &nas);
        assert_eq!(
            (p.len(), p[0].reason.as_str(), p[0].tries),
            (1, "offline", 2)
        );
        // A sidecar with no revision is refused.
        let mut old = Sidecar::default();
        old.record(exposed(9.0));
        let out = write_through(&write(&old), &db, &no_beat());
        assert!(
            matches!(&out[..], [Outcome::Waiting { reason, .. }] if reason.starts_with("no revision")),
            "{out:?}"
        );
        let _ = local;
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Through the window: an edit saved here is written through to
    /// the archive's copy off the window's thread, and the two sidecars
    /// are the same file; a burst of saves is one write; the XMP beside
    /// the copy says what the sidecar says, when XMPs are on.
    #[test]
    fn an_edit_saved_here_is_written_through_and_both_sidecars_are_equal() {
        let dir = scratch("window");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        state.borrow_mut().xmp_sidecars = true;
        state.borrow_mut().sidecars[0].meta.rating = 3;
        save(&state, 0.75);
        let here = Sidecar::load(&files[0]).unwrap().unwrap();
        assert_eq!(here.current.light.exposure, 0.75);
        save(&state, 0.8);
        save(&state, 0.9);
        {
            let st = state.borrow();
            assert!(st.sync.in_flight.contains(&files[0]));
            assert_eq!(st.sync.queued.len(), 1);
        }
        let landed = land_sent(&state, &app, &worker);
        assert_eq!(landed, 2, "the first write, then the latest");
        let copy = nas.join("shoot").join("a.tif");
        let here = Sidecar::load(&files[0]).unwrap().unwrap();
        let there = Sidecar::load(&copy).unwrap().unwrap();
        assert_eq!(there.current.light.exposure, 0.9);
        assert_eq!(compare(&here, &there), Compared::Same);
        assert_eq!(here, there);
        assert!(
            !greycard_edit::xmp::paths_of(&copy).is_empty(),
            "the copy's XMP"
        );
        assert!(state.borrow().sync.queued.is_empty());
        assert!(state.borrow().sync.in_flight.is_empty());
        assert_eq!(app.get_sync_note(), "");
        assert!(pending_for(&db, &nas).is_empty());
        // XMPs off: none written beside the copy.
        let dir2 = scratch("window-noxmp");
        let (local2, nas2, files2, db2) = two_places(&dir2);
        let app2 = window(2);
        let (state2, worker2) = opened(&app2, &dir2, files2.clone(), &local2, &nas2, &db2);
        state2.borrow_mut().sidecars[0].meta.rating = 3;
        save(&state2, 0.5);
        land_sent(&state2, &app2, &worker2);
        assert!(greycard_edit::xmp::paths_of(&nas2.join("shoot").join("a.tif")).is_empty());
        crate::testing::remove_scratch(state2, &dir2);
        crate::testing::remove_scratch(state, &dir);
    }

    /// The archive away: the save is kept here, the write waits in the
    /// index and the status line says so; the window closed and opened
    /// again says it from the index at once; the archive back, the
    /// catch-up writes it and the note goes.
    #[test]
    fn with_the_archive_away_the_write_waits_and_the_catch_up_converges() {
        let dir = scratch("catch-up");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
        save(&state, 0.6);
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("a.tif");
        assert!(
            Sidecar::load(&copy).unwrap().is_none(),
            "nothing written while away"
        );
        assert_eq!(pending_for(&db, &nas).len(), 1);
        assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
        {
            let mut st = state.borrow_mut();
            st.current = Some(1);
        }
        save(&state, -0.6);
        land_sent(&state, &app, &worker);
        assert_eq!(app.get_sync_note(), "2 edits waiting for Archive");
        // The window closes and opens again: the launch says it.
        drop(worker);
        drop(state);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        assert_eq!(app.get_sync_note(), "2 edits waiting for Archive");
        // The archive answers a look: the catch-up runs, one job a frame.
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        {
            let mut st = state.borrow_mut();
            heard_from(&mut st, &app, &nas, true);
            assert_eq!(st.sync.in_flight.len(), 2);
        }
        land_sent(&state, &app, &worker);
        let here = Sidecar::load(&files[0]).unwrap().unwrap();
        let there = Sidecar::load(&copy).unwrap().unwrap();
        assert_eq!(compare(&here, &there), Compared::Same);
        let b_copy = nas.join("shoot").join("b.tif");
        assert_eq!(
            Sidecar::load(&b_copy)
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            -0.6
        );
        assert!(pending_for(&db, &nas).is_empty());
        assert_eq!(app.get_sync_note(), "");
        {
            let mut st = state.borrow_mut();
            heard_from(&mut st, &app, &nas, true);
        }
        assert_eq!(land_sent(&state, &app, &worker), 0);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A copy another machine went on from while ours was saved too:
    /// the write finds it not ours and not behind, hands it back, the
    /// window joins the two against what it holds now, saves, and the
    /// write goes again; both copies end the same file, the laptop's
    /// branch kept.
    #[test]
    fn a_copy_saved_elsewhere_is_joined_here_and_written_back() {
        let dir = scratch("join");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        save(&state, 0.5);
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("a.tif");
        laptop_saves(&copy, -1.0);
        save(&state, 2.0);
        land_sent(&state, &app, &worker);
        let here = Sidecar::load(&files[0]).unwrap().unwrap();
        let there = Sidecar::load(&copy).unwrap().unwrap();
        assert_eq!(compare(&here, &there), Compared::Same, "{here:?}");
        let exposures = exposures_of(&here);
        assert!(
            exposures.contains(&-1.0) && exposures.contains(&2.0),
            "{exposures:?}"
        );
        assert!(
            here.current_label
                .as_deref()
                .is_some_and(|l| l.starts_with("Reconciled"))
        );
        assert_eq!(state.borrow().sidecars[0], here);
        assert!(pending_for(&db, &nas).is_empty());
        crate::testing::remove_scratch(state, &dir);
    }

    /// Probe D: a copy handed back as behind what the job carried lands
    /// after the window saved again. The landing compares with what the
    /// window holds now, so the newer saves stand.
    #[test]
    fn a_take_landing_after_newer_saves_keeps_them() {
        let dir = scratch("stale-take");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        save(&state, 0.5);
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("a.tif");
        let theirs = Sidecar::load(&copy).unwrap().unwrap();
        save(&state, 0.8);
        save(&state, 0.9);
        // The landing of a job that compared the 0.5 snapshot and found
        // ours behind the copy's.
        settle(&state, &app, &worker, &files[0], &nas, &copy, theirs);
        land_sent(&state, &app, &worker);
        let here = Sidecar::load(&files[0]).unwrap().unwrap();
        assert!(
            exposures_of(&here).contains(&0.9),
            "{:?}",
            exposures_of(&here)
        );
        assert_eq!(here.current.light.exposure, 0.9);
        let there = Sidecar::load(&copy).unwrap().unwrap();
        assert_eq!(compare(&here, &there), Compared::Same);
        crate::testing::remove_scratch(state, &dir);
    }

    /// Probe B: a write set aside (the share went quiet) and the window
    /// quit before it said anything: the frame is pending, from the row
    /// the job made before it tried; opened again, the catch-up takes
    /// it.
    #[test]
    fn a_quit_with_a_write_out_leaves_it_pending_and_the_next_launch_catches_up() {
        let dir = scratch("quit");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
        ASIDE_NEXT.with(|a| a.set(true));
        save(&state, 0.5);
        save(&state, 0.6);
        // Heard as set aside; its own end never comes before the quit.
        let first: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..1).collect());
        for j in first {
            j(&state, &app, &worker);
        }
        // The job itself, still out: it ran its first step, the row.
        let out: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        for j in out {
            j(&state, &app, &worker);
        }
        SENT.with(|s| s.borrow_mut().clear());
        let p = pending_for(&db, &nas);
        assert_eq!(
            p.len(),
            1,
            "the save that never reached the archive is pending"
        );
        drop(worker);
        drop(state);
        // Opened again, the archive answering.
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("a.tif");
        assert_eq!(
            Sidecar::load(&copy)
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            0.6
        );
        assert!(pending_for(&db, &nas).is_empty());
        crate::testing::remove_scratch(state, &dir);
    }

    /// Probe E: a write set aside goes on alone; the next save queues
    /// behind it rather than running beside it; when the first lands,
    /// late, the second goes, and the archive ends at the newest save.
    #[test]
    fn a_write_set_aside_keeps_the_frame_in_flight_and_the_newer_one_goes_after() {
        let dir = scratch("aside");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        ASIDE_NEXT.with(|a| a.set(true));
        save(&state, 0.5);
        let first: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        for j in first {
            j(&state, &app, &worker);
        }
        let late: Vec<Sent> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        assert_eq!(late.len(), 1);
        assert!(
            state.borrow().sync.in_flight.contains(&files[0]),
            "still out"
        );
        save(&state, 0.9);
        assert_eq!(
            state.borrow().sync.in_flight.len(),
            1,
            "no second job beside it"
        );
        assert_eq!(land_sent(&state, &app, &worker), 0, "nothing new sent");
        for j in late {
            j(&state, &app, &worker);
        }
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("a.tif");
        let there = Sidecar::load(&copy).unwrap().unwrap();
        let here = Sidecar::load(&files[0]).unwrap().unwrap();
        assert_eq!(there.current.light.exposure, 0.9);
        assert_eq!(compare(&here, &there), Compared::Same);
        assert!(pending_for(&db, &nas).is_empty());
        assert_eq!(app.get_sync_note(), "");
        crate::testing::remove_scratch(state, &dir);
    }

    /// Probe C: one raw in two folders (a shoot and its selects), both
    /// backed up; each local frame's edit goes to its own copy and
    /// nowhere else; with no pairing to say which copy is whose,
    /// nothing is written and the row says why.
    #[test]
    fn two_frames_with_one_hash_keep_their_own_edits() {
        let dir = scratch("two-frames");
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot = local.join("shoot");
        let selects = local.join("selects");
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&selects).unwrap();
        let a = shoot.join("a.tif");
        write_frame(&a, &R5, 1);
        let a2 = selects.join("a.tif");
        std::fs::copy(&a, &a2).unwrap();
        for sub in ["shoot", "selects"] {
            std::fs::create_dir_all(nas.join(sub)).unwrap();
            std::fs::copy(&a, nas.join(sub).join("a.tif")).unwrap();
        }
        let db = dir.join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_tree(&local, &mut |_| {}).unwrap();
            lib.index_tree(&nas, &mut |_| {}).unwrap();
        }
        let app = window(2);
        let (state, worker) = opened(&app, &dir, vec![a.clone(), a2.clone()], &local, &nas, &db);
        // No pairing yet: nothing written, the row says so.
        save(&state, 1.0);
        land_sent(&state, &app, &worker);
        let p = pending_for(&db, &nas);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].copy.as_os_str().is_empty());
        assert!(
            p[0].reason.contains("more than one copy"),
            "{}",
            p[0].reason
        );
        assert!(
            Sidecar::load(&nas.join("shoot").join("a.tif"))
                .unwrap()
                .is_none()
        );
        // The pairings say which is whose.
        {
            let mut st = state.borrow_mut();
            st.library
                .roots
                .set_backup_folder(&shoot, &nas, &nas.join("shoot"));
            st.library
                .roots
                .set_backup_folder(&selects, &nas, &nas.join("selects"));
            st.current = Some(1);
        }
        save(&state, -2.0);
        land_sent(&state, &app, &worker);
        state.borrow_mut().current = Some(0);
        save(&state, 1.5);
        land_sent(&state, &app, &worker);
        let show = |p: &Path| {
            Sidecar::load(p)
                .unwrap()
                .map(|s| exposures_of(&s))
                .unwrap_or_default()
        };
        assert!(
            !show(&a).contains(&-2.0),
            "the selects edit joined into the shoot's frame"
        );
        assert!(!show(&nas.join("selects").join("a.tif")).contains(&1.5));
        assert_eq!(show(&nas.join("shoot").join("a.tif")), show(&a));
        assert_eq!(show(&nas.join("selects").join("a.tif")), show(&a2));
        // The earlier row, re-paired by the catch-up, goes.
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        land_sent(&state, &app, &worker);
        assert!(
            pending_for(&db, &nas).is_empty(),
            "{:?}",
            pending_for(&db, &nas)
        );
        crate::testing::remove_scratch(state, &dir);
    }

    /// A catch-up and a save for one frame at once: the catch-up queues
    /// behind the save's write, so the two never run beside each other,
    /// and the archive ends at the save.
    #[test]
    fn a_catch_up_racing_a_save_queues_behind_it() {
        let dir = scratch("race");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
        save(&state, 0.3);
        land_sent(&state, &app, &worker);
        assert_eq!(pending_for(&db, &nas).len(), 1);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        // A save sends its write; the archive's answer arrives while it
        // is out.
        save(&state, 0.4);
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        assert_eq!(
            state.borrow().sync.in_flight.len(),
            1,
            "one job for the frame"
        );
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("a.tif");
        assert_eq!(
            Sidecar::load(&copy)
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            0.4
        );
        assert!(pending_for(&db, &nas).is_empty());
        crate::testing::remove_scratch(state, &dir);
    }

    /// Probe A, and the follow: the frame on screen follows to the
    /// archive's copy when its root goes, after the copy is checked on
    /// a job; a copy another machine saved first is settled, and the
    /// frame follows after, so the next save never writes over it; with
    /// a write waiting the frame stays.
    #[test]
    fn the_frame_on_screen_follows_to_the_archives_copy_once_the_copy_is_settled() {
        let dir = scratch("follow");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        save(&state, 0.5);
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("a.tif");
        // The laptop saved the copy; the local disk is unplugged (probe
        // K: really gone, so the take and the save here go nowhere).
        laptop_saves(&copy, -1.0);
        let away = dir.join("local-away");
        std::fs::rename(&local, &away).unwrap();
        {
            let mut st = state.borrow_mut();
            st.library.offline.insert(local.clone());
            follow_offline(&mut st, &app);
            assert!(st.sync.checking.contains(&files[0]));
            assert_eq!(st.files[0], files[0], "not followed before the check");
        }
        land_sent(&state, &app, &worker);
        // The check found the laptop's save: taken into the window,
        // written to the copy (the same, recorded), and the frame
        // followed once that landed.
        let there = Sidecar::load(&copy).unwrap().unwrap();
        assert!(
            exposures_of(&there).contains(&-1.0),
            "{:?}",
            exposures_of(&there)
        );
        assert_eq!(state.borrow().files[0], copy, "followed after settling");
        let status = app.get_status().to_string();
        assert!(
            status.contains("is offline") && status.contains("Archive"),
            "{status}"
        );
        // The window holds the copy's, and one revision more: the save
        // it tried here advanced it before the write failed. Not behind
        // the copy, so nothing of the laptop's is lost at the next save.
        assert!(matches!(
            compare(&state.borrow().sidecars[0], &there),
            Compared::Same | Compared::Behind(Side::Other)
        ));
        assert!(
            pending_for(&db, &nas).is_empty(),
            "{:?}",
            pending_for(&db, &nas)
        );
        // The next save goes to the copy, through the check, and the
        // laptop's branch stays.
        save(&state, 0.7);
        land_sent(&state, &app, &worker);
        let there = Sidecar::load(&copy).unwrap().unwrap();
        assert_eq!(there.current.light.exposure, 0.7);
        assert!(exposures_of(&there).contains(&-1.0));
        assert_eq!(compare(&state.borrow().sidecars[0], &there), Compared::Same);
        assert!(pending_for(&db, &nas).is_empty());
        assert_eq!(app.get_sync_note(), "");
        std::fs::rename(&away, &local).unwrap();
        assert_eq!(
            Sidecar::load(&files[0])
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            0.5,
            "the file here, away through it all, is as it was"
        );
        // The root back: the sidecar here is brought up to the copy's,
        // off the window's thread, and the frame goes on following.
        {
            let mut st = state.borrow_mut();
            st.library.offline.clear();
            heard_over(&mut st, &app, &HashSet::new());
            assert!(
                st.sync.in_flight.contains(&copy),
                "one job, through the frame's queue"
            );
        }
        land_sent(&state, &app, &worker);
        let home = Sidecar::load(&files[0]).unwrap().unwrap();
        assert_eq!(home.current.light.exposure, 0.7);
        assert!(exposures_of(&home).contains(&-1.0));
        assert_eq!(compare(&home, &state.borrow().sidecars[0]), Compared::Same);
        assert_eq!(state.borrow().files[0], copy, "still followed");
        // Once a return: another look queues nothing.
        heard_over(&mut state.borrow_mut(), &app, &HashSet::new());
        assert_eq!(land_sent(&state, &app, &worker), 0);
        // With a write waiting, the frame stays.
        let app2 = window(2);
        let (state2, worker2) = opened(&app2, &dir, files.clone(), &local, &nas, &db);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
        state2.borrow_mut().current = Some(1);
        save(&state2, 0.1);
        land_sent(&state2, &app2, &worker2);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        {
            let mut st = state2.borrow_mut();
            st.library.offline.insert(local.clone());
            follow_offline(&mut st, &app2);
            assert!(!st.sync.checking.contains(&files[1]));
            assert_eq!(st.files[1], files[1], "a write waits: it stays");
        }
        drop(worker2);
        drop(state2);
        crate::testing::remove_scratch(state, &dir);
    }

    /// A save made with no window open leaves the frame waiting in the
    /// index, and the next window's catch-up writes it; a frame under
    /// the archive, or with no copy, leaves nothing.
    #[test]
    fn a_save_with_no_window_open_is_noted_and_the_next_window_writes_it() {
        let dir = scratch("no-window");
        let (local, nas, files, db) = two_places(&dir);
        let mut roots = greycard_library::Roots::default();
        roots.add(&local).unwrap();
        roots.add(&nas).unwrap();
        roots.set_archive(&nas, true);
        let mut s = Sidecar::default();
        s.record(exposed(3.0));
        s.save(&files[1]).unwrap();
        assert_eq!(note_disk_save(&db, &roots, &files[1]), 1);
        assert_eq!(
            note_disk_save(&db, &roots, &nas.join("shoot").join("b.tif")),
            0
        );
        assert_eq!(
            note_disk_save(&dir.join("none.sqlite"), &roots, &files[1]),
            0
        );
        assert!(!dir.join("none.sqlite").exists());
        let p = pending_for(&db, &nas);
        assert_eq!(
            (p.len(), p[0].reason.as_str()),
            (1, "saved with no window open")
        );
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("b.tif");
        assert_eq!(
            Sidecar::load(&copy)
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            3.0
        );
        assert!(pending_for(&db, &nas).is_empty());
        assert_eq!(app.get_sync_note(), "");
        crate::testing::remove_scratch(state, &dir);
    }

    /// An export recorded on a frame the window has let go of, and a
    /// stand-in un-rejected, reach the archive from the file.
    #[test]
    fn a_save_made_on_disk_reaches_the_archive_from_the_file() {
        let dir = scratch("disk-save");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        let mut on_disk = Sidecar::default();
        on_disk.record(exposed(4.0));
        on_disk.save(&files[1]).unwrap();
        after_disk_save(&mut state.borrow_mut(), &files[1]);
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("b.tif");
        assert_eq!(
            Sidecar::load(&copy)
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            4.0
        );
        crate::testing::remove_scratch(state, &dir);
    }
    /// One raw in a shoot and in its selects; the archive holds the
    /// shoot's copy, and the selects' too when `both_on_nas`.
    fn dup_setup(
        dir: &Path,
        both_on_nas: bool,
    ) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
        let (local, nas) = (dir.join("local"), dir.join("nas"));
        let shoot = local.join("shoot");
        let selects = local.join("selects");
        std::fs::create_dir_all(&shoot).unwrap();
        std::fs::create_dir_all(&selects).unwrap();
        let a = shoot.join("a.tif");
        write_frame(&a, &R5, 1);
        let a2 = selects.join("a.tif");
        std::fs::copy(&a, &a2).unwrap();
        let subs: &[&str] = if both_on_nas {
            &["shoot", "selects"]
        } else {
            &["shoot"]
        };
        for sub in subs {
            std::fs::create_dir_all(nas.join(sub)).unwrap();
            std::fs::copy(&a, nas.join(sub).join("a.tif")).unwrap();
        }
        let db = dir.join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_tree(&local, &mut |_| {}).unwrap();
            lib.index_tree(&nas, &mut |_| {}).unwrap();
        }
        (local, nas, shoot, a, a2, db)
    }

    fn hash_of(db: &Path, f: &Path) -> String {
        Library::open_read_only(db)
            .unwrap()
            .by_path(f)
            .unwrap()
            .unwrap()
            .hash
    }

    fn show(p: &Path) -> Vec<f32> {
        Sidecar::load(p)
            .unwrap()
            .map(|s| exposures_of(&s))
            .unwrap_or_default()
    }

    /// Probe F: a row for a frame the window does not hold, whose local
    /// disk is unplugged while the archive answers, is kept with the
    /// reason, not dropped as gone; the disk back, the next catch-up
    /// writes it.
    #[test]
    fn a_catch_up_keeps_the_row_of_a_frame_whose_disk_is_away() {
        let dir = scratch("disk-away");
        let (local, nas, files, db) = two_places(&dir);
        {
            let app = window(2);
            let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
            crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
            save(&state, 0.5);
            land_sent(&state, &app, &worker);
            crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
            assert_eq!(pending_for(&db, &nas).len(), 1);
            state.borrow_mut().index_reader = None;
            drop(state);
            drop(worker);
        }
        // Another window, over an archive frame; the local disk gone.
        let app = window(1);
        let other = nas.join("shoot").join("b.tif");
        let (state, worker) = opened(&app, &dir, vec![other], &local, &nas, &db);
        let away = dir.join("local-away");
        std::fs::rename(&local, &away).unwrap();
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        land_sent(&state, &app, &worker);
        let after = pending_for(&db, &nas);
        assert_eq!(after.len(), 1, "{after:?}");
        assert!(
            after[0].reason.contains("disk may be away"),
            "{}",
            after[0].reason
        );
        assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
        // The disk back: the next answering look writes it.
        std::fs::rename(&away, &local).unwrap();
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        land_sent(&state, &app, &worker);
        assert!(pending_for(&db, &nas).is_empty());
        let copy = nas.join("shoot").join("a.tif");
        assert_eq!(
            Sidecar::load(&copy)
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            0.5
        );
        crate::testing::remove_scratch(state, &dir);
    }

    /// Probe G: the selects frame's row saying its copy is not known
    /// is its own; the shoot frame's paired write does not clear it.
    /// A pairing for the selects folder does.
    #[test]
    fn a_frames_unknown_copy_row_is_not_cleared_by_another_frames_write() {
        let dir = scratch("own-row");
        let (local, nas, shoot, a, a2, db) = dup_setup(&dir, true);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, vec![a.clone(), a2.clone()], &local, &nas, &db);
        state
            .borrow_mut()
            .library
            .roots
            .set_backup_folder(&shoot, &nas, &nas.join("shoot"));
        state.borrow_mut().current = Some(1);
        save(&state, -2.0);
        land_sent(&state, &app, &worker);
        let p = pending_for(&db, &nas);
        assert_eq!(p.len(), 1);
        assert!(
            p[0].frame == a2 && p[0].copy.as_os_str().is_empty(),
            "{p:?}"
        );
        assert_eq!(
            app.get_sync_note(),
            "1 edit waiting for Archive; 1 can't tell which copy is its own"
        );
        state.borrow_mut().current = Some(0);
        save(&state, 1.0);
        land_sent(&state, &app, &worker);
        let p = pending_for(&db, &nas);
        assert!(
            p.iter().any(|r| r.frame == a2),
            "the selects frame's row stands: {p:?}"
        );
        assert!(
            show(&nas.join("selects").join("a.tif")).is_empty(),
            "nothing written to it"
        );
        state.borrow_mut().library.roots.set_backup_folder(
            &local.join("selects"),
            &nas,
            &nas.join("selects"),
        );
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        land_sent(&state, &app, &worker);
        assert!(
            pending_for(&db, &nas).is_empty(),
            "{:?}",
            pending_for(&db, &nas)
        );
        assert_eq!(show(&nas.join("selects").join("a.tif")), show(&a2));
        assert_eq!(show(&nas.join("shoot").join("a.tif")), show(&a));
        crate::testing::remove_scratch(state, &dir);
    }

    /// Probe H: one raw in two local folders, the shoot alone backed up
    /// and paired; the selects frame's sole hit on the archive is the
    /// shoot's copy, and is not written.
    #[test]
    fn a_sole_hit_that_is_another_frames_copy_is_not_written() {
        let dir = scratch("sole-hit");
        let (local, nas, shoot, a, a2, db) = dup_setup(&dir, false);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, vec![a.clone(), a2.clone()], &local, &nas, &db);
        state
            .borrow_mut()
            .library
            .roots
            .set_backup_folder(&shoot, &nas, &nas.join("shoot"));
        save(&state, 1.0);
        land_sent(&state, &app, &worker);
        state.borrow_mut().current = Some(1);
        save(&state, -2.0);
        land_sent(&state, &app, &worker);
        let there = nas.join("shoot").join("a.tif");
        assert!(
            !show(&there).contains(&-2.0),
            "the selects edit went into the shoot's copy"
        );
        assert!(
            !show(&a2).contains(&1.0),
            "the shoot's edit joined into the selects frame"
        );
        assert_eq!(show(&there), show(&a));
        let p = pending_for(&db, &nas);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].frame == a2 && p[0].copy.as_os_str().is_empty());
        crate::testing::remove_scratch(state, &dir);
    }

    /// Probe I: a Settle job saved the join to the file and recorded
    /// the copy as ours while the window held the file as read before
    /// it. The window's save after finds the stat ours but the revision
    /// not in its lineage, compares, and comes back to join: the
    /// laptop's edit is in the copy still.
    #[test]
    fn a_window_save_after_a_settle_job_joins_rather_than_writes_over() {
        let dir = scratch("settle-race");
        let (local, nas, files, db) = two_places(&dir);
        let a = files[0].clone();
        let copy = nas.join("shoot").join("a.tif");
        let mut s = Sidecar::default();
        s.record(exposed(0.5));
        s.save(&a).unwrap();
        let hash = hash_of(&db, &a);
        let target = Copy {
            archive: nas.clone(),
            copy: copy.clone(),
        };
        let w = |source: Source| Write {
            frame: a.clone(),
            hash: hash.clone(),
            source,
            placement: Placement::Beside,
            xmp: false,
            copies: vec![target.clone()],
        };
        assert!(matches!(
            write_through(&w(Source::Disk), &db, &no_beat())[..],
            [Outcome::Written { .. }]
        ));
        laptop_saves(&copy, -1.0);
        // The window reads the file now.
        let mut window_s = Sidecar::load(&a).unwrap().unwrap();
        // The Settle job: the catch-up's Take on a frame not held.
        let theirs = Sidecar::read(&Sidecar::find(&copy).unwrap()).unwrap();
        // The file here is behind the copy: taken whole, and recorded.
        let out = write_through(&w(Source::Settle(Box::new(theirs))), &db, &no_beat());
        assert!(matches!(out[..], [Outcome::Same { .. }]), "{out:?}");
        assert!(show(&a).contains(&-1.0), "the copy's taken here");
        // The window's edit, save and write.
        window_s.record(exposed(0.7));
        window_s.save(&a).unwrap();
        let out = write_through(&w(Source::Memory(Box::new(window_s))), &db, &no_beat());
        assert!(matches!(out[..], [Outcome::Take { .. }]), "{out:?}");
        assert!(
            show(&copy).contains(&-1.0),
            "the laptop's edit stands in the copy"
        );
        let _ = local;
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Probe J: a save noted with no window open never brings the
    /// library up: an older library is left as it is, byte for byte,
    /// and the frame's copy waits for a window's save.
    #[test]
    fn a_note_with_no_window_open_leaves_an_older_library_untouched() {
        let dir = scratch("older-index");
        let (local, nas, files, db) = two_places(&dir);
        // The index at schema 5: the sync tables gone, the version down.
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "DROP TABLE archive_writes; DROP TABLE pending; PRAGMA user_version = 5;",
            )
            .unwrap();
        }
        let before = std::fs::read(&db).unwrap();
        let mut roots = greycard_library::Roots::default();
        roots.add(&local).unwrap();
        roots.add(&nas).unwrap();
        roots.set_archive(&nas, true);
        assert_eq!(note_disk_save(&db, &roots, &files[1]), 0);
        assert_eq!(std::fs::read(&db).unwrap(), before, "not a byte changed");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A save made before the index reader is open (a launch preset's)
    /// is not dropped: it waits in memory and is queued when the
    /// reader opens.
    #[test]
    fn a_save_before_the_reader_opens_is_queued_when_it_does() {
        let dir = scratch("deferred");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        {
            let mut st = state.borrow_mut();
            st.index_reader = None;
        }
        save(&state, 0.4);
        {
            let st = state.borrow();
            assert_eq!(st.sync.deferred.len(), 1);
            assert!(st.sync.queued.is_empty() && st.sync.in_flight.is_empty());
        }
        assert_eq!(land_sent(&state, &app, &worker), 0);
        crate::library::open_reader(&mut state.borrow_mut(), &app);
        assert!(state.borrow().sync.deferred.is_empty());
        assert_eq!(state.borrow().sync.in_flight.len(), 1);
        land_sent(&state, &app, &worker);
        let copy = nas.join("shoot").join("a.tif");
        assert_eq!(
            Sidecar::load(&copy)
                .unwrap()
                .unwrap()
                .current
                .light
                .exposure,
            0.4
        );
        crate::testing::remove_scratch(state, &dir);
    }
    /// A frame deleted by another hand from a folder that is there has
    /// its row dropped at the next catch-up; the editor's own Delete
    /// drops it with the frame's row. A folder gone whole keeps the
    /// row (the disk-away test above).
    #[test]
    fn a_frame_gone_from_a_present_folder_has_its_row_dropped() {
        let dir = scratch("deleted");
        let (local, nas, files, db) = two_places(&dir);
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
        save(&state, 0.5);
        land_sent(&state, &app, &worker);
        state.borrow_mut().current = Some(1);
        save(&state, 0.6);
        land_sent(&state, &app, &worker);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        assert_eq!(pending_for(&db, &nas).len(), 2);
        // b deleted by the editor: its row and its pending rows go.
        Library::open(&db)
            .unwrap()
            .forget(std::slice::from_ref(&files[1]))
            .unwrap();
        assert_eq!(pending_for(&db, &nas).len(), 1);
        // a deleted by another hand, the window over other frames now.
        let other = vec![nas.join("shoot").join("b.tif")];
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(worker);
        std::fs::remove_file(&files[0]).unwrap();
        for s in Sidecar::find(&files[0]).into_iter() {
            std::fs::remove_file(s).unwrap();
        }
        let app = window(1);
        let (state, worker) = opened(&app, &dir, other, &local, &nas, &db);
        assert_eq!(app.get_sync_note(), "1 edit waiting for Archive");
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        land_sent(&state, &app, &worker);
        assert!(
            pending_for(&db, &nas).is_empty(),
            "{:?}",
            pending_for(&db, &nas)
        );
        assert_eq!(app.get_sync_note(), "");
        assert!(
            Sidecar::load(&nas.join("shoot").join("a.tif"))
                .unwrap()
                .is_none()
        );
        crate::testing::remove_scratch(state, &dir);
    }
    /// Probe R2: the local sidecar of a followed frame edited on its own
    /// while the root was away (another build, a headless export): the
    /// root back, it is not written over; it is left for Back up or
    /// Bring back to join.
    #[test]
    fn bringing_home_leaves_a_local_sidecar_that_went_its_own_way() {
        let dir = scratch("home-diverged");
        let (local, nas, files, db) = two_places(&dir);
        let copy = nas.join("shoot").join("a.tif");
        let app = window(2);
        let (state, worker) = opened(&app, &dir, files.clone(), &local, &nas, &db);
        save(&state, 0.5);
        land_sent(&state, &app, &worker);
        {
            let mut st = state.borrow_mut();
            st.library.offline.insert(local.clone());
            follow_offline(&mut st, &app);
        }
        land_sent(&state, &app, &worker);
        assert_eq!(state.borrow().files[0], copy);
        save(&state, 0.7);
        land_sent(&state, &app, &worker);
        // The local file edited on its own while the frame was followed.
        let mut there = Sidecar::load(&files[0]).unwrap().unwrap();
        there.record(exposed(-3.0));
        there.save(&files[0]).unwrap();
        {
            let mut st = state.borrow_mut();
            st.library.offline.clear();
            heard_over(&mut st, &app, &HashSet::new());
        }
        land_sent(&state, &app, &worker);
        let home = Sidecar::load(&files[0]).unwrap().unwrap();
        assert!(
            exposures_of(&home).contains(&-3.0),
            "the local edit was written over"
        );
        assert!(!exposures_of(&home).contains(&0.7));
        crate::testing::remove_scratch(state, &dir);
    }

    /// A frame at the top of a root that is a fixed mount point: the
    /// disk unplugged, the folder lists empty, and the row is kept; a
    /// folder with anything else in it and the frame gone is a delete,
    /// and the row goes.
    #[test]
    fn an_empty_folder_keeps_the_row_of_a_frame_that_was_in_it() {
        let dir = scratch("mount");
        let (mount, nas) = (dir.join("mount"), dir.join("nas"));
        std::fs::create_dir_all(&mount).unwrap();
        std::fs::create_dir_all(&nas).unwrap();
        let a = mount.join("a.tif");
        write_frame(&a, &R5, 1);
        std::fs::copy(&a, nas.join("a.tif")).unwrap();
        let db = dir.join("library.sqlite");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.index_tree(&mount, &mut |_| {}).unwrap();
            lib.index_tree(&nas, &mut |_| {}).unwrap();
        }
        let app = window(1);
        let (state, worker) = opened(&app, &dir, vec![a.clone()], &mount, &nas, &db);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().push(nas.clone()));
        save(&state, 0.5);
        land_sent(&state, &app, &worker);
        crate::roots::tests::NOT_ANSWERING.with(|n| n.borrow_mut().clear());
        assert_eq!(pending_for(&db, &nas).len(), 1);
        // The window over the archive's frame; the mount point empty.
        state.borrow_mut().index_reader = None;
        drop(state);
        drop(worker);
        std::fs::remove_file(&a).unwrap();
        for s in Sidecar::find(&a).into_iter() {
            std::fs::remove_file(s).unwrap();
        }
        assert!(std::fs::read_dir(&mount).unwrap().next().is_none());
        let app = window(1);
        let (state, worker) = opened(&app, &dir, vec![nas.join("a.tif")], &mount, &nas, &db);
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        land_sent(&state, &app, &worker);
        let p = pending_for(&db, &nas);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].reason.contains("disk may be away"));
        // Something else in the folder: the frame was deleted.
        std::fs::write(mount.join("notes.txt"), b"x").unwrap();
        heard_from(&mut state.borrow_mut(), &app, &nas, true);
        land_sent(&state, &app, &worker);
        assert!(pending_for(&db, &nas).is_empty());
        crate::testing::remove_scratch(state, &dir);
    }
}
