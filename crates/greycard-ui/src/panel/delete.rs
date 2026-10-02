//! The delete sheet: the selection, or the rejects folder's frames,
//! deleted from disk with their sidecars once the sheet is answered.
//! What goes and how is `crate::delete`'s; this puts the question on
//! the window, runs the delete on a thread of its own, and when it is
//! done takes the frames out of the list, their rows out of the index
//! and their pictures out of the cache.

use crate::delete::{self, Deleted, How, Offer, Plan, TRASH_SUPPORTED};
use crate::panel::browser::{chosen_frames, file_name};
use crate::panel::cull::{drop_files, shoot_rejects_dir};
use crate::panel::edit::{read_edit, save_edit};
use crate::*;

/// What the sheet was opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Which {
    /// The frames chosen: the set, or the frame on screen alone.
    Selection,
    /// The frames in the rejects folder beside the open folder.
    Rejects,
}

impl Which {
    /// The callback's names for the two.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "selection" => Some(Self::Selection),
            "rejects" => Some(Self::Rejects),
            _ => None,
        }
    }
}

/// The sheet's question while it is up.
#[derive(Debug, Clone)]
pub(crate) struct Asked {
    pub(crate) which: Which,
    /// The frames to delete, as they were planned when the sheet
    /// opened; planned again at the confirm.
    pub(crate) frames: Vec<PathBuf>,
    /// The folders the delete may reach, canonical.
    pub(crate) allowed: Vec<PathBuf>,
    /// What the sheet offered; an answer it did not offer is none.
    pub(crate) offer: Offer,
}

/// A delete handed to its thread: everything it needs, read on the
/// window's thread, and nothing of the window's.
pub(crate) struct Job {
    which: Which,
    plan: Plan,
    how: How,
    allowed: Vec<PathBuf>,
}

/// What comes back from the thread.
pub(crate) struct Done {
    which: Which,
    how: How,
    /// The thread panicked, with this message; `deleted` is then what
    /// the disk says went before it did.
    panicked: Option<String>,
    /// The frames the confirm's check left alone, and why.
    refused: Vec<(PathBuf, String)>,
    deleted: Deleted,
    /// Each planned frame's thumbnail key, read before it went.
    keys: Vec<(PathBuf, Option<(String, u64)>)>,
}

impl Job {
    /// The delete itself, off the window's thread: the thumbnails'
    /// keys first (a hash of each frame's head), then the files, then
    /// the rejects folder when it is left empty.
    pub(crate) fn run(self) -> Done {
        let keys = self
            .plan
            .frames
            .iter()
            .map(|d| (d.frame.clone(), crate::worker::thumb_key(&d.frame)))
            .collect();
        let deleted = delete::delete(&self.plan, self.how, &self.allowed, delete::system_trash);
        if self.which == Which::Rejects
            && let Some(dir) = self.allowed.first()
        {
            delete::remove_if_empty(dir);
        }
        Done {
            which: self.which,
            how: self.how,
            panicked: None,
            refused: self.plan.refused,
            deleted,
            keys,
        }
    }

    /// [`Job::run`], a panic caught: the trash crate panics when it
    /// cannot set up COM on Windows, and a delete that never landed
    /// would leave the window refusing deletes for the session. What
    /// went before the panic is read back from the disk, and its
    /// thumbnails stay in the cache for the eviction to take.
    pub(crate) fn run_guarded(self) -> Done {
        let (which, how) = (self.which, self.how);
        let planned: Vec<PathBuf> = self.plan.frames.iter().map(|d| d.frame.clone()).collect();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run())) {
            Ok(done) => done,
            Err(payload) => {
                let message = crate::worker::panic_message(payload.as_ref());
                let frames = planned
                    .into_iter()
                    .filter(|f| std::fs::symlink_metadata(f).is_err())
                    .collect();
                Done {
                    which,
                    how,
                    panicked: Some(message),
                    refused: Vec::new(),
                    deleted: Deleted {
                        frames,
                        ..Deleted::default()
                    },
                    keys: Vec::new(),
                }
            }
        }
    }
}

/// Whether file `c`'s sidecar is held: a delete out on its thread has
/// it planned, and a write now would put a `.gcd` back beside a frame
/// just trashed, or under one being trashed. The change stays in
/// memory; the frame is going.
pub(crate) fn held(st: &State, c: usize) -> bool {
    st.deleting
        .as_ref()
        .is_some_and(|planned| st.files.get(c).is_some_and(|f| planned.contains(f)))
}

/// A path as the sheet shows it: without the `\\?\` Windows puts on a
/// canonical one.
fn shown(path: &Path) -> String {
    dunce::simplified(path).display().to_string()
}

/// The folder each frame is in, canonical, each once.
fn folders_of(frames: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for f in frames {
        let Some(dir) = f.parent() else {
            continue;
        };
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

/// What the sheet says will happen, by what it offers.
pub(crate) fn offer_note(offer: Offer) -> &'static str {
    match (offer.trash, offer.permanent) {
        (true, false) => "They go to the system trash, where they can be restored from.",
        (true, true) => {
            "The trash refused files from this folder earlier. Try the trash again, \
             or delete them permanently. A permanent delete cannot be undone."
        }
        _ => {
            "There is no system trash here, so they will be deleted permanently. \
             This cannot be undone."
        }
    }
}

/// Open the delete sheet for `which`, or say in the status line why
/// there is nothing to ask.
pub(crate) fn ask_delete(st: &mut State, app: &App, which: Which) {
    if st.deleting.is_some() {
        app.set_status("a delete is still under way".into());
        return;
    }
    let (frames, allowed) = match which {
        Which::Selection => {
            let chosen = chosen_frames(st);
            // A frame under a root that is offline is not there to
            // delete, and nothing is written where its root was.
            let (offline, here): (Vec<usize>, Vec<usize>) = chosen
                .into_iter()
                .partition(|&i| crate::rows::is_offline(st, i));
            let frames: Vec<PathBuf> = here
                .into_iter()
                .filter_map(|i| st.files.get(i).cloned())
                .collect();
            if let Some(&i) = offline.first() {
                let what = if frames.is_empty() {
                    ": nothing to delete"
                } else {
                    ": its frames are left out"
                };
                crate::rows::say_offline_for(st, app, i, what);
                if frames.is_empty() {
                    return;
                }
            } else if frames.is_empty() {
                app.set_status("nothing is chosen to delete".into());
                return;
            }
            // The chosen frames' own folders, and the rejects folders
            // beside them (the all-roots view lists those too): only
            // these are canonicalized, not every folder of the list.
            let mut dirs = crate::library::folders_of(&frames);
            let rejects: Vec<PathBuf> = dirs.iter().map(|d| cull::rejects_dir(d)).collect();
            dirs.extend(rejects);
            (frames, delete::allowed(&dirs))
        }
        Which::Rejects => {
            if st.view != crate::roots::View::Folder {
                app.set_status("a rejects folder is a folder's: open the folder first".into());
                return;
            }
            // Beside the first frame's folder, which is the open
            // folder; the sheet names it whatever else is open.
            let Some(dir) = shoot_rejects_dir(st) else {
                app.set_status("nothing is open".into());
                return;
            };
            match std::fs::symlink_metadata(&dir) {
                Ok(m) if m.file_type().is_symlink() => {
                    app.set_status(
                        format!("{} is a link; nothing is deleted through one", shown(&dir)).into(),
                    );
                    return;
                }
                Ok(m) if m.is_dir() => {}
                _ => {
                    app.set_status(format!("there is no rejects folder at {}", shown(&dir)).into());
                    return;
                }
            }
            let frames = match crate::files::list_files(&dir) {
                Ok(f) => f,
                Err(e) => {
                    app.set_status(format!("the rejects folder cannot be read: {e:#}").into());
                    return;
                }
            };
            if frames.is_empty() {
                app.set_status(format!("{} has no frames in it", shown(&dir)).into());
                return;
            }
            (frames, delete::allowed(&[dir]))
        }
    };
    let plan = delete::plan(&frames, &allowed);
    if plan.frames.is_empty() {
        let why = plan
            .refused
            .first()
            .map(|(_, why)| why.clone())
            .unwrap_or_default();
        app.set_status(format!("nothing can be deleted: {why}").into());
        return;
    }
    let planned: Vec<PathBuf> = plan.frames.iter().map(|d| d.frame.clone()).collect();
    let folders = folders_of(&planned);
    let refused_here = folders.iter().any(|f| st.trash_refused.contains(f));
    let offer = delete::offer(TRASH_SUPPORTED, refused_here);
    let n = planned.len();
    let title = match which {
        Which::Selection if n == 1 => format!("Delete {}?", file_name(&planned[0])),
        Which::Selection => format!("Delete {n} frames?"),
        Which::Rejects => format!("Delete the rejects folder's {}?", delete::count(n, "frame")),
    };
    let place = match folders.as_slice() {
        [one] => shown(one),
        many => format!("{} folders", many.len()),
    };
    let mut text = format!(
        "{} and {}, from {place}.",
        delete::count(n, "frame"),
        delete::count(plan.sidecars(), "sidecar")
    );
    if let Some((_, why)) = plan.refused.first() {
        text.push_str(&format!(
            " {} left where {} ({why}).",
            plan.refused.len(),
            if plan.refused.len() == 1 {
                "it is"
            } else {
                "they are"
            }
        ));
    }
    app.set_delete_title(title.into());
    app.set_delete_text(text.into());
    app.set_delete_note(offer_note(offer).into());
    app.set_delete_trash(offer.trash);
    app.set_delete_permanent(offer.permanent);
    // Whether they are safe somewhere: a line, filled in when the look
    // at the archives lands.
    crate::panel::archive::delete_line(st, app, &planned);
    st.delete_asked = Some(Asked {
        which,
        frames: planned,
        allowed,
        offer,
    });
    app.set_delete_open(true);
}

/// The sheet's answer: 0 no, 1 the trash, 2 permanently. An answer
/// the sheet did not offer is a no. A yes plans the delete again here
/// and hands it to its own thread; [`land`] finishes it.
pub(crate) fn answered(st: &mut State, app: &App, answer: i32) {
    app.set_delete_open(false);
    let Some(asked) = st.delete_asked.take() else {
        return;
    };
    let how = match answer {
        1 if asked.offer.trash => How::Trash,
        2 if asked.offer.permanent => How::Permanent,
        _ => return,
    };
    if !st.deletes_allowed {
        let why = "not deleted: a capture, an export or a timing run never deletes";
        tracing::info!("{why}");
        app.set_status(why.into());
        return;
    }
    if st.deleting.is_some() {
        app.set_status("a delete is still under way".into());
        return;
    }
    // An edit of the frame on screen still to be written goes to disk
    // first, so it goes with the frame (to the trash, to be restored
    // with it) or stays with it if the delete is refused.
    if st.save_timer.running()
        && st
            .current
            .and_then(|c| st.files.get(c))
            .is_some_and(|f| asked.frames.contains(f))
    {
        st.save_timer.stop();
        let edit = read_edit(app, &st.edit, st.target);
        save_edit(st, edit);
    }
    let plan = delete::plan(&asked.frames, &asked.allowed);
    let n = plan.frames.len();
    st.deleting = Some(plan.frames.iter().map(|d| d.frame.clone()).collect());
    app.set_status(format!("deleting {}...", delete::count(n, "frame")).into());
    let job = Job {
        which: asked.which,
        plan,
        how,
        allowed: asked.allowed,
    };
    if !send_off(app, job) {
        st.deleting = None;
        app.set_status("the delete could not be started; nothing was deleted".into());
    }
}

/// Run a delete on a thread of its own; what it did is landed on the
/// window's thread when it is done. False when it could not start.
///
/// A fresh thread, not a pool's: on Windows the `trash` crate
/// initializes COM (apartment-threaded) on the first thread that calls
/// it, and must not be called from a thread that initialized COM the
/// other way. winit's main thread is an STA of its own; this thread
/// has no COM set up before the crate's.
#[cfg(not(test))]
fn send_off(app: &App, job: Job) -> bool {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard delete".into())
        .spawn(move || {
            let done = job.run_guarded();
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state), Some(worker)) = (
                    app_weak.upgrade(),
                    crate::STATE.with(|s| s.borrow().clone()),
                    crate::WORKER.with(|w| w.borrow().clone()),
                ) else {
                    return;
                };
                land(&mut state.borrow_mut(), &app, &worker, done);
            });
        });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("delete: {e}");
            false
        }
    }
}

/// In a test the job is queued, and the test runs and lands it
/// (`tests::land_all`): there is no event loop to land it on.
#[cfg(test)]
fn send_off(_app: &App, job: Job) -> bool {
    tests::SENT.with(|s| s.borrow_mut().push(job));
    true
}

/// A delete is done: the rows forgotten, the cache's pictures of the
/// frames that went dropped, a refusal of the trash remembered for
/// the folder, the status said, and the frames out of the list.
pub(crate) fn land(st: &mut State, app: &App, worker: &Worker, done: Done) {
    st.deleting = None;
    let gone = &done.deleted.frames;
    if let Some(indexer) = &st.index {
        indexer.forget(gone.clone());
    }
    {
        let cache = worker.thumb_cache();
        let mut cache = cache.lock().expect("thumbnail cache");
        if let Some(cache) = cache.as_mut() {
            for (frame, key) in &done.keys {
                if let Some((hash, stamp)) = key
                    && gone.contains(frame)
                {
                    cache.remove(hash, *stamp);
                }
            }
        }
    }
    if let Some(r) = &done.deleted.trash_refused
        && let Some(dir) = r.frame.parent()
    {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        st.trash_refused.insert(dir);
    }
    for (f, why) in done.refused.iter().chain(&done.deleted.failed) {
        tracing::warn!("{}: not deleted: {why}", f.display());
    }
    let status = match &done.panicked {
        Some(message) => {
            tracing::error!("the delete failed: {message}");
            format!(
                "the delete failed, and nothing more was deleted ({} went before it did; \
                 the log has why)",
                delete::count(done.deleted.frames.len(), "frame")
            )
        }
        None => delete::summary(&done.deleted, done.how, &done.refused),
    };
    tracing::info!("delete ({:?}): {status}", done.which);
    let gone: Vec<usize> = st
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| gone.contains(f))
        .map(|(i, _)| i)
        .collect();
    drop_files(st, app, worker, &gone, status);
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_delete_asked(move |which| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(which) = Which::from_name(&which) else {
                return;
            };
            ask_delete(&mut state.borrow_mut(), &app, which);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_delete_answered(move |answer| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            answered(&mut state.borrow_mut(), &app, answer);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Indexer, Told};
    use crate::testing::{press, state_for, window};
    use greycard_library::Library;
    use greycard_library::fixture::{A7, R5, R6, write_frame};
    use slint::platform::Key;
    use std::time::Duration;

    thread_local! {
        pub(super) static SENT: RefCell<Vec<Job>> = const { RefCell::new(Vec::new()) };
    }

    /// Run the deletes sent off and land them, as the thread and the
    /// event loop would; how many there were.
    fn land_all(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>) -> usize {
        let jobs: Vec<Job> = SENT.with(|s| s.borrow_mut().drain(..).collect());
        let n = jobs.len();
        for job in jobs {
            let done = std::thread::spawn(move || job.run_guarded())
                .join()
                .unwrap();
            land(&mut state.borrow_mut(), app, worker, done);
        }
        n
    }

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ui-delete-{what}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Five frames: b and d with a `.gcd`, b with an XMP as well, and
    /// a note beside them that is no frame's.
    fn shoot(dir: &Path) -> Vec<PathBuf> {
        let files: Vec<PathBuf> = ["a.tif", "b.tif", "c.tif", "d.tif", "e.tif"]
            .iter()
            .map(|n| dir.join(n))
            .collect();
        for (i, (f, camera)) in files.iter().zip([&R6, &R5, &A7, &R6, &R5]).enumerate() {
            write_frame(f, camera, i as u16 + 1);
        }
        let mut s = Sidecar::default();
        s.meta.rating = 3;
        s.save(&files[1]).unwrap();
        s.save(&files[3]).unwrap();
        greycard_edit::xmp::save(&files[1], &s.meta, None).unwrap();
        std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
        files
    }

    /// The files at the top of `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().unwrap().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    /// `files` open on `app` in a session someone is at, the first
    /// frame on screen.
    fn opened(app: &App, files: Vec<PathBuf>) -> (Rc<RefCell<State>>, Rc<Worker>) {
        let (state, worker) = state_for(app, files);
        {
            let mut st = state.borrow_mut();
            st.deletes_allowed = true;
            st.current = Some(0);
            crate::panel::browser::rebuild_browser(&mut st, app);
        }
        (state, worker)
    }

    /// The trash refused this folder earlier, so the sheet offers the
    /// permanent delete: the one path a test may take.
    fn refused(state: &Rc<RefCell<State>>, dir: &Path) {
        state
            .borrow_mut()
            .trash_refused
            .insert(std::fs::canonicalize(dir).unwrap());
    }

    #[test]
    fn deleting_the_selection_takes_its_frames_sidecars_and_rows_and_leaves_the_neighbors() {
        let dir = scratch("selection");
        let shoot_dir = dir.join("shoot");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        let files = shoot(&shoot_dir);
        // The index, on a library of the test's own.
        let db = dir.join("data").join("library.sqlite");
        let (tx, rx) = std::sync::mpsc::channel();
        let indexer = Indexer::start(db.clone(), move |told| {
            let _ = tx.send(told);
        })
        .expect("the indexer starts");
        let wait = |want: &dyn Fn(&Told) -> bool| loop {
            let told = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("the indexer answers");
            if want(&told) {
                return told;
            }
        };
        wait(&|t| matches!(t, Told::Opened(_)));
        indexer.folders(vec![shoot_dir.clone()], 1);
        wait(&|t| matches!(t, Told::Indexed { .. }));

        let app = window(files.len());
        let (state, worker) = opened(&app, files.clone());
        refused(&state, &shoot_dir);
        {
            let mut st = state.borrow_mut();
            st.index = Some(indexer);
            st.current = Some(1);
            st.picked = vec![1, 3];
        }
        app.invoke_delete_asked("selection".into());
        assert!(app.get_delete_open());
        assert!(app.get_delete_trash() && app.get_delete_permanent());
        assert_eq!(app.get_delete_title(), "Delete 2 frames?");
        let text = app.get_delete_text();
        assert!(text.starts_with("2 frames and 3 sidecars, from "), "{text}");
        assert!(app.get_delete_note().contains("cannot be undone"));

        app.invoke_delete_answered(2);
        land_all(&state, &app, &worker);
        assert!(!app.get_delete_open());
        assert_eq!(names(&shoot_dir), ["a.tif", "c.tif", "e.tif", "notes.txt"]);
        let status = app.get_status();
        assert!(
            status.starts_with("deleted permanently: 2 frames and 3 sidecars"),
            "{status}"
        );
        {
            let st = state.borrow();
            assert_eq!(
                st.files,
                [files[0].clone(), files[2].clone(), files[4].clone()]
            );
            assert_eq!(st.sidecars.len(), 3);
            assert_eq!(st.shown, [0, 1, 2]);
            assert!(st.picked.is_empty());
        }
        // The rows went with the files, and the neighbors' stayed.
        match wait(&|t| matches!(t, Told::Forgotten(_))) {
            Told::Forgotten(n) => assert_eq!(n, 2),
            other => panic!("{other:?}"),
        }
        let reader = Library::open_read_only(&db).unwrap();
        assert!(reader.by_path(&files[1]).unwrap().is_none());
        assert!(reader.by_path(&files[3]).unwrap().is_none());
        for kept in [0, 2, 4] {
            assert!(reader.by_path(&files[kept]).unwrap().is_some());
        }
        let indexer = state.borrow_mut().index.take().unwrap();
        indexer.stop(Duration::from_secs(20));
        drop(reader);
        crate::testing::remove_dir_retry(&dir);
    }

    /// Where the platform has a trash the sheet offers it alone, and an
    /// answer it did not offer deletes nothing. The trash itself is
    /// never taken here: a test does not put files in the user's.
    #[test]
    fn the_sheet_offers_the_trash_and_an_answer_it_did_not_offer_deletes_nothing() {
        let dir = scratch("offer");
        let files = shoot(&dir);
        let app = window(files.len());
        let (state, worker) = opened(&app, files.clone());
        // No rejects folder: said, and no sheet.
        app.invoke_delete_asked("rejects".into());
        assert!(!app.get_delete_open());
        assert!(app.get_status().contains("there is no rejects folder"));
        app.invoke_delete_asked("selection".into());
        assert!(app.get_delete_open());
        assert_eq!(app.get_delete_title(), "Delete a.tif?");
        assert!(
            app.get_delete_text()
                .starts_with("1 frame and 0 sidecars, from ")
        );
        assert_eq!(app.get_delete_trash(), TRASH_SUPPORTED);
        assert!(!app.get_delete_permanent());
        assert!(app.get_delete_note().contains("system trash"));
        app.invoke_delete_answered(2);
        land_all(&state, &app, &worker);
        assert!(!app.get_delete_open());
        assert!(state.borrow().delete_asked.is_none());
        // Cancelled: nothing either.
        app.invoke_delete_asked("selection".into());
        app.invoke_delete_answered(0);
        for f in &files {
            assert!(f.exists(), "{}", f.display());
        }
        assert_eq!(state.borrow().files.len(), files.len());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A capture, an export or a timing run never deletes, even
    /// answered.
    #[test]
    fn a_run_nobody_is_at_never_deletes() {
        let dir = scratch("batch");
        let files = shoot(&dir);
        let app = window(files.len());
        let (state, worker) = opened(&app, files.clone());
        refused(&state, &dir);
        state.borrow_mut().deletes_allowed = false;
        app.invoke_delete_asked("selection".into());
        assert!(app.get_delete_permanent());
        app.invoke_delete_answered(2);
        land_all(&state, &app, &worker);
        assert!(files.iter().all(|f| f.exists()));
        assert!(app.get_status().contains("never deletes"));
        assert_eq!(state.borrow().files.len(), files.len());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A folder swapped for a link to another between the sheet and
    /// the confirm: refused at the confirm, said, and nothing touched
    /// on either side of the link.
    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_for_a_link_after_the_sheet_is_refused_at_the_confirm() {
        let dir = scratch("swapped");
        let shoot_dir = dir.join("shoot");
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&shoot_dir).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let files = shoot(&shoot_dir);
        let app = window(files.len());
        let (state, worker) = opened(&app, files.clone());
        refused(&state, &shoot_dir);
        app.invoke_delete_asked("selection".into());
        assert!(app.get_delete_open());
        std::fs::write(elsewhere.join("a.tif"), b"not this one").unwrap();
        std::fs::rename(&shoot_dir, dir.join("moved")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &shoot_dir).unwrap();
        app.invoke_delete_answered(2);
        land_all(&state, &app, &worker);
        assert!(elsewhere.join("a.tif").exists());
        assert!(dir.join("moved").join("a.tif").exists());
        assert_eq!(
            app.get_status(),
            "nothing was deleted; 1 frame not deleted (a.tif: a.tif is not in the folder open)"
        );
        assert_eq!(state.borrow().files.len(), files.len());
        crate::testing::remove_dir_retry(&dir);
    }

    /// One delete at a time, and none on the window's thread: while one
    /// is out, the sheet does not open for another.
    #[test]
    fn a_second_delete_waits_for_the_first() {
        let dir = scratch("second");
        let files = shoot(&dir);
        let app = window(files.len());
        let (state, worker) = opened(&app, files.clone());
        refused(&state, &dir);
        app.invoke_delete_asked("selection".into());
        app.invoke_delete_answered(2);
        assert!(state.borrow().deleting.is_some());
        assert_eq!(app.get_status(), "deleting 1 frame...");
        assert!(
            files[0].exists(),
            "nothing is deleted on the window's thread"
        );
        app.invoke_delete_asked("selection".into());
        assert!(!app.get_delete_open());
        assert_eq!(app.get_status(), "a delete is still under way");
        assert_eq!(land_all(&state, &app, &worker), 1);
        assert!(state.borrow().deleting.is_none() && !files[0].exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A thread that panics still lands: the window takes deletes
    /// again, and says the delete failed. Here the panic is the test
    /// build's trash, which the sheet's default reaches.
    #[test]
    fn a_delete_whose_thread_panics_still_lands_and_says_so() {
        let dir = scratch("panic");
        let files = shoot(&dir);
        let app = window(files.len());
        let (state, worker) = opened(&app, files.clone());
        app.invoke_delete_asked("selection".into());
        assert!(app.get_delete_trash());
        app.invoke_delete_answered(1);
        assert!(state.borrow().deleting.is_some());
        assert_eq!(land_all(&state, &app, &worker), 1);
        assert!(state.borrow().deleting.is_none());
        assert_eq!(
            app.get_status(),
            "the delete failed, and nothing more was deleted (0 frames went before it \
             did; the log has why)"
        );
        assert!(files.iter().all(|f| f.exists()));
        assert_eq!(state.borrow().files.len(), files.len());
        // And the next delete is taken.
        app.invoke_delete_asked("selection".into());
        assert!(app.get_delete_open());
        crate::testing::remove_dir_retry(&dir);
    }

    /// While a delete is out, the frames it has planned keep their
    /// sidecars as they are on disk: a rating pressed on one is not
    /// written beside a frame being trashed. Other frames write as
    /// ever, and Move rejects waits.
    #[test]
    fn a_frame_being_deleted_is_not_written_and_move_rejects_waits() {
        let dir = scratch("held");
        let files = shoot(&dir);
        let app = window(files.len());
        let (state, worker) = opened(&app, files.clone());
        refused(&state, &dir);
        {
            let mut st = state.borrow_mut();
            st.write_sidecars = true;
            st.picked = vec![0, 2];
        }
        app.invoke_delete_asked("selection".into());
        app.invoke_delete_answered(2);
        {
            let mut st = state.borrow_mut();
            assert!(held(&st, 0) && held(&st, 2) && !held(&st, 1));
            st.sidecars[0].meta.rating = 5;
            crate::panel::edit::write_sidecar(&mut st, 0);
            st.sidecars[4].meta.rating = 5;
            crate::panel::edit::write_sidecar(&mut st, 4);
        }
        assert!(!Sidecar::path_for(&files[0]).exists());
        assert!(Sidecar::path_for(&files[4]).exists());
        app.invoke_rejects_asked();
        assert_eq!(app.get_status(), "a delete is still under way");
        assert!(!app.get_rejects_open());
        land_all(&state, &app, &worker);
        assert!(!files[0].exists() && !Sidecar::path_for(&files[0]).exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A frame deleted by another hand between the sheet and the
    /// confirm: counted gone, out of the list, and no panic.
    #[test]
    fn a_frame_gone_before_the_confirm_leaves_the_list_without_a_panic() {
        let dir = scratch("vanished");
        let files = shoot(&dir);
        let app = window(files.len());
        let (state, worker) = opened(&app, files.clone());
        refused(&state, &dir);
        state.borrow_mut().picked = vec![0, 2];
        app.invoke_delete_asked("selection".into());
        std::fs::remove_file(&files[2]).unwrap();
        app.invoke_delete_answered(2);
        land_all(&state, &app, &worker);
        let status = app.get_status();
        assert!(
            status.starts_with("deleted permanently: 2 frames and 0 sidecars (1 gone already)"),
            "{status}"
        );
        assert_eq!(
            state.borrow().files,
            [files[1].clone(), files[3].clone(), files[4].clone()]
        );
        assert_eq!(
            names(&dir),
            [
                "b.tif",
                "b.tif.gcd",
                "b.xmp",
                "d.tif",
                "d.tif.gcd",
                "e.tif",
                "notes.txt"
            ]
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// The rejects folder's frames go with their sidecars, the folder
    /// with them once it is empty, and the open folder is untouched.
    #[test]
    fn the_rejects_folder_is_emptied_and_taken_away() {
        let dir = scratch("rejects");
        let files = shoot(&dir);
        let rejects = cull::rejects_dir(&dir);
        std::fs::create_dir_all(&rejects).unwrap();
        let (x, y) = (rejects.join("x.tif"), rejects.join("y.tif"));
        write_frame(&x, &R6, 9);
        write_frame(&y, &A7, 10);
        let mut s = Sidecar::default();
        s.meta.flag = meta::Flag::Reject;
        s.save_in(&x, greycard_edit::Placement::Folder).unwrap();
        s.save(&y).unwrap();
        let app = window(files.len());
        let (state, worker) = opened(&app, files.clone());
        refused(&state, &rejects);
        app.invoke_delete_asked("rejects".into());
        assert_eq!(
            app.get_delete_title(),
            "Delete the rejects folder's 2 frames?"
        );
        assert!(
            app.get_delete_text()
                .starts_with("2 frames and 2 sidecars, from ")
        );
        app.invoke_delete_answered(2);
        land_all(&state, &app, &worker);
        assert!(!rejects.exists());
        assert!(files.iter().all(|f| f.exists()));
        assert_eq!(state.borrow().files, files);
        assert!(
            app.get_status()
                .starts_with("deleted permanently: 2 frames and 2 sidecars")
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// Delete opens the sheet over the selection in the loupe, the
    /// grid and culling; Backspace does only on a Mac. The
    /// sheet takes the keys: Escape is its no, Enter its trash, and
    /// Enter is never the permanent delete.
    #[test]
    fn delete_opens_the_sheet_and_enter_is_only_ever_the_trash() {
        let app = window(11);
        let asked = Rc::new(RefCell::new(Vec::<String>::new()));
        let seen = asked.clone();
        app.on_delete_asked(move |w| seen.borrow_mut().push(w.to_string()));
        let answers = Rc::new(RefCell::new(Vec::new()));
        let seen = answers.clone();
        app.on_delete_answered(move |a| seen.borrow_mut().push(a));
        // The loupe, the grid, and culling.
        press(&app, Key::Delete);
        app.set_grid_open(true);
        press(&app, Key::Delete);
        app.set_grid_open(false);
        app.set_culling(true);
        press(&app, Key::Delete);
        assert_eq!(*asked.borrow(), ["selection", "selection", "selection"]);
        // Backspace is the Mac's delete key, and nobody's elsewhere.
        press(&app, Key::Backspace);
        let mac = cfg!(target_os = "macos");
        assert_eq!(asked.borrow().len(), if mac { 4 } else { 3 });
        let before = asked.borrow().len();
        // The sheet up: Delete again does nothing, Enter is the trash.
        app.set_delete_trash(true);
        app.set_delete_permanent(true);
        app.set_delete_open(true);
        press(&app, Key::Delete);
        assert_eq!(asked.borrow().len(), before);
        press(&app, Key::Return);
        assert_eq!(*answers.borrow(), [1]);
        // With no trash to offer, Enter is nothing at all.
        app.set_delete_trash(false);
        press(&app, Key::Return);
        assert_eq!(*answers.borrow(), [1]);
        press(&app, Key::Escape);
        assert_eq!(*answers.borrow(), [1, 0]);
    }
}
