//! Many files' sidecars saved at once, off the window's thread: a
//! paste, a sync, or a preset over a selection, a rating over one. A
//! save syncs its file before the rename, which costs milliseconds a
//! file on a local disk and more on a share, and a few hundred of them
//! held the window for seconds.
//!
//! The window makes each save in memory (the sidecar advanced to its
//! new revision) and hands a copy of each to a thread of the batch's
//! own, which writes and syncs every file under a temporary name first
//! and then renames each into place: a file either lands whole or
//! keeps its old sidecar, whatever happens to the others. The window
//! marks the files saved (the index told, the pictures of the edit
//! kept, an archive's copy queued) when the renames are done, and
//! shows meanwhile what it showed before.
//!
//! A file saved again on the window's thread while its batch is out
//! (an undo, a rating, a second paste) takes it back from the batch
//! first ([`take_back`]): the batch then renames nothing over it, and
//! the newer save is the one on disk. A move or a delete of frames, and
//! the quit, wait for the batches out ([`finish`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use greycard_edit::Sidecar;
use greycard_edit::meta::Meta;
use greycard_edit_io::xmp;

use crate::State;

/// The batches out and the files they still hold.
#[derive(Default)]
pub(crate) struct Saves {
    next: u64,
    /// Each file a batch still holds, by its frame's path, and the
    /// batch that may rename it. A batch takes its files when it is
    /// sent and gives each up as it renames it; a save on the window's
    /// thread takes one back.
    claims: Arc<Mutex<HashMap<PathBuf, u64>>>,
    out: HashMap<u64, Out>,
}

struct Out {
    thread: Option<std::thread::JoinHandle<()>>,
    done: Arc<Mutex<Option<Vec<Done>>>>,
    /// Files written and files renamed so far: whether a batch is still
    /// moving, for a wait that gives up only on one that is not.
    moved: Arc<AtomicUsize>,
    sent: Instant,
    /// What the batch is, for a caller that has the landing say what
    /// reached the disk ([`save_for`]); none says only the failures.
    what: Option<&'static str>,
}

/// One file's save, as the window made it.
struct Item {
    file: usize,
    raw: PathBuf,
    sidecar: Sidecar,
    xmp: Option<XmpAsk>,
}

/// What the XMP beside a file is written from, read on the window's
/// thread from what it already knows.
struct XmpAsk {
    meta: Meta,
    quarters: u8,
    /// The camera's orientation tag when the window has it; read from
    /// the file on the batch's thread when not.
    tag: Option<greycard_core::raw::Orientation>,
    /// The camera's time as the window has it already; read from the
    /// file on the batch's thread when the XMP asks and it has not.
    taken: Option<String>,
}

#[derive(Debug)]
enum Outcome {
    /// Renamed into place, with the mark for the XMP written beside it.
    Saved(Option<xmp::Adopted>),
    /// Saved again on the window's thread before the rename: the
    /// batch's copy was dropped.
    TakenBack,
    Failed(String),
}

#[derive(Debug)]
struct Done {
    file: usize,
    raw: PathBuf,
    /// The save count the batch wrote: the window's sidecar takes the
    /// XMP's mark only while it is still that save.
    saved: u64,
    outcome: Outcome,
}

/// Save `files`' sidecars as one batch off the window's thread: those
/// the window does not write left alone, as a single save leaves
/// them, and those it follows on an archive's copy saved as they are
/// one at a time (in memory, the copy written by the archive write).
pub(crate) fn save(st: &mut State, files: &[usize]) {
    save_for(st, files, None);
}

/// [`save`], the landing saying in the log under `what` how many files
/// were written and naming those that were not.
pub(crate) fn save_for(st: &mut State, files: &[usize], what: Option<&'static str>) {
    let started = Instant::now();
    let Some((id, items)) = prepare(st, files) else {
        return;
    };
    let count = items.len();
    let saves = &mut st.work.saves;
    let done = Arc::new(Mutex::new(None));
    let moved = Arc::new(AtomicUsize::new(0));
    let (claims, slot, placement, counter) = (
        Arc::clone(&saves.claims),
        Arc::clone(&done),
        st.window.placement,
        Arc::clone(&moved),
    );
    // The items go to the thread once it is had, and stay here when
    // it is not.
    let (send, items_in) = std::sync::mpsc::channel::<Vec<Item>>();
    let job = move || {
        let Ok(items) = items_in.recv() else {
            return;
        };
        *slot.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(run(items, placement, id, &claims, &counter));
        #[cfg(not(test))]
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(state) = crate::STATE.with(|s| s.borrow().clone()) {
                land(&mut state.borrow_mut(), id);
            }
        });
    };
    let spawned = std::thread::Builder::new()
        .name("greycard sidecars".into())
        .spawn(job);
    let thread = match spawned {
        Ok(thread) => {
            let _ = send.send(items);
            Some(thread)
        }
        Err(e) => {
            // No thread to be had: written here, as before batches.
            tracing::warn!("sidecars: no thread for {count}, written on the window's: {e}");
            let outcome = run(items, placement, id, &saves.claims, &moved);
            *done.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            None
        }
    };
    let inline = thread.is_none();
    saves.out.insert(
        id,
        Out {
            thread,
            done,
            moved,
            sent: Instant::now(),
            what,
        },
    );
    if inline {
        land(st, id);
        return;
    }
    tracing::debug!(
        "sidecars: {count} sent to be saved, {:.1} ms on the window's thread",
        started.elapsed().as_secs_f64() * 1e3
    );
    #[cfg(test)]
    if !tests::HOLD.get() {
        finish(st, None);
    }
}

/// The batch's work, wherever it runs: every file written and synced,
/// then every one it still holds renamed. A panic in it fails the
/// batch's files that are left, which keep their old sidecars, and
/// gives up its claims, so the batch still lands.
fn run(
    items: Vec<Item>,
    placement: greycard_edit::Placement,
    id: u64,
    claims: &Mutex<HashMap<PathBuf, u64>>,
    moved: &AtomicUsize,
) -> Vec<Done> {
    let each: Vec<(usize, PathBuf, u64)> = items
        .iter()
        .map(|i| (i.file, i.raw.clone(), i.sidecar.saved))
        .collect();
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let staged = stage_all(items, placement, moved);
        commit_all(staged, placement, id, claims, moved)
    }));
    ran.unwrap_or_else(|_| {
        claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, b| *b != id);
        // The renames made before the panic are not known here: each
        // file is said failed, and one that did land is read as it is.
        each.into_iter()
            .map(|(file, raw, saved)| Done {
                file,
                raw,
                saved,
                outcome: Outcome::Failed("the batch's thread failed".into()),
            })
            .collect()
    })
}

/// Each file's save made in memory, as a single save makes it, and a
/// copy of it to be written; the batch's id, holding them all. None
/// when there is nothing to write.
fn prepare(st: &mut State, files: &[usize]) -> Option<(u64, Vec<Item>)> {
    let stamp = greycard_edit::sync::Stamp::now();
    let mut items = Vec::with_capacity(files.len());
    for &file in files {
        if !crate::sidecars::file_writable(st, file) {
            continue;
        }
        if crate::sync::followed(st, file) {
            crate::sidecars::save_file(st, file);
            continue;
        }
        let xmp = st.window.xmp_sidecars.then(|| xmp_ask(st, file));
        st.sidecars[file].advance(&stamp);
        items.push(Item {
            file,
            raw: st.files[file].clone(),
            sidecar: st.sidecars[file].clone(),
            xmp,
        });
    }
    if items.is_empty() {
        return None;
    }
    let saves = &mut st.work.saves;
    let id = saves.next;
    saves.next += 1;
    let mut claims = saves.claims.lock().unwrap_or_else(PoisonError::into_inner);
    for item in &items {
        claims.insert(item.raw.clone(), id);
    }
    drop(claims);
    Some((id, items))
}

/// The XMP's part of file `file`'s save, from what the window knows
/// with nothing read: the rest is read on the batch's thread.
fn xmp_ask(st: &State, file: usize) -> XmpAsk {
    let path = &st.files[file];
    XmpAsk {
        tag: st.develop.stances.get(path).map(|s| s.orientation),
        quarters: st.sidecars[file].turn,
        meta: st.sidecars[file].meta.clone(),
        taken: crate::panel::time_shift::camera_time_kept(st, file),
    }
}

type Staged = Result<(Option<xmp::Staged>, greycard_edit::Staged), String>;

/// The batch's first half: every file written and synced, none
/// renamed.
fn stage_all(
    items: Vec<Item>,
    placement: greycard_edit::Placement,
    moved: &AtomicUsize,
) -> Vec<(Item, Staged)> {
    items
        .into_iter()
        .map(|mut item| {
            let staged = stage(&mut item, placement);
            moved.fetch_add(1, Ordering::Relaxed);
            (item, staged)
        })
        .collect()
}

/// The batch's second half: every file renamed into place that the
/// batch still holds, each on its own, so one that fails keeps its old
/// sidecar and the others land.
fn commit_all(
    staged: Vec<(Item, Staged)>,
    placement: greycard_edit::Placement,
    id: u64,
    claims: &Mutex<HashMap<PathBuf, u64>>,
    moved: &AtomicUsize,
) -> Vec<Done> {
    staged
        .into_iter()
        .map(|(item, staged)| {
            let mut claims = claims.lock().unwrap_or_else(PoisonError::into_inner);
            let ours = claims.get(&item.raw) == Some(&id);
            if ours {
                claims.remove(&item.raw);
            }
            let outcome = match staged {
                // Dropped, its temporary files go with it.
                _ if !ours => Outcome::TakenBack,
                Err(e) => Outcome::Failed(e),
                // The XMP first, as a single save writes it, so the
                // sidecar never names one that is not there; the
                // window's own save waits for the pair at the claims.
                Ok((xmp, sidecar)) => match xmp.map(xmp::Staged::commit).transpose() {
                    Err(e) => Outcome::Failed(format!("xmp: {e}")),
                    Ok(mark) => match sidecar.commit() {
                        Ok(()) => Outcome::Saved(mark),
                        Err(e) => Outcome::Failed(e.to_string()),
                    },
                },
            };
            drop(claims);
            moved.fetch_add(1, Ordering::Relaxed);
            if matches!(outcome, Outcome::Saved(_)) {
                Sidecar::drop_other(&item.raw, placement);
            }
            Done {
                file: item.file,
                saved: item.sidecar.saved,
                raw: item.raw,
                outcome,
            }
        })
        .collect()
}

/// One file's XMP and sidecar written and synced, neither renamed. An
/// XMP that will not read or write costs the interop and nothing else,
/// as in a single save: the sidecar goes either way.
fn stage(item: &mut Item, placement: greycard_edit::Placement) -> Staged {
    let mut xmp_staged = None;
    if let Some(ask) = &item.xmp {
        let tag = ask.tag.or_else(|| {
            greycard_core::decode::stance_path(&item.raw)
                .map(|s| s.orientation)
                .inspect_err(|e| {
                    tracing::warn!(
                        "{}: how the frame stands: {e}",
                        crate::panel::browser::file_name(&item.raw)
                    )
                })
                .ok()
        });
        let turn = tag.map(|o| xmp::Turn::new(o, ask.quarters));
        // The camera's time only when the dates move, or a shift is
        // taken off: read then, as a single save reads it.
        let taken = || {
            ask.taken
                .clone()
                .or_else(|| greycard_library::export_record::camera_time_of(&item.raw))
        };
        match xmp::stage_dated(&item.raw, &ask.meta, turn, taken) {
            Ok(Some(staged)) => {
                item.sidecar.xmp = Some(staged.mark().clone());
                xmp_staged = Some(staged);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(
                "{}: xmp not written: {e}",
                crate::panel::browser::file_name(&item.raw)
            ),
        }
    }
    let sidecar = item
        .sidecar
        .stage_in(&item.raw, placement)
        .map_err(|e| e.to_string())?;
    Ok((xmp_staged, sidecar))
}

/// Batch `id`'s renames landed on the window's thread: each file saved
/// is marked so (its XMP's mark kept, the index told, its pictures
/// kept, an archive's copy queued), and each that failed said in the
/// log as a single save says it. Nothing when the batch has landed
/// already ([`finish`]) or is not done.
fn land(st: &mut State, id: u64) {
    let Some(out) = st.work.saves.out.get(&id) else {
        return;
    };
    let Some(done) = out
        .done
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    else {
        return;
    };
    let Some(mut out) = st.work.saves.out.remove(&id) else {
        return;
    };
    if let Some(thread) = out.thread.take() {
        let _ = thread.join();
    }
    let (mut saved, mut taken, mut failed) = (0, 0, 0);
    let mut not_written = Vec::new();
    for d in done {
        match d.outcome {
            Outcome::Saved(mark) => {
                saved += 1;
                // The list may have been replaced or sorted under the
                // batch: the file found by its path, and one no longer
                // in it saved as a file the window let go of is.
                let file = Some(d.file)
                    .filter(|&f| st.files.get(f) == Some(&d.raw))
                    .or_else(|| st.file_by_path(&d.raw));
                let Some(file) = file else {
                    if let Some(indexer) = &st.views.index {
                        indexer.file(d.raw.clone());
                    }
                    crate::sync::after_disk_save(st, &d.raw);
                    continue;
                };
                // The mark only while the sidecar is the save written: a
                // later save has its own.
                if mark.is_some() && st.sidecars[file].saved == d.saved {
                    st.sidecars[file].xmp = mark;
                }
                crate::library::sidecar_written(st, file);
            }
            Outcome::TakenBack => taken += 1,
            Outcome::Failed(e) => {
                failed += 1;
                let name = crate::panel::browser::file_name(&d.raw);
                tracing::warn!("{name}: sidecar not saved: {e}");
                not_written.push(name);
            }
        }
    }
    tracing::debug!(
        "sidecars: {saved} saved off the window's thread in {:.1} ms, {taken} saved again \
         before them, {failed} failed",
        out.sent.elapsed().as_secs_f64() * 1e3
    );
    if let Some(what) = out.what {
        tracing::info!("{what}: {saved} file(s) written");
        if !not_written.is_empty() {
            tracing::warn!("{what}: not written: {}", not_written.join(", "));
        }
    }
}

/// Take file `file` back from the batch that holds it, before the
/// window saves it itself: the batch renames nothing over it after.
/// A rename of it under way is waited for, so the window's write is
/// always the later one.
pub(crate) fn take_back(st: &State, file: usize) {
    if let Some(path) = st.files.get(file) {
        st.work
            .saves
            .claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(path);
    }
}

/// Wait for the batches out and land them: before frames are moved or
/// deleted, or a folder's sidecars are read again, which a late rename
/// would leave stale or put back where the frames were, and at the
/// quit, which would end their threads. With `stall`, a batch is given
/// up only when it has moved no file for that long (a share that
/// stopped answering), however long it takes while it moves; true
/// when every batch landed.
pub(crate) fn finish(st: &mut State, stall: Option<Duration>) -> bool {
    let ids: Vec<u64> = st.work.saves.out.keys().copied().collect();
    for id in ids {
        let Some(out) = st.work.saves.out.get_mut(&id) else {
            continue;
        };
        match (stall, out.thread.take()) {
            (_, None) => {}
            (None, Some(thread)) => {
                let _ = thread.join();
            }
            (Some(limit), Some(thread)) => {
                let mut seen = out.moved.load(Ordering::Relaxed);
                let mut since = Instant::now();
                while !thread.is_finished() && since.elapsed() < limit {
                    std::thread::sleep(Duration::from_millis(5));
                    let now = out.moved.load(Ordering::Relaxed);
                    if now != seen {
                        (seen, since) = (now, Instant::now());
                    }
                }
                if !thread.is_finished() {
                    out.thread = Some(thread);
                    continue;
                }
                let _ = thread.join();
            }
        }
        land(st, id);
    }
    let left = st.work.saves.out.len();
    if left > 0 {
        tracing::warn!("sidecars: {left} batch(es) still writing, left to finish alone");
    }
    left == 0
}

/// Whether a batch is out.
#[cfg(test)]
pub(crate) fn out(st: &State) -> bool {
    !st.work.saves.out.is_empty()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;
    use std::rc::Rc;

    use super::*;
    use crate::sidecars::write_sidecar;
    use crate::testing::{state_for, window};
    use crate::{App, Shared};
    use greycard_edit::{Edit, Placement};

    thread_local! {
        /// A batch sent is left out until the test lands it
        /// ([`super::finish`]); otherwise a test's batch lands as it
        /// is sent, there being no event loop to land it on.
        pub(crate) static HOLD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Three frames in a folder of their own, their sidecars written
    /// beside them at an exposure of `before`, and each given `after`
    /// in memory.
    fn three(name: &str, before: f32, after: f32) -> (App, Rc<Shared>, Vec<PathBuf>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("greycard-{name}-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..3)
            .map(|i| dir.join(format!("IMG_{i:04}.CR3")))
            .collect();
        let app = window(3);
        let (state, _worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            st.window.placement = Placement::Beside;
            for f in 0..3 {
                st.sidecars[f].record(exposed(before));
                write_sidecar(&mut st, f);
                st.sidecars[f].record(exposed(after));
            }
        }
        (app, state, files, dir)
    }

    fn exposed(ev: f32) -> Edit {
        let mut e = Edit::default();
        e.light.exposure = ev;
        e
    }

    fn on_disk(raw: &Path) -> f32 {
        Sidecar::load(raw)
            .expect("it reads")
            .expect("it is there")
            .current
            .light
            .exposure
    }

    /// Every name under `dir` that is a temporary file.
    fn temps(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("the folder reads")
            .map(|e| {
                e.expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    /// Every file is written and synced before any is renamed, and
    /// each rename is the file's own: one that fails keeps its old
    /// sidecar, and the others land.
    #[test]
    fn a_failed_rename_keeps_its_old_sidecar_and_the_others_land() {
        let (_app, state, files, dir) = three("batch-failed", 0.1, 0.9);
        let mut st = state.borrow_mut();
        let (id, items) = prepare(&mut st, &[0, 1, 2]).expect("three to write");
        let staged = stage_all(items, Placement::Beside, &AtomicUsize::new(0));
        // All three written, none in place yet.
        assert_eq!(temps(&dir).len(), 3);
        for f in &files {
            assert_eq!(on_disk(f), 0.1);
        }
        // The second's temporary file gone before its rename.
        let (_, second) = staged[1].1.as_ref().expect("staged");
        std::fs::remove_file(second.temp()).expect("removed");
        let done = commit_all(
            staged,
            Placement::Beside,
            id,
            &st.work.saves.claims,
            &AtomicUsize::new(0),
        );
        assert!(matches!(done[0].outcome, Outcome::Saved(None)));
        assert!(matches!(done[1].outcome, Outcome::Failed(_)));
        assert!(matches!(done[2].outcome, Outcome::Saved(None)));
        assert_eq!(on_disk(&files[0]), 0.9);
        assert_eq!(on_disk(&files[1]), 0.1);
        assert_eq!(on_disk(&files[2]), 0.9);
        assert!(temps(&dir).is_empty(), "{:?}", temps(&dir));
        assert!(st.work.saves.claims.lock().unwrap().is_empty());
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The files are marked saved on the window's thread only when the
    /// batch's renames have landed: until then the window holds what
    /// it held (here, no mark for the XMPs the batch writes).
    #[test]
    fn the_window_marks_a_batch_saved_only_when_its_renames_land() {
        let (_app, state, files, dir) = three("batch-lands", 0.0, 0.5);
        let mut st = state.borrow_mut();
        st.window.xmp_sidecars = true;
        for s in &mut st.sidecars[..3] {
            s.meta.rating = 3;
        }
        HOLD.set(true);
        save(&mut st, &[0, 1, 2]);
        HOLD.set(false);
        assert!(out(&st));
        for (f, s) in st.sidecars[..3].iter().enumerate() {
            assert_eq!(s.xmp, None, "{f}");
        }
        // The list sorted the other way under the batch: each file is
        // found where it is now.
        st.files.swap(0, 2);
        st.sidecars.swap(0, 2);
        assert!(finish(&mut st, None));
        assert!(!out(&st));
        for raw in &files {
            let f = st.file_by_path(raw).expect("in the list");
            let back = Sidecar::load(raw).unwrap().unwrap();
            assert_eq!(back.current.light.exposure, 0.5);
            assert_eq!(back.saved, st.sidecars[f].saved);
            let mark = st.sidecars[f].xmp.clone().expect("marked");
            assert_eq!(back.xmp, Some(mark.clone()));
            let xmp = std::fs::read(raw.with_extension("xmp")).expect("the XMP");
            assert_eq!(xmp::Adopted::of(&raw.with_extension("xmp"), &xmp), mark);
        }
        // Landed once: a second word of it does nothing.
        land(&mut st, 0);
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// An undo saved while the batch is out takes its frame back: the
    /// batch renames nothing over it, and the undo is what is on disk
    /// (with the paste's step still in its history, to be redone). The
    /// other frames land as pasted.
    #[test]
    fn an_undo_while_a_batch_is_out_is_the_save_that_stays() {
        let (_app, state, files, dir) = three("batch-undo", 0.2, 0.8);
        let mut st = state.borrow_mut();
        let (id, items) = prepare(&mut st, &[0, 1, 2]).expect("three to write");
        let staged = stage_all(items, Placement::Beside, &AtomicUsize::new(0));
        assert!(st.sidecars[1].undo());
        write_sidecar(&mut st, 1);
        assert_eq!(on_disk(&files[1]), 0.2);
        let done = commit_all(
            staged,
            Placement::Beside,
            id,
            &st.work.saves.claims,
            &AtomicUsize::new(0),
        );
        assert!(matches!(done[1].outcome, Outcome::TakenBack));
        assert_eq!(on_disk(&files[0]), 0.8);
        assert_eq!(on_disk(&files[1]), 0.2);
        assert_eq!(on_disk(&files[2]), 0.8);
        let back = Sidecar::load(&files[1]).unwrap().unwrap();
        assert_eq!(back.saved, st.sidecars[1].saved);
        assert!(temps(&dir).is_empty(), "{:?}", temps(&dir));
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A shift taken off frames whose time the window never read, as
    /// after a relaunch: the batch reads the camera's time from the
    /// file, so the XMP's dates go back to it rather than staying
    /// shifted with no shift to say so.
    #[test]
    fn a_shift_taken_off_in_a_batch_puts_the_cameras_time_back() {
        use greycard_library::fixture::{R5, write_frame};
        let dir =
            std::env::temp_dir().join(format!("greycard-batch-unshift-{}", std::process::id()));
        crate::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let files: Vec<PathBuf> = (0..2)
            .map(|i| dir.join(format!("IMG_{i:04}.tif")))
            .collect();
        let shifted = Meta {
            rating: 2,
            time_shift: 3600,
            ..Meta::default()
        };
        for (i, f) in files.iter().enumerate() {
            write_frame(f, &R5, i as u16 + 1);
            xmp::save_dated(f, &shifted, None, || Some(R5.taken.into())).expect("written");
            let text = std::fs::read_to_string(f.with_extension("xmp")).unwrap();
            assert!(text.contains("16:32:39"), "{text}");
        }
        let app = window(2);
        let (state, _worker) = state_for(&app, files.clone());
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.window.xmp_sidecars = true;
        st.window.placement = Placement::Beside;
        for s in &mut st.sidecars[..2] {
            s.meta.rating = 2;
        }
        save(&mut st, &[0, 1]);
        assert!(!out(&st));
        for f in &files {
            let text = std::fs::read_to_string(f.with_extension("xmp")).unwrap();
            assert!(text.contains("15:32:39"), "{text}");
            assert!(!text.contains("16:32:39"), "{text}");
            assert!(!text.contains("TimeShift"), "{text}");
        }
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }
}
