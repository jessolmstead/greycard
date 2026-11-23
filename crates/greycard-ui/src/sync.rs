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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use greycard_edit::sync::{Compared, Side, compare, join};
use greycard_edit::{Placement, Sidecar};
use greycard_edit_io::xmp;
use greycard_library::sync_note::{Copy, note_waiting, pair};
use greycard_library::{ArchiveWrite, Library};
use slint::ComponentHandle;

use crate::archive::{self, Beat, Heard};
use crate::panel::browser::file_name;
use crate::roots::ROOT_WAIT;
use crate::worker::Worker;
use crate::{App, Shared, State, tr};

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
    /// The frames whose write job is running on its thread, which the
    /// quit's write waits for ([`write_followed_at_quit`]).
    running: Arc<Running>,
}

impl Sync {
    /// The file at `was` moved to `to` (`browser::follow_paths`): a
    /// copy the window follows stays followed under its new path. Taken
    /// apart whole, as there, so a field added is placed here.
    pub(crate) fn file_moved(&mut self, was: &Path, to: &Path) {
        let Sync {
            followed,
            // A job's: keyed by the path it was made for, and let go
            // under that path when it ends.
            queued: _,
            in_flight: _,
            checking: _,
            deferred: _,
            running: _,
            // Archives' roots, not files.
            caught_up: _,
        } = self;
        match followed.remove(was) {
            Some(f) => {
                followed.insert(to.to_path_buf(), f);
            }
            None => {
                followed.remove(to);
            }
        }
    }
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

/// Whether frame `c` is one the window follows on an archive's copy.
pub(crate) fn followed(st: &State, c: usize) -> bool {
    st.files
        .get(c)
        .is_some_and(|f| st.work.sync.followed.contains_key(f))
}

/// The local frame the window follows on the archive's copy at `path`,
/// when it follows one there.
pub(crate) fn local_of<'a>(st: &'a State, path: &Path) -> Option<&'a Path> {
    st.work.sync.followed.get(path).map(|f| f.local.as_path())
}

/// Follow the frame at `copy`, an archive's copy of `local`, as an
/// offline root does: for a test of what a save of it does.
#[cfg(test)]
pub(crate) fn follow_for_test(st: &mut State, copy: PathBuf, local: PathBuf, archive: PathBuf) {
    st.work.sync.followed.insert(
        copy,
        Followed {
            local,
            archive,
            homed: false,
        },
    );
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
    if !st.window.write_sidecars {
        return;
    }
    let Some(frame) = st.files.get(c).cloned() else {
        return;
    };
    if let Some(followed) = st.work.sync.followed.get(&frame).cloned() {
        // The frame is the archive's copy: its own write, checked
        // against the copy as any is. With the reader closed (the
        // quit's own save), the quit writes it
        // (`write_followed_at_quit`).
        let Some(lib) = st.views.index_reader.as_ref() else {
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
            placement: st.window.placement,
            xmp: st.window.xmp_sidecars,
            copies: vec![Copy {
                archive: followed.archive,
                copy: frame.clone(),
            }],
        };
        queue(st, write);
        return;
    }
    if st.views.library.roots.archive_of(&frame).is_some() {
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
    if !st.window.write_sidecars || st.views.library.roots.archive_of(frame).is_some() {
        return;
    }
    queue_for(st, frame, Source::Disk);
}

/// The window is closing, its index reader and indexer gone: each
/// save whose archive write never went out is noted waiting in the
/// index, as a save made with no window open is, so the next window's
/// catch-up writes it from the file. Those are the quit's own save of
/// the open frame (deferred, the reader closed before it), a save
/// made before the reader ever opened, and a write queued behind one
/// still out for its frame. Each was saved to the frame's own sidecar
/// first, which is what the catch-up reads. A bring-home or a settle
/// is not a write of the file to the archive and is left; a frame on
/// the archive (one followed there) is not noted (`note_disk_save`):
/// it has no file here to write from, and its write went just before
/// ([`write_followed_at_quit`]). How many rows were noted.
pub(crate) fn note_unsent_at_quit(st: &mut State) -> usize {
    let deferred = std::mem::take(&mut st.work.sync.deferred);
    let queued = std::mem::take(&mut st.work.sync.queued);
    let Some(index) = st
        .views
        .index_path
        .clone()
        .filter(|_| st.window.write_sidecars)
    else {
        return 0;
    };
    let frames: std::collections::BTreeSet<PathBuf> = deferred
        .into_iter()
        .chain(queued.into_values().map(|w| (w.frame, w.source)))
        .filter(|(_, source)| matches!(source, Source::Memory(_) | Source::Disk))
        .map(|(frame, _)| frame)
        .collect();
    frames
        .iter()
        .map(|frame| {
            note_waiting(
                &index,
                &st.views.library.roots,
                frame,
                "saved as the window closed",
            )
        })
        .sum()
}

/// How long the quit waits for the writes of the frames it follows on
/// an archive's copy, a write of the frame's still out before them
/// included: the roots' look (three seconds) and a write, as long as
/// it waits for the worker (`worker::LEAVING`).
pub(crate) const QUIT_WRITE_WAIT: Duration = Duration::from_secs(5);

/// The frames whose write job is running on its thread, counted from
/// its sending to its end there, not to its landing, which a closing
/// window never hears: the quit's write of a frame waits for the
/// frame's job still out, so the two never write the copy at once.
#[derive(Default)]
pub(crate) struct Running {
    frames: std::sync::Mutex<HashMap<PathBuf, usize>>,
    ended: std::sync::Condvar,
}

impl Running {
    fn frames(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, usize>> {
        self.frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn start(&self, frame: &Path) {
        *self.frames().entry(frame.to_path_buf()).or_default() += 1;
    }

    fn end(&self, frame: &Path) {
        let mut frames = self.frames();
        if let Some(n) = frames.get_mut(frame) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                frames.remove(frame);
            }
        }
        drop(frames);
        self.ended.notify_all();
    }

    fn running(&self, frame: &Path) -> bool {
        self.frames().contains_key(frame)
    }

    /// Until no job of `frame`'s is running, however long that is.
    fn wait_for(&self, frame: &Path) {
        let mut frames = self.frames();
        while frames.contains_key(frame) {
            frames = self
                .ended
                .wait(frames)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

/// A job's end on its thread, said however it ends, and when it never
/// runs: a job not sent is dropped unrun.
struct Ends(Arc<Running>, PathBuf);

impl Drop for Ends {
    fn drop(&mut self) {
        self.0.end(&self.1);
    }
}

/// What the quit's writes of followed frames came to.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct QuitWrites {
    /// Frames whose write ended within the wait, whatever it came to.
    pub(crate) ended: usize,
    /// Rows noted waiting for the writes that had not.
    pub(crate) noted: usize,
}

/// The window is closing, its index reader gone: a frame it follows on
/// an archive's copy (§233) has no sidecar here, its root being away,
/// so a save whose write never went out (the quit's own, made after
/// the reader closed; one queued behind a write still out) is held in
/// memory alone, and no job sent now would land. Its write goes now,
/// off the window's thread, as a job's would: checked against what we
/// last wrote there, and a copy saved elsewhere since joined, not
/// written over. A job of the frame's still out goes first: the quit's
/// write waits for it to end, never runs beside it, and then finds its
/// save recorded or writes over it. The window waits for all of it at
/// most `limit`. A frame whose latest revision the index records on
/// its copy already is not written again. A write that has not ended
/// within the limit is noted waiting in the index, "saved as the
/// window closed", and goes on until the process ends: the save is on
/// the copy only if it lands by then, and the next window's catch-up
/// records what stands on the copy.
pub(crate) fn write_followed_at_quit(st: &mut State, limit: Duration) -> QuitWrites {
    write_followed_with(st, limit, write_at_quit)
}

/// [`write_followed_at_quit`], each frame's write made by `run`.
fn write_followed_with(
    st: &mut State,
    limit: Duration,
    run: fn(&Write, &Path) -> Vec<Outcome>,
) -> QuitWrites {
    let mut out = QuitWrites::default();
    let Some(index) = st
        .views
        .index_path
        .clone()
        .filter(|_| st.window.write_sidecars)
    else {
        return out;
    };
    let held: Vec<(usize, PathBuf, PathBuf)> = st
        .files
        .iter()
        .enumerate()
        .filter_map(|(c, f)| {
            let followed = st.work.sync.followed.get(f)?;
            Some((c, f.clone(), followed.archive.clone()))
        })
        .filter(|(c, ..)| {
            crate::sidecars::file_writable(st, *c) && !st.sidecars[*c].revisions.is_empty()
        })
        .collect();
    if held.is_empty() {
        return out;
    }
    // Opened as a save with no window open opens it: only as it is,
    // its lock waited on a quarter second.
    let lib = match Library::open_current(&index, Duration::from_millis(250)) {
        Ok(lib) => lib,
        Err(e) => {
            tracing::warn!(
                "sync: the index not read at the quit: {e}; a followed frame's last save is \
                 not written to its copy"
            );
            return out;
        }
    };
    let mut writes = Vec::new();
    for (c, copy, archive) in held {
        let Some(row) = lib.by_path(&copy).ok().flatten() else {
            continue;
        };
        let latest = st.sidecars[c].revisions.last().map(|r| r.hash.clone());
        let recorded = lib
            .archive_write(&row.hash, &archive, &copy)
            .ok()
            .flatten()
            .map(|w| w.revision);
        if recorded == latest {
            continue;
        }
        // The window's sidecar is the latest: a write queued behind
        // one still out carries no more than it.
        st.work.sync.queued.remove(&copy);
        writes.push(Write {
            frame: copy.clone(),
            hash: row.hash,
            source: Source::Memory(Box::new(st.sidecars[c].clone())),
            placement: st.window.placement,
            xmp: st.window.xmp_sidecars,
            copies: vec![Copy { archive, copy }],
        });
    }
    drop(lib);
    if writes.is_empty() {
        return out;
    }
    let keys: Vec<(String, PathBuf, PathBuf)> = writes
        .iter()
        .map(|w| (w.hash.clone(), w.copies[0].archive.clone(), w.frame.clone()))
        .collect();
    let running = Arc::clone(&st.work.sync.running);
    let job_running = Arc::clone(&running);
    let (tx, rx) = std::sync::mpsc::channel::<(PathBuf, Vec<Outcome>)>();
    let job_index = index.clone();
    let spawned = std::thread::Builder::new()
        .name("greycard quit write".into())
        .spawn(move || {
            for write in writes {
                // After the frame's job still out, never beside it: an
                // older save renamed over this one would stand on the
                // copy, and its record would clear this one's row.
                job_running.wait_for(&write.frame);
                let outcomes = run(&write, &job_index);
                if tx.send((write.frame, outcomes)).is_err() {
                    return;
                }
            }
        });
    let mut ended = HashSet::new();
    match spawned {
        Ok(_) => {
            let deadline = std::time::Instant::now() + limit;
            while ended.len() < keys.len() {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                let Ok((frame, outcomes)) = rx.recv_timeout(left) else {
                    break;
                };
                for outcome in outcomes {
                    said_at_quit(&frame, outcome);
                }
                ended.insert(frame);
            }
        }
        Err(e) => tracing::warn!("sync: the quit's write not started: {e}"),
    }
    out.ended = ended.len();
    let late: Vec<&(String, PathBuf, PathBuf)> =
        keys.iter().filter(|(_, _, f)| !ended.contains(f)).collect();
    if late.is_empty() {
        return out;
    }
    let mut lib = match Library::open_current(&index, Duration::from_millis(250)) {
        Ok(lib) => lib,
        Err(e) => {
            tracing::warn!("sync: the index not written at the quit: {e}");
            return out;
        }
    };
    for (hash, archive, frame) in late {
        if running.running(frame) {
            tracing::warn!(
                "sync: {}: a write of an older save to {} was still out as the window closed; \
                 this save waits behind it, and is lost unless both land before the editor ends",
                file_name(frame),
                archive.display()
            );
        } else {
            tracing::warn!(
                "sync: {}: its write to {} had not ended as the window closed; this save is \
                 lost unless it lands before the editor ends",
                file_name(frame),
                archive.display()
            );
        }
        match lib.add_pending(hash, archive, frame, frame, "saved as the window closed") {
            Ok(()) => out.noted += 1,
            Err(e) => tracing::warn!("sync: {}: pending not noted: {e}", file_name(frame)),
        }
    }
    out
}

/// A followed frame's write at the quit, off the window's thread:
/// nothing when a job of the frame's that was still out has recorded
/// this save on the copy already; else [`write_one`], and a copy that
/// came back not behind (saved elsewhere since) joined with this save
/// here and written, as the window's settle would at a landing that
/// will not come, the archive's answer of a moment ago standing. A
/// copy that holds all of this save already is left, its row for the
/// next catch-up.
fn write_at_quit(write: &Write, index: &Path) -> Vec<Outcome> {
    // Nobody listens: the quit's wait is a deadline, not a silence.
    let beat: Beat = Arc::new(|| {});
    let Source::Memory(ours) = &write.source else {
        return write_through(write, index, &beat);
    };
    let latest = ours.revisions.last().map(|r| r.hash.as_str());
    let recorded = |target: &Copy| {
        archive::open_index(index, false, &beat)
            .and_then(|lib| {
                lib.archive_write(&write.hash, &target.archive, &target.copy)
                    .ok()
                    .flatten()
            })
            .map(|w| w.revision)
    };
    write
        .copies
        .iter()
        .map(|target| {
            if recorded(target).as_deref() == latest {
                return Outcome::Same {
                    archive: target.archive.clone(),
                    copy: target.copy.clone(),
                };
            }
            match write_one(write, target, index, &beat) {
                Outcome::Take {
                    archive,
                    copy,
                    frame,
                    theirs,
                } => match compare(ours, &theirs) {
                    Compared::SameEdit | Compared::Diverged => {
                        let mut joined = join(ours, &theirs);
                        joined.advance(&greycard_edit::sync::Stamp::now());
                        let again = Write {
                            source: Source::Memory(Box::new(joined)),
                            ..write.clone()
                        };
                        let outcome = write_copy(&again, target, index, &beat, true);
                        if let Outcome::Take { archive, .. } = &outcome {
                            tracing::warn!(
                                "sync: {}: the copy on {} changed again while the quit joined \
                                 this save with it; this save is not on it",
                                file_name(&write.frame),
                                archive.display()
                            );
                        }
                        outcome
                    }
                    _ => {
                        tracing::info!(
                            "sync: {}: the copy on {} holds this save already",
                            file_name(&frame),
                            archive.display()
                        );
                        Outcome::Take {
                            archive,
                            copy,
                            frame,
                            theirs,
                        }
                    }
                },
                other => other,
            }
        })
        .collect()
}

/// The log's line for what a quit's write of `frame` came to.
fn said_at_quit(frame: &Path, outcome: Outcome) {
    match outcome {
        Outcome::Written { archive, .. } | Outcome::Same { archive, .. } => tracing::info!(
            "sync: {}: its last save is on {} as the window closed",
            file_name(frame),
            archive.display()
        ),
        Outcome::Waiting { archive, reason } => tracing::warn!(
            "sync: {}: its last save not written to {} as the window closed: {reason}",
            file_name(frame),
            archive.display()
        ),
        Outcome::Take { archive, .. } => tracing::info!(
            "sync: {}: the copy on {} was not behind this save; its row waits for the next window",
            file_name(frame),
            archive.display()
        ),
        Outcome::Gone { .. } | Outcome::Homed { .. } => {}
    }
}

/// The index reader is open now (`library::open_reader`): the saves
/// made before it was are paired and queued.
pub(crate) fn reader_opened(st: &mut State) {
    let deferred = std::mem::take(&mut st.work.sync.deferred);
    for (frame, source) in deferred {
        if st.window.write_sidecars && st.views.library.roots.archive_of(&frame).is_none() {
            queue_for(st, &frame, source);
        }
    }
}

/// Queue a write of `frame`'s sidecar from `source` to each of its
/// copies, and note in memory the archives where which copy is this
/// frame's is not known; the job notes the rest in the index.
fn queue_for(st: &mut State, frame: &Path, source: Source) {
    let Some(lib) = st.views.index_reader.as_ref() else {
        // The index is still opening (a launch preset's save comes
        // before it): kept, and queued when it is.
        st.work.sync.deferred.push((frame.to_path_buf(), source));
        return;
    };
    let Some(paired) = pair(lib, &st.views.library.roots, frame) else {
        return;
    };
    if paired.copies.is_empty() && paired.ambiguous.is_empty() {
        return;
    }
    let write = Write {
        frame: frame.to_path_buf(),
        hash: paired.hash,
        source,
        placement: st.window.placement,
        xmp: st.window.xmp_sidecars,
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
    st.work.sync.queued.insert(frame.clone(), write);
    send_next(st, &frame);
}

/// Send the queued write for `frame`, if there is one and none is out.
fn send_next(st: &mut State, frame: &Path) {
    if st.work.sync.in_flight.contains(frame) {
        return;
    }
    let Some(write) = st.work.sync.queued.remove(frame) else {
        return;
    };
    let Some(index) = st.views.index_path.clone() else {
        return;
    };
    let Some(app) = st.window.app.upgrade() else {
        return;
    };
    st.work.sync.in_flight.insert(frame.to_path_buf());
    let frame = frame.to_path_buf();
    let landing_frame = frame.clone();
    st.work.sync.running.start(&frame);
    let ends = Ends(Arc::clone(&st.work.sync.running), frame.clone());
    let sent = send(
        &app,
        "archive write",
        WRITE_WAIT,
        move |beat| {
            let outcomes = write_through(&write, &index, beat);
            drop(ends);
            outcomes
        },
        move |state, app, worker, heard| {
            landed(state, app, worker, &landing_frame, heard);
        },
    );
    if !sent {
        st.work.sync.in_flight.remove(&frame);
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
    // A newer build's on either side (§724): both left as they are.
    if ours.read_only() {
        tracing::info!("sync: {}: read-only; not brought home", local.display());
        return same;
    }
    match Sidecar::load(local, &ours.start) {
        Ok(None) => {}
        Ok(Some(there)) if there.read_only() => {
            tracing::info!(
                "sync: {}: the sidecar here is read-only (a newer greycard made it); left",
                local.display()
            );
            return same;
        }
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
    write_copy(write, target, index, beat, false)
}

/// [`write_one`]; `again` when the same job wrote this copy a moment
/// ago (the quit's join, [`write_at_quit`]): the archive answered then
/// and the row was made, so it is neither looked at nor counted a try
/// again.
fn write_copy(write: &Write, target: &Copy, index: &Path, beat: &Beat, again: bool) -> Outcome {
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
    if !again
        && let Some(lib) = lib.as_mut()
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
    if !again && crate::roots::answer_of(archive, ROOT_WAIT) != Some(true) {
        return note(lib.as_mut(), "offline");
    }
    beat();
    // The sidecar to write.
    let from_disk = |lib: &mut Option<Library>| -> Result<Sidecar, Outcome> {
        // Written, and held by no window: a line nobody developed writes
        // nothing, whatever its start (§724).
        match Sidecar::load_as_file(&write.frame) {
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
            // A newer build's on either side (§724): both left as they
            // are, the reason noted.
            if let Some(e) = [&ours, &**theirs].iter().find_map(|s| s.writable().err()) {
                return note(lib.as_mut(), &e.to_string());
            }
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
    if let Err(e) = sidecar.writable() {
        return note(lib.as_mut(), &e.to_string());
    }
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
            let theirs = match Sidecar::read(&dest, &sidecar.start) {
                Ok(s) => s,
                Err(e) => return note(lib.as_mut(), &format!("the copy: unreadable: {e}")),
            };
            // The copy a newer build made (§724): both left as they are.
            if let Err(e) = theirs.writable() {
                return note(lib.as_mut(), &format!("the copy: {e}"));
            }
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
            .map(|s| xmp::Turn::new(s.orientation, sidecar.xmp_turn()));
        let taken = || greycard_library::export_record::camera_time_of(copy);
        if let Err(e) = xmp::save_dated(copy, &sidecar.meta, turn, taken) {
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
    if !st.window.write_sidecars || !st.views.library.roots.is_archive(archive) {
        return;
    }
    if !answered {
        st.work.sync.caught_up.remove(archive);
        return;
    }
    if st.work.sync.caught_up.contains(archive) {
        return;
    }
    st.work.sync.caught_up.insert(archive.to_path_buf());
    catch_up(st, app, archive);
}

/// The archives the roots' look found, one way or the other: those in
/// `offline` did not answer, the rest did.
pub(crate) fn heard_over(st: &mut State, app: &App, offline: &HashSet<PathBuf>) {
    let archives = st.views.library.roots.archives().to_vec();
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
    if !st.window.write_sidecars {
        return;
    }
    let due: Vec<(PathBuf, Followed)> = st
        .work
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
        if !crate::sidecars::file_writable(st, c) || st.sidecars[c].revisions.is_empty() {
            continue;
        }
        let Some(hash) = st
            .views
            .index_reader
            .as_ref()
            .and_then(|l| l.by_path(&copy).ok().flatten())
            .map(|r| r.hash)
        else {
            continue;
        };
        if let Some(f) = st.work.sync.followed.get_mut(&copy) {
            f.homed = true;
        }
        let write = Write {
            frame: copy.clone(),
            hash,
            source: Source::Home {
                local: followed.local.clone(),
                ours: Box::new(st.sidecars[c].clone()),
            },
            placement: st.window.placement,
            xmp: false,
            copies: vec![Copy {
                archive: followed.archive.clone(),
                copy: copy.clone(),
            }],
        };
        // Behind a save's write when one is queued: the latest wins the
        // queue, so a home never replaces a save. It goes when nothing
        // is queued or out.
        if !st.work.sync.queued.contains_key(&copy) && !st.work.sync.in_flight.contains(&copy) {
            queue(st, write);
        } else if let Some(f) = st.work.sync.followed.get_mut(&copy) {
            f.homed = false;
        }
    }
}

/// Take up the writes waiting for `archive` now: each goes through
/// the frame's own queue, from the file, so a catch-up and a save's
/// write never run at once for one frame.
pub(crate) fn catch_up(st: &mut State, app: &App, archive: &Path) {
    if !st.window.write_sidecars {
        return;
    }
    let pending = st
        .views
        .index_reader
        .as_ref()
        .and_then(|l| l.pending_for(archive).ok())
        .unwrap_or_default();
    if pending.is_empty() {
        return;
    }
    let (placement, xmp) = (st.window.placement, st.window.xmp_sidecars);
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
                .views
                .index_reader
                .as_ref()
                .and_then(|l| pair(l, &st.views.library.roots, &p.frame))
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
        if st.work.sync.queued.contains_key(&frame) || st.work.sync.in_flight.contains(&frame) {
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
    // The edits not saved here first: their save lands, or fails
    // again without a word, and the note follows either way.
    crate::sidecars::retry(st);
    let counts = st
        .views
        .index_reader
        .as_ref()
        .and_then(|l| l.pending_counts().ok())
        .unwrap_or_default();
    for count in counts {
        if count.unknown > 0
            && let Some(lib) = st.views.index_reader.as_ref()
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
        st.work.sync.caught_up.remove(&count.archive);
        catch_up(st, app, &count.archive);
    }
}
/// Where a write's end lands on the window's thread.
fn landed(
    state: &Rc<Shared>,
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
            state.borrow_mut().work.sync.in_flight.remove(frame);
            refresh_note(&state.borrow(), app);
            return;
        }
        Heard::Done { result, .. } => {
            state.borrow_mut().work.sync.in_flight.remove(frame);
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
                if let Some(indexer) = &state.borrow().views.index {
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
            st.work.sync.caught_up.remove(&a);
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
    state: &Rc<Shared>,
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
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    frame: &Path,
    archive: &Path,
    copy: &Path,
    theirs: Sidecar,
) -> bool {
    let mut st = state.borrow_mut();
    if !st.window.write_sidecars {
        return false;
    }
    let held = st
        .files
        .iter()
        .position(|f| f == frame)
        .filter(|c| crate::sidecars::file_writable(&st, *c));
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
            // new revision, and nothing to write back. Unless another
            // writer saved the local file since the window read it:
            // then the copy's is taken in memory only, still knowing the
            // file as the window did, and its save below keeps that
            // writer's file beside the frame before it goes over it.
            let mut theirs = theirs;
            let mut moved_here = false;
            if !followed(&st, c) {
                let (path, placement) = (st.files[c].clone(), st.window.placement);
                theirs.on_disk = st.sidecars[c].on_disk.clone();
                moved_here = !st.sidecars[c].changed_on_disk(&path, placement).is_empty();
                let dest =
                    Sidecar::find(&path).unwrap_or_else(|| Sidecar::path_in(&path, placement));
                if moved_here {
                    tracing::info!(
                        "{}: its sidecar here was saved by another writer too; kept at its save",
                        file_name(frame)
                    );
                } else if let Err(e) = theirs.write_to_known(&dest) {
                    // The local root may be gone (the frame is about to
                    // follow its copy): the window takes the copy's
                    // anyway, and the file here stays behind it, as
                    // any copy away from a save does.
                    tracing::warn!("{}: sidecar not taken here: {e}", file_name(frame));
                }
            }
            st.sidecars[c] = theirs;
            if moved_here {
                crate::sidecars::save_file(&mut st, c);
            }
            // The copy's versions may not be the cells': one made or
            // deleted on the other machine moves the list once this is
            // done with its numbers.
            if crate::rows::versions_differ(&st, c) {
                crate::versions::settle_later(app);
            }
            // A frame off screen has its marks from what it holds now,
            // and its pictures from its lines as they are now.
            crate::panel::browser::show_badges_of_file(&st, app, c);
            for cell in st.cells_of_file(c) {
                crate::edited::saved(&mut st, cell);
            }
            if let Some(indexer) = &st.views.index {
                indexer.file(st.files[c].clone());
            }
            // The copy and the window are the same now, which the
            // index does not know yet: a write goes, finds the copy the
            // same, records it and clears the frame's row. Without it
            // the "to join" row would stand for good.
            after_save(&mut st, c);
            if st.current.is_some_and(|x| st.file_at(x) == Some(c)) {
                if moved_here {
                    // Saved once above, with the join: shown without a
                    // second save.
                    crate::panel::history::show_settled_later(app);
                    return true;
                }
                drop(st);
                show_settled(state, app, worker);
                return true;
            }
        }
        Compared::SameEdit | Compared::Diverged => {
            // Every change goes through a save, which makes a revision
            // in the same turn, so the window's copy is as its last
            // revision left it: what the join asks of it.
            debug_assert!(
                st.sidecars[c].at_its_revision(),
                "{}: a join of a copy changed since its save",
                file_name(frame)
            );
            // The join knows the local file as the window did, so its
            // save below still asks whether another writer saved that
            // file since.
            let mut joined = join(&st.sidecars[c], &theirs);
            joined.on_disk = st.sidecars[c].on_disk.clone();
            st.sidecars[c] = joined;
            // A version the join took out (deleted on the other copy
            // after it had seen every save of this one) or brought in
            // moves the list; nothing of that is written back here but
            // by the save below, which the join itself asks for.
            if crate::rows::versions_differ(&st, c) {
                crate::versions::settle_later(app);
            }
            crate::panel::browser::show_badges_of_file(&st, app, c);
            if st.current.is_some_and(|x| st.file_at(x) == Some(c)) {
                drop(st);
                crate::panel::history::take_current_settled(&mut state.borrow_mut(), app, worker);
                st = state.borrow_mut();
            } else {
                crate::sidecars::save_file(&mut st, c);
            }
            // The save failed (the disk gone under the frame, about to
            // follow its copy): the join still goes to the copy from
            // memory, so the copy has both edits and the row clears.
            if !st.work.sync.queued.contains_key(frame) && !st.work.sync.in_flight.contains(frame) {
                after_save(&mut st, c);
            }
        }
    }
    true
}

/// The frame on screen holds a sidecar that changed under it without
/// a save of its own: the panel shows it, developed again.
fn show_settled(state: &Rc<Shared>, app: &App, worker: &Rc<Worker>) {
    let mut st = state.borrow_mut();
    if st.culling.cull.is_some() {
        return;
    }
    crate::panel::history::take_current_settled(&mut st, app, worker);
}

/// A frame the window does not hold, whose archive copy is not behind
/// its file: read, compared and settled on a job, then written to the
/// copy. The frame goes through its own queue, so nothing else for it
/// runs beside this.
fn settle_on_disk(st: &mut State, frame: &Path, archive: &Path, copy: &Path, theirs: Sidecar) {
    let Some(lib) = st.views.index_reader.as_ref() else {
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
        xmp: st.window.xmp_sidecars,
        copies: vec![Copy {
            archive: archive.to_path_buf(),
            copy: copy.to_path_buf(),
        }],
    };
    queue(st, write);
}

/// The status line's word for what waits: "3 edits waiting for
/// Archive", one clause an archive, nothing while nothing waits. A
/// click on it is [`retry`]. The edits whose save here failed come
/// first ("2 edits not saved", `sidecars::note`): they are in memory
/// only, where the archive's are on this disk at least.
pub(crate) fn refresh_note(st: &State, app: &App) {
    let counts = st
        .views
        .index_reader
        .as_ref()
        .and_then(|l| l.pending_counts().ok())
        .unwrap_or_default();
    let note = crate::sidecars::note(st)
        .into_iter()
        .chain(counts.iter().map(|count| {
            let name = crate::roots::root_name(&st.views.library.roots, &count.archive);
            let n = count.total;
            let mut clause = tr!(
                "{n} edit waiting for {name}" | "{n} edits waiting for {name}" % n,
                name = name
            );
            if count.unknown > 0 {
                // Waiting on the user, not the archive: the log names
                // the frames when the note is clicked.
                let u = count.unknown;
                clause.push_str(&tr!("; {n} can't tell which copy is its own"
                    | "; {n} can't tell which copy are their own" % u));
            }
            clause
        }))
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
    if !st.window.write_sidecars {
        return;
    }
    let Some(c) = st.current else {
        return;
    };
    let Some(frame) = st.path_at(c).cloned() else {
        return;
    };
    if !crate::rows::is_offline(st, c)
        || st.work.sync.followed.contains_key(&frame)
        || st.work.sync.checking.contains(&frame)
        || st.work.sync.queued.contains_key(&frame)
        || st.work.sync.in_flight.contains(&frame)
        || !crate::sidecars::writable(st, c)
        || st.views.library.roots.archive_of(&frame).is_some()
    {
        return;
    }
    let Some(lib) = st.views.index_reader.as_ref() else {
        return;
    };
    let Some(paired) = pair(lib, &st.views.library.roots, &frame) else {
        return;
    };
    if lib.is_pending(&paired.hash).unwrap_or(true) {
        return;
    }
    let Some(target) = paired
        .copies
        .into_iter()
        .find(|c| !st.views.library.offline.contains(&c.archive))
    else {
        return;
    };
    if st.files.contains(&target.copy) {
        return;
    }
    let Some(index) = st.views.index_path.clone() else {
        return;
    };
    st.work.sync.checking.insert(frame.clone());
    let hash = paired.hash;
    let sidecar = st.sidecar(c).clone();
    let placement = st.window.placement;
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
        st.work.sync.checking.remove(&frame);
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
            let theirs = match Sidecar::read(&dest, &sidecar.start) {
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
    state: &Rc<Shared>,
    app: &App,
    worker: &Rc<Worker>,
    frame: &Path,
    target: &Copy,
    heard: Heard<Followable>,
) {
    let found = match heard {
        Heard::Aside => return,
        Heard::Lost => {
            state.borrow_mut().work.sync.checking.remove(frame);
            return;
        }
        Heard::Done { result, .. } => {
            state.borrow_mut().work.sync.checking.remove(frame);
            result
        }
    };
    match found {
        Followable::Ready | Followable::Written => {
            follow(&mut state.borrow_mut(), app, worker, frame, target)
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
fn follow(st: &mut State, app: &App, worker: &Worker, frame: &Path, target: &Copy) {
    let Some(c) = st.current else {
        return;
    };
    let Some(file) = st.file_at(c) else {
        return;
    };
    if st.files.get(file).map(Path::new) != Some(frame)
        || !crate::rows::is_offline(st, c)
        || st.files.contains(&target.copy)
        || st.work.sync.queued.contains_key(frame)
        || st.work.sync.in_flight.contains(frame)
    {
        return;
    }
    let local_root = crate::rows::offline_root(st, c);
    if st.views.panel_stand_in.as_ref() == Some(&frame.to_path_buf()) {
        st.views.panel_stand_in = Some(target.copy.clone());
    }
    if let Some(id) = st
        .views
        .index_reader
        .as_ref()
        .and_then(|l| l.by_path(&target.copy).ok().flatten())
        .map(|e| e.id)
        && let Some(slot) = st.views.index_ids.get_mut(file)
    {
        *slot = Some(id);
    }
    tracing::info!(
        "{}: its root is offline; following to {}",
        frame.display(),
        target.copy.display()
    );
    // The file's path: every cell of it follows.
    st.move_file(file, target.copy.clone());
    st.work.sync.followed.insert(
        target.copy.clone(),
        Followed {
            local: frame.to_path_buf(),
            archive: target.archive.clone(),
            homed: false,
        },
    );
    let where_ = crate::roots::root_name(&st.views.library.roots, &target.archive);
    let which = local_root
        .map(|r| crate::roots::root_name(&st.views.library.roots, &r))
        .unwrap_or_else(|| tr!("its root"));
    app.set_status(
        tr!(
            "{which} is offline: {} is open from its copy on {where_}",
            file_name(frame),
            which = which,
            where_ = where_
        )
        .into(),
    );
    // Held by the worker under its local path: opened again from the
    // copy (`browser::file_moved`).
    crate::panel::browser::reopen_moved(st, app, worker);
}

/// The window's callbacks.
pub(crate) fn install(app: &App, state: &Rc<Shared>) {
    let st = Rc::clone(state);
    let app_weak = app.as_weak();
    app.on_sync_retry(move || {
        if let Some(app) = app_weak.upgrade() {
            retry(&mut st.borrow_mut(), &app);
        }
    });
}

/// Where a job's end lands on the window's thread.
type Landing<R> = Arc<dyn Fn(&Rc<Shared>, &App, &Rc<Worker>, Heard<R>) + Send + std::marker::Sync>;

/// Run `job` off the window's thread with its beats watched for
/// `wait`, and land what it says on the window's thread. False when no
/// thread could be had.
#[cfg(not(test))]
fn send<R: Send + 'static>(
    app: &App,
    name: &str,
    wait: Duration,
    job: impl FnOnce(&Beat) -> R + Send + 'static,
    land: impl Fn(&Rc<Shared>, &App, &Rc<Worker>, Heard<R>) + Send + std::marker::Sync + 'static,
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
    land: impl Fn(&Rc<Shared>, &App, &Rc<Worker>, Heard<R>) + Send + std::marker::Sync + 'static,
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
pub(crate) mod tests;
