use greycard_edit::Edit;
use slint::Model;

use super::*;
use crate::export;
use crate::sheet::{ExportPreset, Sheet};
use crate::worker::Outcome;

/// A folder of `n` frames that are there on disk (empty files: the
/// queue asks only that they exist).
fn shoot(tag: &str, n: usize) -> (PathBuf, Vec<PathBuf>) {
    let dir = std::env::temp_dir().join(format!("greycard-queue-{tag}-{}", std::process::id()));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<PathBuf> = (0..n)
        .map(|i| {
            let f = dir.join(format!("IMG_{i:04}.CR3"));
            std::fs::write(&f, b"").unwrap();
            f
        })
        .collect();
    (dir, files)
}

fn queue_file(dir: &Path) -> PathBuf {
    dir.join("config").join(export_queue::FILE)
}

/// An entry of `frames` (by row) under each frame's sidecar edit,
/// with `preset` and a long edge of `size`.
fn entry(st: &State, frames: &[usize], preset: Option<&str>, size: &str) -> Entry {
    let frames = frames
        .iter()
        .map(|&i| {
            (
                st.files[i].clone(),
                st.sidecars[i].current().clone(),
                0,
                false,
                None,
            )
        })
        .collect();
    Entry {
        folder: None,
        ..entry_of(
            frames,
            Sheet {
                size: size.into(),
                ..Sheet::default()
            },
            preset.map(str::to_string),
        )
    }
}

/// Each frame given an edit of its own, recorded as a state, so an
/// export of it has a state to be a line on.
fn give_edits(st: &mut State) {
    for i in 0..st.files.len() {
        let mut e = Edit::default();
        e.light.exposure = 0.25 * (i as f32 + 1.0);
        st.sidecars[i].record(e);
    }
}

fn exported(path: &str) -> queue::Done {
    queue::Done::Exported {
        path: PathBuf::from(path),
        seconds: 1.0,
        note: None,
        left_out: Vec::new(),
    }
}

/// Frame `index` of the running set came back as `done`.
fn frame(app: &App, state: &Rc<Shared>, index: usize, done: queue::Done) {
    let (set, source, edit) = {
        let st = state.borrow();
        let run = st.work.queue_run.as_ref().expect("the queue runs");
        let f = &st.work.export_queue[run.entry].frames[run.at[index]];
        // The name the worker was sent, as it hands it back.
        let set = run.set.clone().expect("the entry was sent");
        (set, run.sent[index].clone(), f.edit.clone())
    };
    crate::panel::deliver::deliver(
        app,
        Outcome::SetFrameDone {
            set,
            index,
            source,
            edit,
            version: None,
            done,
        },
    );
}

/// The running set is finished.
fn finish(app: &App, state: &Rc<Shared>) {
    let set = state
        .borrow()
        .work
        .queue_run
        .as_ref()
        .unwrap()
        .set
        .clone()
        .unwrap();
    let tally = set.tally();
    crate::panel::deliver::deliver(app, Outcome::SetDone { set, tally });
}

/// Add to queue takes the selection as Export would (the frame on
/// screen under the panel's edit, the other under its sidecar's),
/// the sheet and the preset in use, writes the file and shows the
/// count; a later change to the sheet or to a sidecar leaves the
/// entry as it was queued.
#[test]
fn add_to_queue_keeps_what_export_would_take_and_writes_it() {
    let (dir, files) = shoot("add", 3);
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    let web = Sheet {
        size: "2048".into(),
        quality: 85.0,
        ..Sheet::default()
    };
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.views.picked = vec![0, 2];
        {
            let mut e = st.sidecars[2].current().clone();
            e.light.exposure = 1.0;
            st.sidecars[2].record(e);
        }
        st.sidecars[2].turn = 3;
        st.work.export_queue_file = Some(queue_file(&dir));
        st.work.export_presets = vec![ExportPreset {
            name: "Web".into(),
            sheet: web.clone(),
        }];
    }
    app.invoke_export_preset_chosen("Web".into());
    app.set_exposure(0.5);
    // The folder the sheet's row shows, last chosen: no chooser.
    state.borrow_mut().work.export_last_folder = Some(dir.join("out"));
    app.invoke_export_queue_add();

    let st = state.borrow();
    assert_eq!(st.work.export_queue.len(), 1);
    let e = &st.work.export_queue[0];
    let got: Vec<(PathBuf, f32, u8)> = e
        .frames
        .iter()
        .map(|f| (f.source.clone(), f.edit.light.exposure, f.turn))
        .collect();
    assert_eq!(
        got,
        vec![(files[0].clone(), 0.5, 0), (files[2].clone(), 1.0, 3)]
    );
    assert!(e.sheet.same(&web));
    assert_eq!(e.preset.as_deref(), Some("Web"));
    assert_eq!(e.folder.as_deref(), Some(dir.join("out").as_path()));
    assert_eq!(export_queue::load(&queue_file(&dir)), st.work.export_queue);
    assert_eq!(app.get_export_queue_sets(), 1);
    assert_eq!(app.get_export_queue_line(), "1 set, 2 frames queued");
    assert_eq!(app.get_status(), "2 frames queued; 1 set in the queue");
    // Nothing was exported.
    assert!(st.work.exporting.is_none() && st.work.queue_run.is_none());
    assert!(!dir.join("out").exists());
    // The panel's edit was made a state, as Export makes it.
    assert_eq!(st.sidecars[0].current().light.exposure, 0.5);
    drop(st);

    // The sheet and the sidecar move on; the entry does not.
    app.set_export_size("1024".into());
    app.invoke_export_sheet_changed();
    {
        let mut e = state.borrow_mut().sidecars[2].current().clone();
        e.light.exposure = -1.0;
        state.borrow_mut().sidecars[2].record(e);
    }
    let queued = state.borrow().work.export_queue[0].clone();
    assert_eq!(queued.sheet.size, "2048");
    assert_eq!(queued.frames[1].edit.light.exposure, 1.0);
    assert_eq!(export_queue::load(&queue_file(&dir))[0], queued);

    // A second set: the count says both.
    // None chosen: the frame's own folder, as the row shows it.
    {
        let mut st = state.borrow_mut();
        st.views.picked = vec![0];
        st.work.export_last_folder = None;
    }
    app.invoke_export_queue_add();
    assert_eq!(app.get_export_queue_line(), "2 sets, 3 frames queued");
    assert_eq!(app.get_status(), "1 frame queued; 2 sets in the queue");
    let second = state.borrow().work.export_queue[1].clone();
    assert_eq!(second.sheet.size, "1024");
    assert_eq!(second.preset, None, "an edited sheet names no preset");
    assert_eq!(second.folder.as_deref(), Some(dir.as_path()));

    // With a subfolder on the sheet, the entry goes beside each
    // frame, into it.
    app.set_export_subfolder("export".into());
    app.invoke_export_queue_add();
    let third = state.borrow().work.export_queue[2].clone();
    assert_eq!(third.folder, None);
    assert_eq!(
        third.sheet.subfolder().unwrap().as_deref(),
        Some(Path::new("export"))
    );
    crate::testing::remove_dir_retry(&dir);
}

/// Export the queue runs the entries in order, each as a set under
/// its own sheet and preset, each frame a line in its history, and
/// an entry leaves the queue (and the file) once its frames are
/// done; the last one done says so.
#[test]
fn the_queue_runs_in_order_and_each_entry_leaves_when_done() {
    let (dir, files) = shoot("run", 3);
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.work.export_queue_file = Some(queue_file(&dir));
        give_edits(&mut st);
        let a = entry(&st, &[0, 1], Some("Web"), "2048");
        let b = entry(&st, &[2], Some("Print"), "Full");
        add(&mut st, &app, a);
        add(&mut st, &app, b);
    }
    app.invoke_export_queue_run();
    {
        let st = state.borrow();
        let set = st.work.exporting.as_ref().expect("the first set runs");
        assert_eq!(set.total, 2);
        assert_eq!(set.preset.as_deref(), Some("Web"));
        assert_eq!(set.settings.long_edge, Some(2048));
        assert!(app.get_export_running());
        assert!(app.get_export_queue_running());
    }
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
    frame(&app, &state, 1, exported("/nowhere/out/IMG_0001.jpg"));
    finish(&app, &state);
    {
        let st = state.borrow();
        assert_eq!(st.work.export_queue.len(), 1, "the first entry is done");
        assert_eq!(st.work.export_queue[0].preset.as_deref(), Some("Print"));
        assert_eq!(export_queue::load(&queue_file(&dir)), st.work.export_queue);
        let set = st.work.exporting.as_ref().expect("the second set runs");
        assert_eq!(set.total, 1);
        assert_eq!(set.preset.as_deref(), Some("Print"));
        assert_eq!(set.settings.long_edge, None);
        assert_eq!(app.get_export_queue_line(), "1 set, 1 frame queued");
    }
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0002.jpg"));
    finish(&app, &state);
    let st = state.borrow();
    assert!(st.work.export_queue.is_empty());
    assert!(st.work.exporting.is_none() && st.work.queue_run.is_none());
    assert!(!queue_file(&dir).exists(), "an empty queue leaves no file");
    assert_eq!(app.get_export_queue_sets(), 0);
    assert!(!app.get_export_running());
    assert!(
        app.get_status().ends_with("; the queue is done"),
        "{}",
        app.get_status()
    );
    // Each frame a line in its history, with its entry's preset.
    for (i, preset) in [(0, "Web"), (1, "Web"), (2, "Print")] {
        let s = &st.sidecars[i];
        assert_eq!(s.current_exports.len(), 1, "frame {i}");
        assert_eq!(s.current_exports[0].preset.as_deref(), Some(preset));
        assert!(
            s.current_exports[0]
                .file
                .ends_with(&format!("IMG_000{i}.jpg"))
        );
    }
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// The status line counts across the whole run, not within each
/// entry's set: two entries of two and one frames say "3 of 3" on
/// the last, and the finished line counts all three.
#[test]
fn the_progress_counts_across_the_whole_run() {
    let (dir, files) = shoot("whole", 3);
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.work.export_queue_file = Some(queue_file(&dir));
        give_edits(&mut st);
        let a = entry(&st, &[0, 1], Some("Web"), "2048");
        let b = entry(&st, &[2], Some("Web"), "2048");
        add(&mut st, &app, a);
        add(&mut st, &app, b);
    }
    app.invoke_export_queue_run();
    let started = |index: usize, name: &str| {
        let set = state.borrow().work.exporting.clone().unwrap();
        crate::panel::deliver::deliver(
            &app,
            Outcome::SetFrameStarted {
                set,
                index,
                name: name.to_string(),
            },
        );
        app.get_status().to_string()
    };
    assert_eq!(started(0, "IMG_0000.jpg"), "exporting 1 of 3: IMG_0000.jpg");
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
    assert_eq!(started(1, "IMG_0001.jpg"), "exporting 2 of 3: IMG_0001.jpg");
    frame(&app, &state, 1, exported("/nowhere/out/IMG_0001.jpg"));
    finish(&app, &state);
    assert_eq!(started(0, "IMG_0002.jpg"), "exporting 3 of 3: IMG_0002.jpg");
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0002.jpg"));
    finish(&app, &state);
    // The finished line is the whole run's: the worker's own threads
    // fail these empty files at their pace, so only its shape is
    // fixed here, and the sum is checked below.
    let status = app.get_status().to_string();
    assert!(
        status.starts_with("exported ") && status.ends_with("; the queue is done"),
        "{status}"
    );
    let mut whole = Whole::new();
    let one = |n| queue::Tally {
        exported: n,
        failed: vec![("x".into(), "why".into())],
        ..queue::Tally::default()
    };
    let set = |n| queue::Set::new(n, None, Default::default(), Default::default());
    whole.add(&set(2), &one(1));
    whole.add(&set(1), &one(1));
    assert_eq!(
        (whole.before, whole.tally.exported, whole.tally.failed.len()),
        (3, 2, 2)
    );
    assert_eq!(whole.places, ["beside their files"]);
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// Stop while an entry's frame is in hand: that frame is done, the
/// frames passed over stay queued in their entry, the entries after
/// it are left as they were, and nothing more begins.
#[test]
fn a_stop_leaves_the_rest_queued() {
    let (dir, files) = shoot("stop", 4);
    let app = crate::testing::window(4);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.work.export_queue_file = Some(queue_file(&dir));
        give_edits(&mut st);
        let a = entry(&st, &[0, 1, 2], None, "2048");
        let b = entry(&st, &[3], None, "2048");
        add(&mut st, &app, a);
        add(&mut st, &app, b);
    }
    app.invoke_export_queue_run();
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
    app.invoke_export_stop();
    frame(&app, &state, 1, queue::Done::Canceled);
    frame(&app, &state, 2, queue::Done::Canceled);
    finish(&app, &state);
    let st = state.borrow();
    assert!(st.work.exporting.is_none() && st.work.queue_run.is_none());
    assert_eq!(st.work.export_queue.len(), 2);
    let left: Vec<&Path> = st.work.export_queue[0]
        .frames
        .iter()
        .map(|f| f.source.as_path())
        .collect();
    assert_eq!(left, vec![files[1].as_path(), files[2].as_path()]);
    assert_eq!(st.work.export_queue[1].frames.len(), 1);
    assert_eq!(export_queue::load(&queue_file(&dir)), st.work.export_queue);
    assert!(
        app.get_status()
            .ends_with("; 2 sets, 3 frames queued left in the queue"),
        "{}",
        app.get_status()
    );
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame that fails is the set's failure as in any set: no line in
/// its history, the rest of its entry written, the entry done, and
/// the queue goes on to the next.
#[test]
fn a_failed_frame_does_not_stop_the_queue() {
    let (dir, files) = shoot("fail", 3);
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        give_edits(&mut st);
        let a = entry(&st, &[0, 1], Some("Web"), "2048");
        let b = entry(&st, &[2], Some("Web"), "2048");
        add(&mut st, &app, a);
        add(&mut st, &app, b);
    }
    app.invoke_export_queue_run();
    frame(
        &app,
        &state,
        0,
        queue::Done::Failed {
            message: "not a raw".into(),
        },
    );
    frame(&app, &state, 1, exported("/nowhere/out/IMG_0001.jpg"));
    finish(&app, &state);
    let st = state.borrow();
    assert_eq!(
        st.work.export_queue.len(),
        1,
        "the failed frame's entry is done"
    );
    assert!(st.work.queue_run.is_some(), "and the next entry runs");
    assert!(st.sidecars[0].current_exports.is_empty());
    assert_eq!(st.sidecars[1].current_exports.len(), 1);
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// The queue across a restart: a fresh window and state read the
/// file, show the count, and start nothing.
#[test]
fn the_queue_is_kept_across_a_restart_and_not_run() {
    let (dir, files) = shoot("restart", 2);
    let settings = dir.join("config").join("settings.json");
    let queued = {
        let app = crate::testing::window(2);
        let (state, _worker) = crate::testing::state_for(&app, files.clone());
        let mut st = state.borrow_mut();
        open_at_launch(&mut st, &app, Some(&settings), true);
        assert!(st.work.export_queue.is_empty());
        give_edits(&mut st);
        let a = entry(&st, &[0], Some("Web"), "2048");
        let b = entry(&st, &[0, 1], None, "Full");
        add(&mut st, &app, a);
        add(&mut st, &app, b);
        st.work.export_queue.clone()
    };
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    open_at_launch(&mut state.borrow_mut(), &app, Some(&settings), true);
    let st = state.borrow();
    assert_eq!(st.work.export_queue, queued);
    assert_eq!(st.work.export_queue[1].frames[1].edit.light.exposure, 0.5);
    assert_eq!(app.get_export_queue_sets(), 2);
    assert_eq!(app.get_export_queue_line(), "2 sets, 3 frames queued");
    assert!(st.work.exporting.is_none() && st.work.queue_run.is_none());
    assert!(!app.get_export_running());
    drop(st);
    // A run that may not write (a snapshot) reads it and leaves it.
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, files);
    open_at_launch(&mut state.borrow_mut(), &app, Some(&settings), false);
    assert_eq!(state.borrow().work.export_queue, queued);
    clear(&mut state.borrow_mut(), &app);
    assert_eq!(export_queue::load(&queue_file(&dir)), queued);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame whose file is not there when the queue reaches it (a
/// drive unplugged, a root offline, or deleted) is said and left
/// queued, not sent and not dropped; an entry with none of its files
/// there is passed over, kept, and the next one begun.
#[test]
fn a_missing_file_is_left_queued_and_said() {
    let (dir, files) = shoot("gone", 3);
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.work.export_queue_file = Some(queue_file(&dir));
        give_edits(&mut st);
        let a = entry(&st, &[0], None, "2048");
        let b = entry(&st, &[1, 2], None, "2048");
        add(&mut st, &app, a);
        add(&mut st, &app, b);
    }
    std::fs::remove_file(&files[0]).unwrap();
    std::fs::remove_file(&files[1]).unwrap();
    app.invoke_export_queue_run();
    {
        let st = state.borrow();
        assert_eq!(st.work.export_queue.len(), 2, "nothing is dropped");
        let run = st.work.queue_run.as_ref().expect("the next entry runs");
        assert_eq!(run.entry, 1);
        assert_eq!(run.set.as_ref().unwrap().total, 1);
        assert_eq!(run.at, vec![1]);
    }
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0002.jpg"));
    finish(&app, &state);
    let st = state.borrow();
    assert!(st.work.queue_run.is_none());
    let left: Vec<Vec<&Path>> = st
        .work
        .export_queue
        .iter()
        .map(|e| e.frames.iter().map(|f| f.source.as_path()).collect())
        .collect();
    assert_eq!(
        left,
        vec![vec![files[0].as_path()], vec![files[1].as_path()]]
    );
    assert_eq!(export_queue::load(&queue_file(&dir)), st.work.export_queue);
    let status = app.get_status().to_string();
    assert!(
        status.contains("; 2 frames not there, left queued; 2 sets, 2 frames queued left"),
        "{status}"
    );
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// An entry whose folder, or whose watermark's PNG, is not there
/// stops the run with the reason and is left whole: running later
/// with the drive unplugged must not drain the queue.
#[test]
fn a_missing_folder_or_mark_stops_the_queue_and_keeps_the_entry() {
    let (dir, files) = shoot("nofolder", 2);
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        let mut a = entry(&st, &[0, 1], None, "2048");
        a.folder = Some(dir.join("unplugged"));
        add(&mut st, &app, a);
    }
    app.invoke_export_queue_run();
    {
        let st = state.borrow();
        assert!(st.work.queue_run.is_none() && st.work.exporting.is_none());
        assert_eq!(st.work.export_queue[0].frames.len(), 2);
    }
    let status = app.get_status().to_string();
    assert!(status.starts_with("the queue stopped: "), "{status}");
    assert!(status.contains("unplugged is not there"), "{status}");

    // The folder back, a mark whose PNG is gone.
    std::fs::create_dir_all(dir.join("unplugged")).unwrap();
    {
        let mut st = state.borrow_mut();
        st.work.export_queue[0].sheet.mark = crate::sheet::MARK_IMAGE.into();
        st.work.export_queue[0].sheet.mark_image = dir.join("logo.png").to_string_lossy().into();
    }
    app.invoke_export_queue_run();
    assert!(state.borrow().work.queue_run.is_none());
    assert_eq!(state.borrow().work.export_queue[0].frames.len(), 2);
    let status = app.get_status().to_string();
    assert!(status.contains("the watermark's PNG"), "{status}");

    // Both there: it runs.
    image::RgbaImage::new(4, 4)
        .save(dir.join("logo.png"))
        .unwrap();
    app.invoke_export_queue_run();
    assert!(
        state
            .borrow()
            .work
            .queue_run
            .as_ref()
            .unwrap()
            .set
            .is_some()
    );
    crate::testing::remove_dir_retry(&dir);
}

/// A window closed (or a crash) after some of an entry's frames are
/// written: the file no longer holds those, so the next run does not
/// send them again; the entry in memory keeps them for the run's
/// count.
#[test]
fn a_window_closed_mid_entry_does_not_send_its_done_frames_again() {
    let (dir, files) = shoot("crash", 3);
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.work.export_queue_file = Some(queue_file(&dir));
        give_edits(&mut st);
        let a = entry(&st, &[0, 1, 2], None, "2048");
        let b = entry(&st, &[0], None, "Full");
        add(&mut st, &app, a);
        add(&mut st, &app, b);
    }
    app.invoke_export_queue_run();
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
    // No SetDone: the window goes away here.
    assert_eq!(state.borrow().work.export_queue[0].frames.len(), 3);
    let on_disk = export_queue::load(&queue_file(&dir));
    assert_eq!(on_disk.len(), 2);
    let left: Vec<&Path> = on_disk[0]
        .frames
        .iter()
        .map(|f| f.source.as_path())
        .collect();
    assert_eq!(left, vec![files[1].as_path(), files[2].as_path()]);
    // All three done: the entry is gone from the file, the next kept.
    frame(&app, &state, 1, exported("/nowhere/out/IMG_0001.jpg"));
    frame(&app, &state, 2, exported("/nowhere/out/IMG_0002.jpg"));
    let on_disk = export_queue::load(&queue_file(&dir));
    assert_eq!(on_disk.len(), 1);
    assert_eq!(on_disk[0].sheet.size, "Full");
    crate::testing::remove_dir_retry(&dir);
}

/// A folder opened by a relative path: the queue keeps each frame
/// whole, and sends it under the list's own name, so the history
/// line lands on the frame in the list.
#[test]
fn a_relative_name_is_kept_whole_and_sent_as_the_list_has_it() {
    let (dir, files) = shoot("relative", 1);
    // The file by a path relative to the working directory: up
    // to what the two share, then down to the file. On Windows
    // the two may sit on different drives, which no relative
    // path can join; then there is nothing to test.
    let cwd = std::env::current_dir().unwrap();
    let shared = cwd
        .components()
        .zip(files[0].components())
        .take_while(|(a, b)| a == b)
        .count();
    if shared == 0 {
        crate::testing::remove_dir_retry(&dir);
        return;
    }
    let mut rel = PathBuf::new();
    for _ in cwd.components().skip(shared) {
        rel.push("..");
    }
    for c in files[0].components().skip(shared) {
        rel.push(c);
    }
    assert!(rel.is_relative() && rel.is_file(), "{}", rel.display());
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, vec![rel.clone()]);
    {
        let mut st = state.borrow_mut();
        give_edits(&mut st);
        let e = entry(&st, &[0], Some("Web"), "2048");
        assert!(e.frames[0].source.is_absolute());
        add(&mut st, &app, e);
    }
    app.invoke_export_queue_run();
    assert_eq!(
        state.borrow().work.queue_run.as_ref().unwrap().sent,
        vec![rel]
    );
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0000.jpg"));
    finish(&app, &state);
    assert_eq!(state.borrow().sidecars[0].current_exports.len(), 1);
    crate::testing::remove_dir_retry(&dir);
}

/// A mark with nothing to draw is refused at Add to queue, the sheet
/// left up, nothing queued.
#[test]
fn an_empty_mark_is_not_queued() {
    let (dir, files) = shoot("mark", 1);
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, files);
    state.borrow_mut().current = Some(0);
    app.set_export_mark(crate::sheet::MARK_IMAGE.into());
    app.set_export_mark_image("".into());
    app.set_export_open(false);
    app.invoke_export_queue_add();
    assert!(state.borrow().work.export_queue.is_empty());
    assert!(app.get_export_open());
    assert!(
        app.get_status().starts_with("not queued: "),
        "{}",
        app.get_status()
    );
    crate::testing::remove_dir_retry(&dir);
}

/// A plain set's frames and end, while the queue runs an entry of its
/// own, do not touch the queue.
#[test]
fn a_plain_set_leaves_the_queue_alone() {
    let (dir, files) = shoot("plain", 2);
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        give_edits(&mut st);
        let a = entry(&st, &[0, 1], None, "2048");
        add(&mut st, &app, a);
    }
    app.invoke_export_queue_run();
    let other = Arc::new(queue::Set::new(
        1,
        None,
        export::Settings::default(),
        export::OnExists::Increment,
    ));
    crate::panel::deliver::deliver(
        &app,
        Outcome::SetFrameDone {
            set: other.clone(),
            index: 0,
            source: files[0].clone(),
            edit: Edit::default(),
            version: None,
            done: exported("/nowhere/out/X.jpg"),
        },
    );
    crate::panel::deliver::deliver(
        &app,
        Outcome::SetDone {
            set: other,
            tally: queue::Tally::default(),
        },
    );
    let st = state.borrow();
    let run = st
        .work
        .queue_run
        .as_ref()
        .expect("the queue's entry still runs");
    assert!(run.done.iter().all(|d| !d));
    assert!(st.work.exporting.is_some());
    assert_eq!(st.work.export_queue[0].frames.len(), 2);
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame queued with its blend still to be seeded comes back under
/// the edit the worker rendered, seeded: it is still a line in its
/// history, and the frame takes the seed, as any set's frame does.
#[test]
fn a_seeded_frame_is_a_line_in_its_history() {
    let (dir, files) = shoot("seed", 1);
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, files);
    let queued = {
        let mut st = state.borrow_mut();
        assert!(crate::rows::wants_seed(&st, 0), "a raw never developed");
        let mut e = entry(&st, &[0], Some("Web"), "2048");
        e.frames[0].seed_blend = true;
        let queued = e.frames[0].edit.clone();
        add(&mut st, &app, e);
        queued
    };
    app.invoke_export_queue_run();
    let (set, source) = {
        let st = state.borrow();
        let run = st.work.queue_run.as_ref().unwrap();
        (run.set.clone().unwrap(), run.sent[0].clone())
    };
    let mut rendered = queued.clone();
    rendered.noise.learned_strength = 0.35;
    assert_ne!(rendered, queued);
    crate::panel::deliver::deliver(
        &app,
        Outcome::SetFrameDone {
            set,
            index: 0,
            source,
            edit: rendered,
            version: None,
            done: exported("/nowhere/out/IMG_0000.jpg"),
        },
    );
    finish(&app, &state);
    let st = state.borrow();
    assert_eq!(st.sidecars[0].current_exports.len(), 1);
    assert_eq!(
        st.sidecars[0].current_exports[0].preset.as_deref(),
        Some("Web")
    );
    assert_eq!(st.sidecars[0].current().noise.learned_strength, 0.35);
    assert!(!crate::rows::wants_seed(&st, 0));
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// Clear empties the queue and removes the file; not while it runs.
#[test]
fn clear_empties_the_queue_and_the_file() {
    let (dir, files) = shoot("clear", 2);
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, files);
    {
        let mut st = state.borrow_mut();
        st.work.export_queue_file = Some(queue_file(&dir));
        let a = entry(&st, &[0, 1], None, "2048");
        let b = entry(&st, &[1], None, "2048");
        add(&mut st, &app, a);
        add(&mut st, &app, b);
    }
    assert!(queue_file(&dir).exists());
    app.invoke_export_queue_run();
    app.invoke_export_queue_clear();
    assert_eq!(
        state.borrow().work.export_queue.len(),
        2,
        "not while it runs"
    );
    app.invoke_export_stop();
    frame(&app, &state, 0, queue::Done::Canceled);
    frame(&app, &state, 1, queue::Done::Canceled);
    finish(&app, &state);
    assert_eq!(state.borrow().work.export_queue.len(), 2);
    app.invoke_export_queue_clear();
    assert!(state.borrow().work.export_queue.is_empty());
    assert!(!queue_file(&dir).exists());
    assert_eq!(app.get_export_queue_sets(), 0);
    assert_eq!(app.get_status(), "export queue cleared (2 sets)");
    crate::testing::remove_dir_retry(&dir);
}

/// A sidecar written by a later build (its edits at a schema this
/// build does not read) is Unsure, not Deleted: the frame stays
/// queued for a build that reads it.
#[test]
fn a_later_build_s_sidecar_is_unsure() {
    let (dir, files) = shoot("later-build-sidecar", 1);
    let mut s = greycard_edit::Sidecar::default();
    let id = s.new_version(None, Some("Warm")).unwrap();
    s.save(&files[0]).unwrap();
    let gcd = greycard_edit::Sidecar::find(&files[0]).unwrap();
    let text = std::fs::read_to_string(&gcd).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["current"]["version"] = serde_json::json!(99);
    v["versions"][0]["line"]["current"]["version"] = serde_json::json!(99);
    std::fs::write(&gcd, serde_json::to_vec(&v).unwrap()).unwrap();
    let p = version_presence(&files[0], &id);
    crate::testing::remove_dir_retry(&dir);
    assert_eq!(p, Presence::Unsure);
}

/// A sidecar there but unreadable (a later build's, a corrupt file,
/// a share that cuts the read short) is not a version deleted: the
/// look cannot tell, and the frame is left queued; nor is a frame
/// whose sidecar is gone altogether. Only a sidecar that reads and
/// lacks the version says deleted.
#[test]
fn an_unreadable_sidecar_is_not_a_deleted_version() {
    let (dir, files) = shoot("unreadable-version", 1);
    let mut s = greycard_edit::Sidecar::default();
    let id = s.new_version(None, Some("Warm")).unwrap();
    s.save(&files[0]).unwrap();
    assert_eq!(version_presence(&files[0], &id), Presence::There);
    assert_eq!(version_presence(&files[0], "none"), Presence::Deleted);
    let gcd = greycard_edit::Sidecar::find(&files[0]).unwrap();
    std::fs::write(&gcd, b"{ not json").unwrap();
    assert_eq!(version_presence(&files[0], &id), Presence::Unsure);
    std::fs::remove_file(&gcd).unwrap();
    assert_eq!(version_presence(&files[0], &id), Presence::Unsure);
    // Through the run: the frame is left queued and said not there,
    // not dropped from the entry.
    std::fs::write(&gcd, b"{ not json").unwrap();
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        let mut e = entry(&st, &[], Some("Web"), "2048");
        e.frames.push(crate::export_queue::QueuedFrame {
            source: files[0].clone(),
            edit: Edit::default(),
            turn: 0,
            seed_blend: false,
            version: Some(queue::Version {
                id: id.clone(),
                name: "Warm".into(),
            }),
            id: export_queue::fresh_id(),
        });
        add(&mut st, &app, e);
    }
    app.invoke_export_queue_run();
    let st = state.borrow();
    assert_eq!(st.work.export_queue.len(), 1, "left queued");
    assert_eq!(st.work.export_queue[0].frames.len(), 1);
    assert!(st.work.queue_run.is_none());
    assert!(
        app.get_status().contains("not there; left queued"),
        "{}",
        app.get_status()
    );
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// An entry whose every frame was a version deleted before the run
/// goes from the queue with them, and the run goes on to the next
/// entry in its place.
#[test]
fn an_entry_of_deleted_versions_alone_leaves_the_queue() {
    let (dir, files) = shoot("deleted-versions-entry", 1);
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        let mut e = entry(&st, &[0], Some("Web"), "2048");
        e.frames[0].version = Some(queue::Version {
            id: "0123456789abcdef".into(),
            name: "Warm".into(),
        });
        add(&mut st, &app, e);
        let e = entry(&st, &[0], Some("Web"), "2048");
        add(&mut st, &app, e);
    }
    app.invoke_export_queue_run();
    let st = state.borrow();
    assert_eq!(st.work.export_queue.len(), 1, "the emptied entry went");
    assert_eq!(st.work.export_queue[0].frames[0].version, None);
    let run = st
        .work
        .queue_run
        .as_ref()
        .expect("the run went on to the next entry");
    assert_eq!(run.entry, 0, "which is at the first place now");
    assert_eq!(run.sent.len(), 1);
    drop(st);
    finish(&app, &state);
    crate::testing::remove_dir_retry(&dir);
}

/// An entry queued for a version that was deleted before the run is
/// left out of the run and said, rather than exported from the edit
/// the entry stored and recorded nowhere; the file's own frame beside
/// it goes out.
#[test]
fn a_queued_version_deleted_before_the_run_is_left_out_and_said() {
    let (dir, files) = shoot("deleted-version", 1);
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        let mut e = entry(&st, &[0, 0], Some("Web"), "2048");
        e.frames[1].version = Some(queue::Version {
            id: "0123456789abcdef".into(),
            name: "Warm".into(),
        });
        add(&mut st, &app, e);
    }
    app.invoke_export_queue_run();
    let st = state.borrow();
    let run = st
        .work
        .queue_run
        .as_ref()
        .expect("the run began on the file's own");
    assert_eq!(run.sent.len(), 1, "the file's own frame alone");
    assert_eq!(
        st.work.export_queue[0].frames.len(),
        1,
        "the version's out of the entry"
    );
    assert!(
        app.get_status()
            .contains("IMG_0000 (Warm) was deleted; not exported"),
        "{}",
        app.get_status()
    );
    drop(st);
    finish(&app, &state);
    crate::testing::remove_dir_retry(&dir);
}

/// The sheet's rows, as (name, frames, folder, running).
fn rows(app: &App) -> Vec<(String, String, String, bool)> {
    app.get_export_queue_rows()
        .iter()
        .map(|r| {
            (
                r.name.to_string(),
                r.frames.to_string(),
                r.folder.to_string(),
                r.running,
            )
        })
        .collect()
}

/// The presets of the entries in the file, in order.
fn presets_on_disk(dir: &Path) -> Vec<Option<String>> {
    export_queue::load(&queue_file(dir))
        .into_iter()
        .map(|e| e.preset)
        .collect()
}

/// The sheet lists the sets as the file holds them: the preset or
/// the format when there is none, the frames, and the folder with
/// home shortened, or the subfolder beside each file.
#[test]
fn the_rows_mirror_the_file() {
    let (dir, files) = shoot("rows", 3);
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    let home = dirs::home_dir().unwrap_or_default().join("out");
    {
        let mut st = state.borrow_mut();
        st.work.export_queue_file = Some(queue_file(&dir));
        let a = Entry {
            folder: Some(home.clone()),
            ..entry(&st, &[0, 1], Some("Web"), "2048")
        };
        let mut b = entry(&st, &[2], None, "Full");
        b.sheet.subfolder = "export".into();
        add(&mut st, &app, a);
        add(&mut st, &app, b);
    }
    let on_disk = export_queue::load(&queue_file(&dir));
    assert_eq!(on_disk.len(), 2);
    assert_eq!(
        rows(&app),
        vec![
            (
                "Web".into(),
                "2 frames".into(),
                crate::report::path_shown(&home),
                false
            ),
            ("JPEG".into(), "1 frame".into(), "export".into(), false),
        ]
    );
    if cfg!(unix) {
        assert_eq!(rows(&app)[0].2, "~/out");
    }
    crate::testing::remove_dir_retry(&dir);
}

/// A set's bin takes it out of the sheet and the file at once, the
/// others left in their order; the last one out leaves no file.
#[test]
fn removing_the_middle_set_leaves_the_others_in_order() {
    let (dir, files) = shoot("remove", 3);
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.work.export_queue_file = Some(queue_file(&dir));
        for (i, name) in ["A", "B", "C"].into_iter().enumerate() {
            let e = entry(&st, &[i], Some(name), "2048");
            add(&mut st, &app, e);
        }
    }
    app.invoke_export_queue_remove(1);
    let names: Vec<String> = rows(&app).into_iter().map(|r| r.0).collect();
    assert_eq!(names, ["A", "C"]);
    assert_eq!(
        presets_on_disk(&dir),
        [Some("A".to_string()), Some("C".to_string())]
    );
    assert_eq!(app.get_export_queue_line(), "2 sets, 2 frames queued");
    // Out of range is nothing.
    app.invoke_export_queue_remove(7);
    app.invoke_export_queue_remove(-1);
    assert_eq!(rows(&app).len(), 2);
    app.invoke_export_queue_remove(0);
    app.invoke_export_queue_remove(0);
    assert!(rows(&app).is_empty());
    assert!(!queue_file(&dir).exists());
    crate::testing::remove_dir_retry(&dir);
}

/// While the queue runs, the set it is on is marked and cannot be
/// removed; a set after it removed is not begun, and one before it
/// (left queued, its file not there) moves the run up with it, so
/// the run goes on to the right set.
#[test]
fn a_set_removed_during_a_run_is_skipped() {
    let (dir, files) = shoot("remove-run", 4);
    let app = crate::testing::window(4);
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    {
        let mut st = state.borrow_mut();
        st.work.export_queue_file = Some(queue_file(&dir));
        give_edits(&mut st);
        for (i, name) in ["A", "B", "C", "D"].into_iter().enumerate() {
            let e = entry(&st, &[i], Some(name), "2048");
            add(&mut st, &app, e);
        }
    }
    // A's file is gone: it stays queued and the run is on B.
    std::fs::remove_file(&files[0]).unwrap();
    app.invoke_export_queue_run();
    let running = |app: &App| -> Vec<bool> { rows(app).into_iter().map(|r| r.3).collect() };
    assert_eq!(running(&app), [false, true, false, false]);
    // B is the run's: its bin does nothing.
    app.invoke_export_queue_remove(1);
    assert_eq!(rows(&app).len(), 4);
    // C, after it, and A, before it.
    app.invoke_export_queue_remove(2);
    app.invoke_export_queue_remove(0);
    assert_eq!(running(&app), [true, false]);
    assert_eq!(
        presets_on_disk(&dir),
        [Some("B".to_string()), Some("D".to_string())]
    );
    assert_eq!(state.borrow().work.queue_run.as_ref().unwrap().entry, 0);
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0001.jpg"));
    finish(&app, &state);
    {
        let st = state.borrow();
        let set = st.work.exporting.as_ref().expect("the next set runs");
        assert_eq!(set.preset.as_deref(), Some("D"), "C is not begun");
    }
    assert_eq!(rows(&app).len(), 1);
    assert_eq!(running(&app), [true]);
    frame(&app, &state, 0, exported("/nowhere/out/IMG_0003.jpg"));
    finish(&app, &state);
    let st = state.borrow();
    assert!(st.work.export_queue.is_empty() && st.work.queue_run.is_none());
    assert!(
        st.sidecars[2].current_exports.is_empty(),
        "C was not exported"
    );
    assert!(!queue_file(&dir).exists());
    drop(st);
    crate::testing::remove_dir_retry(&dir);
}

/// Past five rows the list scrolls in its own box: the sheet is as
/// tall with nine sets as with five, and taller than with one.
#[test]
fn the_sheet_does_not_grow_past_five_rows() {
    use i_slint_backend_testing::ElementHandle;
    let (dir, files) = shoot("rows-tall", 9);
    let app = crate::testing::window(9);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 1400.0));
    let (state, _worker) = crate::testing::state_for(&app, files.clone());
    app.invoke_select(0);
    app.set_export_open(true);
    let height = |app: &App| {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        ElementHandle::find_by_element_id(app, "ExportSheet::card")
            .next()
            .expect("the sheet")
            .size()
            .height
    };
    let mut seen = Vec::new();
    for i in 0..9 {
        {
            let mut st = state.borrow_mut();
            let e = entry(&st, &[i], Some("Web"), "2048");
            add(&mut st, &app, e);
        }
        seen.push(height(&app));
    }
    assert!(seen[0] < seen[4], "{seen:?}");
    assert!(seen[3] < seen[4], "{seen:?}");
    for h in &seen[5..] {
        assert_eq!(*h, seen[4], "{seen:?}");
    }
    // Well inside the window: the room was there to grow.
    assert!(seen[4] < 1400.0 - 200.0, "{seen:?}");
    crate::testing::remove_dir_retry(&dir);
}
