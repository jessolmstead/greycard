//! The window's side of the edit's pictures: which of its cells the
//! worker holds as edited and under what key, told as the list, the
//! saves, and the rows change, and the cells asked again when what they
//! show may have moved. The pictures themselves, and the map the worker
//! keeps of them, are `edited_pictures`'s.

use crate::edited_pictures::{Iso, is_edited};
use crate::versions::Key;

/// Cell `c`'s ISO as its file's row gave it (`rows::FromRow::iso`).
fn iso_of(st: &crate::State, c: usize) -> Iso {
    st.from_row.get(st.file_at(c)?).and_then(|r| r.iso)
}

/// Whether the window holds cell `c` as edited: by its row while its
/// file stands in from one (the index's `edited`, the version's own
/// for a version), by its line's edit once the sidecar is read, under
/// the rule the cell's edited mark goes by.
pub(crate) fn held_edited(st: &crate::State, c: usize) -> bool {
    crate::panel::browser::frame_marks(st, c).0
}

/// Cell `c`'s develop key as its row gave it, while the row stands in
/// for the sidecar (`rows::FromRow::develop_key`, the version's own for
/// a version).
fn key_of(st: &crate::State, c: usize) -> Option<u64> {
    let cell = st.cell(c)?;
    let row = st.from_row.get(cell.file).filter(|r| !r.read)?;
    match cell.version.as_deref() {
        None => row.develop_key,
        Some(id) => row
            .versions
            .iter()
            .find(|v| v.id == id)
            .and_then(|v| v.develop_key)
            .filter(|&k| k != 0),
    }
}

/// The look or profile directory may have changed under the window (a
/// fit or a refit of the camera match, a look removed or renamed, the
/// LOOK or CAMERA PROFILE section opened, which reads its directory
/// again and so picks up a file put there by hand): when what the keys
/// are made under moved, every edited frame's key is read again, its
/// cells ask for their pictures again (a frame whose key moved shows the
/// camera's until the pool has made its new ones), and the index's keys
/// are cleared to be made anew by the next pass.
pub(crate) fn looks_moved(st: &mut crate::State) {
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    if !worker.edited().files_moved() {
        return;
    }
    // The Original kept may be under a profile or a look moved.
    crate::panel::original::drop_picture(st);
    let held: std::collections::HashSet<Key> = worker.edited().held_frames().into_iter().collect();
    tracing::info!(
        "the edit's pictures: the look or profile directory changed; {} edited frame(s) read again",
        held.len()
    );
    for c in 0..st.cells.len() {
        if st.key_of(c).is_some_and(|k| held.contains(&k)) {
            ask_again(st, &worker, c);
        }
    }
    if st.culling.cull.is_some() {
        crate::panel::cull::cull_refresh(st);
    }
    if let Some(indexer) = &st.views.index {
        indexer.keys_stale();
    }
}

/// What the window is doing, as the reduced develops go by it: the frame
/// open in the editor, or none while culling, in the grid (`grid`), or
/// with no frame. Said on every report of the strip's or the grid's
/// range, and on the way into culling.
pub(crate) fn note_editing(
    st: &mut crate::State,
    app: &crate::App,
    worker: &crate::worker::Worker,
    grid: bool,
) {
    let open = st
        .current
        .filter(|_| st.culling.cull.is_none() && !grid)
        .and_then(|c| st.key_of(c));
    let back = open.is_some() && !worker.edited().any_open();
    // The highlight fill's round waits while the editor is left: its
    // words go with the editor, and come back with the frame.
    if open.is_none() {
        crate::panel::deliver::show_fill_pending(app, false);
    } else if back && st.develop.highlight_pending {
        crate::panel::deliver::show_fill_pending(app, true);
    }
    worker.set_open_frame(open);
    // Back from culling or the grid on the frame still open: its
    // neighbors, let go on the way out, are sent again, in the order
    // the strip has now.
    if back && let Some(c) = st.current {
        crate::panel::browser::neighbors_back(st, worker, c);
    }
}

/// A new list in the window: the worker is told which of its frames
/// are edited, and under what key where the index knows, before their
/// thumbnails are asked for.
pub(crate) fn note_list(st: &crate::State, worker: &crate::worker::Worker) {
    let list: Vec<(Key, bool, Iso, Option<u64>)> = (0..st.cells.len())
        .filter_map(|c| {
            Some((
                st.key_of(c)?,
                held_edited(st, c),
                iso_of(st, c),
                key_of(st, c),
            ))
        })
        .collect();
    worker.edited().note_all(list);
}

/// Cell `c`'s sidecar was written, or taken whole from its archive
/// copy. The open frame's edit goes to the worker, which keeps the
/// pictures of it from the develop in hand (or when that lands, or when
/// the frame is left); any other cell (rated in the browser, culled,
/// pasted onto, or joined) is held as edited or not by what its line is
/// now, its key read again, and asked again when that may change its
/// picture. A file's cells are each their own here: a save of the file
/// is said for every cell of it whose line moved
/// ([`crate::library::sidecar_written`]).
pub(crate) fn saved(st: &mut crate::State, c: usize) {
    let (Some(frame), Some(line), Some(sidecar)) = (st.key_of(c), st.line(c), st.sidecar_at(c))
    else {
        return;
    };
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    let iso = iso_of(st, c);
    if st.current == Some(c) && st.culling.cull.is_none() {
        // Nothing kept of a state whose masks wait on the frame's shape
        // (§724): it is not the picture.
        if let Some(edit) = line.ready() {
            worker.send(crate::worker::Job::Keep {
                frame,
                edit: edit.clone(),
                turn: sidecar.turn,
                iso,
            });
        }
        return;
    }
    // A base of it the worker shelved under the edit before is no base
    // an open of it would take now.
    worker.edit_changed(frame.path.clone(), file_edits(st, c));
    // The line's key under the edit just written is its hint until the
    // thumbnail reads it: what the cell shows now, not before the save.
    let edited = is_edited(&frame.path, line.current(), iso);
    let key = edited.then(|| worker.edited().key_known(&frame.path, line.current()));
    if worker.edited().note(&frame, edited, iso, key) {
        worker.send(crate::rows::thumb_job(st, c));
    }
    // A neighbor of the open frame is made again under its new edit.
    crate::panel::browser::neighbors_moved(st);
}

/// The window's develop of the open frame landed at `turn`: when what it
/// developed is the frame's saved edit, the worker keeps its pictures
/// from it (or finds them kept already, a key and a few stats), so a
/// frame edited before its pictures were kept has them once it is
/// opened. A frame with no edit, and none held, asks nothing.
pub(crate) fn landed(st: &crate::State, turn: u8) {
    let Some(c) = st.current.filter(|_| st.culling.cull.is_none()) else {
        return;
    };
    let (Some(frame), Some(line), Some(sidecar)) = (st.key_of(c), st.line(c), st.sidecar_at(c))
    else {
        return;
    };
    // The sidecar on disk is the frame's own, and holds what was
    // developed: nothing on the panel is unsaved.
    if !crate::sidecars::writable(st, c)
        || *line.current() != st.develop.edit
        || sidecar.turn != turn
    {
        return;
    }
    // Nor of a state whose masks wait on the frame's shape (§724).
    if line.current_waits() {
        return;
    }
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    let iso = iso_of(st, c);
    if !is_edited(&frame.path, line.current(), iso) && !worker.edited().holds(&frame) {
        return;
    }
    worker.send(crate::worker::Job::Keep {
        frame,
        edit: line.current().clone(),
        turn,
        iso,
    });
}

/// What the window holds of cell `c` changed (its row came in, or its
/// sidecar was read): the worker holds it as edited or not to match, and
/// when that turned, the cell asks for its picture again.
pub(crate) fn refresh(st: &mut crate::State, c: usize) {
    let Some(frame) = st.key_of(c) else {
        return;
    };
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    if worker
        .edited()
        .hold(&frame, held_edited(st, c), iso_of(st, c), key_of(st, c))
    {
        kept(st, &worker, &frame);
    }
}

/// [`refresh`] over every cell of file `file`: its sidecar was read, or
/// its row came in, which says something of each of its lines.
pub(crate) fn refresh_file(st: &mut crate::State, file: usize) {
    for c in st.cells_of_file(file) {
        refresh(st, c);
    }
}

/// Cell `c`'s sidecar changed on disk under the window, as its row now
/// says (another machine's save joined, a pass that found it rewritten):
/// its key is the row's when the row has one, else read again; and the
/// cell asks for its picture again.
pub(crate) fn changed(st: &mut crate::State, c: usize) {
    let Some(frame) = st.key_of(c) else {
        return;
    };
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    // Its bases on the worker's shelf: kept only under an edit the
    // window has read; a sidecar still standing in from its row says
    // nothing of what changed, and they all go.
    worker.edit_changed(frame.path.clone(), file_edits(st, c));
    let (edited, iso) = (held_edited(st, c), iso_of(st, c));
    let moved = match key_of(st, c) {
        Some(key) => {
            worker.edited().hold(&frame, edited, iso, Some(key));
            true
        }
        None => worker.edited().note(&frame, edited, iso, None),
    };
    if moved {
        kept(st, &worker, &frame);
    }
    crate::panel::browser::neighbors_moved(st);
}

/// File `file` was written again in place under the window (a merge
/// made again, an app's save): the pictures of each of its lines kept
/// under its old content `hash` are taken out, since a file of the
/// same head and length would take them for its own, and each cell
/// holds its edit as before, its key read again, and asks for its
/// picture again. The loupe's copies go whatever they show, the
/// camera's too, since the file under them is another.
pub(crate) fn rewritten(st: &mut crate::State, file: usize, hash: &str) {
    let Some(worker) = crate::WORKER.with(|w| w.borrow().clone()) else {
        return;
    };
    let cells: Vec<(usize, Key)> = st
        .cells_of_file(file)
        .into_iter()
        .filter_map(|c| Some((c, st.key_of(c)?)))
        .collect();
    // Every line let go before any is held again: one still held at a
    // key keeps the pictures of that key from going.
    for (_, frame) in &cells {
        worker.edited().reset(frame, Some(hash));
    }
    for (c, frame) in &cells {
        for cull in [st.culling.cull.as_mut(), st.culling.hold.as_mut()]
            .into_iter()
            .flatten()
        {
            let dropped = cull.cache.forget_file(*c);
            cull.let_go(dropped);
        }
        worker
            .edited()
            .note(frame, held_edited(st, *c), iso_of(st, *c), None);
        kept(st, &worker, frame);
    }
}

/// The file at `path` written again in place under the window, by a
/// merge made again or by another editor's save over a file Edit in
/// handed it: what the window kept of its old content let go, nothing
/// of its edit. Its thumbnails and previews in the cache under `old`,
/// its hash and stamp before, are taken out; the pictures of its edit
/// under that hash too, or under `unknown` when `old` is not known,
/// each of its cells asking for its picture again ([`rewritten`]); its
/// stack's reference takes `new`, its hash now; and when it is the
/// frame open, its picture is read again and nothing else of an open
/// done ([`crate::panel::browser::reread_open`]). True when it was read
/// again.
pub(crate) fn file_rewritten(
    st: &mut crate::State,
    app: &crate::App,
    worker: &crate::worker::Worker,
    path: &std::path::Path,
    old: Option<&(String, u64)>,
    unknown: Option<&str>,
    new: Option<&str>,
) -> bool {
    if let Some((hash, stamp)) = old {
        let cache = worker.thumb_cache();
        if let Some(cache) = cache.lock().expect("thumbnail cache").as_mut() {
            cache.remove(hash, *stamp);
        }
    }
    let file = st.file_by_path(path);
    if let (Some(f), Some(hash)) = (file, old.map(|(h, _)| h.as_str()).or(unknown)) {
        rewritten(st, f, hash);
    }
    if let (Some((old, _)), Some(new)) = (old, new)
        && old != new
    {
        let hint = file
            .and_then(|f| crate::stacks::hint_of_file(st, f))
            .unwrap_or_else(|| crate::stacks::hint_of_path(st, path));
        let rehashed = [greycard_library::Rehash {
            path: std::path::PathBuf::from(hint),
            old: old.clone(),
            new: new.to_string(),
        }];
        if st.views.stacks.heal(crate::stacks::Found {
            moved: &[],
            folders: &[],
            rehashed: &rehashed,
        }) {
            crate::panel::stacks::settle(st, app, worker);
        }
    }
    st.current
        .is_some_and(|c| st.path_at(c).map(std::path::PathBuf::as_path) == Some(path))
        && crate::panel::browser::reread_open(st, app, worker)
}

/// Every edit cell `c`'s file holds, its own and its versions', and its
/// quarter turns, as the window has read them from its sidecar: what an
/// open of any of them would make a base under. None while the sidecar
/// stands in from the file's row, which does not say.
fn file_edits(st: &crate::State, c: usize) -> Option<crate::worker::FileNow> {
    let file = st.file_at(c)?;
    if !crate::rows::file_is_loaded(st, file) {
        return None;
    }
    let sidecar = st.sidecars.get(file)?;
    Some(crate::worker::FileNow {
        edits: std::iter::once(&sidecar.line)
            .chain(sidecar.versions.iter().map(|v| &v.line))
            .map(|l| l.current().clone())
            .collect(),
        turn: sidecar.turn,
    })
}

/// The pictures `frame` shows may have changed (`Outcome::EditedKept`):
/// its cell asks for its again, and the loupe drops its copy so its
/// next refresh asks too.
pub(crate) fn kept(st: &mut crate::State, worker: &crate::worker::Worker, frame: &Key) {
    let Some(c) = st.find_cell(frame) else {
        return;
    };
    ask_again(st, worker, c);
    if st.culling.cull.is_some() {
        crate::panel::cull::cull_refresh(st);
    }
}

/// Cell `c` asks for its picture again and the loupe drops its copy
/// where it is the edit's: the develop view's stand-in, and a versions
/// compare's tile. Cull on the camera's JPEG keeps the camera's. The
/// loupe's refresh is the caller's.
fn ask_again(st: &mut crate::State, worker: &crate::worker::Worker, c: usize) {
    worker.send(crate::rows::thumb_job(st, c));
    crate::lightbox::forget(st, c);
    if let Some(cull) = st.culling.cull.as_mut()
        && cull.shows_edit(c)
    {
        cull.cache.forget(c);
    }
    if let Some(hold) = st.culling.hold.as_mut() {
        hold.cache.forget(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edited_pictures::BIG;
    use greycard_edit::Edit;
    use greycard_engine::testing::edited::{mean, scratch};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// A picture's mean, and the strip's drawn copy of it when the row
    /// has a cell: what frame `i` shows in the window.
    fn shown(st: &crate::State, app: &crate::App, i: usize) -> (f64, Option<f64>) {
        use slint::Model;
        let (_, _, rgb) = st.views.thumb_base[i].as_ref().expect("a picture held");
        let drawn = crate::panel::browser::row_of(st, i)
            .and_then(|row| app.get_thumbs().row_data(row))
            .and_then(|t| t.image.to_rgb8())
            .map(|buf| mean(buf.as_bytes()));
        (mean(rgb), drawn)
    }

    /// The window's own hooks, headless: a frame opened, brightened and
    /// saved through the panel's save shows its edit in the strip; a
    /// reset saved the same way shows the camera's; and a frame not open
    /// turned in the strip keeps its edit's picture, the turn being drawn
    /// and not kept.
    #[test]
    fn the_window_shows_a_saved_edit_and_a_turned_frame_keeps_it() {
        use crate::worker::{Job, Outcome, Worker};
        use std::time::{Duration, Instant};
        let dir = scratch("window");
        let files: Vec<PathBuf> = ["a.png", "b.png"]
            .iter()
            .map(|name| {
                let path = dir.join(name);
                image::RgbImage::from_fn(300, 200, |x, y| {
                    image::Rgb([(30 + x / 6) as u8, (40 + y / 5) as u8, 50])
                })
                .save(&path)
                .unwrap();
                path
            })
            .collect();
        let app = crate::testing::window(2);
        let (state, _) = crate::testing::state_for(&app, files.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let worker = std::rc::Rc::new(Worker::new(move |o| {
            let _ = tx.lock().unwrap().send(o);
        }));
        worker.set_thumb_cache(Some(
            greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(BIG, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        ));
        crate::WORKER.with(|w| *w.borrow_mut() = Some(worker.clone()));
        {
            let mut st = state.borrow_mut();
            st.window.write_sidecars = true;
            for s in &mut st.sidecars {
                s.record(Edit::for_picture());
            }
            st.current = Some(0);
            crate::panel::browser::rebuild_browser(&mut st, &app);
            note_list(&st, &worker);
            for i in 0..2 {
                worker.send(crate::rows::thumb_job(&st, i));
            }
        }
        // The window takes in what comes, as its event loop would; a
        // develop's own landing is the viewport's and not this test's.
        let developed = std::cell::Cell::new(0);
        let pump = |until: &dyn Fn(&crate::State) -> bool, what: &str| {
            let deadline = Instant::now() + Duration::from_secs(120);
            while !until(&state.borrow()) {
                assert!(Instant::now() < deadline, "waiting for {what}");
                let Ok(outcome) = rx.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                if matches!(outcome, Outcome::Developed { .. }) {
                    developed.set(developed.get() + 1);
                }
                if matches!(
                    outcome,
                    Outcome::Thumbnail { .. }
                        | Outcome::NoThumbnail { .. }
                        | Outcome::Thumbnails(_)
                        | Outcome::EditedKept { .. }
                ) {
                    crate::panel::deliver::deliver(&app, outcome);
                }
            }
        };
        pump(
            &|st| st.views.thumb_base.iter().all(Option::is_some),
            "the camera's",
        );
        let camera = shown(&state.borrow(), &app, 0);
        // Open the frame: its develop in the worker's hand.
        let opened = |path: &Path, generation: u64| {
            worker.send(Job::Open {
                path: path.to_path_buf(),
                version: None,
                edit: Edit::for_picture(),
                generation,
                seed_blend: false,
                turn: 0,
                stance_known: false,
                learned: Vec::new(),
            });
        };
        opened(&files[0], 1);
        pump(&|_| developed.get() == 1, "the develop");
        // Brightened on the panel and saved by the panel's own save.
        {
            let mut st = state.borrow_mut();
            {
                let mut e = st.sidecars[0].current().clone();
                e.light.exposure = 1.0;
                st.sidecars[0].record(e);
            }
            assert!(crate::sidecars::save_sidecar(&mut st, 0));
        }
        pump(
            &|st| shown(st, &app, 0).0 > camera.0 + 20.0,
            "the edit's picture in the strip",
        );
        let edited = shown(&state.borrow(), &app, 0);
        if let (Some(drawn), Some(before)) = (edited.1, camera.1) {
            assert!(
                drawn > before + 20.0,
                "drawn: {drawn:.1} against {before:.1}"
            );
        }
        // Reset and saved: the camera's again.
        {
            let mut st = state.borrow_mut();
            st.sidecars[0].record(Edit::for_picture());
            assert!(crate::sidecars::save_sidecar(&mut st, 0));
        }
        pump(
            &|st| (shown(st, &app, 0).0 - camera.0).abs() < 1.0,
            "the camera's again",
        );
        // Brightened and kept again, then left for the other frame.
        {
            let mut st = state.borrow_mut();
            {
                let mut e = st.sidecars[0].current().clone();
                e.light.exposure = 1.0;
                st.sidecars[0].record(e);
            }
            assert!(crate::sidecars::save_sidecar(&mut st, 0));
        }
        pump(
            &|st| shown(st, &app, 0).0 > camera.0 + 20.0,
            "the edit's picture again",
        );
        state.borrow_mut().current = Some(1);
        opened(&files[1], 2);
        // Turned in the strip while it is not open: its sidecar written,
        // its cell asked again, and it keeps the edit's picture.
        {
            let mut st = state.borrow_mut();
            crate::panel::browser::turn_frames(&mut st, &app, &worker, &[0], 1);
            assert_eq!(st.sidecars[0].turn, 1);
            st.views.thumb_base[0] = None;
            worker.send(crate::rows::thumb_job(&st, 0));
        }
        pump(
            &|st| st.views.thumb_base[0].is_some(),
            "the turned frame's picture",
        );
        let turned = shown(&state.borrow(), &app, 0).0;
        assert!(
            (turned - edited.0).abs() < 1.0,
            "the turned frame shows its edit: {turned:.1}, the edit's {:.1}, the camera's {:.1}",
            edited.0,
            camera.0
        );
        crate::WORKER.with(|w| *w.borrow_mut() = None);
        crate::STATE.with(|s| *s.borrow_mut() = None);
        worker.stop();
        drop(state);
        crate::testing::remove_dir_retry(&dir);
    }

    /// The edit's pictures as the window's loupe reads them.
    mod loupe {
        use greycard_edit::Edit;
        use greycard_engine::testing::edited::{Heard, Rig, rig};

        trait Loupe {
            fn loupe(&self) -> (bool, bool, u32, u32);
            fn loupe_as(&self, camera: bool) -> (bool, bool, u32, u32);
        }

        impl Loupe for Rig {
            /// The loupe's picture of the frame at a view's size where it
            /// shows the edit (a versions compare's tile, the develop view's
            /// stand-in): whether it is the edit's, whether it is a local
            /// preview, and its size.
            fn loupe(&self) -> (bool, bool, u32, u32) {
                self.loupe_as(false)
            }

            /// The same, or with `camera` the loupe's in Cull on the
            /// camera's JPEG.
            fn loupe_as(&self, camera: bool) -> (bool, bool, u32, u32) {
                let previews = self.worker.previews();
                let got = std::cell::RefCell::new(None);
                crate::cull::fetch(
                    &crate::cull::Want::for_loupe(0, self.png.clone(), 1000).camera_only(camera),
                    Some(&previews),
                    &|loaded| {
                        if let crate::cull::Loaded::Ok { preview, .. } = loaded {
                            *got.borrow_mut() = Some(preview);
                        }
                    },
                );
                let p = got.into_inner().expect("a picture");
                (p.edited, p.local, p.width, p.height)
            }
        }

        /// The open frame brightened and saved: the strip's picture of it is
        /// made from the edit, brighter than the camera's, at the made size,
        /// and so is the loupe's at 2048; saved again unchanged (a rating)
        /// nothing is made again; and reset, the camera's is the picture
        /// again and the edit's are gone from the cache.
        #[test]
        fn a_saved_edit_shows_in_the_strip_and_a_reset_shows_the_camera_s() {
            let rig = rig("bright");
            let (cw, ch, camera) = rig.thumb();
            assert_eq!(
                (cw, ch),
                (150, 100),
                "the camera's picture, a whole factor down"
            );
            assert!(!rig.loupe().0, "no edit, the camera's");
            let mut bright = Edit::for_picture();
            bright.light.exposure = 1.0;
            rig.save(&bright, 0);
            rig.kept();
            let (w, h, edited) = rig.thumb();
            assert_eq!((w, h), (176, 117), "the edit's, to the made size");
            assert!(
                edited > camera + 20.0,
                "a stop up: {edited:.1} against the camera's {camera:.1}"
            );
            let (edited_up, local, lw, lh) = rig.loupe();
            assert!(edited_up && !local);
            assert_eq!((lw, lh), (600, 400), "no larger than it is");
            assert!(
                !rig.loupe_as(true).0,
                "Cull on the camera's JPEG: the camera's"
            );
            let kept = rig.entries();
            assert!(kept >= 6, "the camera's and the edit's five: {kept}");
            // A save that changes nothing of the picture (a rating) makes
            // nothing, and asks no cell again.
            rig.save(&bright, 0);
            rig.quiet();
            assert_eq!(rig.entries(), kept);
            // Another edit replaces the last one's pictures, not adds to them.
            bright.light.exposure = 1.5;
            rig.save(&bright, 0);
            rig.kept();
            assert_eq!(rig.entries(), kept);
            assert!(rig.thumb().2 > edited);
            // The crop is in the picture: the left half of the frame.
            bright.geometry.crop = Some(greycard_edit::geometry::Crop {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 1.0,
            });
            rig.save(&bright, 0);
            rig.kept();
            let (w, h, _) = rig.thumb();
            assert_eq!((w, h), (132, 176), "300 by 400, to the made size");
            assert_eq!(rig.entries(), kept);
            // Reset: the camera's again, the edit's gone.
            rig.save(&Edit::for_picture(), 0);
            rig.kept();
            let (w, h, again) = rig.thumb();
            assert_eq!((w, h), (cw, ch), "the camera's again");
            assert!(
                (again - camera).abs() < 1.0,
                "{again:.1} against {camera:.1}"
            );
            assert_eq!(rig.entries(), kept - 5);
            assert!(!rig.loupe().0);
            crate::testing::remove_dir_retry(&rig.dir);
        }

        /// A frame turned a quarter is kept as the camera's tag turns it,
        /// for the strip to turn at draw time; a save whose develop is not
        /// in hand waits for it; and a frame left before it lands shows the
        /// camera's.
        #[test]
        fn a_turned_frame_is_kept_unturned_and_a_save_waits_for_its_develop() {
            use crate::worker::Job;
            let rig = rig("turned");
            let (_, _, camera) = rig.thumb();
            let (right, down) = rig.slope.get();
            assert!(
                right > 10.0 && down > 10.0,
                "the camera's runs right and down"
            );
            // The frame turned: the window develops it again at the turn.
            let mut bright = Edit::for_picture();
            bright.light.exposure = 1.0;
            rig.worker.send(Job::Develop {
                edit: bright.clone(),
                generation: 2,
                turn: 1,
                of: None,
            });
            assert!(matches!(rig.next(), Heard::Developed));
            rig.save(&bright, 1);
            rig.kept();
            let (w, h, edited) = rig.thumb();
            assert_eq!((w, h), (176, 117), "unturned, as the camera's is");
            assert!(edited > camera + 20.0);
            let (right, down) = rig.slope.get();
            assert!(
                right > 10.0 && down > 10.0,
                "and runs the camera's way: {right:.0} right, {down:.0} down"
            );
            // Turned a quarter and mirrored by the crop panel's own turn.
            let mut mirrored = bright.clone();
            mirrored.geometry.flip = true;
            mirrored.geometry.turns = 1;
            rig.worker.send(Job::Develop {
                edit: mirrored.clone(),
                generation: 5,
                turn: 1,
                of: None,
            });
            assert!(matches!(rig.next(), Heard::Developed));
            rig.save(&mirrored, 1);
            rig.kept();
            let (w, h, _) = rig.thumb();
            assert_eq!((w, h), (176, 117));
            let (right, down) = rig.slope.get();
            assert!(
                right > 10.0 && down > 10.0,
                "{right:.0} right, {down:.0} down"
            );
            // Texture is a develop's: the save comes before the develop
            // lands, and its pictures with it.
            let mut textured = bright.clone();
            textured.detail.texture = 30.0;
            rig.save(&textured, 1);
            rig.worker.send(Job::Develop {
                edit: textured.clone(),
                generation: 3,
                turn: 1,
                of: None,
            });
            rig.kept();
            assert!(rig.thumb().2 > camera + 20.0);
            // Saved again, darker, and the frame left before a develop of
            // it: the camera's stands in, the key having moved on.
            let mut dark = textured.clone();
            dark.detail.texture = 0.0;
            dark.light.exposure = -1.0;
            let mut sidecar = greycard_edit::Sidecar {
                line: greycard_edit::Line::of(dark.clone(), 0).unwrap(),
                turn: 1,
                ..Default::default()
            };
            sidecar.save(&rig.png).unwrap();
            rig.save(&dark, 1);
            let other = rig.dir.join("other.png");
            image::RgbImage::from_pixel(40, 30, image::Rgb([9, 9, 9]))
                .save(&other)
                .unwrap();
            rig.worker.send(Job::Open {
                path: other,
                version: None,
                edit: Edit::for_picture(),
                generation: 4,
                seed_blend: false,
                turn: 0,
                stance_known: false,
                learned: Vec::new(),
            });
            rig.kept();
            let (_, _, left) = rig.thumb();
            assert!((left - camera).abs() < 1.0, "{left:.1} against {camera:.1}");
            crate::testing::remove_dir_retry(&rig.dir);
        }

        /// A picture without something its edit asks for (here a look not in
        /// the look directory) is not kept under the edit's key: the frame
        /// shows the camera's.
        #[test]
        fn a_picture_missing_what_its_edit_asks_for_is_not_kept() {
            let rig = rig("left-out");
            let (_, _, camera) = rig.thumb();
            let mut bright = Edit::for_picture();
            bright.light.exposure = 1.0;
            bright.look_lut = greycard_edit::LookLut {
                lut: greycard_edit::look::LutChoice::Named("Not here at all".into()),
                strength: 1.0,
            };
            rig.save(&bright, 0);
            rig.kept();
            let (_, _, shown) = rig.thumb();
            assert!(
                (shown - camera).abs() < 1.0,
                "{shown:.1} against {camera:.1}"
            );
            assert!(!rig.loupe().0);
            crate::testing::remove_dir_retry(&rig.dir);
        }
    }
}
