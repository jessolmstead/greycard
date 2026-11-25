//! The worker's handle: the editor's side of the queue, and the model
//! and lens fetches it starts.

use super::*;

impl Worker {
    /// A worker whose thumbnails come to `deliver` one call each: the
    /// tests', which read them off a channel.
    #[cfg(any(test, feature = "testing"))]
    pub fn new(deliver: impl Fn(Outcome) + Send + Sync + 'static) -> Self {
        Self::build(Arc::new(deliver), false, true)
    }

    /// A worker whose thumbnails come to `deliver` in batches, as
    /// [`Outcome::Thumbnails`]: the window's, where a call is a turn
    /// of the event loop and a warm folder of thousands would take
    /// every one before drawing.
    pub fn batched(deliver: impl Fn(Outcome) + Send + Sync + 'static) -> Self {
        Self::build(Arc::new(deliver), true, true)
    }

    /// A worker with no background side, no thumbnail pool and no
    /// reduced develops: headless export's, which asks for neither and
    /// should start no thread it would leave idle.
    pub fn headless(deliver: impl Fn(Outcome) + Send + Sync + 'static) -> Self {
        Self::build(Arc::new(deliver), false, false)
    }

    /// The worker's queue, held weakly: gone once every thread that
    /// held it has ended, for the tests that a dropped worker leaves
    /// none behind.
    #[cfg(any(test, feature = "testing"))]
    pub fn queue_held(&self) -> std::sync::Weak<dyn std::any::Any + Send + Sync> {
        let queue: Arc<dyn std::any::Any + Send + Sync> = self.queue.clone();
        Arc::downgrade(&queue)
    }

    fn build(deliver: Deliver, batched: bool, background: bool) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let thumbs: ThumbCache = Arc::new(Mutex::new(None));
        let edited = Arc::new(crate::edited_pictures::Edited::new(thumbs.clone()));
        let previews =
            Arc::new(crate::previews::Previews::new(thumbs.clone()).with_edited(edited.clone()));
        // The background side: the thumbnails' pool, which makes the
        // previews and the edit's pictures behind the thumbnails, and
        // the reduced develops it runs.
        let background = background.then(|| {
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
                        // Behind the worker's prefetch of a neighbor's base,
                        // which wakes the pool when it ends.
                        None => super::prefetch::behind_prefetch(edited.prefetching(), || {
                            crate::reduced::begin(threads, edited.any_open(), edited.idle_for())
                        }),
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
            Arc::new(Background { pool, reducer })
        });
        let (q, d, e, b) = (
            queue.clone(),
            deliver.clone(),
            edited.clone(),
            background.clone(),
        );
        let thread = std::thread::Builder::new()
            .name("greycard worker".into())
            .spawn(move || run(q, d, e, b))
            .expect("spawning the worker");
        Self {
            queue,
            deliver,
            thread: Mutex::new(Some(thread)),
            thumbs,
            caps: Arc::new(Mutex::new(None)),
            background,
            previews,
            edited,
        }
    }

    /// The thumbnails' pool, which a worker with no background side has
    /// not got.
    pub(super) fn pool(&self) -> Option<&crate::thumbpool::Pool> {
        self.background.as_ref().map(|b| &*b.pool)
    }

    /// Which frames show their edit: the window says which it holds as
    /// edited, and the culling loupe looks their pictures up.
    pub fn edited(&self) -> Arc<crate::edited_pictures::Edited> {
        self.edited.clone()
    }

    /// The frame open in the editor, or none: culling, the grid, or no
    /// frame. While there is one, the reduced develops run one at a
    /// time and the learned masks for them wait (`Job::MasksFor`); the
    /// pool and the worker look again when it changes.
    pub fn set_open_frame(&self, frame: Option<Frame>) {
        // No frame open: the neighbors of the last one are not wanted.
        if frame.is_none() {
            self.queue.0.lock().expect("worker queue").neighbors = None;
            self.queue.1.notify_one();
        }
        if self.edited.set_open(frame) {
            if let Some(b) = &self.background {
                b.pool.wake();
            }
            // Told under the queue's lock, so a worker that has just
            // found no frame open (its fills paused) is already waiting.
            let (lock, cv) = &*self.queue;
            let _q = lock.lock().expect("worker queue");
            cv.notify_one();
        }
    }

    /// How to tell whether a folder may be read ahead (the window's
    /// `mounts::not_to_peek`): a neighbor in a folder on a network mount
    /// or an automount not yet mounted is not read (`prefetch::read_first`).
    /// Asked at every read, nothing of its answers kept; handed over
    /// again when the mounts may have changed, the folders the worker
    /// has said it passed over are said again.
    pub fn set_remote_check(&self, check: super::prefetch::RemoteFs) {
        let mut q = self.queue.0.lock().expect("worker queue");
        q.remote_check = Some(check);
        #[cfg(any(test, feature = "testing"))]
        {
            q.remote_checks_by_test.0 += 1;
            q.remote_checks_by_test.1 = Some(check);
        }
    }

    /// How many checks of what a folder is on were handed over, and the
    /// last: what the window's tests read of its wiring.
    #[cfg(any(test, feature = "testing"))]
    pub fn remote_checks(&self) -> (usize, Option<super::prefetch::RemoteFs>) {
        self.queue
            .0
            .lock()
            .expect("worker queue")
            .remote_checks_by_test
    }

    /// The roots on a network mount: a frame under one has the reduced
    /// develop of its pictures alone.
    pub fn set_network_roots(&self, roots: Vec<PathBuf>) {
        if let Some(b) = &self.background {
            b.reducer.set_network(roots);
        }
    }

    /// The local previews, for the culling loupe's decode threads,
    /// which read them for a frame out of reach and make them from a
    /// picture they decoded anyway.
    pub fn previews(&self) -> Arc<crate::previews::Previews> {
        self.previews.clone()
    }

    /// The roots whose frames get a local preview from now on.
    pub fn set_preview_roots(&self, roots: Vec<PathBuf>) {
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
    pub fn thumb_caps(&self) -> Option<(u64, u64)> {
        *self.caps.lock().expect("thumbnail caps")
    }

    /// The caps as last set, for a thread that applies them: one
    /// that waited on the cache's lock applies the newest, not the one
    /// it was started for.
    pub fn thumb_caps_shared(&self) -> ThumbCaps {
        self.caps.clone()
    }

    /// Record a new thumbnails' cap for the sheet to show, ahead of the
    /// cache taking it on a thread of its own.
    pub fn note_thumbs_cap(&self, bytes: u64) {
        if let Some(caps) = self.caps.lock().expect("thumbnail caps").as_mut() {
            caps.0 = bytes;
        }
    }

    /// The same for the previews' cap.
    pub fn note_previews_cap(&self, bytes: u64) {
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

    /// [`Worker::stop`], then the thumbnails' pool and the keeping of
    /// the edit's pictures waited on too, up to `limit`, so nothing of
    /// any of them writes to the cache after: what a test does before
    /// it removes its folder. Whether all of them ended.
    #[cfg(any(test, feature = "testing"))]
    pub fn stop_all(&self, limit: std::time::Duration) -> bool {
        let asked = std::time::Instant::now();
        let stopped = self.stop_within(limit);
        while self.pool().is_some_and(|p| !p.ended()) || !self.edited.keeping_idle() {
            if asked.elapsed() >= limit {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        stopped
    }

    /// [`Worker::stop`], waiting `limit` rather than [`LEAVING`].
    pub(super) fn stop_within(&self, limit: std::time::Duration) -> bool {
        self.close();
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

    /// Ask the worker's thread and the pool's to stop, and wait for
    /// neither: what [`Drop`] does.
    fn close(&self) {
        if let Some(b) = &self.background {
            b.pool.stop();
        }
        let (lock, cv) = &*self.queue;
        // Poisoned or not: a drop while a panic unwinds aborts if it
        // panics too.
        lock.lock().unwrap_or_else(|e| e.into_inner()).stopping = true;
        cv.notify_all();
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

    /// The profile folder walked again behind the open frame, off the
    /// window's thread, as the LENS section opens: on a file system whose
    /// folders keep their clock when a file is added (FAT, exFAT, a
    /// share's cache), only a walk finds it. Another list than the one
    /// shown comes as [`Outcome::LensOptions`], with `chosen` among it:
    /// always, since this look may take the place of one asked for the
    /// open that has not run, whose list the window has not had.
    /// Nothing for no frame open.
    pub fn look_at_lens_profiles(&self, chosen: &greycard_edit::LensChoice) {
        let look = self.queue.0.lock().expect("worker queue").lens_look.clone();
        if let Some(look) = look {
            look.ask(chosen, None, true, &self.deliver);
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
        if let Job::Fetch { model, warm } = job {
            let (deliver, reducer) = (self.deliver.clone(), self.background.clone());
            std::thread::Builder::new()
                .name("greycard fetch".into())
                .spawn(move || {
                    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if fetch(model, warm, &*deliver) {
                            // The frames whose pictures waited on a model
                            // are tried again: their cells ask.
                            for frame in reducer.iter().flat_map(|b| b.reducer.models_arrived()) {
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
        if let Job::FetchRuntime { runtime } = job {
            let (deliver, reducer) = (self.deliver.clone(), self.background.clone());
            std::thread::Builder::new()
                .name("greycard runtime fetch".into())
                .spawn(move || {
                    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if fetch_runtime(runtime, &*deliver) {
                            for frame in reducer.iter().flat_map(|b| b.reducer.models_arrived()) {
                                deliver(Outcome::EditedKept { frame });
                            }
                        }
                    }));
                    if let Err(payload) = held {
                        deliver(Outcome::FetchFailed {
                            id: runtime.id,
                            name: runtime.name,
                            message: format!(
                                "the fetch panicked: {}",
                                panic_message(payload.as_ref())
                            ),
                        });
                    }
                })
                .expect("spawning the runtime fetch");
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
            if let Some(pool) = self.pool() {
                pool.push(index, frame);
            }
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
                crate::made::made_size(self.pool().map_or(THUMB_WIDTH, |p| p.size())),
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
                                    let at = |recipe| Tag { recipe, stamp };
                                    c.get(&hash, size, at(super::thumbs::thumb_recipe(&frame.path)))
                                        .or_else(|| {
                                            let earlier =
                                                super::thumbs::earlier_recipe(&frame.path)?;
                                            c.get(&hash, size, at(earlier))
                                        })
                                })
                                .map(|t| {
                                    super::thumbs::cut_to_camera(
                                        t,
                                        &frame.path,
                                        Some(&edited),
                                        false,
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
            job @ Job::MaskPicture { .. } => q.mask_picture = Some(job),
            job @ Job::PersonOn(_) => q.people_on.push_back(job),
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
                let opening = match &q.develop {
                    Some(Job::Open { generation, .. }) => Some(*generation),
                    _ => None,
                };
                let (job, folded) = into_slot(q.develop.take(), job);
                match (&job, opening) {
                    // The open lands at the develop's generation now.
                    (Job::Open { generation, .. }, Some(old)) if folded => {
                        q.open_folded(old, *generation);
                    }
                    (Job::Open { .. }, _) => q.folded = None,
                    _ => {}
                }
                // The frame being opened is the open frame from here:
                // the pool must not develop it for its pictures while
                // the open is on its way.
                if let Job::Open { path, version, .. } = &job
                    && self.edited.set_open(Some(Frame {
                        path: path.clone(),
                        version: version.clone(),
                        name: None,
                    }))
                    && let Some(pool) = self.pool()
                {
                    pool.wake();
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

    /// The open or develop waiting in the slot, for a window's test: its
    /// kind, its generation, and the file it names.
    #[cfg(any(test, feature = "testing"))]
    pub fn slot_by_test(&self) -> Option<(&'static str, u64, Option<std::path::PathBuf>)> {
        let q = self.queue.0.lock().expect("worker queue");
        q.develop.as_ref().map(|job| match job {
            Job::Open {
                generation, path, ..
            } => ("open", *generation, Some(path.clone())),
            Job::Develop { generation, of, .. } => {
                ("develop", *generation, of.as_ref().map(|o| o.0.clone()))
            }
            Job::DevelopAside { generation, of, .. } => {
                ("aside", *generation, of.as_ref().map(|o| o.0.clone()))
            }
            _ => ("other", 0, None),
        })
    }

    /// The frames around the one just opened at `generation`, the
    /// strip walked as `heading` says, in the order their bases are to
    /// be made (the nearest ahead first): made onto the shelf while the
    /// worker has nothing else to do, once that open's develop has
    /// landed, as deep as the shelf has room for (`prefetch`). Those of
    /// an earlier open still waiting are let go.
    pub fn neighbors(&self, generation: u64, heading: Heading, neighbors: Vec<Neighbor>) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("worker queue");
        let generation = q.folded_to(generation);
        q.neighbors = Some(super::prefetch::Neighbors::new(
            generation, heading, neighbors,
        ));
        drop(q);
        cv.notify_one();
    }

    /// The frames the arrow keys reach from `open`, the frame the
    /// editor has open under the open the window sent at `generation`,
    /// sent again because what they are has moved since (`why`, for the
    /// log): the editor came back from culling or the grid, which let
    /// the last ones go, or the strip's filter, its order, or a
    /// neighbor's edit changed. Taken in place of the list that open
    /// sent, or of none once that open's develop has landed on that
    /// file; let go when another open has been sent since, or the open
    /// never landed and its list is gone. Whether it was taken.
    pub fn neighbors_again(
        &self,
        generation: u64,
        open: &std::path::Path,
        heading: Heading,
        neighbors: Vec<Neighbor>,
        why: &str,
    ) -> bool {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("worker queue");
        let generation = q.folded_to(generation);
        let ours = match &q.neighbors {
            Some(waiting) => waiting.generation == generation,
            None => q
                .landed
                .as_ref()
                .is_some_and(|(g, path)| *g == generation && path == open),
        };
        if !ours {
            return false;
        }
        let named: Vec<String> = neighbors
            .iter()
            .map(|n| format!("{} {}", n.place(), super::engine::file_label(&n.path)))
            .collect();
        tracing::info!(
            "neighbors resent after {why}: {}",
            if named.is_empty() {
                "none".to_string()
            } else {
                named.join(", ")
            }
        );
        q.neighbors = Some(super::prefetch::Neighbors::new(
            generation, heading, neighbors,
        ));
        cv.notify_one();
        true
    }

    /// The neighbors waiting to be made, with the open they are for:
    /// what the window's tests read of what it sent.
    #[cfg(any(test, feature = "testing"))]
    pub fn neighbors_waiting(&self) -> Option<(u64, Vec<(std::path::PathBuf, Side)>)> {
        let q = self.queue.0.lock().expect("worker queue");
        q.neighbors.as_ref().map(|n| {
            (
                n.generation,
                n.waiting.iter().map(|n| (n.path.clone(), n.side)).collect(),
            )
        })
    }

    /// The edit of the file at `path` changed without an open of it (a
    /// sync, a paste, a sidecar written by another hand): its bases on
    /// the shelf that what it holds `now` no longer serves
    /// ([`FileNow::serves`]) are let go, every one of the file's when
    /// the window does not have that at hand. A neighbor sent again
    /// under the new edit is then made afresh.
    pub fn edit_changed(&self, path: std::path::PathBuf, now: Option<FileNow>) {
        let (lock, cv) = &*self.queue;
        lock.lock()
            .expect("worker queue")
            .edits_changed
            .push((path, now));
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

    /// Run the engine's GPU ops on `ctx`, built by the caller on a
    /// device of its own (`Context::for_export`), whose lost callback
    /// is the context's: the command line's export, which has no window
    /// to lend one and would otherwise build the ops' pipelines twice.
    pub fn set_context(&self, ctx: greycard_gpu::Context) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").context = Some(ctx);
        cv.notify_one();
    }

    /// What tells the worker the window's device was lost, from
    /// whichever thread found the loss (the device's lost callback
    /// runs there): the engine's ops go to the CPU from then on, and
    /// with `again` the develop on screen, made on the lost device, is
    /// made again there.
    pub fn gpu_lost_teller(&self) -> impl Fn(bool) + Send + Sync + use<> {
        let queue = self.queue.clone();
        move |again| {
            let (lock, cv) = &*queue;
            lock.lock().unwrap_or_else(|e| e.into_inner()).gpu_lost = Some(again);
            cv.notify_one();
        }
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

    /// A model arrived in the store: a learned mask that could not be
    /// made again on the retouched picture is tried again.
    pub fn models_arrived(&self) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").models_arrived = true;
        cv.notify_one();
    }

    /// Make the thumbnails for `first..=last` before the folder's
    /// others, the rest outward from that range. The strip says what
    /// it shows; a folder of hundreds is otherwise decoded in file
    /// order, and the frames on screen wait for every earlier one.
    pub fn want_thumbnails(&self, first: usize, last: usize) {
        if let Some(pool) = self.pool() {
            pool.want(first, last);
        }
    }

    /// The cells of the rows near those on screen
    /// ([`crate::thumbpool::near_rows`]): the thumbnails the cache lacks,
    /// the edit's pictures, and the previews outside culling are made
    /// for these alone.
    pub fn near_thumbnails(&self, cells: std::collections::HashSet<usize>) {
        if let Some(pool) = self.pool() {
            pool.set_near(cells);
        }
    }

    /// The card as a test has it: `free` bytes free, `freed` more once
    /// the fills' sessions are dropped, and `elsewhere` more each time
    /// the card is given back across the worker's models.
    #[cfg(test)]
    pub(super) fn free_by_test(&self, free: u64, freed: u64, elsewhere: u64) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("worker queue");
        q.free_by_test = Some((free, freed));
        q.elsewhere_by_test = elsewhere;
        cv.notify_one();
    }

    /// A UNet held for a queue of fills at `side`, holding `bytes` of
    /// the card until it is let go.
    #[cfg(test)]
    pub(super) fn unet_by_test(&self, side: usize, bytes: u64) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").unet_by_test = Some((side, bytes));
        cv.notify_one();
    }

    /// Each of the open frame's fills begun so far, by name, and the
    /// side it ran at.
    #[cfg(test)]
    pub(super) fn ran_at_by_test(&self) -> Vec<(String, usize)> {
        let (lock, _) = &*self.queue;
        lock.lock().expect("worker queue").ran_at_by_test.clone()
    }

    /// How many times the worker has given the card back.
    #[cfg(test)]
    pub(super) fn gave_back_by_test(&self) -> usize {
        let (lock, _) = &*self.queue;
        lock.lock().expect("worker queue").gave_back_by_test
    }

    /// The open frame's fills let go of `bytes` of the card each time a
    /// removal is short of it, counted in `calls`, as the device's
    /// working sets would be.
    #[cfg(test)]
    pub(super) fn fills_release_by_test(
        &self,
        bytes: u64,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").fills_release_by_test =
            Some(super::engine::FillsReleaseByTest { bytes, calls });
        cv.notify_one();
    }

    /// Whether a card release was left set on the open frame's models
    /// between two jobs.
    #[cfg(test)]
    pub(super) fn release_left_by_test(&self) -> bool {
        let (lock, _) = &*self.queue;
        lock.lock().expect("worker queue").release_left_by_test
    }

    /// The shelf made anew with `budget`, before the first open.
    #[cfg(test)]
    pub(super) fn shelf_by_test(&self, budget: super::shelf::Budget) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").shelf_by_test = Some(budget);
        cv.notify_one();
    }

    /// The open frame's masks read from and made with the models in
    /// `store`, and the mask cache beside it, before the first open.
    #[cfg(test)]
    pub(super) fn models_by_test(&self, store: greycard_ai::Store) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").models_by_test = Some(store);
        cv.notify_one();
    }

    /// The next open held once its file is read, before what it read is
    /// said: the first receiver hears it held, the open goes on once the
    /// sender is used or dropped.
    #[cfg(test)]
    pub(super) fn hold_open_by_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (told, held) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel();
        self.queue.0.lock().expect("worker queue").open_held_by_test = Some((told, wait));
        (held, go)
    }

    /// The next open panics where `at` says.
    #[cfg(test)]
    pub(super) fn open_panics_by_test(&self, at: super::OpenPanic) {
        self.queue
            .0
            .lock()
            .expect("worker queue")
            .open_panics_by_test = Some(at);
    }

    /// The patient waits slept by the end of each export frame so far.
    #[cfg(test)]
    pub(super) fn waits_by_test(&self) -> Vec<usize> {
        let (lock, _) = &*self.queue;
        lock.lock().expect("worker queue").waits_by_test.clone()
    }

    /// The open frame's fills made with no model, every value `v`.
    #[cfg(test)]
    pub(super) fn fill_by_test(&self, v: f32) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").fill_by_test = Some(v);
        cv.notify_one();
    }

    /// Make the local previews of these cells as well as the near ones
    /// (culling), or of the near ones alone (`None`): see
    /// `thumbpool::Pool::previews_ahead`.
    pub fn previews_ahead(&self, cells: Option<std::collections::HashSet<usize>>) {
        if let Some(pool) = self.pool() {
            pool.previews_ahead(cells);
        }
    }

    /// The cells whose previews are made ahead now.
    #[cfg(any(test, feature = "testing"))]
    pub fn previews_ahead_of(&self) -> std::collections::HashSet<usize> {
        self.pool()
            .map(|p| p.previews_ahead_of())
            .unwrap_or_default()
    }

    /// Another folder's list: the thumbnails still waiting are
    /// dropped, and one in hand is not delivered. Its numbering is
    /// the old list's, and a late picture must not land on the new.
    pub fn forget_thumbnails(&self) {
        if let Some(pool) = self.pool() {
            pool.forget();
        }
    }

    /// How many threads make thumbnails.
    pub fn thumb_threads(&self) -> usize {
        self.pool().map_or(0, |p| p.threads())
    }

    /// Hold the thumbnails' threads for a develop the window is
    /// about to ask for, or let them go: see `thumbpool::Pool::hold`.
    pub fn hold_thumbnails(&self, on: bool) {
        if let Some(pool) = self.pool() {
            pool.hold(on);
        }
    }

    /// Put these thumbnails in the queue in place of the ones there:
    /// a new list, or the same list renumbered by a merge, whose
    /// queued jobs name files by numbers that now mean others. Those
    /// would be made and thrown away on arrival (a delivery checks the
    /// path), and a frame whose job was among them would never get its
    /// picture, which is how a merge blanked the grid.
    /// `wanted`, the files on screen, are made first, as the grid's
    /// range would have them. `cells` numbers every frame of the list
    /// now, for what waits behind the thumbnails.
    pub fn replace_thumbnails(
        &self,
        list: Vec<(usize, Frame)>,
        wanted: Option<(usize, usize)>,
        cells: &std::collections::HashMap<Frame, usize>,
    ) {
        let Some(pool) = self.pool() else {
            return;
        };
        pool.renumber_to(cells);
        pool.push_all(list);
        if let Some((first, last)) = wanted {
            pool.want(first, last);
        }
    }

    /// Make thumbnails at this long edge from now on. The grid's
    /// cells grow past the strip's 178, and a picture made for the
    /// strip is mush in a large one; the size follows the cell both
    /// ways, so a cell that has shrunk stops paying for the one
    /// before it. Whether a picture already made is made again is
    /// the caller's business, and that only ever steps up.
    pub fn set_thumb_size(&self, size: u32) {
        if let Some(pool) = self.pool() {
            pool.set_size(size);
        }
    }
}

/// What the worker's one develop slot holds once `job` is sent over
/// `waiting`, what it held, and whether the two were folded: `job`, in
/// its place, but for a develop (its own or the Original's aside) for
/// the very frame, file and version, of an open still waiting there.
/// That develop becomes the open under its edit, generation, and turn:
/// dropped, the open's file would never be read (a frame chosen, or its
/// file written again under the window), the develop would run on the
/// base before it, and the open's `Opened` would never come. A develop
/// for none (the worker's own) takes the slot as it is; so does one for
/// another frame, which the window does not send (it asks develops only
/// of the frame of the last open it sent, `edit::develops_here`) and
/// the worker would not make (`holds`).
/// An aside's edit is the Original's, which the window's generation
/// says the develop that lands is (`original::landing`). An open the
/// worker has already taken folds a develop the same way once its file
/// is read (`Queue::take_develop_of`).
pub(super) fn into_slot(waiting: Option<Job>, job: Job) -> (Job, bool) {
    let folds = match &waiting {
        Some(Job::Open { path, version, .. }) => develops(&job, path, version),
        _ => false,
    };
    if !folds {
        return (job, false);
    }
    match (waiting, job) {
        (
            Some(Job::Open {
                path,
                version,
                seed_blend,
                stance_known,
                learned,
                ..
            }),
            Job::Develop {
                edit,
                generation,
                turn,
                ..
            }
            | Job::DevelopAside {
                edit,
                generation,
                turn,
                ..
            },
        ) => (
            Job::Open {
                path,
                version,
                edit,
                generation,
                seed_blend,
                turn,
                stance_known,
                learned,
            },
            true,
        ),
        (_, job) => (job, false),
    }
}

/// Whether `job` is a develop, the frame's own or the Original's aside,
/// the window sent for the frame `path`, `version`.
pub(super) fn develops(job: &Job, path: &std::path::Path, version: &Option<Arc<str>>) -> bool {
    match job {
        Job::Develop { of: Some(of), .. } | Job::DevelopAside { of: Some(of), .. } => {
            of.0 == path && of.1 == *version
        }
        _ => false,
    }
}

/// What a develop for `of` is to the worker holding the file
/// `opened`, whose open of `failed` failed last.
#[derive(Debug, PartialEq)]
pub(super) enum Holds<'a> {
    /// The file in hand, or no frame named (the worker's own): made.
    Frame,
    /// Another file, whose open failed last: answered with that
    /// failure.
    Failed(&'a str),
    /// Another file, never opened here: nothing is made, since a
    /// picture of the file in hand would be shown as this one's. The
    /// window sends none: it asks develops only of the file of the last
    /// open it sent (`edit::develops_here`), which the worker holds,
    /// is opening, or failed to open.
    Other,
}

/// Whether a develop sent for `of` is of the file the worker holds
/// (`opened`), and what it is answered with if not.
pub(super) fn holds<'a>(
    failed: Option<&'a (std::path::PathBuf, String)>,
    opened: Option<&std::path::Path>,
    of: Option<&DevelopOf>,
) -> Holds<'a> {
    let Some((wanted, _)) = of else {
        return Holds::Frame;
    };
    if opened == Some(wanted.as_path()) {
        return Holds::Frame;
    }
    match failed {
        Some((path, message)) if path == wanted => Holds::Failed(message),
        _ => Holds::Other,
    }
}

/// The frames on the strip first, in file order, then the rest
/// outward from the middle of that range: what a scroll asks for
/// next is whichever end it is heading towards.
pub fn order_thumbnails<T>(pending: &mut [(usize, T)], first: usize, last: usize) {
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
fn fetch(model: &'static greycard_ai::Model, warm: bool, deliver: &dyn Fn(Outcome)) -> bool {
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
                file: p.file,
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
    // On a Mac a denoiser tier's fp16 file follows on a thread of its
    // own, after the tier is said to be here, since the tier runs
    // without it and what waits on this fetch should not wait on that
    // one. The tier's first develop meanwhile loads its own file and
    // loads again when the fp16 one is here; a failure is logged, and
    // the editor's background fetch tries once more
    // (`ai::fetch_core_ml_quietly`).
    // Asked to warm, as a fetch of every model is, the thread makes
    // Core ML's compile too, after that file (`ai::warm_core_ml`, one
    // model at a time), for Sky as well as the tiers.
    let warms = warm && (model.id == greycard_ai::SKY.id || greycard_ai::tier_of(model).is_some());
    let core_ml_file = greycard_ai::registry::core_ml_file(model).is_some()
        && !greycard_ai::denoise::fp32_refused();
    if fetched && cfg!(target_os = "macos") && (core_ml_file || warms) {
        let spawned = std::thread::Builder::new()
            .name("core-ml-fetch".into())
            .spawn(move || {
                if core_ml_file && let Err(e) = store.fetch_core_ml(model, |_| {}) {
                    tracing::info!("{name}: its file for Core ML did not arrive: {e}");
                }
                if warms {
                    crate::ai::warm_core_ml(&store, model);
                }
            });
        if let Err(e) = spawned {
            tracing::info!("{name}: no thread to fetch its file for Core ML: {e}");
        }
    }
    fetched
}

/// Fetch Microsoft's CPU-only ONNX Runtime into the store, saying how
/// it goes as a model's fetch does, then decide again and load it, so
/// a library that will not load here is said now; whether it is
/// loaded.
fn fetch_runtime(runtime: &'static greycard_ai::Runtime, deliver: &dyn Fn(Outcome)) -> bool {
    let failed = |message: String| {
        deliver(Outcome::FetchFailed {
            id: runtime.id,
            name: runtime.name,
            message,
        })
    };
    let store = match greycard_ai::Store::user() {
        Ok(s) => s,
        Err(e) => {
            failed(e.to_string());
            return false;
        }
    };
    let mut last = Instant::now();
    let fetched = store.fetch_runtime(runtime, |p| {
        if p.done == 0 || p.done == p.total || last.elapsed().as_millis() > 200 {
            last = Instant::now();
            deliver(Outcome::Fetching {
                name: runtime.name,
                file: p.file,
                done: p.done,
                total: p.total,
            });
        }
    });
    if let Err(e) = fetched {
        failed(e.to_string());
        return false;
    }
    match greycard_ai::runtime::load_fetched(&store, runtime) {
        Ok(loaded) => {
            tracing::info!(
                "{} fetched{}",
                runtime.name,
                if loaded {
                    " and loaded: the AI features run on the CPU"
                } else {
                    ", and another ONNX Runtime comes first here"
                }
            );
            deliver(Outcome::RuntimeFetched { runtime });
            true
        }
        // Here whole, and it will not load (or the decision still will
        // not have it): said as that, not as a failed download. One
        // that would not load is gone from the store again, so the
        // next launch offers it afresh rather than trusting it.
        Err(why) => {
            deliver(Outcome::RuntimeUnusable {
                id: runtime.id,
                message: match why {
                    greycard_ai::runtime::Unavailable::WouldNotLoad { .. } => {
                        format!("{why}; it is removed again, and offered at the next start")
                    }
                    _ => why.to_string(),
                },
            });
            false
        }
    }
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
            greycard_library::set_lens_database(crate::edited_pictures::lens_database());
            deliver(Outcome::LensesFetched);
        }
        Err(e) => deliver(Outcome::LensesFetchFailed {
            message: e.to_string(),
        }),
    }
}

/// A worker dropped closes its queue, so its thread ends once out of
/// the job in hand, and what was queued behind it is dropped: nobody is
/// left to take the outcome. Without it every worker made leaves a
/// thread asleep on the queue for the life of the process, and a run
/// of tests that each make one meets the thread cap macOS sets per
/// process. No join here: the window's thread would wait out a develop
/// or a model run, and [`Worker::stop`] is the way out that waits.
impl Drop for Worker {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod slot {
    use super::*;
    use std::path::PathBuf;

    fn open(path: &str, generation: u64) -> Job {
        Job::Open {
            path: PathBuf::from(path),
            version: None,
            edit: Edit::default(),
            generation,
            seed_blend: true,
            turn: 0,
            stance_known: true,
            learned: Vec::new(),
        }
    }

    fn bright() -> Edit {
        let mut edit = Edit::default();
        edit.light.exposure = 1.3;
        edit
    }

    fn of(path: &str) -> Option<DevelopOf> {
        Some((PathBuf::from(path), None))
    }

    /// A develop for the frame of an open still waiting becomes that
    /// open under the develop's edit, generation, and turn, the open's
    /// own the rest; an aside likewise. A develop for another frame, or
    /// for another version of the file, or for none, takes the slot as
    /// it is, as it does with no open waiting or a develop waiting; so
    /// does an open of another file over a waiting open.
    #[test]
    fn a_develop_for_a_waiting_open_s_frame_is_that_open() {
        for develop in [
            Job::Develop {
                edit: bright(),
                generation: 5,
                turn: 1,
                of: of("/a.tif"),
            },
            Job::DevelopAside {
                edit: bright(),
                generation: 5,
                turn: 1,
                of: of("/a.tif"),
            },
        ] {
            let (
                Job::Open {
                    path,
                    edit,
                    generation,
                    seed_blend,
                    turn,
                    stance_known,
                    ..
                },
                true,
            ) = into_slot(Some(open("/a.tif", 4)), develop)
            else {
                panic!("still the open");
            };
            assert_eq!(path, PathBuf::from("/a.tif"));
            assert_eq!(edit.light.exposure, 1.3);
            assert_eq!((generation, turn), (5, 1));
            assert!(seed_blend && stance_known);
        }
        let develop = |of| Job::Develop {
            edit: bright(),
            generation: 6,
            turn: 0,
            of,
        };
        for other in [
            of("/b.tif"),
            Some((PathBuf::from("/a.tif"), Some("v1".into()))),
            None,
        ] {
            assert!(matches!(
                into_slot(Some(open("/a.tif", 4)), develop(other)),
                (Job::Develop { generation: 6, .. }, false)
            ));
        }
        assert!(matches!(
            into_slot(None, develop(of("/a.tif"))),
            (Job::Develop { generation: 6, .. }, false)
        ));
        assert!(matches!(
            into_slot(Some(develop(None)), develop(of("/a.tif"))),
            (Job::Develop { generation: 6, .. }, false)
        ));
        let (
            Job::Open {
                path, generation, ..
            },
            false,
        ) = into_slot(Some(open("/a.tif", 4)), open("/b.tif", 7))
        else {
            panic!("the newer open");
        };
        assert_eq!((path, generation), (PathBuf::from("/b.tif"), 7));
    }

    /// A fold leaves the prefetch going: the neighbors the window sent
    /// with the open, under its generation, are the folded open's, and
    /// sent again under the open's generation they are taken as such.
    #[test]
    fn a_fold_keeps_the_open_s_neighbors() {
        let heading = Heading::steady(Side::Next);
        let worker = Worker::new(|_| {});
        // Stopped: what is sent waits in the queue, as behind a busy
        // worker, for the test to read.
        worker.stop();
        worker.send(open("/a.tif", 4));
        worker.neighbors(4, heading, Vec::new());
        worker.send(Job::Develop {
            edit: bright(),
            generation: 5,
            turn: 0,
            of: of("/a.tif"),
        });
        {
            let q = worker.queue.0.lock().unwrap();
            assert!(matches!(q.develop, Some(Job::Open { generation: 5, .. })));
            assert_eq!(q.neighbors.as_ref().map(|n| n.generation), Some(5));
            assert_eq!(q.folded, Some((4, 5)));
        }
        // Sent again by the window under the open's generation.
        worker.neighbors(4, heading, Vec::new());
        assert_eq!(
            worker
                .queue
                .0
                .lock()
                .unwrap()
                .neighbors
                .as_ref()
                .map(|n| n.generation),
            Some(5)
        );
        assert!(worker.neighbors_again(
            4,
            std::path::Path::new("/a.tif"),
            heading,
            Vec::new(),
            "a test"
        ));
        // Another open: the fold is over.
        worker.send(open("/b.tif", 6));
        assert_eq!(worker.queue.0.lock().unwrap().folded, None);
    }
}
