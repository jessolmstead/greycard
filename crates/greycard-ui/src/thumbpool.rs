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

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use greycard_library::thumbs::Thumb;

use crate::worker::{Deliver, Outcome, THUMB_WIDTH, order_thumbnails, panic_message};

/// How a picture is made: a file and the long edge asked for, to the
/// picture and whether it came from the cache.
pub(crate) type Make = Arc<MakeFn>;
pub(crate) type MakeFn = dyn Fn(&Path, u32) -> anyhow::Result<(Thumb, bool)> + Send + Sync;

/// How a picture is looked up in the cache alone, without making it:
/// the picture, or `None` for a miss.
pub(crate) type Lookup = Arc<LookupFn>;
pub(crate) type LookupFn = dyn Fn(&Path, u32) -> Option<Thumb> + Send + Sync;

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

#[derive(Default)]
struct Pending {
    /// What is waiting, in the order it is wanted.
    jobs: VecDeque<(usize, PathBuf)>,
    /// The range the waiting ones are ordered for, so the strip can
    /// report where it stands as often as it likes. Cleared when one
    /// is pushed, since that is a new folder's list or a step up.
    wanted: Option<(usize, usize)>,
    /// The long edge pictures are made at now; `THUMB_WIDTH` until
    /// the grid asks otherwise. Read as each is begun.
    size: Option<u32>,
    /// Raised by a folder change: a picture begun under an older one
    /// is not delivered.
    epoch: u64,
    /// The files in hand, one entry a thread at work, looking up or
    /// making.
    making: Vec<PathBuf>,
    /// How many of those are being made rather than looked up.
    busy: usize,
    /// Files looked up while the pool was held and not found: they
    /// wait to be made, and are not looked up again meanwhile.
    missed: std::collections::HashSet<PathBuf>,
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
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.shared.pending.lock().expect("thumbnail pool")
    }

    /// Make `path`'s picture, as file `index` of the folder. An ask
    /// for one already waiting is dropped; the waiting one is made at
    /// whatever size is wanted when it is begun anyway.
    pub(crate) fn push(&self, index: usize, path: PathBuf) {
        let mut q = self.lock();
        q.wanted = None;
        if !q.jobs.iter().any(|(i, p)| *i == index && *p == path) {
            q.jobs.push_back((index, path));
        }
        if !q.started {
            q.started = true;
            for n in 0..self.threads {
                let (shared, lookup, make, deliver) = (
                    self.shared.clone(),
                    self.lookup.clone(),
                    self.make.clone(),
                    self.deliver.clone(),
                );
                let spawned = std::thread::Builder::new()
                    .name(format!("greycard thumbnails {n}"))
                    .spawn(move || serve(&shared, lookup.as_deref(), &*make, &*deliver));
                if let Err(e) = spawned {
                    tracing::warn!("thumbnail thread {n} not started: {e}");
                }
            }
        }
        self.shared.cv.notify_one();
    }

    /// Make `first..=last` before the rest, the rest outward from
    /// that range; applies to whatever has not been begun.
    pub(crate) fn want(&self, first: usize, last: usize) {
        let mut q = self.lock();
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

    /// Another folder: what is waiting is dropped, and a picture in
    /// hand is not delivered when it is done.
    pub(crate) fn forget(&self) {
        let mut q = self.lock();
        q.jobs.clear();
        q.missed.clear();
        q.wanted = None;
        q.epoch += 1;
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

/// A thread of the pool: the next picture wanted that no other thread
/// has in hand, made, and delivered unless the folder has changed
/// since it was begun. While the pool is held, or as many are being
/// made as it allows, the next one not yet looked up is looked up in
/// the cache instead: a hit is delivered, a miss goes back to wait to
/// be made. A panic in the making — rawler has a few on damaged files
/// — costs that file its picture and nothing else.
fn serve(shared: &Shared, lookup: Option<&LookupFn>, make: &MakeFn, deliver: &dyn Fn(Outcome)) {
    loop {
        let (index, path, size, epoch, full) = {
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
                    let (index, path) = if full {
                        q.busy += 1;
                        q.jobs.remove(at).expect("a job at a found place")
                    } else {
                        q.jobs[at].clone()
                    };
                    q.making.push(path.clone());
                    let size = crate::grid::made_size(q.size.unwrap_or(THUMB_WIDTH));
                    break (index, path, size, q.epoch, full);
                }
                q = shared.cv.wait(q).expect("thumbnail pool");
            }
        };
        let started = Instant::now();
        let made = if full {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make(&path, size)))
        } else {
            // A panic in a lookup is the making's to meet again, and
            // to report.
            let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lookup.and_then(|l| l(&path, size))
            }))
            .ok()
            .flatten();
            {
                let mut q = shared.pending.lock().expect("thumbnail pool");
                if q.epoch == epoch {
                    if hit.is_some() {
                        let found = q.jobs.iter().position(|(i, f)| *i == index && *f == path);
                        if let Some(at) = found {
                            q.jobs.remove(at);
                        }
                    } else {
                        q.missed.insert(path.clone());
                    }
                }
                if hit.is_none() {
                    if let Some(at) = q.making.iter().position(|f| *f == path) {
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
            if let Some(at) = q.making.iter().position(|f| *f == path) {
                q.making.swap_remove(at);
            }
            if full {
                q.busy -= 1;
                q.missed.remove(&path);
            }
            // A thread may be waiting for this file, or for room.
            shared.cv.notify_all();
            q.epoch == epoch
        };
        if !current {
            continue;
        }
        deliver(match made {
            Ok(Ok((thumb, cached))) => Outcome::Thumbnail {
                index,
                path,
                size,
                width: thumb.width,
                height: thumb.height,
                rgb: thumb.rgb,
                cached,
                seconds,
            },
            Ok(Err(e)) => {
                tracing::debug!("thumbnail {}: {e}", path.display());
                Outcome::NoThumbnail { index, path }
            }
            Err(payload) => {
                tracing::warn!(
                    "thumbnail {}: the decode panicked: {}",
                    path.display(),
                    panic_message(payload.as_ref())
                );
                Outcome::NoThumbnail { index, path }
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

    fn file(i: usize) -> PathBuf {
        PathBuf::from(format!("IMG_{i:04}.CR3"))
    }

    fn index_of(path: &Path) -> usize {
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
}
