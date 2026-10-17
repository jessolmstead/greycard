//! The worker's handle: the editor's side of the queue, and the model
//! and lens fetches it starts.

use super::*;

impl Worker {
    /// A worker whose thumbnails come to `deliver` one call each: the
    /// tests', which read them off a channel.
    #[cfg(test)]
    pub fn new(deliver: impl Fn(Outcome) + Send + Sync + 'static) -> Self {
        Self::build(Arc::new(deliver), false)
    }

    /// A worker whose thumbnails come to `deliver` in batches, as
    /// [`Outcome::Thumbnails`]: the window's, where a call is a turn
    /// of the event loop and a warm folder of thousands would take
    /// every one before drawing.
    pub fn batched(deliver: impl Fn(Outcome) + Send + Sync + 'static) -> Self {
        Self::build(Arc::new(deliver), true)
    }

    fn build(deliver: Deliver, batched: bool) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let thumbs: ThumbCache = Arc::new(Mutex::new(None));
        let edited = Arc::new(crate::edited::Edited::new(thumbs.clone()));
        let previews =
            Arc::new(crate::previews::Previews::new(thumbs.clone()).with_edited(edited.clone()));
        let make: crate::thumbpool::Make = {
            let (thumbs, previews, edited) = (thumbs.clone(), previews.clone(), edited.clone());
            Arc::new(move |frame, size| {
                cached_thumbnail_noting(
                    &thumbs,
                    Some(&previews),
                    Some(&edited),
                    frame,
                    size,
                    thumbnail,
                )
            })
        };
        let lookup: crate::thumbpool::Lookup = {
            let (thumbs, previews, edited) = (thumbs.clone(), previews.clone(), edited.clone());
            Arc::new(move |frame, size| {
                thumb_lookup(&thumbs, frame, size, Some(&previews), Some(&edited)).2
            })
        };
        let hooks = crate::thumbpool::PreviewHooks {
            owes: {
                let previews = previews.clone();
                Arc::new(move |path| previews.owes(path))
            },
            make: {
                let previews = previews.clone();
                Arc::new(move |path| previews.make(path).map(drop))
            },
        };
        let reducer = Arc::new(crate::reduced::Reducer::new());
        let edits = crate::thumbpool::EditHooks {
            wants: {
                let (reducer, edited) = (reducer.clone(), edited.clone());
                Arc::new(move |frame| reducer.wants(&edited, frame))
            },
            make: {
                let (reducer, edited, queue) = (reducer.clone(), edited.clone(), queue.clone());
                Arc::new(move |frame| match reducer.make(&edited, frame) {
                    Ok(kept) => Ok(kept),
                    // The worker makes the masks, behind everything
                    // else, and the frame is asked for again after.
                    Err(crate::reduced::Why::AskMasks(which)) => {
                        tracing::info!(
                            "the edit's pictures of {}: {} not in the mask cache; the worker is asked",
                            frame.said(),
                            which.join(", ")
                        );
                        let (lock, cv) = &*queue;
                        let mut q = lock.lock().expect("worker queue");
                        if !q.masks_for.iter().any(|p| p == frame) {
                            q.masks_for.push_back(frame.clone());
                        }
                        cv.notify_one();
                        Ok(false)
                    }
                    // Asked already: the worker's answer brings it back.
                    Err(crate::reduced::Why::Masks(_)) => Ok(false),
                    Err(why) => Err(why),
                })
            },
            at_once: {
                let (edited, threads) = (edited.clone(), crate::thumbpool::default_threads());
                // For a measurement: how many at once whatever the window
                // is doing.
                let fixed = std::env::var("GREYCARD_REDUCED_AT_ONCE")
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|&n| n > 0);
                Arc::new(move || match fixed {
                    Some(n) => (n, None),
                    None => crate::reduced::begin(threads, edited.any_open(), edited.idle_for()),
                })
            },
            alone: {
                let reducer = reducer.clone();
                Arc::new(move |frame| reducer.alone(frame))
            },
        };
        let pool = crate::thumbpool::Pool::with_lookup(
            crate::thumbpool::default_threads(),
            lookup,
            make,
            deliver.clone(),
        )
        .with_previews(hooks)
        .with_edits(edits);
        let pool = Arc::new(if batched { pool.batched() } else { pool });
        let (q, d, p, e, r) = (
            queue.clone(),
            deliver.clone(),
            pool.clone(),
            edited.clone(),
            reducer.clone(),
        );
        let thread = std::thread::Builder::new()
            .name("greycard worker".into())
            .spawn(move || run(q, d, p, e, r))
            .expect("spawning the worker");
        Self {
            queue,
            deliver,
            thread: Mutex::new(Some(thread)),
            thumbs,
            caps: Arc::new(Mutex::new(None)),
            pool,
            previews,
            edited,
            reducer,
        }
    }

    /// Which frames show their edit: the window says which it holds as
    /// edited, and the culling loupe looks their pictures up.
    pub(crate) fn edited(&self) -> Arc<crate::edited::Edited> {
        self.edited.clone()
    }

    /// The frame open in the editor, or none: culling, the grid, or no
    /// frame. While there is one, the reduced develops run one at a
    /// time and the learned masks for them wait (`Job::MasksFor`); the
    /// pool and the worker look again when it changes.
    pub(crate) fn set_open_frame(&self, frame: Option<Frame>) {
        if self.edited.set_open(frame) {
            self.pool.wake();
            let (_, cv) = &*self.queue;
            cv.notify_one();
        }
    }

    /// The roots on a network mount: a frame under one has the reduced
    /// develop of its pictures alone.
    pub(crate) fn set_network_roots(&self, roots: Vec<PathBuf>) {
        self.reducer.set_network(roots);
    }

    /// The local previews, for the culling loupe's decode threads,
    /// which read them for a frame out of reach and make them from a
    /// picture they decoded anyway.
    pub(crate) fn previews(&self) -> Arc<crate::previews::Previews> {
        self.previews.clone()
    }

    /// The roots whose frames get a local preview from now on.
    pub(crate) fn set_preview_roots(&self, roots: Vec<PathBuf>) {
        self.previews.set_roots(roots);
    }

    /// Look thumbnails up in `cache`, and keep the ones made, from
    /// the next thumbnail on.
    /// The cache's one walk to count what it holds starts at once, on
    /// a thread of its own, so neither the window nor a thumbnail waits
    /// on it.
    pub fn set_thumb_cache(&self, cache: Option<Thumbs>) {
        *self.caps.lock().expect("thumbnail caps") =
            cache.as_ref().map(|c| (c.cap(), c.preview_cap()));
        *self.thumbs.lock().expect("thumbnail cache") = cache;
        count_thumb_cache(&self.thumbs, |_| {});
    }

    /// The thumbnail cache, for the settings sheet.
    pub fn thumb_cache(&self) -> ThumbCache {
        self.thumbs.clone()
    }

    /// The cache's caps in bytes, thumbnails' then previews', as last
    /// set, or `None` with no cache. Never waits on the cache.
    pub(crate) fn thumb_caps(&self) -> Option<(u64, u64)> {
        *self.caps.lock().expect("thumbnail caps")
    }

    /// The caps as last set, for a thread that applies them: one
    /// that waited on the cache's lock applies the newest, not the one
    /// it was started for.
    pub(crate) fn thumb_caps_shared(&self) -> ThumbCaps {
        self.caps.clone()
    }

    /// Record a new thumbnails' cap for the sheet to show, ahead of the
    /// cache taking it on a thread of its own.
    pub(crate) fn note_thumbs_cap(&self, bytes: u64) {
        if let Some(caps) = self.caps.lock().expect("thumbnail caps").as_mut() {
            caps.0 = bytes;
        }
    }

    /// The same for the previews' cap.
    pub(crate) fn note_previews_cap(&self, bytes: u64) {
        if let Some(caps) = self.caps.lock().expect("thumbnail caps").as_mut() {
            caps.1 = bytes;
        }
    }

    /// Stop the worker and wait for it to put its GPU buffers down.
    ///
    /// A process that exits while the worker is inside the driver
    /// dies there rather than at its own hand: the buffers of a
    /// develop half way through go with a device the window has
    /// already torn down. So the editor asks the worker to stop as
    /// its window closes and waits [`LEAVING`] for it to say it has.
    ///
    /// True when it has, its thread joined. False when it is still in
    /// a job after that: its thread is kept, so [`Worker::running`]
    /// and [`Worker::finished`] still say so, and the caller must not
    /// let the process go through the C library's exit with it there.
    /// The exit handlers tear the GPU driver down under a worker that
    /// may be waiting on a fence in it, and the process dies there
    /// (`startup::main` leaves by `_exit` instead).
    pub fn stop(&self) -> bool {
        self.stop_within(LEAVING)
    }

    /// [`Worker::stop`], waiting `limit` rather than [`LEAVING`].
    pub(super) fn stop_within(&self, limit: std::time::Duration) -> bool {
        self.pool.stop();
        {
            let (lock, cv) = &*self.queue;
            lock.lock().expect("worker queue").stopping = true;
            cv.notify_all();
        }
        let Some(thread) = self.thread.lock().expect("worker thread").take() else {
            return true;
        };
        let asked = std::time::Instant::now();
        while !thread.is_finished() && asked.elapsed() < limit {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if thread.is_finished() {
            let _ = thread.join();
            tracing::debug!("worker: stopped in {:.0} ms", asked.elapsed().as_millis());
            true
        } else {
            tracing::warn!(
                "worker: still busy after {:.1} s; leaving it",
                limit.as_secs_f64()
            );
            *self.thread.lock().expect("worker thread") = Some(thread);
            false
        }
    }

    /// Whether the worker's thread has ended: joined by
    /// [`Worker::stop`], or ended since and joined here. False while
    /// it is still in the job a stop left it in, or has not been asked
    /// to stop.
    pub fn finished(&self) -> bool {
        let mut held = self.thread.lock().expect("worker thread");
        match held.take_if(|t| t.is_finished()) {
            Some(thread) => {
                let _ = thread.join();
                true
            }
            None => held.is_none(),
        }
    }

    /// Whether the worker's thread is still there to take a job: a
    /// panic outside any job's guard ends it, and whoever waits on an
    /// outcome would otherwise wait for ever.
    pub fn running(&self) -> bool {
        self.thread
            .lock()
            .expect("worker thread")
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    }

    pub fn send(&self, job: Job) {
        if let Job::Fetch { model } = job {
            let (deliver, reducer) = (self.deliver.clone(), self.reducer.clone());
            std::thread::Builder::new()
                .name("greycard fetch".into())
                .spawn(move || {
                    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if fetch(model, &*deliver) {
                            // The frames whose pictures waited on a model
                            // are tried again: their cells ask.
                            for frame in reducer.models_arrived() {
                                deliver(Outcome::EditedKept { frame });
                            }
                        }
                    }));
                    if let Err(payload) = held {
                        deliver(Outcome::FetchFailed {
                            id: model.id,
                            name: model.name,
                            message: format!(
                                "the fetch panicked: {}",
                                panic_message(payload.as_ref())
                            ),
                        });
                    }
                })
                .expect("spawning the fetch");
            return;
        }
        if let Job::FetchLenses = job {
            let (deliver, queue) = (self.deliver.clone(), self.queue.clone());
            std::thread::Builder::new()
                .name("greycard lens fetch".into())
                .spawn(move || {
                    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        fetch_lenses(&queue, &*deliver)
                    }));
                    if let Err(payload) = held {
                        deliver(Outcome::LensesFetchFailed {
                            message: format!(
                                "the fetch panicked: {}",
                                panic_message(payload.as_ref())
                            ),
                        });
                    }
                })
                .expect("spawning the lens fetch");
            return;
        }
        if let Job::Thumbnail { index, frame } = job {
            self.pool.push(index, frame);
            return;
        }
        if let Job::CachedThumbnail {
            index,
            frame,
            hash,
            stamp,
        } = job
        {
            // Off the window's thread, since a hit reads the entry's
            // file; not through the pool, whose lookup and making both
            // start from the file, which this frame's cannot be. At
            // the size the pool makes and keeps pictures at, or the
            // entry it kept is never found.
            let (thumbs, deliver, size, edited) = (
                self.thumbs.clone(),
                self.deliver.clone(),
                crate::grid::made_size(self.pool.size()),
                self.edited.clone(),
            );
            rayon::spawn(move || {
                let started = Instant::now();
                // Caught here: a panic on rayon's pool with no handler
                // takes the editor down with it. A frame held as edited
                // shows the last picture of its edit kept, its sidecar
                // being out of reach with its file.
                let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    edited
                        .lookup(&hash, size, edited.shows_offline(&frame))
                        .or_else(|| {
                            thumbs
                                .lock()
                                .expect("thumbnail cache")
                                .as_mut()
                                .and_then(|c| {
                                    c.get(
                                        &hash,
                                        size,
                                        Tag {
                                            recipe: THUMB_RECIPE,
                                            stamp,
                                        },
                                    )
                                })
                        })
                }))
                .unwrap_or_else(|payload| {
                    tracing::warn!(
                        "thumbnail {}: the cache's lookup panicked: {}",
                        frame.said(),
                        panic_message(payload.as_ref())
                    );
                    None
                });
                deliver(match hit {
                    Some(thumb) => Outcome::Thumbnail {
                        index,
                        frame,
                        size,
                        width: thumb.width,
                        height: thumb.height,
                        rgb: thumb.rgb,
                        cached: true,
                        seconds: started.elapsed().as_secs_f64(),
                    },
                    None => Outcome::NoThumbnail { index, frame },
                });
            });
            return;
        }
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("worker queue");
        match job {
            Job::Thumbnail { .. } | Job::CachedThumbnail { .. } => {
                unreachable!("thumbnails go to the pool above")
            }
            job @ Job::ExportFrame { .. } => q.exports.push_back(job),
            Job::ExportSet { set, frames } => {
                for (index, frame) in frames.into_iter().enumerate() {
                    q.exports.push_back(Job::ExportFrame {
                        set: set.clone(),
                        index,
                        frame,
                    });
                }
            }
            Job::Mask { key, shape, model } => {
                q.masks.retain(|(k, _, _)| *k != key);
                q.masks.push_back((key, shape, model));
            }
            Job::People => q.people = true,
            Job::Landscape => q.landscape = true,
            Job::Keep {
                frame,
                edit,
                turn,
                iso,
            } => {
                q.keeps
                    .retain(|k| !matches!(k, Job::Keep { frame: f, .. } if *f == frame));
                q.keeps.push_back(Job::Keep {
                    frame,
                    edit,
                    turn,
                    iso,
                });
            }
            job => {
                // The frame being opened is the open frame from here:
                // the pool must not develop it for its pictures while
                // the open is on its way.
                if let Job::Open { path, version, .. } = &job
                    && self.edited.set_open(Some(Frame {
                        path: path.clone(),
                        version: version.clone(),
                        name: None,
                    }))
                {
                    self.pool.wake();
                }
                // The editor is being worked in: the queue waits for it
                // to be quiet again.
                if matches!(job, Job::Open { .. } | Job::Develop { .. }) {
                    self.edited.touch();
                }
                q.develop = Some(job);
            }
        }
        cv.notify_one();
    }

    /// Run the engine's GPU ops on this device from now on. The
    /// context is built on the worker's thread, so the shaders compile
    /// there and not in the window's setup.
    pub fn set_gpu(&self, device: &greycard_gpu::wgpu::Device, queue: &greycard_gpu::wgpu::Queue) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").gpu = Some((device.clone(), queue.clone()));
        cv.notify_one();
    }

    /// A Subject model just arrived in the store: the worker's loaded
    /// one, if any, is dropped, so the next Subject mask loads
    /// whichever file `step` now picks rather than the one already
    /// running.
    pub fn forget_subject(&self) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").forget_subject = true;
        cv.notify_one();
    }

    /// Make the thumbnails for `first..=last` before the folder's
    /// others, the rest outward from that range. The strip says what
    /// it shows; a folder of hundreds is otherwise decoded in file
    /// order, and the frames on screen wait for every earlier one.
    pub fn want_thumbnails(&self, first: usize, last: usize) {
        self.pool.want(first, last);
    }

    /// Another folder's list: the thumbnails still waiting are
    /// dropped, and one in hand is not delivered. Its numbering is
    /// the old list's, and a late picture must not land on the new.
    pub fn forget_thumbnails(&self) {
        self.pool.forget();
    }

    /// How many threads make thumbnails.
    pub fn thumb_threads(&self) -> usize {
        self.pool.threads()
    }

    /// Hold the thumbnails' threads for a develop the window is
    /// about to ask for, or let them go: see `thumbpool::Pool::hold`.
    pub fn hold_thumbnails(&self, on: bool) {
        self.pool.hold(on);
    }

    /// Put these thumbnails in the queue in place of the ones there:
    /// a new list, or the same list renumbered by a merge, whose
    /// queued jobs name files by numbers that now mean others. Those
    /// would be made and thrown away on arrival (a delivery checks the
    /// path), and a frame whose job was among them would never get its
    /// picture, which is how a merge blanked the grid.
    /// `wanted`, the files on screen, are made first, as the grid's
    /// range would have them.
    pub fn replace_thumbnails(&self, list: Vec<(usize, Frame)>, wanted: Option<(usize, usize)>) {
        self.pool.forget();
        self.pool.push_all(list);
        if let Some((first, last)) = wanted {
            self.pool.want(first, last);
        }
    }

    /// Make thumbnails at this long edge from now on. The grid's
    /// cells grow past the strip's 178, and a picture made for the
    /// strip is mush in a large one; the size follows the cell both
    /// ways, so a cell that has shrunk stops paying for the one
    /// before it. Whether a picture already made is made again is
    /// the caller's business, and that only ever steps up.
    pub fn set_thumb_size(&self, size: u32) {
        self.pool.set_size(size);
    }
}

/// The frames on the strip first, in file order, then the rest
/// outward from the middle of that range: what a scroll asks for
/// next is whichever end it is heading towards.
pub(crate) fn order_thumbnails<T>(pending: &mut [(usize, T)], first: usize, last: usize) {
    // Ties broken by the frame's number, so two the same distance
    // from the center come in the list's order whichever thread
    // queued them first.
    let center = (first + last) / 2;
    pending.sort_by_key(|(i, _)| {
        let away = if (first..=last).contains(i) {
            0
        } else {
            i.abs_diff(center)
        };
        (away, *i)
    });
}

/// Fetch a model, reporting as it comes.
/// Fetch `model` into the store, saying how it goes; whether it is
/// there now.
fn fetch(model: &'static greycard_ai::Model, deliver: &dyn Fn(Outcome)) -> bool {
    let name = model.name;
    let store = match greycard_ai::Store::user() {
        Ok(s) => s,
        Err(e) => {
            deliver(Outcome::FetchFailed {
                id: model.id,
                name,
                message: e.to_string(),
            });
            return false;
        }
    };
    let mut last = Instant::now();
    let result = store.fetch(model, |p| {
        // A report every so often, and the first and the last.
        if p.done == 0 || p.done == p.total || last.elapsed().as_millis() > 200 {
            last = Instant::now();
            deliver(Outcome::Fetching {
                name,
                done: p.done,
                total: p.total,
            });
        }
    });
    let fetched = result.is_ok();
    deliver(match result {
        Ok(()) => Outcome::Fetched { model },
        Err(e) => Outcome::FetchFailed {
            id: model.id,
            name,
            message: e.to_string(),
        },
    });
    fetched
}

/// Fetch the lens database, reporting as it comes, and hand it to the
/// worker.
fn fetch_lenses(queue: &Arc<(Mutex<Queue>, Condvar)>, deliver: &dyn Fn(Outcome)) {
    let store = match greycard_lens::Store::user() {
        Ok(s) => s,
        Err(e) => {
            deliver(Outcome::LensesFetchFailed {
                message: e.to_string(),
            });
            return;
        }
    };
    let mut last = Instant::now();
    let result = store.fetch(|p| {
        if p.done == 0 || p.done == p.total || last.elapsed().as_millis() > 200 {
            last = Instant::now();
            deliver(Outcome::LensesFetching {
                done: p.done,
                total: p.total,
            });
        }
    });
    let loaded =
        result.and_then(|()| greycard_lens::Database::load_dir(&store.dir()).map(Arc::new));
    match loaded {
        Ok(db) => {
            let (lock, cv) = &**queue;
            lock.lock().expect("worker queue").lenses = Some(db);
            cv.notify_one();
            // The index's keys are made under the new name from here,
            // before the window asks it to clear the old ones.
            greycard_library::set_lens_database(crate::edited::lens_database());
            deliver(Outcome::LensesFetched);
        }
        Err(e) => deliver(Outcome::LensesFetchFailed {
            message: e.to_string(),
        }),
    }
}
