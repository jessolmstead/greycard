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
use crate::sidecars::Why;

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
    /// The file was saved by another writer since the window read it or
    /// last wrote it: what the check made of the two (the file taken,
    /// the meta joined, or the batch's with the kept file in its lineage:
    /// [`Checked`]) written in its place with the XMP from it, and to be
    /// the window's.
    ///
    /// [`Checked`]: greycard_edit::Checked
    Remade(Box<Sidecar>, greycard_edit::Checked),
    /// The file was saved by another writer since, and the batch's was
    /// behind it: nothing written, the file's to be the window's.
    Took(Box<Sidecar>),
    /// Saved again on the window's thread before the rename: the
    /// batch's copy was dropped.
    TakenBack,
    Failed(String, Why),
}

#[derive(Debug)]
struct Done {
    file: usize,
    raw: PathBuf,
    /// The save count the batch wrote: the window's sidecar takes the
    /// XMP's mark only while it is still that save.
    saved: u64,
    /// What the batch left on disk, as the window's sidecar is to know
    /// it: what the window's next save asks the disk about.
    known: Option<greycard_edit::OnDisk>,
    /// The sidecars that would not parse, set aside on the way, and
    /// other writers' files kept beside their frames.
    asides: greycard_edit::Asides,
    /// For a join or a file taken, the sidecar the batch started from:
    /// the window's takes the result only while it is still exactly
    /// that one.
    started: Option<Box<Sidecar>>,
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
    send_batch(st, files, what);
}

/// [`save`] waited for while it moves, and given up once it has moved
/// no file for `wait`: left to finish alone, and never waited for
/// again, so a share that stopped answering holds nothing after it
/// (a folder change, the quit). What it had not renamed keeps its old
/// sidecar.
pub(crate) fn save_bounded(st: &mut State, files: &[usize], wait: Duration) {
    let Some(id) = send_batch(st, files, None) else {
        return;
    };
    finish_one(st, id, Some(wait));
    if let Some(out) = st.work.saves.out.remove(&id) {
        tracing::warn!(
            "sidecars: a batch of {} moved nothing for {wait:?}; left to finish alone",
            files.len()
        );
        // Dropped, the handle lets the thread go.
        drop(out);
    }
}

/// [`save_for`]'s work: the batch's id, none when there was nothing to
/// write.
fn send_batch(st: &mut State, files: &[usize], what: Option<&'static str>) -> Option<u64> {
    let started = Instant::now();
    let (id, items) = prepare(st, files)?;
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
        return Some(id);
    }
    tracing::debug!(
        "sidecars: {count} sent to be saved, {:.1} ms on the window's thread",
        started.elapsed().as_secs_f64() * 1e3
    );
    #[cfg(test)]
    if !tests::HOLD.get() {
        finish(st, None);
    }
    Some(id)
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
                known: None,
                asides: Default::default(),
                started: None,
                outcome: Outcome::Failed("the batch's thread failed".into(), Why::Other),
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
        quarters: st.sidecars[file].xmp_turn(),
        meta: st.sidecars[file].meta.clone(),
        taken: crate::panel::time_shift::camera_time_kept(st, file),
    }
}

type Staged = Result<(Option<xmp::Staged>, greycard_edit::Staged), (String, Why)>;

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
        .map(|(mut item, staged)| {
            let saved = item.sidecar.saved;
            let mut claims = claims.lock().unwrap_or_else(PoisonError::into_inner);
            let ours = claims.get(&item.raw) == Some(&id);
            if ours {
                claims.remove(&item.raw);
            }
            let committed = match staged {
                // Dropped, its temporary files go with it.
                _ if !ours => Committed::of(Outcome::TakenBack),
                Err((e, why)) => Committed::of(Outcome::Failed(e, why)),
                Ok(staged) => checked_commit(&mut item, staged, placement),
            };
            drop(claims);
            moved.fetch_add(1, Ordering::Relaxed);
            let Committed {
                outcome,
                bytes,
                stat,
                asides,
                started,
            } = committed;
            let known = match (&outcome, bytes) {
                (Outcome::Saved(_) | Outcome::Remade(..), Some(bytes)) => {
                    Sidecar::drop_other(&item.raw, placement);
                    item.sidecar.remember_written(stat, bytes);
                    item.sidecar.on_disk.clone()
                }
                _ => None,
            };
            Done {
                file: item.file,
                saved,
                known,
                asides,
                started,
                raw: item.raw,
                outcome,
            }
        })
        .collect()
}

/// What one file's commit did: the outcome, the length and hash of the
/// sidecar written, the files set aside for not parsing (whether the
/// save then landed or not), and for a join or a file taken the sidecar
/// the batch started from.
struct Committed {
    outcome: Outcome,
    bytes: Option<greycard_edit::Bytes>,
    /// The sidecar's staged file as a stat found it before the rename.
    stat: Option<greycard_edit::FileStat>,
    asides: greycard_edit::Asides,
    started: Option<Box<Sidecar>>,
}

impl Committed {
    fn of(outcome: Outcome) -> Self {
        Self {
            outcome,
            bytes: None,
            stat: None,
            asides: Default::default(),
            started: None,
        }
    }
}

/// One file's commit, the files on disk asked first whether they are
/// still the ones the window read or last wrote, as a single save asks
/// ([`Sidecar::save_checked`]), right before the rename. When they are,
/// or another writer's save is behind the batch's, `staged` goes into
/// place; when the batch's has nothing of its own, the file's is taken
/// and nothing written (but the XMP from it, when XMPs are written, and
/// the sidecar with its mark); otherwise what the check made (the meta
/// joined, or the batch's with the other's file kept beside the frame)
/// is written with an XMP from it. A file from a newer build is never
/// written over.
fn checked_commit(
    item: &mut Item,
    staged: (Option<xmp::Staged>, greycard_edit::Staged),
    placement: greycard_edit::Placement,
) -> Committed {
    use greycard_edit::Checked;
    let stamp = greycard_edit::sync::Stamp::now();
    let (reconciled, asides) = item.sidecar.reconciled(&item.raw, placement, &stamp);
    let mut done = Committed {
        asides,
        ..Committed::of(Outcome::TakenBack)
    };
    let (checked, kept) = match reconciled {
        Ok(r) => r,
        Err(e) => {
            done.outcome = Outcome::Failed(e.to_string(), Why::of(&e));
            return done;
        }
    };
    match (checked, kept) {
        (Checked::Took, Some(theirs)) if item.xmp.is_none() => {
            done.started = Some(Box::new(item.sidecar.clone()));
            done.outcome = Outcome::Took(Box::new(theirs));
        }
        // Taken, joined, or ours with the kept file's revision in its
        // lineage: staged again from what is to be written.
        (Checked::Took | Checked::Joined | Checked::Kept, Some(mut theirs)) => {
            drop(staged);
            if checked == Checked::Took {
                theirs.advance(&stamp);
            }
            if let Some(ask) = &mut item.xmp {
                ask.meta = theirs.meta.clone();
                ask.quarters = theirs.xmp_turn();
            }
            done.started = Some(Box::new(std::mem::replace(&mut item.sidecar, theirs)));
            // The XMP's mark is the sidecar's own, set as it is staged.
            done.outcome = match stage(item, placement) {
                Err((e, why)) => Outcome::Failed(e, why),
                Ok(staged) => {
                    let (bytes, stat) = (staged.1.bytes(), staged.1.stat().ok());
                    match commit(staged) {
                        Outcome::Saved(_) => {
                            (done.bytes, done.stat) = (bytes, stat);
                            Outcome::Remade(Box::new(item.sidecar.clone()), checked)
                        }
                        failed => failed,
                    }
                }
            };
        }
        // Ours written: the files were as known, held only what ours
        // knew, or were behind it.
        (_, kept) => {
            if let Some(kept) = kept {
                item.sidecar = kept;
            }
            (done.bytes, done.stat) = (staged.1.bytes(), staged.1.stat().ok());
            done.outcome = commit(staged);
        }
    }
    done
}

/// The XMP into place first, as a single save writes it, so the
/// sidecar never names one that is not there; the window's own save
/// waits for the pair at the claims. An XMP that will not go into place
/// keeps the sidecar from it too: the edit is not saved.
fn commit((xmp, sidecar): (Option<xmp::Staged>, greycard_edit::Staged)) -> Outcome {
    match xmp.map(xmp::Staged::commit).transpose() {
        Err(e) => Outcome::Failed(format!("xmp: {e}"), Why::of_io(e.kind())),
        Ok(mark) => match sidecar.commit() {
            Ok(()) => Outcome::Saved(mark),
            Err(e) => Outcome::Failed(e.to_string(), Why::of_io(e.kind())),
        },
    }
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
        .map_err(|e| (e.to_string(), Why::of(&e)))?;
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
    // Told as one line once every file is seen.
    let mut failures = Vec::new();
    let (mut bad, mut kept) = (Vec::new(), Vec::new());
    for d in done {
        if matches!(d.outcome, Outcome::Failed(..)) {
            crate::sidecars::log_asides(&d.raw, &d.asides);
        } else {
            bad.extend(d.asides.bad.iter().map(|to| (d.raw.clone(), to.clone())));
            kept.extend(d.asides.kept.iter().map(|to| (d.raw.clone(), to.clone())));
        }
        // The list may have been replaced or sorted under the batch:
        // the file found by its path.
        let file = Some(d.file)
            .filter(|&f| st.files.get(f) == Some(&d.raw))
            .or_else(|| st.file_by_path(&d.raw));
        // Whether this is still the frame's latest save: one made on
        // the window's thread since has said for itself whether the
        // edit is on disk, and this one says nothing over it.
        let latest = file.is_some_and(|f| st.sidecars[f].saved == d.saved);
        let remade = match &d.outcome {
            Outcome::Remade(_, checked) => Some(*checked),
            _ => None,
        };
        match d.outcome {
            Outcome::Saved(mark) => {
                saved += 1;
                if latest {
                    crate::sidecars::landed(st, &d.raw);
                }
                // One no longer in the list saved as a file the window
                // let go of is.
                let Some(file) = file else {
                    if let Some(indexer) = &st.views.index {
                        indexer.file(d.raw.clone());
                    }
                    crate::sync::after_disk_save(st, &d.raw);
                    continue;
                };
                // The mark, and the file as written, only while the
                // sidecar is the save written: a later save has its own.
                if latest {
                    if mark.is_some() {
                        st.sidecars[file].xmp = mark;
                    }
                    st.sidecars[file].on_disk = d.known;
                }
                crate::library::sidecar_written(st, file);
            }
            Outcome::Remade(sidecar, _) | Outcome::Took(sidecar) => {
                saved += 1;
                tracing::info!(
                    "{}: its sidecar was saved by another writer since it was read; {}",
                    crate::panel::browser::file_name(&d.raw),
                    match remade {
                        Some(greycard_edit::Checked::Joined) => "the meta joined with it",
                        Some(greycard_edit::Checked::Kept) => {
                            "that file kept beside the frame, and the edit here written"
                        }
                        _ => "behind it, and taken from it",
                    }
                );
                // A save made on the window's thread since, or a change
                // made there after the batch started, meets the file for
                // itself at the next save, and joins what is there.
                let exact = |f: usize| {
                    d.started.as_ref().is_some_and(|started| {
                        let mut started = started.clone();
                        // The XMP's mark, set as the batch staged.
                        started.xmp = st.sidecars[f].xmp.clone();
                        *started == st.sidecars[f]
                    })
                };
                let Some(file) = file.filter(|&f| latest && exact(f)) else {
                    if file.is_none() {
                        if let Some(indexer) = &st.views.index {
                            indexer.file(d.raw.clone());
                        }
                        crate::sync::after_disk_save(st, &d.raw);
                    }
                    continue;
                };
                crate::sidecars::landed(st, &d.raw);
                let mut sidecar = *sidecar;
                if d.known.is_some() {
                    sidecar.on_disk = d.known;
                }
                st.sidecars[file] = sidecar;
                crate::sidecars::taken_from_disk(st, file);
                crate::library::sidecar_written(st, file);
            }
            Outcome::TakenBack => taken += 1,
            Outcome::Failed(e, why) => {
                failed += 1;
                // A newer build's file refused it: the window's copy is
                // read-only from now, as the file is, its edit kept
                // beside the frame by the check (§724).
                if why == Why::Newer
                    && let Some(f) = st.file_by_path(&d.raw)
                    && let Ok(Some(disk)) = greycard_edit::Sidecar::load_as_file(&d.raw)
                    && disk.newer.is_some()
                {
                    st.sidecars[f].newer = disk.newer;
                }
                let name = crate::panel::browser::file_name(&d.raw);
                tracing::warn!("{name}: sidecar not saved: {e}");
                not_written.push(name);
                if latest {
                    failures.push((d.raw, why));
                }
            }
        }
    }
    // Not over a failure's word: the log has them then.
    if failures.is_empty() {
        crate::sidecars::said_asides(st, &bad, &kept);
    } else {
        for (raw, to) in bad.iter().chain(&kept) {
            tracing::warn!(
                "{}: its sidecar was put aside as {}",
                crate::panel::browser::file_name(raw),
                to.display()
            );
        }
    }
    crate::sidecars::failed_all(st, &failures);
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
        finish_one(st, id, stall);
    }
    let left = st.work.saves.out.len();
    if left > 0 {
        tracing::warn!("sidecars: {left} batch(es) still writing, left to finish alone");
    }
    left == 0
}

/// [`finish`] of batch `id`: left out when `stall` gives it up.
fn finish_one(st: &mut State, id: u64, stall: Option<Duration>) {
    let Some(out) = st.work.saves.out.get_mut(&id) else {
        return;
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
                return;
            }
            let _ = thread.join();
        }
    }
    land(st, id);
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

    /// Wait for every batch out to have done its work on its thread,
    /// none of them landed.
    pub(crate) fn wait_moved(st: &State) {
        while st
            .work
            .saves
            .out
            .values()
            .any(|o| o.thread.as_ref().is_some_and(|t| !t.is_finished()))
        {
            std::thread::sleep(Duration::from_millis(5));
        }
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
        Sidecar::load_as_file(raw)
            .expect("it reads")
            .expect("it is there")
            .current()
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
        assert!(matches!(done[1].outcome, Outcome::Failed(..)));
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
            let back = Sidecar::load_as_file(raw).unwrap().unwrap();
            assert_eq!(back.current().light.exposure, 0.5);
            assert_eq!(back.saved, st.sidecars[f].saved);
            let mark = st.sidecars[f].xmp.clone().expect("marked");
            assert_eq!(back.xmp, Some(mark.clone()));
            let xmp = std::fs::read(raw.with_extension("xmp")).expect("the XMP");
            assert_eq!(xmp::Adopted::of(&raw.with_extension("xmp"), &xmp), mark);
            // The mark comes after the revision, and is not in it: the
            // window's copy and the file are both as their revision left
            // them, as a join with an archive's copy asks (§724).
            assert!(st.sidecars[f].at_its_revision(), "the window's");
            assert!(back.at_its_revision(), "the file's");
        }
        // Landed once: a second word of it does nothing.
        land(&mut st, 0);
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A newer greycard saves a frame while a batch is out for it: the
    /// batch writes nothing over it and removes no copy of it, the
    /// window's edit is kept beside the frame, and the window's copy is
    /// read-only from then (§724).
    #[test]
    fn a_batch_over_a_newer_build_s_save_writes_nothing_and_goes_read_only() {
        let (_app, state, files, dir) = three("batch-newer", 0.0, 0.5);
        let mut st = state.borrow_mut();
        let newer = r#"{"floors": {"read": 10, "write": 11}, "current": {"version": 11, "light": {"exposure": -1.0}}, "saved": 50}"#;
        let path = Sidecar::path_for(&files[0]);
        std::fs::write(&path, newer).unwrap();
        HOLD.set(true);
        save(&mut st, &[0, 1, 2]);
        HOLD.set(false);
        assert!(finish(&mut st, None));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        assert!(st.sidecars[0].read_only());
        assert_eq!(on_disk(&files[1]), 0.5, "the others land");
        let kept = path.with_extension("gcd.other");
        assert_eq!(
            Sidecar::read_as_file(&kept)
                .unwrap()
                .current()
                .light
                .exposure,
            0.5,
            "the window's edit, beside it"
        );
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
        let back = Sidecar::load_as_file(&files[1]).unwrap().unwrap();
        assert_eq!(back.saved, st.sidecars[1].saved);
        assert!(temps(&dir).is_empty(), "{:?}", temps(&dir));
        drop(st);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A batch that failed, landing after a newer save of one of its
    /// frames on the window's thread landed: the newer save is what is
    /// on disk, and the batch's failure says nothing over it.
    #[test]
    fn a_stale_batch_failure_says_nothing_over_a_newer_save() {
        let (app, state, dir, missing) = crate::sidecars::tests::unwritable("batch-stale", 2);
        let mut st = state.borrow_mut();
        HOLD.set(true);
        save(&mut st, &[0, 1]);
        HOLD.set(false);
        // The batch's thread done, its failures not landed.
        wait_moved(&st);
        std::fs::create_dir_all(&missing).expect("the folder");
        write_sidecar(&mut st, 0);
        assert!(finish(&mut st, None));
        assert_eq!(app.get_sync_note(), "1 edit not saved");
        assert!(!st.work.unsaved.edits.contains(&st.files[0]));
        assert!(st.work.unsaved.edits.contains(&st.files[1]));
        drop(st);
        crate::testing::remove_scratch(state, &dir);
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
