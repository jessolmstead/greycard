//! The thumbnails' own threads. A folder's pictures are made here, a
//! few at a time, beside the worker rather than behind it: a cold
//! folder's thumbnails fill in parallel and never queue behind a
//! develop or an export.
//!
//! The order is the worker's queue's as it was: the frames the strip
//! or the grid shows first, the rest outward from them, and a
//! re-order applies to whatever has not been started. A file is never
//! made by two threads at once — a second ask for one in hand waits
//! for the first to finish, and an ask for one already waiting is
//! dropped — and a folder change drops what is waiting and throws
//! away what comes back from the old folder's numbering.
//!
//! While the pool is held (for a develop) nothing is made, but every
//! thread still looks pictures up in the cache: a hit is a tenth of a
//! millisecond and takes nothing from the develop, so a warm folder's
//! strip fills at once whatever is running.
//!
//! Behind the thumbnails, and only when no thumbnail is waiting to be
//! begun, each frame whose thumbnail found no local preview kept has
//! one made (`previews`): never while the pool is held, and in the
//! thumbnails' order, the frames shown first. Behind those again, each
//! frame whose thumbnail found its edit's pictures not kept has them
//! made by the reduced develop (`reduced`): the lowest thing the pool
//! does, never while it is held, the frames shown first, and a thread
//! always left for a thumbnail; one at a time while a frame is open in
//! the editor and two while culling or browsing (`reduced::at_once`),
//! and a frame under a root on a network mount alone.
//!
//! What the threads finish goes to the window one call each, or
//! through an [`Outbox`] ([`Pool::batched`], the editor's). A warm
//! folder of twenty thousand is twenty thousand hits in a few
//! seconds; at one call on the window's event loop each, and winit
//! taking in every call already queued before it draws, the window
//! stood blank until the last was in. Batched, the pool calls once
//! for as many as pile up before the window takes them, and the
//! window takes them a slice at a time, the files it shows first.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use greycard_library::thumbs::Thumb;

use crate::versions::Key;
use crate::worker::{Deliver, Outcome, THUMB_WIDTH, order_thumbnails, panic_message};

/// How a picture is made: a frame (a file, and the version of it whose
/// edit's picture is wanted) and the long edge asked for, to the
/// picture and whether it came from the cache.
pub(crate) type Make = Arc<MakeFn>;
pub(crate) type MakeFn = dyn Fn(&Key, u32) -> anyhow::Result<(Thumb, bool)> + Send + Sync;

/// How a picture is looked up in the cache alone, without making it:
/// the picture, or `None` for a miss.
pub(crate) type Lookup = Arc<LookupFn>;
pub(crate) type LookupFn = dyn Fn(&Key, u32) -> Option<Thumb> + Send + Sync;

/// The local previews behind the thumbnails: whether a file whose
/// thumbnail was just delivered is owed one (a question of memory,
/// asked on every delivery), and the making of it.
#[derive(Clone)]
pub(crate) struct PreviewHooks {
    pub(crate) owes: Arc<OwesFn>,
    pub(crate) make: Arc<MakePreviewFn>,
}

pub(crate) type OwesFn = dyn Fn(&Path) -> bool + Send + Sync;
pub(crate) type MakePreviewFn = dyn Fn(&Path) -> anyhow::Result<()> + Send + Sync;

/// The edit's pictures behind the previews: whether a frame whose
/// thumbnail was just delivered wants them made (held as edited, its
/// pictures not kept under its key), and the making of them, which
/// says whether any were kept. A making that kept them has the window
/// told (`Outcome::EditedKept`). `at_once` says how many may be in hand
/// now, asked as each is begun; `alone` whether a frame's making runs
/// with no other beside it. A frame is a file and a version of it: each
/// version's edit has pictures of its own.
#[derive(Clone)]
pub(crate) struct EditHooks {
    pub(crate) wants: Arc<WantsFn>,
    pub(crate) make: Arc<MakeEditFn>,
    pub(crate) at_once: Arc<AtOnceFn>,
    pub(crate) alone: Arc<WantsFn>,
}

pub(crate) type WantsFn = dyn Fn(&Key) -> bool + Send + Sync;
/// How many edits may be in hand now, and, when that holds one back
/// only for a while (`reduced::begin`), when to look again.
pub(crate) type AtOnceFn = dyn Fn() -> (usize, Option<Duration>) + Send + Sync;
pub(crate) type MakeEditFn = dyn Fn(&Key) -> Result<bool, crate::reduced::Why> + Send + Sync;

/// How many threads make thumbnails on a machine of `cores`: half of
/// them, at least two and at most eight. The develop runs on rayon's
/// pool over every core, so half leaves it room; past eight the
/// preview decodes wait on the disk more than on each other, and each
/// thread holds a full-frame preview while it downscales one (134 MB
/// for an R5 II's 8192 by 5464).
pub(crate) fn threads_for(cores: usize) -> usize {
    (cores / 2).clamp(2, 8)
}

/// The pool's size on this machine: `threads_for` the cores, or
/// `GREYCARD_THUMB_THREADS` when that is set, for measuring.
pub(crate) fn default_threads() -> usize {
    std::env::var("GREYCARD_THUMB_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| threads_for(std::thread::available_parallelism().map_or(2, |n| n.get())))
}

/// What the threads have finished and the window has not yet taken
/// in: the files the window shows apart from the rest, so it takes
/// them first.
#[derive(Default)]
pub(crate) struct Outbox {
    inner: Mutex<Waiting>,
}

#[derive(Default)]
struct Waiting {
    /// Finished, for files in `shown`, in the order finished.
    shown_done: VecDeque<Outcome>,
    /// Finished, for the rest, in the order finished.
    rest_done: VecDeque<Outcome>,
    /// The files on screen, as the window last said (`Pool::want`).
    shown: Option<(usize, usize)>,
    /// Whether the window has been told there is something here and
    /// has not found it empty since: while it has, the threads add to
    /// what is waiting and say nothing more.
    told: bool,
}

/// The file an outcome of the pool's is for.
fn file_of(outcome: &Outcome) -> Option<usize> {
    match outcome {
        Outcome::Thumbnail { index, .. } | Outcome::NoThumbnail { index, .. } => Some(*index),
        _ => None,
    }
}

impl Waiting {
    fn on_screen(&self, outcome: &Outcome) -> bool {
        file_of(outcome).is_some_and(|i| {
            self.shown
                .is_some_and(|(first, last)| (first..=last).contains(&i))
        })
    }

    fn is_empty(&self) -> bool {
        self.shown_done.is_empty() && self.rest_done.is_empty()
    }
}

impl Outbox {
    fn lock(&self) -> std::sync::MutexGuard<'_, Waiting> {
        self.inner.lock().expect("thumbnail outbox")
    }

    /// Put one in, and say whether the window needs telling: only
    /// when it has not been told since it last found the outbox
    /// empty.
    fn put(&self, outcome: Outcome) -> bool {
        let mut w = self.lock();
        if w.on_screen(&outcome) {
            w.shown_done.push_back(outcome);
        } else {
            w.rest_done.push_back(outcome);
        }
        !std::mem::replace(&mut w.told, true)
    }

    /// The window shows `first..=last` now: what is waiting for those
    /// files goes ahead of the rest.
    fn show(&self, first: usize, last: usize) {
        let mut w = self.lock();
        if w.shown == Some((first, last)) {
            return;
        }
        w.shown = Some((first, last));
        let waiting: Vec<Outcome> = {
            let w = &mut *w;
            w.shown_done
                .drain(..)
                .chain(w.rest_done.drain(..))
                .collect()
        };
        for outcome in waiting {
            if w.on_screen(&outcome) {
                w.shown_done.push_back(outcome);
            } else {
                w.rest_done.push_back(outcome);
            }
        }
    }

    /// Another list: what is waiting was the old one's.
    fn clear(&self) {
        let mut w = self.lock();
        w.shown_done.clear();
        w.rest_done.clear();
    }
}

/// The window's handle on the outbox, which
/// [`Outcome::Thumbnails`] carries.
#[derive(Clone)]
pub struct Batch(Arc<Outbox>);

impl Batch {
    /// A batch of these, as the threads would leave it, with the
    /// window told.
    #[cfg(test)]
    pub(crate) fn of(outcomes: Vec<Outcome>) -> Self {
        let outbox = Outbox::default();
        for outcome in outcomes {
            outbox.put(outcome);
        }
        Self(Arc::new(outbox))
    }

    /// Up to `n` of what is waiting: the files on screen first, then
    /// the rest, each in the order they were finished; and whether any
    /// are left. When none are, the next one finished tells the window
    /// again.
    pub(crate) fn take(&self, n: usize) -> (Vec<Outcome>, bool) {
        let mut w = self.0.lock();
        let from_shown = n.min(w.shown_done.len());
        let mut taken: Vec<Outcome> = w.shown_done.drain(..from_shown).collect();
        let from_rest = (n - taken.len()).min(w.rest_done.len());
        taken.extend(w.rest_done.drain(..from_rest));
        let left = !w.is_empty();
        if !left {
            w.told = false;
        }
        (taken, left)
    }
}

#[derive(Default)]
struct Pending {
    /// What is waiting, in the order it is wanted: each the row it is
    /// for and the frame.
    jobs: VecDeque<(usize, Key)>,
    /// The range the waiting ones are ordered for, so the strip can
    /// report where it stands as often as it likes. Cleared when one
    /// is pushed, since that is a new folder's list or a step up.
    wanted: Option<(usize, usize)>,
    /// The local previews waiting to be made, in the order wanted:
    /// begun only when no thumbnail can be.
    previews: VecDeque<(usize, PathBuf)>,
    /// The range the waiting previews are ordered for; cleared when
    /// one is added.
    previews_wanted: Option<(usize, usize)>,
    /// The frames whose edit's pictures are waiting to be made, in the
    /// order wanted: begun only when no thumbnail and no preview can
    /// be.
    edits: VecDeque<(usize, Key)>,
    /// The same frames, to be asked of without a walk of the queue.
    edits_queued: HashSet<Key>,
    /// The range the waiting edits are ordered for; cleared when one
    /// is added.
    edits_wanted: Option<(usize, usize)>,
    /// How many edits are being made now, and whether one of them runs
    /// alone.
    edits_busy: usize,
    edits_alone: bool,
    /// How many times a thread has looked at all of the above and found
    /// nothing to begin, before it waits: what a test reads to know the
    /// pool has declined, rather than not yet looked.
    #[cfg(test)]
    looked: u64,
    /// The long edge pictures are made at now; `THUMB_WIDTH` until
    /// the grid asks otherwise. Read as each is begun.
    size: Option<u32>,
    /// Raised by a folder change: a picture begun under an older one
    /// is not delivered.
    epoch: u64,
    /// The frames in hand, one entry a thread at work, looking up or
    /// making (a preview under its file's own frame).
    making: Vec<Key>,
    /// How many of those are being made rather than looked up.
    busy: usize,
    /// Frames looked up while the pool was held and not found: they
    /// wait to be made, and are not looked up again meanwhile.
    missed: std::collections::HashSet<Key>,
    /// How many may be made at once now.
    limit: usize,
    /// Held for the first develop of a folder opened in the loupe:
    /// nothing is begun until it is let go.
    held: bool,
    /// Whether the threads have been started: not until the first
    /// thumbnail is asked for.
    started: bool,
    stopping: bool,
}

struct Shared {
    pending: Mutex<Pending>,
    cv: Condvar,
}

/// The pool: `threads` threads, started with the first thumbnail
/// asked for, each making one picture at a time.
pub(crate) struct Pool {
    shared: Arc<Shared>,
    threads: usize,
    lookup: Option<Lookup>,
    make: Make,
    deliver: Deliver,
    /// Where the threads put what they finish, when the window takes
    /// it in batches.
    outbox: Option<Arc<Outbox>>,
    /// The local previews made behind the thumbnails, if any.
    previews: Option<PreviewHooks>,
    /// The edit's pictures made behind the previews, if any.
    edits: Option<EditHooks>,
}

impl Pool {
    /// A pool with nothing to look up in: held, it does nothing.
    #[cfg(test)]
    pub(crate) fn new(threads: usize, make: Make, deliver: Deliver) -> Self {
        Self::build(threads, None, make, deliver)
    }

    /// A pool that looks pictures up with `lookup` while it is held,
    /// and makes them (a lookup included) with `make` otherwise.
    pub(crate) fn with_lookup(
        threads: usize,
        lookup: Lookup,
        make: Make,
        deliver: Deliver,
    ) -> Self {
        Self::build(threads, Some(lookup), make, deliver)
    }

    fn build(threads: usize, lookup: Option<Lookup>, make: Make, deliver: Deliver) -> Self {
        let threads = threads.max(1);
        Self {
            shared: Arc::new(Shared {
                pending: Mutex::new(Pending {
                    limit: threads,
                    ..Pending::default()
                }),
                cv: Condvar::new(),
            }),
            threads,
            lookup,
            make,
            deliver,
            outbox: None,
            previews: None,
            edits: None,
        }
    }

    /// Make the edit's pictures behind the previews for each file
    /// delivered that `hooks` says wants them. Before the first push.
    pub(crate) fn with_edits(mut self, hooks: EditHooks) -> Self {
        self.edits = Some(hooks);
        self
    }

    /// Make a local preview behind each thumbnail delivered whose file
    /// `hooks` says is owed one. Before the first push.
    pub(crate) fn with_previews(mut self, hooks: PreviewHooks) -> Self {
        self.previews = Some(hooks);
        self
    }

    /// Deliver through an outbox: one [`Outcome::Thumbnails`] for as
    /// many as are finished before the window takes them, rather than
    /// a call each. Before the first push, which starts the threads.
    pub(crate) fn batched(mut self) -> Self {
        self.outbox = Some(Arc::new(Outbox::default()));
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.shared.pending.lock().expect("thumbnail pool")
    }

    /// Make `frame`'s picture, as cell `index` of the list. An ask for
    /// one already waiting is dropped; the waiting one is made at
    /// whatever size is wanted when it is begun anyway.
    pub(crate) fn push(&self, index: usize, frame: Key) {
        let mut q = self.lock();
        q.wanted = None;
        if !q.jobs.iter().any(|(i, p)| *i == index && *p == frame) {
            q.jobs.push_back((index, frame));
        }
        if !q.started {
            q.started = true;
            for n in 0..self.threads {
                let (shared, lookup, make, deliver, outbox, previews, edits) = (
                    self.shared.clone(),
                    self.lookup.clone(),
                    self.make.clone(),
                    self.deliver.clone(),
                    self.outbox.clone(),
                    self.previews.clone(),
                    self.edits.clone(),
                );
                // Straight to the window, or into the outbox with the
                // window told only when it is not already.
                let send = move |outcome: Outcome| match &outbox {
                    Some(outbox) => {
                        if outbox.put(outcome) {
                            deliver(Outcome::Thumbnails(Batch(outbox.clone())));
                        }
                    }
                    None => deliver(outcome),
                };
                let spawned = std::thread::Builder::new()
                    .name(format!("greycard thumbnails {n}"))
                    .spawn(move || {
                        serve(
                            &shared,
                            lookup.as_deref(),
                            &*make,
                            previews.as_ref(),
                            edits.as_ref(),
                            &send,
                        )
                    });
                if let Err(e) = spawned {
                    tracing::warn!("thumbnail thread {n} not started: {e}");
                }
            }
        }
        self.shared.cv.notify_one();
    }

    /// Make all of these, in place of nothing: for a list asked for
    /// again after [`Pool::forget`], whose queue is empty, so the check
    /// `push` makes for one waiting already (the whole queue, each
    /// time, which is quadratic over a list of twenty thousand) is
    /// skipped.
    pub(crate) fn push_all(&self, list: Vec<(usize, Key)>) {
        let mut list = list.into_iter();
        let Some((index, frame)) = list.next() else {
            return;
        };
        // The first through `push`, which starts the threads.
        self.push(index, frame);
        let mut q = self.lock();
        q.jobs.extend(list);
        drop(q);
        self.shared.cv.notify_all();
    }

    /// Make `first..=last` before the rest, the rest outward from
    /// that range; applies to whatever has not been begun.
    pub(crate) fn want(&self, first: usize, last: usize) {
        // What is finished and waiting for the window is taken in
        // with that range first as well.
        if let Some(outbox) = &self.outbox {
            outbox.show(first, last);
        }
        let mut q = self.lock();
        if q.previews.len() >= 2 && q.previews_wanted != Some((first, last)) {
            q.previews_wanted = Some((first, last));
            order_thumbnails(q.previews.make_contiguous(), first, last);
        }
        if q.edits.len() >= 2 && q.edits_wanted != Some((first, last)) {
            q.edits_wanted = Some((first, last));
            order_thumbnails(q.edits.make_contiguous(), first, last);
        }
        if q.jobs.len() < 2 || q.wanted == Some((first, last)) {
            return;
        }
        q.wanted = Some((first, last));
        order_thumbnails(q.jobs.make_contiguous(), first, last);
    }

    /// The long edge to make pictures at from the next one begun.
    pub(crate) fn set_size(&self, size: u32) {
        self.lock().size = Some(size);
    }

    /// The long edge pictures are made at now.
    pub(crate) fn size(&self) -> u32 {
        self.lock().size.unwrap_or(crate::worker::THUMB_WIDTH)
    }

    /// Another folder: what is waiting is dropped, and a picture in
    /// hand is not delivered when it is done.
    pub(crate) fn forget(&self) {
        let mut q = self.lock();
        q.jobs.clear();
        q.missed.clear();
        q.wanted = None;
        q.previews.clear();
        q.previews_wanted = None;
        q.edits.clear();
        q.edits_queued.clear();
        q.edits_wanted = None;
        q.epoch += 1;
        drop(q);
        // Finished for the old list and not yet taken in: the window
        // would only throw each away on its path.
        if let Some(outbox) = &self.outbox {
            outbox.clear();
        }
    }

    /// How many pictures may be in hand at once from now on, up to
    /// the pool's size: fewer while the first develop runs, all of
    /// them after. Those in hand past it finish.
    pub(crate) fn set_limit(&self, limit: usize) {
        self.lock().limit = limit.min(self.threads);
        self.shared.cv.notify_all();
    }

    /// Hold the pool, or let it go: while held nothing new is begun,
    /// whatever the limit. The window holds it for the first develop
    /// of a folder opened in the loupe and lets go when that develop
    /// is delivered; the worker's own limit during every develop is
    /// separate, so neither lets go of the other's hold.
    pub(crate) fn hold(&self, on: bool) {
        let mut q = self.lock();
        if q.held != on {
            q.held = on;
            self.shared.cv.notify_all();
        }
    }

    /// The pool's size.
    pub(crate) fn threads(&self) -> usize {
        self.threads
    }

    /// Something a thread waits on outside the pool changed (how many
    /// edits may be in hand): each looks again.
    pub(crate) fn wake(&self) {
        let _q = self.lock();
        self.shared.cv.notify_all();
    }

    /// Stop the threads once each has finished what it has in hand.
    pub(crate) fn stop(&self) {
        self.lock().stopping = true;
        self.shared.cv.notify_all();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What a thread of the pool took in hand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum What {
    Thumb,
    Preview,
    Edit,
}

/// A thread of the pool: the next picture wanted that no other thread
/// has in hand, made, and delivered unless the folder has changed
/// since it was begun. While the pool is held, or as many are being
/// made as it allows, the next one not yet looked up is looked up in
/// the cache instead: a hit is delivered, a miss goes back to wait to
/// be made. A panic in the making — rawler has a few on damaged files
/// — costs that file its picture and nothing else.
///
/// With nothing of the thumbnails' to begin and room to make, a local
/// preview owed is made instead; it is delivered to nobody, since it
/// is for the cache. With no preview to begin either, the edit's
/// pictures of a frame that wants them are made, and the window is told
/// when they are kept.
fn serve(
    shared: &Shared,
    lookup: Option<&LookupFn>,
    make: &MakeFn,
    previews: Option<&PreviewHooks>,
    edits: Option<&EditHooks>,
    deliver: &dyn Fn(Outcome),
) {
    loop {
        let (index, frame, size, epoch, full, what) = {
            let mut q = shared.pending.lock().expect("thumbnail pool");
            loop {
                if q.stopping {
                    return;
                }
                let full = !q.held && q.busy < q.limit;
                let free = {
                    let p = &*q;
                    p.jobs.iter().position(|(_, f)| {
                        !p.making.contains(f)
                            && (full || (lookup.is_some() && !p.missed.contains(f)))
                    })
                };
                if let Some(at) = free {
                    // One being made leaves the queue; one being looked
                    // up keeps its place in it, in hand, and leaves only
                    // when it is found.
                    let (index, frame) = if full {
                        q.busy += 1;
                        q.jobs.remove(at).expect("a job at a found place")
                    } else {
                        q.jobs[at].clone()
                    };
                    q.making.push(frame.clone());
                    let size = crate::grid::made_size(q.size.unwrap_or(THUMB_WIDTH));
                    break (index, frame, size, q.epoch, full, What::Thumb);
                }
                // The lowest of all: a preview, only when no thumbnail
                // can be begun and two more may be made, so one thread
                // is always left for a thumbnail. A preview of a raw
                // with no JPEG in it is a develop, and one on a slow
                // share waits on the share: neither may take the whole
                // pool from the next folder's thumbnails. A limit of
                // one, then, never makes a preview.
                let preview = !q.held && q.busy + 1 < q.limit && previews.is_some();
                let at = preview
                    .then(|| {
                        let p = &*q;
                        p.previews
                            .iter()
                            .position(|(_, f)| !p.making.iter().any(|m| m.path == *f))
                    })
                    .flatten();
                if let Some(at) = at {
                    q.busy += 1;
                    let (index, path) = q.previews.remove(at).expect("a preview at a found place");
                    let frame = Key::file(path);
                    q.making.push(frame.clone());
                    break (index, frame, 0, q.epoch, true, What::Preview);
                }
                // Lower still: the edit's pictures, a decode and a
                // develop each, so one or two at a time (as `at_once`
                // says now), one under a network root with nothing
                // beside it, and like a preview never on the last thread.
                let mut again = None;
                let edit = edits.filter(|h| {
                    if q.held || q.busy + 1 >= q.limit || q.edits_alone || q.edits.is_empty() {
                        return false;
                    }
                    let (at_once, later) = (h.at_once)();
                    again = later;
                    q.edits_busy < at_once
                });
                let at = edit.and_then(|h| {
                    let p = &*q;
                    p.edits
                        .iter()
                        .position(|(_, f)| {
                            !p.making.contains(f) && (p.edits_busy == 0 || !(h.alone)(f))
                        })
                        .map(|at| (at, (h.alone)(&p.edits[at].1)))
                });
                if let Some((at, alone)) = at {
                    q.busy += 1;
                    q.edits_busy += 1;
                    q.edits_alone = alone;
                    let (index, frame) = q.edits.remove(at).expect("an edit at a found place");
                    q.edits_queued.remove(&frame);
                    q.making.push(frame.clone());
                    break (index, frame, 0, q.epoch, true, What::Edit);
                }
                #[cfg(test)]
                {
                    q.looked += 1;
                }
                // An edit held back only until the editor is quiet is
                // looked at again then, with nothing else to wake for it.
                q = match again {
                    Some(after) => shared.cv.wait_timeout(q, after).expect("thumbnail pool").0,
                    None => shared.cv.wait(q).expect("thumbnail pool"),
                };
            }
        };
        let started = Instant::now();
        if what == What::Edit {
            let hooks = edits.expect("an edit only with hooks");
            let made =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (hooks.make)(&frame)));
            {
                let mut q = shared.pending.lock().expect("thumbnail pool");
                if let Some(at) = q.making.iter().position(|f| *f == frame) {
                    q.making.swap_remove(at);
                }
                q.busy -= 1;
                q.edits_busy -= 1;
                q.edits_alone = false;
                shared.cv.notify_all();
            }
            match made {
                Ok(Ok(true)) => deliver(Outcome::EditedKept { frame }),
                Ok(Ok(false)) => {}
                Ok(Err(why)) => tracing::info!("the edit's pictures of {}: {why}", frame.said()),
                Err(payload) => tracing::warn!(
                    "the edit's pictures of {}: the making panicked: {}",
                    frame.said(),
                    panic_message(payload.as_ref())
                ),
            }
            continue;
        }
        if what == What::Preview {
            let hooks = previews.expect("a preview only with hooks");
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (hooks.make)(&frame.path)
            }));
            {
                let mut q = shared.pending.lock().expect("thumbnail pool");
                if let Some(at) = q.making.iter().position(|f| *f == frame) {
                    q.making.swap_remove(at);
                }
                q.busy -= 1;
                shared.cv.notify_all();
            }
            match made {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::debug!("preview {}: {e:#}", frame.path.display()),
                Err(payload) => tracing::warn!(
                    "preview {}: the making panicked: {}",
                    frame.path.display(),
                    panic_message(payload.as_ref())
                ),
            }
            continue;
        }
        let made = if full {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make(&frame, size)))
        } else {
            // A panic in a lookup is the making's to meet again, and
            // to report.
            let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lookup.and_then(|l| l(&frame, size))
            }))
            .ok()
            .flatten();
            {
                let mut q = shared.pending.lock().expect("thumbnail pool");
                if q.epoch == epoch {
                    if hit.is_some() {
                        let found = q.jobs.iter().position(|(i, f)| *i == index && *f == frame);
                        if let Some(at) = found {
                            q.jobs.remove(at);
                        }
                    } else {
                        q.missed.insert(frame.clone());
                    }
                }
                if hit.is_none() {
                    if let Some(at) = q.making.iter().position(|f| *f == frame) {
                        q.making.swap_remove(at);
                    }
                    shared.cv.notify_all();
                }
            }
            match hit {
                Some(thumb) => Ok(Ok((thumb, true))),
                None => continue,
            }
        };
        let seconds = started.elapsed().as_secs_f64();
        let current = {
            let mut q = shared.pending.lock().expect("thumbnail pool");
            if let Some(at) = q.making.iter().position(|f| *f == frame) {
                q.making.swap_remove(at);
            }
            if full {
                q.busy -= 1;
                q.missed.remove(&frame);
            }
            // A thread may be waiting for this file, or for room.
            shared.cv.notify_all();
            q.epoch == epoch
        };
        if !current {
            continue;
        }
        // Delivered with a picture: a preview behind it, if the file is
        // owed one, in this folder's numbering. A preview is the file's,
        // whichever of its frames brought it.
        if let Some(hooks) = previews
            && matches!(made, Ok(Ok(_)))
            && (hooks.owes)(&frame.path)
        {
            let mut q = shared.pending.lock().expect("thumbnail pool");
            if q.epoch == epoch && !q.previews.iter().any(|(_, f)| *f == frame.path) {
                q.previews.push_back((index, frame.path.clone()));
                q.previews_wanted = None;
                shared.cv.notify_one();
            }
        }
        // And the edit's pictures behind it, if the frame wants them. A
        // frame already in hand is queued again: its edit may have moved
        // since that making began, and a making finds its pictures kept
        // and does nothing when it has not.
        if let Some(hooks) = edits
            && matches!(made, Ok(Ok(_)))
            && (hooks.wants)(&frame)
        {
            let mut q = shared.pending.lock().expect("thumbnail pool");
            if q.epoch == epoch && q.edits_queued.insert(frame.clone()) {
                q.edits.push_back((index, frame.clone()));
                q.edits_wanted = None;
                shared.cv.notify_one();
            }
        }
        deliver(match made {
            Ok(Ok((thumb, cached))) => Outcome::Thumbnail {
                index,
                frame,
                size,
                width: thumb.width,
                height: thumb.height,
                rgb: thumb.rgb,
                cached,
                seconds,
            },
            Ok(Err(e)) => {
                tracing::debug!("thumbnail {}: {e}", frame.said());
                Outcome::NoThumbnail { index, frame }
            }
            Err(payload) => {
                tracing::warn!(
                    "thumbnail {}: the decode panicked: {}",
                    frame.said(),
                    panic_message(payload.as_ref())
                );
                Outcome::NoThumbnail { index, frame }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(10);

    /// What came back: the index, and whether it had a picture.
    fn collector() -> (Deliver, mpsc::Receiver<(usize, bool)>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let deliver: Deliver = Arc::new(move |o| {
            let got = match o {
                Outcome::Thumbnail { index, .. } => (index, true),
                Outcome::NoThumbnail { index, .. } => (index, false),
                _ => return,
            };
            let _ = tx.lock().unwrap().send(got);
        });
        (deliver, rx)
    }

    fn file(i: usize) -> Key {
        Key::file(format!("IMG_{i:04}.CR3"))
    }

    fn index_of(frame: &Key) -> usize {
        index_of_path(&frame.path)
    }

    fn index_of_path(path: &Path) -> usize {
        path.to_string_lossy()[4..8].parse().unwrap()
    }

    fn picture() -> Thumb {
        Thumb {
            width: 1,
            height: 1,
            rgb: vec![0; 3],
        }
    }

    /// A maker that says which file it began, in order, and makes a
    /// one-pixel picture.
    fn recording() -> (Make, Arc<Mutex<Vec<usize>>>) {
        let began = Arc::new(Mutex::new(Vec::new()));
        let seen = began.clone();
        let make: Make = Arc::new(move |path, _| {
            seen.lock().unwrap().push(index_of(path));
            Ok((picture(), false))
        });
        (make, began)
    }

    fn receive(rx: &mpsc::Receiver<(usize, bool)>, n: usize) -> Vec<(usize, bool)> {
        (0..n)
            .map(|_| rx.recv_timeout(WAIT).expect("delivered"))
            .collect()
    }

    #[test]
    fn the_size_is_half_the_cores_between_two_and_eight() {
        assert_eq!(threads_for(1), 2);
        assert_eq!(threads_for(4), 2);
        assert_eq!(threads_for(8), 4);
        assert_eq!(threads_for(12), 6);
        assert_eq!(threads_for(32), 8);
        assert_eq!(threads_for(128), 8);
    }

    /// Held while the folder is queued, as the first develop holds
    /// it, then let go one at a time: the frames on screen, then
    /// outward from them.
    #[test]
    fn the_frames_shown_are_made_first_then_outward() {
        let (make, began) = recording();
        let (deliver, rx) = collector();
        let pool = Pool::new(1, make, deliver);
        pool.set_limit(0);
        for i in 0..10 {
            pool.push(i, file(i));
        }
        pool.want(4, 6);
        pool.set_limit(1);
        receive(&rx, 10);
        assert_eq!(*began.lock().unwrap(), vec![4, 5, 6, 3, 7, 2, 8, 1, 9, 0]);
    }

    /// Held for the loupe's first develop, nothing is begun, and the
    /// worker's own limit going back up after a develop does not let
    /// go of it; the window's release does.
    #[test]
    fn a_hold_outlasts_the_limit_and_ends_with_its_release() {
        let (make, began) = recording();
        let (deliver, rx) = collector();
        let pool = Pool::new(2, make, deliver);
        pool.hold(true);
        for i in 0..3 {
            pool.push(i, file(i));
        }
        pool.set_limit(0);
        pool.set_limit(2);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert!(began.lock().unwrap().is_empty());
        pool.hold(false);
        assert_eq!(receive(&rx, 3).len(), 3);
    }

    /// Held, the pool still looks pictures up: the cache's hits are
    /// delivered at once, the misses are looked up once each and wait,
    /// unmade, until the hold is let go, and are then made in order.
    #[test]
    fn held_the_cache_is_still_read_and_nothing_is_made() {
        let (make, began) = recording();
        let looked = Arc::new(Mutex::new(Vec::new()));
        let seen = looked.clone();
        let lookup: Lookup = Arc::new(move |path, _| {
            let i = index_of(path);
            seen.lock().unwrap().push(i);
            i.is_multiple_of(2).then(picture)
        });
        let (deliver, rx) = collector();
        let pool = Pool::with_lookup(2, lookup, make, deliver);
        pool.hold(true);
        for i in 0..6 {
            pool.push(i, file(i));
        }
        let mut hits: Vec<usize> = receive(&rx, 3).into_iter().map(|(i, _)| i).collect();
        hits.sort();
        assert_eq!(hits, vec![0, 2, 4]);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert!(began.lock().unwrap().is_empty(), "nothing made while held");
        let mut once = looked.lock().unwrap().clone();
        once.sort();
        assert_eq!(once, vec![0, 1, 2, 3, 4, 5], "each looked up once");
        pool.set_limit(1);
        pool.hold(false);
        receive(&rx, 3);
        assert_eq!(*began.lock().unwrap(), vec![1, 3, 5]);
    }

    /// Preview hooks that owe every file but those in `kept`, and
    /// record the previews made, in order, into `log` beside whatever
    /// else is recorded there.
    fn preview_hooks(log: Arc<Mutex<Vec<String>>>, kept: &'static [usize]) -> PreviewHooks {
        PreviewHooks {
            owes: Arc::new(move |path| !kept.contains(&index_of_path(path))),
            make: Arc::new(move |path| {
                log.lock()
                    .unwrap()
                    .push(format!("p{}", index_of_path(path)));
                Ok(())
            }),
        }
    }

    /// Wait until a thread, woken now, has looked at the pool as it
    /// stands and begun nothing: what a test waits on before it
    /// asserts that something did not start. Needs a thread not at
    /// work to look.
    fn settled(pool: &Pool) {
        let mark = pool.lock().looked;
        pool.wake();
        let started = Instant::now();
        while pool.lock().looked <= mark {
            assert!(started.elapsed() < WAIT, "the pool never looked again");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Wait until no picture is in hand on any thread.
    fn until_idle(pool: &Pool) {
        let started = Instant::now();
        while pool.lock().busy > 0 {
            assert!(started.elapsed() < WAIT, "still at work");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Wait until `log` holds `n` entries.
    fn until_logged(log: &Mutex<Vec<String>>, n: usize) -> Vec<String> {
        let started = std::time::Instant::now();
        while log.lock().unwrap().len() < n {
            assert!(started.elapsed() < WAIT, "{:?}", log.lock().unwrap());
            std::thread::sleep(Duration::from_millis(2));
        }
        log.lock().unwrap().clone()
    }

    /// The local previews come after every thumbnail, and a file with
    /// its preview kept is owed none.
    #[test]
    fn the_previews_are_made_after_the_thumbnails() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        let make: Make = Arc::new(move |path, _| {
            seen.lock().unwrap().push(format!("t{}", index_of(path)));
            Ok((picture(), false))
        });
        let (deliver, rx) = collector();
        let pool = Pool::new(2, make, deliver).with_previews(preview_hooks(log.clone(), &[3]));
        pool.set_limit(0);
        for i in 0..6 {
            pool.push(i, file(i));
        }
        pool.want(2, 3);
        pool.set_limit(2);
        receive(&rx, 6);
        let order = until_logged(&log, 11);
        let (thumbs, previews) = order.split_at(6);
        assert!(thumbs.iter().all(|e| e.starts_with('t')), "{order:?}");
        let mut previews = previews.to_vec();
        previews.sort();
        assert_eq!(previews, ["p0", "p1", "p2", "p4", "p5"]);
        until_idle(&pool);
        settled(&pool);
        assert_eq!(log.lock().unwrap().len(), 11, "one a file, none for 3");
    }

    /// The previews waiting are made in the thumbnails' order about the
    /// range shown, one at a time on a pool of two.
    #[test]
    fn the_previews_of_the_frames_shown_are_made_first() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let lookup: Lookup = Arc::new(|_, _| Some(picture()));
        let (deliver, rx) = collector();
        let pool = Pool::with_lookup(2, lookup, instant(), deliver)
            .with_previews(preview_hooks(log.clone(), &[3]));
        pool.hold(true);
        for i in 0..6 {
            pool.push(i, file(i));
        }
        receive(&rx, 6);
        pool.want(2, 3);
        pool.hold(false);
        assert_eq!(until_logged(&log, 5), ["p2", "p1", "p0", "p4", "p5"]);
    }

    /// A preview never takes the last thread: with one in hand on a
    /// pool of two, the second waits, and a thumbnail asked for
    /// meanwhile is made at once.
    #[test]
    fn a_preview_leaves_a_thread_for_the_thumbnails() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (gate_tx, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let seen = log.clone();
        let hooks = PreviewHooks {
            owes: Arc::new(|_| true),
            make: Arc::new(move |path| {
                seen.lock()
                    .unwrap()
                    .push(format!("p{}", index_of_path(path)));
                gate.lock().unwrap().recv_timeout(WAIT).expect("let go");
                Ok(())
            }),
        };
        let (deliver, rx) = collector();
        let pool = Pool::new(2, instant(), deliver).with_previews(hooks);
        pool.push(0, file(0));
        pool.push(1, file(1));
        receive(&rx, 2);
        until_logged(&log, 1);
        pool.push(5, file(5));
        assert_eq!(receive(&rx, 1), [(5, true)], "made beside the preview");
        settled(&pool);
        assert_eq!(log.lock().unwrap().len(), 1, "one preview at a time");
        for _ in 0..3 {
            gate_tx.send(()).unwrap();
        }
        until_logged(&log, 3);
    }

    /// Held, the cache's thumbnails are delivered and no preview is
    /// begun; a folder change drops the previews waiting; let go, a
    /// new folder's are made.
    #[test]
    fn held_no_preview_is_made_and_a_folder_change_drops_them() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let lookup: Lookup = Arc::new(|_, _| Some(picture()));
        let (deliver, rx) = collector();
        let pool = Pool::with_lookup(2, lookup, instant(), deliver)
            .with_previews(preview_hooks(log.clone(), &[]));
        pool.hold(true);
        for i in 0..3 {
            pool.push(i, file(i));
        }
        assert_eq!(receive(&rx, 3).len(), 3, "the hits, held");
        settled(&pool);
        assert!(log.lock().unwrap().is_empty(), "no preview while held");
        pool.forget();
        pool.hold(false);
        settled(&pool);
        assert!(log.lock().unwrap().is_empty(), "dropped with the folder");
        pool.push(7, file(7));
        receive(&rx, 1);
        assert_eq!(until_logged(&log, 1), ["p7"]);
    }

    /// Edit hooks that want every file but those in `kept`, and record
    /// the makings, in order, into `log`; each says it kept pictures.
    /// Two at once, none alone.
    /// Hooks that record each making into `log` and keep every picture,
    /// one making at a time so the log is the order the pool chose and
    /// not the order the threads happened to run.
    fn edit_hooks(log: Arc<Mutex<Vec<String>>>, kept: &'static [usize]) -> EditHooks {
        EditHooks {
            wants: Arc::new(move |path| !kept.contains(&index_of(path))),
            make: Arc::new(move |path| {
                log.lock().unwrap().push(format!("e{}", index_of(path)));
                Ok(true)
            }),
            at_once: Arc::new(|| (1, None)),
            alone: Arc::new(|_| false),
        }
    }

    /// Hooks whose makings each wait on `gate` and record into `log`,
    /// `at_once` and `alone` as given.
    fn gated_hooks(
        log: Arc<Mutex<Vec<String>>>,
        gate: mpsc::Receiver<()>,
        at_once: Arc<AtOnceFn>,
        alone: Arc<WantsFn>,
    ) -> EditHooks {
        let gate = Mutex::new(gate);
        EditHooks {
            wants: Arc::new(|_| true),
            make: Arc::new(move |path| {
                log.lock().unwrap().push(format!("e{}", index_of(path)));
                gate.lock().unwrap().recv_timeout(WAIT).expect("let go");
                Ok(false)
            }),
            at_once,
            alone,
        }
    }

    /// The edit's pictures come after every thumbnail and every
    /// preview, the frames shown first; a file with its pictures kept
    /// wants none; and each kept has the window told.
    #[test]
    fn the_edits_are_made_after_the_previews_the_frames_shown_first() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let lookup: Lookup = Arc::new(|_, _| Some(picture()));
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let deliver: Deliver = Arc::new(move |outcome| match outcome {
            Outcome::Thumbnail { index, .. } => tx.lock().unwrap().send(("t", index)).unwrap(),
            Outcome::EditedKept { frame } => {
                tx.lock().unwrap().send(("e", index_of(&frame))).unwrap()
            }
            _ => {}
        });
        let pool = Pool::with_lookup(4, lookup, instant(), deliver)
            .with_previews(preview_hooks(log.clone(), &[0, 1, 2, 4, 5]))
            .with_edits(edit_hooks(log.clone(), &[3]));
        pool.hold(true);
        for i in 0..6 {
            pool.push(i, file(i));
        }
        let mut thumbs = Vec::new();
        while thumbs.len() < 6 {
            let (what, i) = rx.recv_timeout(WAIT).unwrap();
            assert_eq!(what, "t");
            thumbs.push(i);
        }
        settled(&pool);
        assert!(log.lock().unwrap().is_empty(), "nothing while held");
        pool.want(2, 3);
        pool.hold(false);
        let order = until_logged(&log, 6);
        // The preview is handed out before any edit, but it is made on
        // its own thread while the first edit begins on another, so
        // the two may log in either order.
        assert!(
            order[..2].contains(&"p3".to_string()),
            "the preview before every edit: {order:?}"
        );
        let edits: Vec<&str> = order
            .iter()
            .filter(|s| *s != "p3")
            .map(String::as_str)
            .collect();
        // Edits one at a time, so this is the pool's order: the frames
        // nearest the range shown first, the rest outward.
        assert_eq!(edits, ["e2", "e1", "e0", "e4", "e5"]);
        let mut kept = Vec::new();
        while kept.len() < 5 {
            let (what, i) = rx.recv_timeout(WAIT).unwrap();
            assert_eq!(what, "e");
            kept.push(i);
        }
        kept.sort();
        assert_eq!(kept, [0, 1, 2, 4, 5]);
    }

    /// Edits are made at most two at once on four threads, one on two,
    /// and never on the last thread: a thumbnail asked for meanwhile is
    /// made at once beside them.
    #[test]
    fn edits_are_made_two_at_once_and_leave_a_thread_for_the_thumbnails() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (gate_tx, gate) = mpsc::channel::<()>();
        let hooks = gated_hooks(
            log.clone(),
            gate,
            Arc::new(|| (crate::reduced::at_once(4, false), None)),
            Arc::new(|_| false),
        );
        let (deliver, rx) = collector();
        let pool = Pool::new(4, instant(), deliver).with_edits(hooks);
        for i in 0..4 {
            pool.push(i, file(i));
        }
        receive(&rx, 4);
        until_logged(&log, 2);
        settled(&pool);
        assert_eq!(log.lock().unwrap().len(), 2, "two at once, no more");
        pool.push(7, file(7));
        assert_eq!(receive(&rx, 1), [(7, true)], "made beside the edits");
        for _ in 0..6 {
            gate_tx.send(()).unwrap();
        }
        until_logged(&log, 5);

        // On two threads, one at a time.
        let log = Arc::new(Mutex::new(Vec::new()));
        let (gate_tx, gate) = mpsc::channel::<()>();
        let hooks = gated_hooks(
            log.clone(),
            gate,
            Arc::new(|| (crate::reduced::at_once(2, false), None)),
            Arc::new(|_| false),
        );
        let (deliver, rx) = collector();
        let pool = Pool::new(2, instant(), deliver).with_edits(hooks);
        for i in 0..3 {
            pool.push(i, file(i));
        }
        receive(&rx, 3);
        until_logged(&log, 1);
        settled(&pool);
        assert_eq!(log.lock().unwrap().len(), 1, "one at a time on two");
        for _ in 0..4 {
            gate_tx.send(()).unwrap();
        }
        until_logged(&log, 3);
    }

    /// One at a time while a frame is open in the editor, two once it is
    /// not (the pool woken to begin the second); and a frame under a
    /// network root alone, with nothing begun beside it.
    #[test]
    fn edits_are_one_at_a_time_while_editing_and_alone_on_a_network_root() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (gate_tx, gate) = mpsc::channel::<()>();
        let editing = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let at_once = {
            let editing = editing.clone();
            Arc::new(move || {
                (
                    crate::reduced::at_once(8, editing.load(std::sync::atomic::Ordering::SeqCst)),
                    None,
                )
            })
        };
        let hooks = gated_hooks(log.clone(), gate, at_once, Arc::new(|_| false));
        let (deliver, rx) = collector();
        let pool = Pool::new(8, instant(), deliver).with_edits(hooks);
        for i in 0..4 {
            pool.push(i, file(i));
        }
        receive(&rx, 4);
        until_logged(&log, 1);
        settled(&pool);
        assert_eq!(log.lock().unwrap().len(), 1, "one while editing");
        editing.store(false, std::sync::atomic::Ordering::SeqCst);
        pool.wake();
        until_logged(&log, 2);
        settled(&pool);
        assert_eq!(log.lock().unwrap().len(), 2, "two while browsing");
        for _ in 0..4 {
            gate_tx.send(()).unwrap();
        }
        until_logged(&log, 4);

        // Files 0 and 2 are under a network root: each begins only with
        // nothing in hand, and nothing begins beside it.
        let log = Arc::new(Mutex::new(Vec::new()));
        let (gate_tx, gate) = mpsc::channel::<()>();
        let hooks = gated_hooks(
            log.clone(),
            gate,
            Arc::new(|| (2, None)),
            Arc::new(|path| [0, 2].contains(&index_of(path))),
        );
        let (deliver, rx) = collector();
        let lookup: Lookup = Arc::new(|_, _| Some(picture()));
        let pool = Pool::with_lookup(8, lookup, instant(), deliver).with_edits(hooks);
        pool.hold(true);
        for i in 0..4 {
            pool.push(i, file(i));
        }
        receive(&rx, 4);
        pool.want(0, 3);
        pool.hold(false);
        until_logged(&log, 1);
        settled(&pool);
        assert_eq!(*log.lock().unwrap(), ["e0"], "the network frame alone");
        gate_tx.send(()).unwrap();
        // Then the two local ones together; the second network frame
        // waits for both.
        until_logged(&log, 3);
        settled(&pool);
        // Begun together on two threads, so logged in either order.
        let mut begun = log.lock().unwrap().clone();
        begun[1..].sort();
        assert_eq!(begun, ["e0", "e1", "e3"]);
        gate_tx.send(()).unwrap();
        let started = Instant::now();
        while pool.lock().edits_busy > 1 {
            assert!(started.elapsed() < WAIT, "one never finished");
            std::thread::sleep(Duration::from_millis(1));
        }
        settled(&pool);
        assert_eq!(log.lock().unwrap().len(), 3, "not beside the one left");
        gate_tx.send(()).unwrap();
        until_logged(&log, 4);
        assert_eq!(log.lock().unwrap()[3], "e2");
        gate_tx.send(()).unwrap();
    }

    /// A scroll while a picture is in hand re-orders what is left,
    /// and the one in hand is finished.
    #[test]
    fn a_reorder_while_busy_applies_to_what_has_not_begun() {
        let (gate_tx, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let (began_tx, began_rx) = mpsc::channel();
        let began_tx = Mutex::new(began_tx);
        let make: Make = Arc::new(move |path, _| {
            let i = index_of(path);
            began_tx.lock().unwrap().send(i).unwrap();
            if i == 0 {
                gate.lock().unwrap().recv_timeout(WAIT).unwrap();
            }
            Ok((picture(), false))
        });
        let (deliver, rx) = collector();
        let pool = Pool::new(1, make, deliver);
        for i in 0..10 {
            pool.push(i, file(i));
        }
        assert_eq!(began_rx.recv_timeout(WAIT).unwrap(), 0);
        pool.want(7, 8);
        gate_tx.send(()).unwrap();
        receive(&rx, 10);
        let order: Vec<usize> = began_rx.try_iter().collect();
        // The range, then outward from its middle, 7: 6, then 5 and 9.
        assert_eq!(order, vec![7, 8, 6, 5, 9, 4, 3, 2, 1]);
    }

    /// Two threads never have one file in hand: a second ask for it
    /// (a larger picture for the grid) waits for the first to finish,
    /// and an ask for one already waiting is dropped.
    #[test]
    fn a_file_is_never_made_twice_at_once() {
        let at_once = Arc::new(Mutex::new((0usize, 0usize)));
        let seen = at_once.clone();
        let (gate_tx, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let made = Arc::new(Mutex::new(0usize));
        let count = made.clone();
        let (began_tx, began) = mpsc::channel::<()>();
        let began_tx = Mutex::new(began_tx);
        let make: Make = Arc::new(move |_, _| {
            {
                let mut s = seen.lock().unwrap();
                s.0 += 1;
                s.1 = s.1.max(s.0);
            }
            *count.lock().unwrap() += 1;
            began_tx.lock().unwrap().send(()).unwrap();
            // Each making waits to be let go, so the next ask arrives
            // while it is in hand.
            gate.lock().unwrap().recv_timeout(WAIT).unwrap();
            seen.lock().unwrap().0 -= 1;
            Ok((picture(), false))
        });
        let (deliver, rx) = collector();
        let pool = Pool::new(4, make, deliver);
        pool.push(3, file(3));
        // In hand, then asked for twice more: one waits for it, and
        // the repeat of the one waiting is dropped.
        began.recv_timeout(WAIT).unwrap();
        pool.push(3, file(3));
        pool.push(3, file(3));
        // Three threads are free, and none takes it while it is in
        // hand.
        assert!(began.recv_timeout(Duration::from_millis(200)).is_err());
        gate_tx.send(()).unwrap();
        began.recv_timeout(WAIT).unwrap();
        gate_tx.send(()).unwrap();
        let got = receive(&rx, 2);
        assert_eq!(got, vec![(3, true), (3, true)]);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        assert_eq!(*made.lock().unwrap(), 2);
        assert_eq!(at_once.lock().unwrap().1, 1, "never two at once");
    }

    /// A folder change drops what is waiting, and the picture in hand
    /// for the old folder is never delivered; the new folder's are.
    #[test]
    fn a_folder_change_drops_the_old_folder_s_pictures() {
        let (gate_tx, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let (began_tx, began_rx) = mpsc::channel();
        let began_tx = Mutex::new(began_tx);
        let make: Make = Arc::new(move |path, _| {
            let i = index_of(path);
            began_tx.lock().unwrap().send(i).unwrap();
            if i == 0 {
                gate.lock().unwrap().recv_timeout(WAIT).unwrap();
            }
            Ok((picture(), false))
        });
        let (deliver, rx) = collector();
        let pool = Pool::new(1, make, deliver);
        for i in 0..6 {
            pool.push(i, file(i));
        }
        assert_eq!(began_rx.recv_timeout(WAIT).unwrap(), 0);
        pool.forget();
        pool.push(0, file(100));
        gate_tx.send(()).unwrap();
        assert_eq!(receive(&rx, 1), vec![(0, true)]);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        let began: Vec<usize> = began_rx.try_iter().collect();
        assert_eq!(began, vec![100], "nothing of the old folder begun");
    }

    /// A panic in one file's decode is that file's failure; the
    /// others are made and the threads go on serving.
    #[test]
    fn a_panic_costs_one_cell() {
        let make: Make = Arc::new(|path, _| {
            if index_of(path) == 2 {
                panic!("capacity overflow");
            }
            if index_of(path) == 4 {
                anyhow::bail!("no preview");
            }
            Ok((picture(), false))
        });
        let (deliver, rx) = collector();
        let pool = Pool::new(2, make, deliver);
        for i in 0..6 {
            pool.push(i, file(i));
        }
        let mut got = receive(&rx, 6);
        got.sort();
        assert_eq!(
            got,
            vec![
                (0, true),
                (1, true),
                (2, false),
                (3, true),
                (4, false),
                (5, true)
            ]
        );
        // Both threads still serve after it.
        for i in 6..10 {
            pool.push(i, file(i));
        }
        assert_eq!(receive(&rx, 4).len(), 4);
    }

    /// Made in parallel: four slow pictures on four threads take
    /// about one picture's time, not four.
    #[test]
    fn the_threads_make_pictures_at_once() {
        let make: Make = Arc::new(|_, _| {
            std::thread::sleep(Duration::from_millis(300));
            Ok((picture(), false))
        });
        let (deliver, rx) = collector();
        let pool = Pool::new(4, make, deliver);
        let started = Instant::now();
        for i in 0..4 {
            pool.push(i, file(i));
        }
        receive(&rx, 4);
        assert!(started.elapsed() < Duration::from_millis(1100));
    }

    /// The size is read as each picture is begun, and rounded up to
    /// a made size.
    #[test]
    fn the_size_is_the_one_wanted_when_begun() {
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let seen = sizes.clone();
        let make: Make = Arc::new(move |_, size| {
            seen.lock().unwrap().push(size);
            Ok((picture(), false))
        });
        let (deliver, rx) = collector();
        let pool = Pool::new(1, make, deliver);
        pool.set_limit(0);
        pool.push(0, file(0));
        pool.set_size(300);
        pool.set_limit(1);
        receive(&rx, 1);
        pool.set_size(96);
        pool.push(1, file(1));
        receive(&rx, 1);
        assert_eq!(*sizes.lock().unwrap(), vec![360, 128]);
    }

    /// What a batched pool tells the window: each batch it is handed.
    fn batches() -> (Deliver, mpsc::Receiver<Batch>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let deliver: Deliver = Arc::new(move |o| {
            if let Outcome::Thumbnails(batch) = o {
                let _ = tx.lock().unwrap().send(batch);
            }
        });
        (deliver, rx)
    }

    fn instant() -> Make {
        Arc::new(|_, _| Ok((picture(), true)))
    }

    /// How many are waiting in a batch's outbox.
    fn waiting(batch: &Batch) -> usize {
        let w = batch.0.lock();
        w.shown_done.len() + w.rest_done.len()
    }

    /// Wait until `n` are waiting, as a window busy drawing would find
    /// them.
    fn until_waiting(batch: &Batch, n: usize) {
        let until = Instant::now() + WAIT;
        while waiting(batch) < n {
            assert!(Instant::now() < until, "{} of {n} came", waiting(batch));
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn files_of(outcomes: &[Outcome]) -> Vec<usize> {
        outcomes.iter().filter_map(file_of).collect()
    }

    /// Two thousand pictures that take no time, on eight threads: one
    /// call on the window's event loop each kept a warm folder of
    /// twenty thousand from drawing for 3 s. The window is told once
    /// and takes them all from the one batch, and is told again only
    /// after it has found the outbox empty.
    #[test]
    fn many_instant_hits_come_in_one_batch() {
        let (deliver, rx) = batches();
        let pool = Pool::new(8, instant(), deliver).batched();
        pool.push_all((0..2000).map(|i| (i, file(i))).collect());
        let batch = rx.recv_timeout(WAIT).expect("the window is told");
        until_waiting(&batch, 2000);
        assert!(rx.try_recv().is_err(), "told once for all of them");
        let (first, left) = batch.take(64);
        assert_eq!((first.len(), left), (64, true));
        let (rest, left) = batch.take(usize::MAX);
        assert_eq!((rest.len(), left), (1936, false));
        let mut all = files_of(&first);
        all.extend(files_of(&rest));
        all.sort_unstable();
        assert_eq!(all, (0..2000).collect::<Vec<_>>(), "each once");
        assert!(rx.try_recv().is_err());
        // Found empty, the window is told of the next ones.
        pool.push_all((2000..3000).map(|i| (i, file(i))).collect());
        let again = rx.recv_timeout(WAIT).expect("told again");
        until_waiting(&again, 1000);
        assert!(rx.try_recv().is_err(), "once for the second lot too");
        assert_eq!(again.take(usize::MAX).0.len(), 1000);
    }

    /// A window that takes what is waiting in slices while the threads
    /// go on finishing, as the editor's does: nothing lost or taken
    /// twice, and far fewer batches than pictures.
    #[test]
    fn a_window_taking_slices_meanwhile_gets_each_once() {
        let (deliver, rx) = batches();
        let pool = Pool::new(8, instant(), deliver).batched();
        let n = 5000;
        pool.push_all((0..n).map(|i| (i, file(i))).collect());
        let (mut seen, mut told) = (Vec::new(), 0);
        while seen.len() < n {
            let batch = rx.recv_timeout(WAIT).expect("told");
            told += 1;
            loop {
                let (outcomes, left) = batch.take(32);
                seen.extend(files_of(&outcomes));
                if !left {
                    break;
                }
            }
            // A frame's worth of drawing between turns.
            std::thread::sleep(Duration::from_millis(2));
        }
        seen.sort_unstable();
        assert_eq!(seen, (0..n).collect::<Vec<_>>());
        assert!(told < n / 10, "{told} batches for {n} pictures");
    }

    /// What waits for the files on screen is taken first, in the order
    /// it was finished, then the rest in theirs: when the window said
    /// what it shows before they were made, and when it scrolls while
    /// they wait.
    #[test]
    fn the_files_on_screen_are_taken_first() {
        let (deliver, rx) = batches();
        let pool = Pool::new(1, instant(), deliver).batched();
        pool.set_limit(0);
        pool.push_all((0..100).map(|i| (i, file(i))).collect());
        pool.want(20, 24);
        pool.set_limit(1);
        let batch = rx.recv_timeout(WAIT).expect("told");
        until_waiting(&batch, 100);
        assert_eq!(files_of(&batch.take(5).0), vec![20, 21, 22, 23, 24]);
        // Scrolled while the rest wait: those, then the earliest
        // finished of the rest, which were made outward from 20..=24.
        pool.want(60, 69);
        assert_eq!(
            files_of(&batch.take(12).0),
            vec![60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 19, 25]
        );
        let (rest, left) = batch.take(usize::MAX);
        assert!(!left);
        assert_eq!(rest.len(), 83);
    }

    /// Another folder: what waits for the window from the old one is
    /// dropped.
    #[test]
    fn a_folder_change_empties_the_outbox() {
        let (deliver, rx) = batches();
        let pool = Pool::new(2, instant(), deliver).batched();
        pool.push_all((0..50).map(|i| (i, file(i))).collect());
        let batch = rx.recv_timeout(WAIT).expect("told");
        until_waiting(&batch, 50);
        pool.forget();
        let (taken, left) = batch.take(usize::MAX);
        assert!(taken.is_empty() && !left);
    }

    /// A folder change leaves the window told, with nothing waiting.
    /// Its take finds the outbox empty, and that alone is what lets
    /// the new folder's pictures tell it again: a flag left set would
    /// leave them unseen for good.
    #[test]
    fn after_a_folder_change_the_window_is_told_again() {
        let (deliver, rx) = batches();
        let pool = Pool::new(2, instant(), deliver).batched();
        pool.push_all((0..50).map(|i| (i, file(i))).collect());
        let batch = rx.recv_timeout(WAIT).expect("told");
        until_waiting(&batch, 50);
        pool.forget();
        assert!(rx.try_recv().is_err());
        assert_eq!(batch.take(32).0.len(), 0);
        pool.push_all((0..10).map(|i| (i, file(100 + i))).collect());
        let again = rx
            .recv_timeout(WAIT)
            .expect("the new folder tells it again");
        until_waiting(&again, 10);
        let (taken, left) = again.take(usize::MAX);
        assert_eq!((taken.len(), left), (10, false));
    }
}
