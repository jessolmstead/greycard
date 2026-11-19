use super::*;
use crate::testing::{click, folder, press, state_for, window};
use crate::{Pt, ScopeMark};
use slint::Model;
use slint::platform::{Key, WindowEvent};
use std::cell::RefCell;

/// The camera's picture of `file`, as a decode thread delivers
/// one: a small copy of a JPEG of `source` pixels.
fn camera_picture(st: &State, file: usize, source: (u32, u32)) -> cull::Loaded {
    let (w, h) = (8u32, 6u32);
    cull::Loaded::Ok {
        file,
        frame: st.key_of(file).unwrap(),
        size: 1024,
        preview: Arc::new(cull::Preview::new(
            w,
            h,
            vec![0; (w * h * 4) as usize],
            source,
            false,
        )),
    }
}

/// A launch in Cull: the restored frame noted at the start
/// (`open_note`) comes off once the loupe has drawn its picture,
/// through the window's own frame (`land_pending`, `cull_frame`),
/// with no develop drawn. Skipped without a GPU.
#[test]
fn a_launch_in_cull_takes_the_open_note_off_once_the_loupe_draws() {
    let Some((device, queue)) = greycard_viewport::testing::device("the open note in Cull") else {
        return;
    };
    let dir = crate::testing::scratch_dir("open-note-cull");
    let app = window(4);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 800.0));
    let (state, _worker) = state_for(&app, folder(4));
    struct Unload(Rc<Shared>, std::path::PathBuf);
    impl Drop for Unload {
        fn drop(&mut self) {
            if let Ok(mut st) = self.0.try_borrow_mut() {
                st.develop.renderer = None;
            }
            crate::open_note::forget();
            crate::testing::remove_dir_retry(&self.1);
        }
    }
    let _unload = Unload(state.clone(), dir.clone());
    let restored = state.borrow().path_of(1).clone();
    assert_eq!(crate::open_note::start_in(&dir, Some(&restored)), None);
    let note = dir.join("opening");
    assert!(note.exists(), "the restored frame noted");
    {
        let mut st = state.borrow_mut();
        st.develop.renderer = Some(crate::render::Renderer::new(&device, &queue));
        st.current = Some(1);
        enter_cull(&mut st, &app, 1);
    }
    let frame = |state: &Rc<Shared>| {
        let mut st = state.borrow_mut();
        if crate::panel::startup::land_pending(&mut st, &app).loupe {
            cull_frame(&mut st, &app, state);
        }
    };
    // Nothing of it in hand yet: drawn, but not the frame.
    frame(&state);
    assert!(note.exists(), "the loupe has not drawn it yet");
    let picture = camera_picture(&state.borrow(), 1, (8, 6));
    deliver_preview(&app, picture);
    frame(&state);
    assert!(!note.exists(), "the loupe drew it");
}

/// A window in Cull over real files in a scratch folder, its
/// settings a scratch file and the timer that keeps the panel's
/// choices running, as a run starts it.
struct Kept {
    app: App,
    state: Rc<Shared>,
    worker: Rc<Worker>,
    files: Vec<PathBuf>,
    dir: PathBuf,
    choices: crate::panel::startup::Choices,
    _timer: slint::Timer,
}

impl Drop for Kept {
    fn drop(&mut self) {
        if let Ok(mut st) = self.state.try_borrow_mut() {
            st.develop.renderer = None;
        }
        crate::settings::use_file(None);
        crate::testing::remove_dir_retry(&self.dir);
    }
}

impl Kept {
    /// None without a GPU for the loupe to draw on.
    fn new(name: &str, count: usize) -> Option<Kept> {
        let gpu = greycard_viewport::testing::device("the last file in Cull")?;
        Some(Kept::with(name, count, Some(gpu)))
    }

    /// With no renderer: the loupe draws nothing, and a test says
    /// where it would have drawn (`startup::loupe_drew`).
    fn bare(name: &str, count: usize) -> Kept {
        Kept::with(name, count, None)
    }

    fn with(name: &str, count: usize, gpu: Option<(gpu::Device, gpu::Queue)>) -> Kept {
        let dir = crate::testing::scratch_dir(name);
        let files: Vec<PathBuf> = (0..count)
            .map(|i| {
                let f = dir.join(format!("IMG_{i:04}.CR3"));
                std::fs::write(&f, b"not a raw").unwrap();
                f
            })
            .collect();
        crate::settings::use_file(Some(dir.join("settings.json")));
        let app = window(count);
        app.window()
            .set_size(slint::LogicalSize::new(1200.0, 800.0));
        let (state, worker) = state_for(&app, files.clone());
        let choices = crate::panel::startup::Choices::new(&app, &state.borrow(), false, false);
        let timer = crate::panel::startup::keep_choices(&app, &state, &choices);
        {
            let mut st = state.borrow_mut();
            st.develop.renderer = gpu
                .as_ref()
                .map(|(device, queue)| crate::render::Renderer::new(device, queue));
            st.current = Some(0);
            enter_cull(&mut st, &app, 0);
        }
        Kept {
            app,
            state,
            worker,
            files,
            dir,
            choices,
            _timer: timer,
        }
    }

    /// The arrow onto `cell` in Cull, and the loupe's draw of it as
    /// `draw_cull` reports one, with no GPU to draw on.
    fn step_bare(&self, cell: usize) {
        let mut st = self.state.borrow_mut();
        cull_select(&mut st, &self.app, cell);
        crate::panel::startup::loupe_drew(&mut st);
    }

    /// The arrow onto `cell`, its camera picture in, and the
    /// window's frame drawn by the loupe.
    fn step(&self, cell: usize) {
        cull_select(&mut self.state.borrow_mut(), &self.app, cell);
        let picture = camera_picture(&self.state.borrow(), cell, (8, 6));
        deliver_preview(&self.app, picture);
        let mut st = self.state.borrow_mut();
        if crate::panel::startup::land_pending(&mut st, &self.app).loupe {
            cull_frame(&mut st, &self.app, &self.state);
        }
    }

    /// The open frame's develop landing, as the worker sends one.
    fn land(&self) {
        let generation = self.state.borrow().develop.generation;
        crate::panel::deliver::deliver(
            &self.app,
            crate::worker::Outcome::Developed {
                generation,
                turn: 0,
                kind: crate::finish::Source::SCENE,
                image: crate::worker::Developed::Halves(Arc::new(crate::worker::Halves {
                    width: 60,
                    height: 40,
                    pixels: vec![Default::default(); 60 * 40 * 4],
                })),
                guide: Arc::new(crate::finish::Guide::NONE),
                planes: None,
                white: crate::worker::WhiteBase::IDENTITY,
                seconds: 0.1,
                sharpen: None,
                sources: Vec::new(),
                learned: crate::worker::LearnedReport::Off,
                fills: crate::worker::FillReport::default(),
            },
        );
    }

    /// The window's frame drawn again, the loupe on the same frame.
    fn redraw(&self) {
        let mut st = self.state.borrow_mut();
        if crate::panel::startup::land_pending(&mut st, &self.app).loupe {
            cull_frame(&mut st, &self.app, &self.state);
        }
    }

    /// One tick of the choices' timer.
    fn tick(&self) {
        i_slint_backend_testing::mock_elapsed_time(crate::panel::startup::CHOICES_EVERY);
    }

    fn last_file(&self) -> String {
        crate::settings::Settings::load().shared.last_file
    }

    /// The file's `last_file` set by hand, to see whether anything
    /// writes over it.
    fn plant(&self, text: &str) {
        crate::settings::Settings::change(|s| s.shared.last_file = text.into());
    }

    fn canonical(&self, cell: usize) -> String {
        std::fs::canonicalize(&self.files[cell])
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }
}

/// Cull lands no develop: the frame it rests on is the next run's
/// all the same. The arrow held across many frames writes nothing
/// as it passes; the frame it stops on is written at the next tick
/// of the choices' timer, once, and not again while it stays; and
/// the close keeps a frame drawn since the last tick. Skipped
/// without a GPU.
#[test]
fn a_frame_stepped_to_in_cull_is_kept_for_the_next_run_once_it_settles() {
    let Some(kept) = Kept::new("cull-last-file", 4) else {
        return;
    };
    kept.plant("planted");
    // Forty frames down the arrow, no tick between: no write.
    for step in 1..=40 {
        kept.step(step % 4);
    }
    assert_eq!(kept.last_file(), "planted", "not written as it passed");
    kept.tick();
    assert_eq!(kept.last_file(), kept.canonical(0), "the frame it rests on");
    // Resting there, the loupe drawing it again between the ticks
    // (a redraw owes nothing): no write at the ticks after.
    kept.plant("planted");
    kept.redraw();
    kept.tick();
    kept.redraw();
    kept.tick();
    assert_eq!(kept.last_file(), "planted", "not written again");
    // A frame drawn and the window closed before the next tick: the
    // close keeps it (`keep_last_file`, from the loop's exit).
    kept.step(2);
    crate::panel::startup::keep_last_file(&mut kept.state.borrow_mut());
    assert_eq!(kept.last_file(), kept.canonical(2), "kept at the close");
}

/// A screenshot, a snapshot or a batch run steps through Cull and
/// writes the user's settings at neither the tick nor the close; a
/// run by hand does. Skipped without a GPU.
#[test]
fn a_capture_or_batch_run_in_cull_keeps_no_last_file() {
    let Some(kept) = Kept::new("cull-last-file-batch", 4) else {
        return;
    };
    kept.plant("planted");
    let owned: [fn(&mut State, bool); 3] = [
        |st, on| st.window.batch = on,
        |st, on| st.launch.screenshot = on.then(|| PathBuf::from("a.png")),
        |st, on| st.launch.snapshot = on.then(|| PathBuf::from("a.png")),
    ];
    for (i, set) in owned.iter().enumerate() {
        // Set after the draw: a capture's draw takes its path and
        // writes it, and the run ends there.
        kept.step(1 + i);
        set(&mut kept.state.borrow_mut(), true);
        kept.tick();
        crate::panel::startup::keep_last_file(&mut kept.state.borrow_mut());
        assert_eq!(kept.last_file(), "planted", "run {i}");
        set(&mut kept.state.borrow_mut(), false);
    }
    kept.step(0);
    kept.tick();
    assert_eq!(kept.last_file(), kept.canonical(0), "a run by hand");
}

/// A develop still writes its frame as it lands, at once, and a
/// frame Cull drew before it and had not written yet does not
/// write over it at the next tick; back in Cull, drawing that
/// frame again owes it again. Skipped without a GPU.
#[test]
fn a_develop_landing_writes_at_once_and_wins_over_an_older_cull_frame() {
    let Some(kept) = Kept::new("cull-last-file-develop", 4) else {
        return;
    };
    kept.step(1);
    // The arrow onto frame 2 and D, its develop landing before the
    // timer ticks.
    {
        let mut st = kept.state.borrow_mut();
        cull_select(&mut st, &kept.app, 2);
        to_develop(&mut st, &kept.app, &kept.worker);
    }
    kept.land();
    assert_eq!(kept.last_file(), kept.canonical(2), "as the develop lands");
    kept.tick();
    assert_eq!(kept.last_file(), kept.canonical(2), "not the older frame");
    to_cull(&mut kept.state.borrow_mut(), &kept.app, &kept.worker);
    kept.step(1);
    kept.tick();
    assert_eq!(kept.last_file(), kept.canonical(1), "drawn again");
}

/// In Develop, the camera's picture standing in for a raw's develop
/// is drawn by the same loupe: that is not Cull's frame, and the
/// last file waits for the develop to land, as it always has, the
/// open note with it. Skipped without a GPU.
#[test]
fn a_stand_in_drawn_in_develop_keeps_no_last_file_until_its_develop_lands() {
    let Some(kept) = Kept::new("develop-stand-in-last-file", 4) else {
        return;
    };
    to_develop(&mut kept.state.borrow_mut(), &kept.app, &kept.worker);
    kept.plant("planted");
    kept.app.invoke_select(2);
    // No smaller than the view, so up before the open says the size.
    let picture = camera_picture(&kept.state.borrow(), 2, (8192, 5464));
    deliver_preview(&kept.app, picture);
    {
        let mut st = kept.state.borrow_mut();
        assert_eq!(standing_in(&st), Some(2), "the camera's picture stands in");
        assert!(crate::panel::startup::land_pending(&mut st, &kept.app).loupe);
        cull_frame(&mut st, &kept.app, &kept.state);
    }
    kept.tick();
    crate::panel::startup::keep_last_file(&mut kept.state.borrow_mut());
    assert_eq!(kept.last_file(), "planted", "not before the develop");
    kept.land();
    assert_eq!(kept.last_file(), kept.canonical(2), "as the develop lands");
}

/// The rules of the last file with no GPU: Cull's loupe drawing a
/// frame owes it and the tick writes it; Develop's stand-in owes
/// nothing; a develop landing writes at once.
#[test]
fn the_last_file_is_cull_s_drawn_frame_or_develop_s_landed_one() {
    let kept = Kept::bare("last-file-bare", 4);
    kept.plant("planted");
    kept.step_bare(1);
    assert_eq!(kept.last_file(), "planted", "not at the draw");
    kept.tick();
    assert_eq!(kept.last_file(), kept.canonical(1), "at the tick");
    // Drawn again as it rests: owed no more, written no more.
    kept.plant("planted");
    crate::panel::startup::loupe_drew(&mut kept.state.borrow_mut());
    kept.tick();
    assert_eq!(kept.last_file(), "planted", "not written again");
    // Develop on another frame, its stand-in drawn.
    {
        let mut st = kept.state.borrow_mut();
        to_develop(&mut st, &kept.app, &kept.worker);
        st.current = Some(2);
        crate::panel::startup::loupe_drew(&mut st);
    }
    kept.tick();
    assert_eq!(kept.last_file(), "planted", "no stand-in kept");
    crate::panel::startup::remember_last_file(&mut kept.state.borrow_mut());
    assert_eq!(kept.last_file(), kept.canonical(2), "the develop's");
}

/// A frame Cull drew and has not written yet is written only while
/// it is still the open frame: not after Develop moves to another
/// frame before the tick; Cull drawing it again owes it again.
#[test]
fn a_frame_left_before_the_tick_is_not_written() {
    let kept = Kept::bare("last-file-left", 4);
    kept.plant("planted");
    // Develop on another frame.
    kept.step_bare(1);
    {
        let mut st = kept.state.borrow_mut();
        to_develop(&mut st, &kept.app, &kept.worker);
        st.current = Some(3);
    }
    kept.tick();
    assert_eq!(kept.last_file(), "planted", "Develop moved on");
    // Back in Cull on the frame: owed again.
    to_cull(&mut kept.state.borrow_mut(), &kept.app, &kept.worker);
    kept.step_bare(1);
    kept.tick();
    assert_eq!(kept.last_file(), kept.canonical(1), "owed again");
}

/// Another folder opened in the Library within the second of a
/// frame Cull drew: the old folder's frame is not written.
#[test]
fn a_frame_left_for_another_folder_before_the_tick_is_not_written() {
    let kept = Kept::bare("last-file-folder", 4);
    kept.plant("planted");
    kept.step_bare(2);
    let other = kept.dir.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let files: Vec<PathBuf> = (0..2)
        .map(|i| {
            let f = other.join(format!("DSC_{i:04}.CR3"));
            std::fs::write(&f, b"not a raw").unwrap();
            f
        })
        .collect();
    let held: Vec<crate::rows::Held> = files
        .iter()
        .map(|f| crate::rows::from_disk(f, None, Default::default()))
        .collect();
    kept.app.set_grid_open(true);
    crate::panel::browser::open_loaded(&kept.state, &kept.app, &kept.worker, files, held, true, 0);
    kept.tick();
    crate::panel::startup::keep_last_file(&mut kept.state.borrow_mut());
    assert_eq!(kept.last_file(), "planted", "the old folder's frame");
}

/// Develop's landings forget the frame Cull last drew: Cull drawing
/// it again, after Develop wrote another, owes it again.
#[test]
fn cull_drawing_its_old_frame_after_develop_moved_writes_it_again() {
    let kept = Kept::bare("last-file-back", 4);
    kept.step_bare(0);
    kept.tick();
    assert_eq!(kept.last_file(), kept.canonical(0), "Cull's");
    to_develop(&mut kept.state.borrow_mut(), &kept.app, &kept.worker);
    for frame in [0, 1] {
        let mut st = kept.state.borrow_mut();
        st.current = Some(frame);
        crate::panel::startup::remember_last_file(&mut st);
    }
    assert_eq!(kept.last_file(), kept.canonical(1), "Develop's");
    {
        let mut st = kept.state.borrow_mut();
        st.current = Some(0);
        to_cull(&mut st, &kept.app, &kept.worker);
        crate::panel::startup::loupe_drew(&mut st);
    }
    kept.tick();
    assert_eq!(kept.last_file(), kept.canonical(0), "Cull's again");
}

/// The writes after the loop returns (`after_loop`, for a loop
/// whose exit hook did not run) keep the frame Cull rests on.
#[test]
fn the_writes_after_the_loop_keep_cull_s_frame() {
    let kept = Kept::bare("last-file-after-loop", 4);
    kept.plant("planted");
    kept.step_bare(2);
    crate::panel::startup::after_loop(&kept.app, &kept.state, &kept.choices);
    assert_eq!(kept.last_file(), kept.canonical(2));
}

/// A run the command line owns (a script of keys, a timing run,
/// `--cull-develop`: `Launch::owned`) keeps no frame Cull draws, at
/// the tick or the close; Develop's landing keeps its own rule.
#[test]
fn a_run_the_command_line_owns_keeps_no_cull_frame() {
    let kept = Kept::bare("last-file-owned", 4);
    kept.plant("planted");
    kept.state.borrow_mut().launch.owned = true;
    kept.step_bare(1);
    kept.tick();
    crate::panel::startup::keep_last_file(&mut kept.state.borrow_mut());
    assert_eq!(kept.last_file(), "planted");
    crate::panel::startup::remember_last_file(&mut kept.state.borrow_mut());
    assert_eq!(kept.last_file(), kept.canonical(1), "Develop's as it was");
}

/// The loop's exit runs the hook the run installs (`at_exit`),
/// which writes the frame Cull rests on; the write after the loop
/// returns (for a lost device's way out) finds the owe spent and
/// writes nothing again, and writes it itself when the hook did not
/// run.
#[test]
fn the_loops_exit_keeps_cull_s_frame_once() {
    let kept = Kept::bare("last-file-exit", 4);
    crate::panel::startup::at_exit(&kept.app, &kept.state, &kept.choices);
    kept.plant("planted");
    kept.step_bare(2);
    crate::panel::startup::exiting();
    assert_eq!(kept.last_file(), kept.canonical(2), "at the exit");
    kept.plant("planted");
    crate::panel::startup::keep_last_file(&mut kept.state.borrow_mut());
    assert_eq!(kept.last_file(), "planted", "once");
    // The hook never ran (a panic on a lost device): the write
    // after the loop keeps it.
    kept.step_bare(3);
    crate::panel::startup::keep_last_file(&mut kept.state.borrow_mut());
    assert_eq!(kept.last_file(), kept.canonical(3), "after the loop");
}

/// The lightbox's loupe over Cull is Cull: its frame is kept as
/// Cull's is.
#[test]
fn the_lightbox_loupe_over_cull_keeps_its_frame() {
    let kept = Kept::bare("last-file-lightbox", 4);
    kept.plant("planted");
    press(&kept.app, "l");
    assert!(kept.app.get_lightbox());
    press(&kept.app, Key::Return);
    assert!(kept.app.get_lightbox_loupe() && !kept.app.get_grid_open());
    press(&kept.app, Key::RightArrow);
    {
        let mut st = kept.state.borrow_mut();
        assert!(st.culling.cull.is_some(), "the loupe is Cull's");
        crate::panel::startup::loupe_drew(&mut st);
    }
    kept.tick();
    let open = kept.state.borrow().current.unwrap();
    assert_eq!(kept.last_file(), kept.canonical(open));
}

/// A frame's local preview and its file land in either order, on
/// two threads: a preview after the camera's JPEG does not replace
/// it, and a file failure before the preview leaves the preview up
/// with no failure said over it and the file not asked for again.
#[test]
fn a_late_local_preview_and_an_early_file_failure_leave_the_right_picture() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    {
        let mut st = state.borrow_mut();
        st.current = Some(1);
        enter_cull(&mut st, &app, 1);
    }
    let local = |st: &State, file: usize| cull::Loaded::Ok {
        file,
        frame: st.key_of(file).unwrap(),
        size: 1024,
        preview: Arc::new(cull::Preview::local(
            greycard_library::thumbs::Thumb {
                width: 4,
                height: 2,
                rgb: vec![0; 24],
            },
            false,
        )),
    };
    // The camera's JPEG first, then a late local preview.
    let camera = camera_picture(&state.borrow(), 1, (8192, 5464));
    deliver_preview(&app, camera);
    let late = local(&state.borrow(), 1);
    deliver_preview(&app, late);
    {
        let st = state.borrow();
        let shown = st.culling.cull.as_ref().unwrap().cache.get(1).unwrap();
        assert!(!shown.local, "the camera's JPEG stays");
    }
    // The file's failure first, then the local preview.
    let failed = cull::Loaded::Failed {
        file: 2,
        frame: state.borrow().key_of(2).unwrap(),
        size: 1024,
        message: "the share did not answer".into(),
    };
    deliver_preview(&app, failed);
    assert!(
        state
            .borrow()
            .culling
            .cull
            .as_ref()
            .unwrap()
            .failed
            .contains_key(&2)
    );
    let preview = local(&state.borrow(), 2);
    deliver_preview(&app, preview);
    let mut st = state.borrow_mut();
    {
        let cull = st.culling.cull.as_ref().unwrap();
        assert!(cull.cache.get(2).is_some_and(|p| p.local), "up");
        assert!(!cull.failed.contains_key(&2), "no failure said over it");
        assert!(cull.file_failed.contains(&2));
    }
    cull_refresh(&mut st);
    assert!(!st.culling.prefetch.asked().iter().any(|w| w.file == 2));
}

/// A raw with no camera picture in it comes back developed: it is
/// up, no failure is said over it, the status line names it as the
/// local preview and not the camera's JPEG, and its file is not
/// asked for again, since a develop is what the file has to give.
#[test]
fn a_developed_picture_is_up_named_as_the_local_preview_and_not_asked_again() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    {
        let mut st = state.borrow_mut();
        st.current = Some(1);
        enter_cull(&mut st, &app, 1);
    }
    let (w, h) = (64u32, 48u32);
    let mut preview = cull::Preview::new(w, h, vec![0; (w * h * 4) as usize], (w, h), false);
    preview.developed = true;
    let loaded = cull::Loaded::Ok {
        file: 1,
        frame: state.borrow().key_of(1).unwrap(),
        size: 1024,
        preview: Arc::new(preview),
    };
    // The stand-in's failure for it first, as when culling is
    // entered with the stand-in's decode still in flight.
    let failed = cull::Loaded::Failed {
        file: 1,
        frame: state.borrow().key_of(1).unwrap(),
        size: 1024,
        message: "no camera preview in the file".into(),
    };
    deliver_preview(&app, failed);
    assert!(
        state
            .borrow()
            .culling
            .cull
            .as_ref()
            .unwrap()
            .failed
            .contains_key(&1)
    );
    deliver_preview(&app, loaded);
    let mut st = state.borrow_mut();
    {
        let cull = st.culling.cull.as_ref().unwrap();
        assert!(!cull.file_failed.contains(&1), "the file gave its picture");
        let shown = cull.cache.get(1).expect("up");
        assert_eq!(shown.origin(), cull::Origin::Local);
        assert!(!cull.failed.contains_key(&1), "no failure said over it");
        let names = [Named {
            file: 1,
            size: (w, h),
            small: shown.small(),
            local: shown.origin(),
            own: shown.own_size(),
            version: None,
        }];
        assert_eq!(
            cull_status(cull, 1, 0.0, true, &names),
            "culling: the local preview, 64 \u{d7} 48, fitted; Enter develops"
        );
    }
    cull_refresh(&mut st);
    assert!(!st.culling.prefetch.asked().iter().any(|w| w.file == 1));
}

/// Culling left on another frame than the one opened before it: the
/// part asks and the dismissals of the frame opened before go, as
/// on any open (a part pasted across is the same key and shape on
/// both), and the masks' rasters are this frame's from here. Left
/// on that same frame, its asks stand, since its raster does and
/// nothing would ask again; only a pick under way is put down.
#[test]
fn leaving_culling_on_another_frame_drops_the_last_ones_asks() {
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    let mut st = state.borrow_mut();
    assert_eq!(st.develop.learned_of.as_ref(), Some(&st.files[0]));
    st.develop
        .part_asks
        .insert((1, 0), crate::ai::Ask::default());
    st.develop.part_dismissed.insert((1, 1));
    let pending = || crate::panel::parts::Pending {
        id: 1,
        part: crate::panel::parts::by_label("Lips").unwrap(),
        mode: greycard_edit::mask::Mode::Add,
    };
    st.develop.part_pending = Some(pending());
    // In and out on the same frame.
    enter_cull(&mut st, &app, 1);
    leave_cull(&mut st, &app, &worker, None);
    assert_eq!(st.develop.part_asks.len(), 1);
    assert_eq!(st.develop.part_dismissed.len(), 1);
    assert!(st.develop.part_pending.is_none());
    assert_eq!(st.develop.learned_of.as_ref(), Some(&st.files[0]));
    // In, and out on another.
    st.develop.part_pending = Some(pending());
    enter_cull(&mut st, &app, 1);
    st.current = Some(2);
    leave_cull(&mut st, &app, &worker, None);
    assert!(st.develop.part_pending.is_none());
    assert!(st.develop.part_asks.is_empty());
    assert!(st.develop.part_dismissed.is_empty());
    assert_eq!(st.develop.learned_of.as_ref(), Some(&st.files[2]));
}

/// Culling left on a frame of the other mosaic: the demosaic menu
/// is that frame's, from what the session knows of it, or resting
/// while nothing is known; never the frame before's list, enabled.
#[test]
fn leaving_culling_onto_an_x_trans_frame_shows_its_own_menu() {
    use crate::panel::browser::OpenMosaic;
    use greycard_core::raw::Mosaic;
    let app = window(3);
    let (state, worker) = state_for(&app, folder(3));
    let mut st = state.borrow_mut();
    let (bayer, xtrans) = (st.files[0].clone(), st.files[2].clone());
    st.develop
        .mosaics
        .insert(bayer, OpenMosaic::Known(Mosaic::Bayer));
    st.develop
        .mosaics
        .insert(xtrans.clone(), OpenMosaic::Known(Mosaic::XTrans));
    drop(st);
    app.invoke_select(0);
    assert_eq!(app.get_demosaic(), "amaze-vng4");
    assert_eq!(app.get_demosaics().row_count(), 6);
    let mut st = state.borrow_mut();
    // Into culling on the Bayer frame, across to the X-Trans one as
    // a key in the mode does, and out there.
    enter_cull(&mut st, &app, 1);
    cull_select(&mut st, &app, 2);
    leave_cull(&mut st, &app, &worker, None);
    assert_eq!(st.open_mosaic(), OpenMosaic::Known(Mosaic::XTrans));
    assert_eq!(app.get_demosaic(), "markesteijn-3");
    let names: Vec<String> = app.get_demosaics().iter().map(|s| s.to_string()).collect();
    assert_eq!(names, ["markesteijn-3", "markesteijn-1", "bilinear"]);
    assert_eq!(app.get_demosaic_hint(), "");
    // A frame nothing is known of yet: the menu rests.
    st.develop.mosaics.remove(&xtrans);
    enter_cull(&mut st, &app, 1);
    cull_select(&mut st, &app, 2);
    leave_cull(&mut st, &app, &worker, None);
    assert_eq!(st.open_mosaic(), OpenMosaic::Unknown);
    assert_eq!(app.get_demosaic_hint(), "reading the raw");
    assert_eq!(app.get_demosaics().row_count(), 1);
    drop(st);
    // The worker's word lands, through the hop from its thread to
    // the window's: the menu fills.
    crate::STATE.with(|s| *s.borrow_mut() = Some(state.clone()));
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Mosaic {
            path: xtrans.clone(),
            mosaic: Some(Mosaic::XTrans),
        },
    );
    crate::STATE.with(|s| *s.borrow_mut() = None);
    assert_eq!(app.get_demosaic_hint(), "");
    assert_eq!(app.get_demosaic(), "markesteijn-3");
    assert_eq!(
        state.borrow().develop.mosaics.get(&xtrans),
        Some(&OpenMosaic::Known(Mosaic::XTrans))
    );
    // An open that fails while nothing has said what the raw is:
    // the menu stops waiting and says so.
    {
        let mut st = state.borrow_mut();
        st.develop.mosaics.clear();
        enter_cull(&mut st, &app, 1);
        cull_select(&mut st, &app, 2);
        leave_cull(&mut st, &app, &worker, None);
        assert_eq!(app.get_demosaic_hint(), "reading the raw");
    }
    let generation = state.borrow().develop.generation;
    crate::STATE.with(|s| *s.borrow_mut() = Some(state.clone()));
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Failed {
            generation,
            message: "not a raw this build reads".into(),
        },
    );
    crate::STATE.with(|s| *s.borrow_mut() = None);
    assert_eq!(app.get_demosaic_hint(), "the raw could not be read");
    assert_eq!(state.borrow().open_mosaic(), OpenMosaic::Unreadable);
}

/// A raw with the camera's JPEG beside it is culled on that JPEG:
/// the loupe's wants for it name the JPEG, at the view's size and
/// at 1:1; its neighbors with none name nothing.
#[test]
fn a_raw_with_a_companion_is_culled_on_the_jpeg_beside_it() {
    let app = window(4);
    let files = folder(4);
    let (state, _worker) = state_for(&app, files.clone());
    let jpeg = files[1].with_extension("JPG");
    let mut st = state.borrow_mut();
    st.views
        .companions
        .insert(files[1].clone(), vec![jpeg.clone()]);
    st.current = Some(1);
    enter_cull(&mut st, &app, 1);
    let asked = st.culling.prefetch.asked();
    let of = |file: usize| asked.iter().filter(move |w| w.file == file);
    assert!(of(1).count() > 0, "{asked:?}");
    assert!(
        of(1).all(|w| w.picture.as_ref() == Some(&jpeg)),
        "{asked:?}"
    );
    assert!(of(2).all(|w| w.picture.is_none()), "{asked:?}");
    // At 1:1, the full copy first, from the JPEG too.
    st.develop.zoom = 1.0;
    cull_refresh(&mut st);
    let asked = st.culling.prefetch.asked();
    let full = asked
        .iter()
        .find(|w| w.file == 1 && w.size == 0)
        .expect("the full copy");
    assert_eq!(full.picture.as_ref(), Some(&jpeg));
}

/// Two frames compared at 1:1: each is asked for at its full size,
/// the chosen one first, each is held whole once delivered, and a
/// frame that leaves the set lets its copy go. A full copy of a
/// frame not shown is not taken in.
#[test]
fn two_frames_compared_at_one_to_one_are_each_held_whole() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    {
        let mut st = state.borrow_mut();
        st.current = Some(1);
        enter_cull(&mut st, &app, 2);
        st.develop.zoom = 1.0;
        cull_refresh(&mut st);
        let asked = st.culling.prefetch.asked();
        let full: Vec<usize> = asked
            .iter()
            .filter(|w| w.size == 0)
            .map(|w| w.file)
            .collect();
        assert_eq!(full, vec![1, 0], "{asked:?}");
        assert!(asked.iter().position(|w| w.size != 0) >= Some(2));
    }
    let whole = |st: &State, file: usize| cull::Loaded::Ok {
        file,
        frame: st.key_of(file).unwrap(),
        size: 0,
        preview: Arc::new(cull::Preview::new(8, 6, vec![0; 192], (8, 6), true)),
    };
    // The log waits for the set: one frame in is not it.
    let loaded = whole(&state.borrow(), 1);
    deliver_preview(&app, loaded);
    assert_eq!(
        state.borrow().culling.cull.as_ref().unwrap().full_said,
        None
    );
    for file in [0, 3] {
        let loaded = whole(&state.borrow(), file);
        deliver_preview(&app, loaded);
    }
    {
        let st = state.borrow();
        let cull = st.culling.cull.as_ref().unwrap();
        assert!(cull.cache.has_own_size(0) && cull.cache.has_own_size(1));
        assert!(cull.cache.full(3).is_none(), "frame 3 is not shown");
        assert_eq!(cull.full_said.as_deref(), Some(&[0, 1][..]));
    }
    // On to frame 2: the set is 1 and 2, so frame 0's copy goes, 1
    // keeps its own, and 2 is asked for whole.
    let mut st = state.borrow_mut();
    cull_select(&mut st, &app, 2);
    let cull = st.culling.cull.as_ref().unwrap();
    assert!(cull.cache.full(0).is_none() && cull.cache.full(1).is_some());
    let asked = st.culling.prefetch.asked();
    let full: Vec<usize> = asked
        .iter()
        .filter(|w| w.size == 0)
        .map(|w| w.file)
        .collect();
    assert_eq!(full, vec![2], "{asked:?}");
}

/// The full sizes put at the front of the queue since `from`.
fn pushed_whole(st: &State, from: usize) -> Vec<usize> {
    let mut files: Vec<usize> = st.culling.prefetch.pushed()[from..]
        .iter()
        .filter(|w| w.size == 0)
        .map(|w| w.file)
        .collect();
    files.sort_unstable();
    files
}

/// Asks a fit drops from the queue are asked again at the next 1:1:
/// a frame still waiting for its full copy when the view was fitted,
/// and the selection moved within the set, is not left soft.
#[test]
fn a_fit_and_an_arrow_within_the_set_leave_no_ask_behind() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let mut st = state.borrow_mut();
    st.current = Some(1);
    enter_cull(&mut st, &app, 2);
    st.develop.zoom = 1.0;
    ask_whole(&mut st);
    assert_eq!(pushed_whole(&st, 0), vec![0, 1]);
    // Fitted, the copies let go, then an arrow onto frame 0, which
    // keeps the set at 0 and 1; its list asks for nothing whole.
    st.develop.zoom = 0.0;
    let _ = st.culling.cull.as_mut().unwrap().cache.drop_full();
    cull_select(&mut st, &app, 0);
    assert!(st.culling.cull.as_ref().unwrap().full_asked.is_empty());
    // 1:1 again: both are asked for.
    let from = st.culling.prefetch.pushed().len();
    st.develop.zoom = 1.0;
    ask_whole(&mut st);
    assert_eq!(pushed_whole(&st, from), vec![0, 1]);
}

/// A new list forgets what was asked of the last one's numbers: a
/// cell of the same number at 1:1 is asked for whole.
#[test]
fn a_new_list_forgets_the_full_asks_of_the_last() {
    let app = window(4);
    let (state, worker) = state_for(&app, folder(4));
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        enter_cull(&mut st, &app, 1);
        st.develop.zoom = 1.0;
        ask_whole(&mut st);
        assert_eq!(pushed_whole(&st, 0), vec![0]);
    }
    let files: Vec<PathBuf> = (0..4)
        .map(|i| PathBuf::from(format!("/other/B_{i}.CR3")))
        .collect();
    let held = files
        .iter()
        .map(|f| crate::rows::from_disk(f, None, Default::default()))
        .collect();
    crate::panel::browser::open_loaded(&state, &app, &worker, files, held, true, 0);
    let mut st = state.borrow_mut();
    assert!(st.culling.cull.as_ref().unwrap().full_asked.is_empty());
    // Frame 0 of the new list, at 1:1 (as Space leaves it: no list).
    let from = st.culling.prefetch.pushed().len();
    st.current = Some(0);
    st.develop.zoom = 1.0;
    ask_whole(&mut st);
    assert_eq!(pushed_whole(&st, from), vec![0]);
}

/// A versions compare at 1:1 asks for the chosen tile's camera JPEG
/// whole and no other: its versions share that JPEG, and the other
/// tiles keep the pictures of their edits.
#[test]
fn a_versions_compare_at_one_to_one_holds_the_chosen_tile_alone_whole() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    {
        let mut st = state.borrow_mut();
        st.current = Some(1);
        enter_cull(&mut st, &app, 2);
        let path = st.path_of(1).clone();
        let cull = st.culling.cull.as_mut().unwrap();
        cull.versions_of = Some(path);
        cull.set = Some(vec![0, 1]);
        st.develop.zoom = 1.0;
        cull_refresh(&mut st);
        let asked = st.culling.prefetch.asked();
        let full: Vec<usize> = asked
            .iter()
            .filter(|w| w.size == 0)
            .map(|w| w.file)
            .collect();
        assert_eq!(full, vec![1], "{asked:?}");
    }
    let whole = |st: &State, file: usize| cull::Loaded::Ok {
        file,
        frame: st.key_of(file).unwrap(),
        size: 0,
        preview: Arc::new(cull::Preview::new(8, 6, vec![0; 192], (8, 6), true)),
    };
    for file in [1, 0] {
        let loaded = whole(&state.borrow(), file);
        deliver_preview(&app, loaded);
    }
    let st = state.borrow();
    let cull = st.culling.cull.as_ref().unwrap();
    assert!(cull.cache.full(1).is_some() && cull.cache.full(0).is_none());
}

/// The frame's full-size copy held from 1:1 and no view-size one
/// (the compare view entered from 1:1 sets the fit and asks again
/// before the next frame lets the full copy go): the view-size copy
/// is asked for all the same, or its tile would stay blank.
#[test]
fn a_held_full_copy_does_not_stand_in_for_the_view_size_one() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let mut st = state.borrow_mut();
    st.current = Some(1);
    enter_cull(&mut st, &app, 1);
    let size = st.culling.cull.as_ref().unwrap().size;
    let full = Arc::new(cull::Preview::new(8, 6, vec![0; 192], (8192, 5464), true));
    st.culling.cull.as_mut().unwrap().cache.insert(1, full);
    st.develop.zoom = 0.0;
    cull_refresh(&mut st);
    let asked = st.culling.prefetch.asked();
    assert!(
        asked
            .iter()
            .any(|w| w.file == 1 && w.size == size && w.from == cull::Source::File),
        "{asked:?}"
    );
}

/// A frame chosen in the develop view shows its camera JPEG at
/// once and keeps it until its develop lands, and the panel takes
/// its readings of the developed picture off the screen while it
/// is up. Driven through the window's own select callback.
#[test]
fn a_frame_chosen_shows_its_camera_picture_until_its_develop_lands() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    // A scope and a navigator from the frame before it, and the
    // color under a pointer resting on the picture.
    app.set_nav_partial(true);
    app.set_clip_highlights_lit(true);
    app.set_scope_readout("R 1  G 2  B 3  L 2".into());
    app.set_scope_marks(ModelRc::new(VecModel::from(vec![ScopeMark {
        x: 0.5,
        y: 0.5,
        channel: 3,
    }])));
    state.borrow_mut().develop.hover = Some((300.0, 200.0));
    app.invoke_select(1);
    {
        let st = state.borrow();
        let wait = st
            .culling
            .placeholder
            .as_ref()
            .expect("a frame is waited on");
        assert_eq!(wait.file, 1);
        assert!(!wait.up(), "nothing stands in until the JPEG is decoded");
        assert_eq!(standing_in(&st), None);
    }
    assert!(
        !app.get_placeholder(),
        "the picture on screen is still ours"
    );

    // The decode comes back: the camera's picture stands in.
    let loaded = camera_picture(&state.borrow(), 1, (8192, 5464));
    deliver_preview(&app, loaded);
    {
        let st = state.borrow();
        assert!(st.culling.placeholder.as_ref().unwrap().up());
        assert_eq!(standing_in(&st), Some(1));
        assert_eq!(st.develop.zoom, 0.0, "a placeholder is the fit view's");
    }
    assert!(app.get_placeholder());
    assert_eq!(
        app.get_placeholder_status(),
        "camera preview: the camera JPEG, 8192 \u{d7} 5464, fitted; developing..."
    );
    // The readings of a picture that is not on screen wait.
    assert!(!app.get_nav_partial());
    assert!(!app.get_clip_highlights_lit());
    assert!(state.borrow().develop.bins.is_none());
    assert_eq!(app.get_scope_picture().size().width, 0);
    assert_eq!(app.get_scope_readout(), "");
    assert_eq!(app.get_scope_marks().row_count(), 0);

    // The develop lands: ours takes its place and the panel comes
    // back to it.
    let generation = state.borrow().develop.generation;
    assert!(develop_landed(&mut state.borrow_mut(), &app, generation));
    assert!(!app.get_placeholder());
    assert_eq!(app.get_placeholder_status(), "");
    assert!(state.borrow().culling.placeholder.is_none());
    assert!(
        state
            .borrow()
            .culling
            .hold
            .as_ref()
            .unwrap()
            .cache
            .get(1)
            .is_some(),
        "the picture is kept for the arrow back"
    );
}

/// The command line's zoom stands through the first file's camera
/// picture, as it does into the develop, and the line under it
/// says so; a frame opened from one that is on screen still opens
/// fitted.
#[test]
fn the_command_lines_zoom_stands_through_the_first_placeholder() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    state.borrow_mut().develop.zoom = 1.0;
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (8192, 5464));
    deliver_preview(&app, loaded);
    assert_eq!(standing_in(&state.borrow()), Some(1));
    assert_eq!(state.borrow().develop.zoom, 1.0, "the command line's zoom");
    assert_eq!(
        app.get_placeholder_status(),
        "camera preview: the camera JPEG, 8192 \u{d7} 5464, 100%, \
         screen-size copy until the full one decodes; developing..."
    );

    // Its develop lands at that zoom, and the user moves on from
    // it: the next frame is fitted.
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(&app, developed(generation, (6000, 4000)));
    assert_eq!(state.borrow().develop.zoom, 1.0, "the develop keeps it");
    state.borrow_mut().develop.base_white = Some(crate::worker::WhiteBase::IDENTITY);
    app.invoke_select(2);
    assert!(
        state.borrow().develop.held.is_some(),
        "the picture on screen is held"
    );
    let loaded = camera_picture(&state.borrow(), 2, (8192, 5464));
    deliver_preview(&app, loaded);
    assert_eq!(standing_in(&state.borrow()), Some(2));
    assert_eq!(
        state.borrow().develop.zoom,
        0.0,
        "a later frame opens fitted"
    );
    assert!(
        app.get_placeholder_status()
            .ends_with("fitted; developing...")
    );
}

/// A run started in culling (`--cull`) opens its first file in the
/// loupe, where no mask is painted: the command line's mask and
/// patch go with that open, and are not left for a frame opened
/// later.
#[test]
fn a_first_open_in_culling_leaves_no_mask_for_a_later_frame() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    {
        let mut st = state.borrow_mut();
        st.launch.show_mask = Some(0);
        st.launch.show_patch = Some(0);
        st.launch.cull_at_start = Some(1);
    }
    app.invoke_select(1);
    let st = state.borrow();
    assert!(st.culling.cull.is_some());
    assert!(st.launch.opened);
    assert_eq!((st.launch.show_mask, st.launch.show_patch), (None, None));
    assert!(
        !st.launch.zoom_stands,
        "no develop for the zoom to stand into"
    );
}

/// The frame on screen deleted, zoomed in: the nearest frame left
/// opens fitted, as a click on it would, and not at the zoom the
/// deleted one was at. With no frame current there is nothing to
/// leave, so only the open's being the session's first or not can
/// say the zoom is the command line's.
#[test]
fn a_frame_after_one_deleted_while_zoomed_opens_fitted() {
    let app = window(4);
    let (state, worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(&app, developed(generation, (6000, 4000)));
    {
        let mut st = state.borrow_mut();
        st.develop.base_white = Some(crate::worker::WhiteBase::IDENTITY);
        st.develop.zoom = 2.0;
    }
    drop_files(&mut state.borrow_mut(), &app, &worker, &[1], "gone".into());
    assert_eq!(state.borrow().current, None);
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (8192, 5464));
    deliver_preview(&app, loaded);
    assert_eq!(standing_in(&state.borrow()), Some(1));
    assert_eq!(
        state.borrow().develop.zoom,
        0.0,
        "fitted under the camera's picture"
    );
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(&app, developed(generation, (6000, 4000)));
    assert_eq!(state.borrow().develop.zoom, 0.0, "and under the develop");
}

/// Frame `file` chosen in Develop after a developed one, so the
/// picture on screen is held, with its camera picture of `source`
/// standing in (and its focus point told), laid out as `cull_frame`
/// lays it: fitted, centered.
fn stand_in_for(
    app: &App,
    state: &Rc<Shared>,
    file: usize,
    source: (u32, u32),
    point: Option<(
        greycard_core::raw::FocusPoint,
        greycard_core::raw::Orientation,
    )>,
) {
    app.invoke_select(file as i32);
    let mut st = state.borrow_mut();
    assert!(st.develop.held.is_some(), "the last frame is held");
    focus_frame(&mut st, file, source, point);
    assert_eq!(standing_in(&st), Some(file));
    st.develop.zoom = 0.0;
    st.develop.image_size = source;
    st.develop.center = (source.0 as f32 / 2.0, source.1 as f32 / 2.0);
}

/// A frame drawn by the render as far as a window without a GPU
/// goes: a develop that landed taken up (`land_pending`), and the
/// develop's frame laid out by the render's own layout
/// (`viewport::frame_on_show`) when no stand-in draws.
fn draw_laid_out(app: &App, state: &Rc<Shared>) {
    draw(app, state);
    let mut st = state.borrow_mut();
    if loupe_draws(&st) {
        return;
    }
    let panel = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    let edit = crate::panel::startup::view_edit(&st, &panel);
    crate::panel::viewport::frame_on_show(&mut st, app, &edit, &panel);
}

/// The open frame's develop of `size` landing and drawn.
fn land_and_lay_out(app: &App, state: &Rc<Shared>, size: (u32, u32)) {
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(app, developed(generation, size));
    draw_laid_out(app, state);
    assert_eq!(
        standing_in(&state.borrow()),
        None,
        "the develop replaced the stand-in"
    );
}

fn view_of(state: &Rc<Shared>) -> (f32, (f32, f32)) {
    let st = state.borrow();
    (st.develop.zoom, st.develop.center)
}

fn near_view(got: (f32, (f32, f32)), want: (f32, (f32, f32)), what: &str) {
    assert!(
        (got.0 - want.0).abs() < 1e-3
            && (got.1.0 - want.1.0).abs() < 1.0
            && (got.1.1 - want.1.1).abs() < 1.0,
        "{what}: {got:?}, not {want:?}"
    );
}

/// A window with frame 0 developed and its white known, so the
/// next frame chosen holds it while it develops.
fn developed_window(count: usize, names: Option<&str>) -> (App, Rc<Shared>, Rc<Worker>) {
    let files = match names {
        Some(dir) => (0..count)
            .map(|i| format!("/nowhere/{dir}/IMG_{i:04}.CR3").into())
            .collect(),
        None => folder(count),
    };
    developed_window_of(files)
}

/// The same, of `files`.
fn developed_window_of(files: Vec<PathBuf>) -> (App, Rc<Shared>, Rc<Worker>) {
    let app = window(files.len());
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = state_for(&app, files);
    app.invoke_select(0);
    land_and_lay_out(&app, &state, (6000, 4000));
    state.borrow_mut().develop.base_white = Some(crate::worker::WhiteBase::IDENTITY);
    (app, state, worker)
}

/// A frame zoomed into by Space or Z while its camera picture
/// stands in stays zoomed on the same place when its develop
/// lands, at the level the key named: Space on the camera's focus
/// point at 1:1, Z at 3:1 about the middle and a pan on a JPEG half
/// the frame's size. It once snapped to fit.
#[test]
fn a_zoom_key_pressed_over_the_stand_in_stays_when_the_develop_lands() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let (app, state, _worker) = developed_window(4, Some("stood-zoom"));

    // Space: 1:1 on the camera's point.
    let point = FocusPoint {
        area: [0.7, 0.2, 0.1, 0.1],
        aspect: 1.5,
    };
    stand_in_for(
        &app,
        &state,
        1,
        (6000, 4000),
        Some((point, Orientation::Normal)),
    );
    press(&app, " ");
    near_view(view_of(&state), (1.0, (4500.0, 1000.0)), "Space");
    land_and_lay_out(&app, &state, (6024, 4016));
    near_view(
        view_of(&state),
        (1.0, (4518.0, 1004.0)),
        "Space, the develop landed",
    );

    // Z: 3:1 about the middle, then a pan, on a JPEG half the
    // frame's size: 3:1 of the develop, on the same place.
    stand_in_for(&app, &state, 2, (3000, 2000), None);
    press(&app, "z");
    near_view(view_of(&state), (3.0, (1500.0, 1000.0)), "Z");
    app.invoke_pan(30.0, -60.0);
    let (_, (cx, cy)) = view_of(&state);
    land_and_lay_out(&app, &state, (6000, 4000));
    near_view(
        view_of(&state),
        (3.0, (cx * 2.0, cy * 2.0)),
        "Z, the develop landed",
    );
}

/// On a camera JPEG a quarter of the frame's width (the small
/// preview some raws carry), Space stays 1:1 of the develop on the
/// focus point, while a wheel's zoom keeps its scale on screen.
#[test]
fn a_small_stand_in_keeps_a_named_level_and_a_wheels_scale() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let (app, state, _worker) = developed_window(4, Some("stood-small"));
    let point = FocusPoint {
        area: [0.7, 0.2, 0.1, 0.1],
        aspect: 1.5,
    };
    stand_in_for(
        &app,
        &state,
        1,
        (1500, 1000),
        Some((point, Orientation::Normal)),
    );
    press(&app, " ");
    near_view(view_of(&state), (1.0, (1125.0, 250.0)), "Space");
    land_and_lay_out(&app, &state, (6000, 4000));
    near_view(
        view_of(&state),
        (1.0, (4500.0, 1000.0)),
        "1:1 of the develop",
    );

    stand_in_for(&app, &state, 2, (1500, 1000), None);
    app.invoke_zoom(1.25, 500.0, 300.0);
    app.invoke_zoom(4.0, 500.0, 300.0);
    let (z, (cx, cy)) = view_of(&state);
    land_and_lay_out(&app, &state, (6000, 4000));
    near_view(
        view_of(&state),
        (z / 4.0, (cx * 4.0, cy * 4.0)),
        "the wheel's scale on screen",
    );
}

/// The worker's open of `file` says the frame is `size`, as
/// `deliver`'s `Opened` arm records it.
fn open_said(app: &App, state: &Rc<Shared>, file: usize, size: (u32, u32)) {
    let mut st = state.borrow_mut();
    let path = st.path_of(file).clone();
    st.develop.opened_sizes.push((path, Some(size)));
    size_opened(&mut st, app);
}

/// The loupe's current picture laid out as `draw_cull` lays it.
fn lay_out_stand_in(app: &App, state: &Rc<Shared>) {
    let mut st = state.borrow_mut();
    let c = standing_in(&st).expect("a picture stands in");
    let panel = Some(c) == st.current;
    lay_out_current(&mut st, app, c, panel);
}

/// The tile `draw_cull` draws a small camera picture standing in
/// with (`tile_views`) covers on screen what the develop will,
/// fitted and at 1:1: it is drawn as the develop's size, not its
/// own.
#[test]
fn the_stand_ins_tile_covers_what_the_develop_will() {
    let (app, state, _worker) = developed_window(4, Some("small-tile"));
    app.invoke_select(1);
    open_said(&app, &state, 1, (5912, 2896));
    focus_frame(&mut state.borrow_mut(), 1, (400, 196), None);
    let mut st = state.borrow_mut();
    st.develop.mat = false;
    let current = lay_out_current(&mut st, &app, 1, true);
    let rows = cull::row_of_shown(&st.views.shown, 1)
        .into_iter()
        .collect::<Vec<_>>();
    // A room 1000 by 600, where the develop fits at 1000 / 5912; the
    // copy on the GPU is 8 by 6 (`focus_frame`).
    let mut on_screen = |zoom: f32| {
        st.develop.zoom = zoom;
        let planned = tile_views(&st, &app, 1, current, &rows, &[(0, 0, 1000, 600)], [0.0; 3]);
        let (view, named) = planned[0].clone().expect("a tile");
        // The line names the camera's picture by its own size.
        assert_eq!(named, (400, 196));
        view.zoom * 8.0
    };
    assert!((on_screen(0.0) - 1000.0).abs() < 0.5, "fitted");
    assert!((on_screen(1.0) - 5912.0).abs() < 0.5, "1:1");
}

/// A camera JPEG far smaller than its raw (a merge's DNG carries
/// one of 1024 px) stands in at the develop's footprint: it
/// waits for the open to say the frame's size rather than go up at
/// its own pixels, which a fit never enlarges, and is then laid out
/// at the develop's size, so the develop lands fitted where it
/// stood. It once sat small in the view and grew when the develop
/// landed.
#[test]
fn a_small_camera_picture_stands_in_at_the_develops_size() {
    let (app, state, _worker) = developed_window(4, Some("small-camera"));
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (400, 196));
    deliver_preview(&app, loaded);
    assert_ne!(
        standing_in(&state.borrow()),
        Some(1),
        "the small picture waits for the frame's size"
    );
    open_said(&app, &state, 1, (5912, 2896));
    assert_eq!(standing_in(&state.borrow()), Some(1), "then goes up");
    lay_out_stand_in(&app, &state);
    let stood = view_of(&state);
    assert_eq!(state.borrow().develop.image_size, (5912, 2896));
    assert_eq!(stood.0, 0.0, "fitted");
    land_and_lay_out(&app, &state, (5912, 2896));
    assert_eq!(state.borrow().develop.image_size, (5912, 2896));
    near_view(view_of(&state), stood, "the develop landed");
}

/// Space on a small camera JPEG standing in, its frame's size
/// known, goes to 1:1 of the develop on the focus point, so the
/// develop lands with nothing moving.
#[test]
fn space_on_a_small_camera_picture_is_the_develops_one_to_one() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let (app, state, _worker) = developed_window(4, Some("small-space"));
    app.invoke_select(1);
    open_said(&app, &state, 1, (5912, 2896));
    let point = FocusPoint {
        area: [0.7, 0.2, 0.1, 0.1],
        aspect: 400.0 / 196.0,
    };
    focus_frame(
        &mut state.borrow_mut(),
        1,
        (400, 196),
        Some((point, Orientation::Normal)),
    );
    lay_out_stand_in(&app, &state);
    assert_eq!(state.borrow().develop.image_size, (5912, 2896));
    press(&app, " ");
    let stood = view_of(&state);
    near_view(stood, (1.0, (4434.0, 724.0)), "Space");
    land_and_lay_out(&app, &state, (5912, 2896));
    near_view(view_of(&state), stood, "the develop landed");
}

/// A camera picture already up in its own pixels when the open says
/// the frame's size is taken into the develop's: a wheel's zoom on
/// it keeps its scale and its place on screen through the change and
/// through the develop.
#[test]
fn a_wheel_zoom_on_the_camera_picture_stays_as_the_size_comes() {
    let (app, state, _worker) = developed_window(4, Some("small-wheel"));
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (3000, 2000));
    deliver_preview(&app, loaded);
    assert_eq!(
        standing_in(&state.borrow()),
        Some(1),
        "no room to spare: up at once"
    );
    lay_out_stand_in(&app, &state);
    assert_eq!(state.borrow().develop.image_size, (3000, 2000));
    app.invoke_zoom(1.25, 500.0, 300.0);
    app.invoke_zoom(4.0, 500.0, 300.0);
    let (z, (cx, cy)) = view_of(&state);
    assert!(z > 0.0, "the wheel zoomed");
    open_said(&app, &state, 1, (6000, 4000));
    lay_out_stand_in(&app, &state);
    assert_eq!(state.borrow().develop.image_size, (6000, 4000));
    let want = (z / 2.0, (cx * 2.0, cy * 2.0));
    near_view(view_of(&state), want, "the size came");
    land_and_lay_out(&app, &state, (6000, 4000));
    near_view(view_of(&state), want, "the develop landed");
}

/// The size frame 1's camera picture of `source` is laid out at in
/// Develop once the open has said `opened`: cut by the camera's
/// `frame` where one is given, on a file of the kind that has one.
fn stood_size(
    dir: &str,
    source: (u32, u32),
    opened: (u32, u32),
    frame: Option<greycard_core::raw::CameraFrame>,
) -> (u32, u32) {
    let ext = if frame.is_some() { "ORF" } else { "CR3" };
    let files = (0..3)
        .map(|i| format!("/nowhere/{dir}/IMG_{i:04}.{ext}").into())
        .collect();
    let (app, state, _worker) = developed_window_of(files);
    app.invoke_select(1);
    open_said(&app, &state, 1, opened);
    {
        let mut st = state.borrow_mut();
        focus_frame(&mut st, 1, source, None);
        if let Some(frame) = frame {
            let path = st.path_of(1).clone();
            crate::edited_pictures::note_decoded(&path, Some(frame));
            let hold = st.culling.hold.as_mut().unwrap();
            let mut preview = cull::Preview::new(8, 6, vec![0; 192], source, false);
            preview.cut = true;
            hold.cache.insert(1, Arc::new(preview));
        }
    }
    lay_out_stand_in(&app, &state);
    state.borrow().develop.image_size
}

/// In Cull the camera's picture is the picture, and 1:1 is its own
/// pixels: a small JPEG is laid out at its own size there, though
/// the open has said the frame's.
#[test]
fn in_cull_a_small_camera_picture_keeps_its_own_size() {
    let (app, state, _worker) = developed_window(4, Some("small-cull"));
    app.invoke_select(1);
    open_said(&app, &state, 1, (5912, 2896));
    let mut st = state.borrow_mut();
    enter_cull(&mut st, &app, 1);
    assert!(st.culling.cull.is_some());
    focus_frame(&mut st, 1, (400, 196), None);
    lay_out_current(&mut st, &app, 1, false);
    assert_eq!(st.develop.image_size, (400, 196));
}

/// A camera picture of another shape than its raw (the camera set
/// to 16:9, 1:1, or 4:3 on a 3:2 sensor) is the largest of its shape
/// inside the develop: a small one grows until it meets the
/// develop's nearer side, and a full-size one stays its own size,
/// so 1:1 on it is the develop's 1:1.
#[test]
fn a_camera_picture_of_another_shape_keeps_its_shape() {
    let open = (6000, 4000);
    assert_eq!(
        stood_size("shape-small", (1600, 900), open, None),
        (6000, 3375)
    );
    for (dir, own) in [
        ("shape-wide", (6000, 3375)),
        ("shape-square", (4000, 4000)),
        ("shape-four-three", (5328, 4000)),
    ] {
        assert_eq!(stood_size(dir, own, open, None), own, "{dir}");
    }
}

/// A portrait frame, its picture lying as the develop does or the
/// other way, takes the develop's size as the picture lies.
#[test]
fn a_portrait_camera_picture_takes_the_develops_size() {
    assert_eq!(
        stood_size("portrait", (400, 600), (4000, 6000), None),
        (4000, 6000)
    );
    assert_eq!(
        stood_size("portrait-across", (400, 600), (6000, 4000), None),
        (4000, 6000)
    );
}

/// A camera picture cut to the camera's frame (the middle 3:2 band of
/// a 4:3 sensor) takes the develop cut the same way, lying as the
/// develop does or the other way.
#[test]
fn a_cut_camera_picture_takes_the_develop_cut_alike() {
    use greycard_core::raw::{CameraFrame, Orientation, Rect};
    let band = |orientation| CameraFrame {
        area: Rect {
            x: 0,
            y: 0,
            width: 4000,
            height: 3000,
        },
        frame: Rect {
            x: 0,
            y: 166,
            width: 4000,
            height: 2667,
        },
        orientation,
    };
    assert_eq!(
        stood_size(
            "cut",
            (1000, 667),
            (4000, 3000),
            Some(band(Orientation::Normal))
        ),
        (4000, 2667)
    );
    // Turned by the tag: the develop and the cut stand, the picture
    // lies across them.
    assert_eq!(
        stood_size(
            "cut-across",
            (1000, 667),
            (3000, 4000),
            Some(band(Orientation::Rotate90))
        ),
        (4000, 2667)
    );
}

/// Each tile's view as `draw_cull` worked it out before the tiles
/// were planned apart from the draw (at f8af2e05), for
/// `tiles_are_planned_as_they_were_drawn`.
fn views_as_drawn_before(
    st: &State,
    app: &App,
    c: usize,
    rows: &[usize],
    rects: &[(u32, u32, u32, u32)],
) -> Vec<Option<View>> {
    let holding = st.culling.cull.is_none();
    let from_panel = |file: usize| holding && Some(file) == st.current;
    let stand_in = stands_for(st, app, c);
    let cull = st
        .culling
        .cull
        .as_ref()
        .or(st.culling.hold.as_ref())
        .unwrap();
    let current_full = cull.cache.best(c).map(|p| {
        let source = stand_in.unwrap_or(p.source);
        let (turns, flip) =
            thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, c, app, from_panel(c));
        Geometry {
            turns,
            flip,
            ..Geometry::default()
        }
        .plane_size(source.0 as f32, source.1 as f32)
    });
    rows.iter()
        .zip(rects)
        .map(|(&row, rect)| {
            let file = st.views.shown[row];
            let preview = cull.cache.best(file)?;
            let (turns, flip) = thumb_turns_of(
                &st.sidecars,
                &st.from_row,
                &st.cells,
                file,
                app,
                from_panel(file),
            );
            let geometry = Geometry {
                turns,
                flip,
                ..Geometry::default()
            };
            let (tw, th) = (preview.width as f32, preview.height as f32);
            let source = stand_in.filter(|_| file == c).unwrap_or(preview.source);
            let (fw, fh) = (source.0 as f32, source.1 as f32);
            let scale = tw / fw;
            let full = geometry.plane_size(fw, fh);
            let frame = geometry.frame(tw, th, None);
            let room = if st.develop.mat && rows.len() <= 1 {
                crate::zoom::matted((rect.2, rect.3))
            } else {
                (rect.2, rect.3)
            };
            let zoom = if st.develop.zoom > 0.0 {
                st.develop.zoom
            } else {
                (room.0 as f32 / full.0)
                    .min(room.1 as f32 / full.1)
                    .min(1.0)
            };
            let center = match current_full {
                Some(cur) if file != c => (
                    st.develop.center.0 / cur.0.max(1.0) * full.0,
                    st.develop.center.1 / cur.1.max(1.0) * full.1,
                ),
                _ => st.develop.center,
            };
            Some(View {
                zoom: zoom / scale,
                center: (center.0 * scale, center.1 * scale),
                matrix: geometry.matrix(),
                plane: geometry.plane_size(tw, th),
                frame_origin: frame.origin,
                frame_size: frame.size,
                canvas: [0.1, 0.2, 0.3],
                ..View::blank()
            })
        })
        .collect()
}

/// Ordinary frames in Cull, one, two, and four to a compare, fitted
/// and zoomed off the middle, the mat on and off, turned and not,
/// with a full-size copy in hand: each tile is planned exactly as
/// it was drawn before the tiles were planned apart from the draw,
/// the shared center, the mat, and the fit with them.
#[test]
fn tiles_are_planned_as_they_were_drawn() {
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(6));
    let mut st = state.borrow_mut();
    st.current = Some(1);
    let mut checked = 0;
    for compare in [1usize, 2, 4] {
        st.culling.cull = None;
        enter_cull(&mut st, &app, compare);
        st.sidecars[2].turn = 1;
        st.sidecars[3].turn = 2;
        {
            let cull = st.culling.cull.as_mut().unwrap();
            let sizes = [
                (6000, 4000),
                (4000, 6000),
                (5328, 4000),
                (6000, 3375),
                (1616, 1080),
                (3000, 3000),
            ];
            for (f, s) in sizes.iter().enumerate() {
                let preview = cull::Preview::new(8, 6, vec![0; 192], *s, false);
                cull.cache.insert(f, Arc::new(preview));
            }
            // And frame 1's full-size copy.
            let full = cull::Preview::new(16, 12, vec![0; 768], (6000, 4000), true);
            cull.cache.insert(1, Arc::new(full));
        }
        for c in [1usize, 2] {
            for mat in [false, true] {
                for (zoom, off) in [
                    (0.0, (0.0, 0.0)),
                    (1.0, (-700.0, 333.0)),
                    (2.5, (911.0, -42.0)),
                ] {
                    st.develop.mat = mat;
                    st.develop.zoom = zoom;
                    st.develop.image_size = (0, 0);
                    let current = lay_out_current(&mut st, &app, c, false);
                    let (cx, cy) = st.develop.center;
                    st.develop.center = (cx + off.0, cy + off.1);
                    let rows = st.culling.cull.as_ref().unwrap().rows(&st.views.shown);
                    let rects = cull::tile_rects(1400, 900, rows.len().max(1));
                    let planned = tile_views(&st, &app, c, current, &rows, &rects, [0.1, 0.2, 0.3]);
                    let before = views_as_drawn_before(&st, &app, c, &rows, &rects);
                    assert_eq!(planned.len(), before.len());
                    for (now, then) in planned.iter().zip(&before) {
                        let now = now.as_ref().map(|(view, _)| view);
                        assert_eq!(
                            now,
                            then.as_ref(),
                            "{compare} to a compare, frame {c}, mat {mat}, zoom {zoom}"
                        );
                        checked += usize::from(now.is_some());
                    }
                }
            }
        }
    }
    assert!(checked > 30, "{checked}");
}

/// The first fit after 1:1 lets the full-size copy go before the
/// tiles are planned (`plan_tiles`), so the tile is planned on the
/// screen-size copy it is drawn from: fitted, which never enlarges,
/// an 80 px picture is 80 px on screen. It was once planned on the
/// full copy and drawn from the small one, a tenth the size.
#[test]
fn a_fit_after_one_to_one_plans_the_copy_it_draws() {
    let app = window(4);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(4));
    let mut st = state.borrow_mut();
    st.current = Some(1);
    enter_cull(&mut st, &app, 1);
    {
        let cull = st.culling.cull.as_mut().unwrap();
        let small = cull::Preview::new(8, 6, vec![0; 192], (80, 60), false);
        cull.cache.insert(1, Arc::new(small));
        let full = cull::Preview::new(80, 60, vec![0; 19200], (80, 60), true);
        cull.cache.insert(1, Arc::new(full));
    }
    st.develop.mat = false;
    st.develop.zoom = 0.0;
    let current = lay_out_current(&mut st, &app, 1, false);
    let rows = st.culling.cull.as_ref().unwrap().rows(&st.views.shown);
    let rects = cull::tile_rects(1000, 600, rows.len().max(1));
    let planned = plan_tiles(&mut st, &app, 1, current, &rows, &rects, [0.0; 3]);
    let drawn = st
        .culling
        .cull
        .as_ref()
        .unwrap()
        .cache
        .best(1)
        .unwrap()
        .clone();
    let (view, _) = planned[0].clone().unwrap();
    let on_screen = view.zoom * drawn.width as f32;
    assert!(
        (on_screen - 80.0).abs() < 0.5,
        "{on_screen} px on screen from a copy {} x {}",
        drawn.width,
        drawn.height
    );
}

/// The wheel and a click to 1:1 over the stand-in are kept as the
/// keys are; a crop in the edit takes the place through it, the
/// camera's picture being of the whole sensor; and the next frame,
/// chosen while zoomed, still opens fitted under its stand-in and
/// its develop.
#[test]
fn a_wheel_or_click_zoom_over_the_stand_in_stays_through_a_crop() {
    let (app, state, _worker) = developed_window(4, None);

    // The wheel, about a point off the middle.
    stand_in_for(&app, &state, 1, (6000, 4000), None);
    app.invoke_zoom(1.25, 500.0, 300.0);
    app.invoke_zoom(8.0, 500.0, 300.0);
    let wheeled = view_of(&state);
    assert!(wheeled.0 > 0.0, "the wheel zoomed");
    land_and_lay_out(&app, &state, (6000, 4000));
    near_view(view_of(&state), wheeled, "the wheel");

    // A click to 1:1, with the edit's crop the right half.
    stand_in_for(&app, &state, 2, (6000, 4000), None);
    app.set_geo_has_crop(true);
    app.set_geo_crop_x(0.5);
    app.set_geo_crop_y(0.0);
    app.set_geo_crop_w(0.5);
    app.set_geo_crop_h(1.0);
    let (vx, vy) = (
        app.get_view_width() as f32 / 2.0,
        app.get_view_height() as f32 / 2.0,
    );
    app.invoke_toggle_zoom(vx + 150.0, vy - 50.0);
    let (zoom, (cx, cy)) = view_of(&state);
    assert_eq!(zoom, 1.0, "the click went to 1:1");
    assert!(
        cx > 3500.0 && cy < 2000.0,
        "on the crop, off its middle: {cx}, {cy}"
    );
    land_and_lay_out(&app, &state, (6000, 4000));
    assert_eq!(state.borrow().develop.image_size, (3000, 4000));
    near_view(
        view_of(&state),
        (1.0, (cx - 3000.0, cy)),
        "1:1 through the crop",
    );
    app.set_geo_has_crop(false);

    // The next frame, chosen at that zoom: fitted, as ever.
    app.invoke_select(3);
    let loaded = camera_picture(&state.borrow(), 3, (6000, 4000));
    deliver_preview(&app, loaded);
    assert_eq!(
        state.borrow().develop.zoom,
        0.0,
        "fitted under the stand-in"
    );
    land_and_lay_out(&app, &state, (6000, 4000));
    assert_eq!(state.borrow().develop.zoom, 0.0, "and under the develop");
}

/// A zoom on the stand-in noted as its develop lands is for that
/// frame alone: the next frame chosen before the window draws,
/// whose own develop then fails, shows the held picture fitted,
/// not at the last frame's place.
#[test]
fn a_frame_chosen_before_the_draw_takes_no_zoom_from_the_last() {
    let (app, state, _worker) = developed_window(4, None);
    stand_in_for(&app, &state, 1, (3000, 2000), None);
    press(&app, "z");
    app.invoke_pan(30.0, -60.0);
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(&app, developed(generation, (6000, 4000)));
    assert!(state.borrow().develop.stood_view.is_some(), "noted");
    app.invoke_select(2);
    assert!(
        state.borrow().develop.stood_view.is_none(),
        "gone with its frame"
    );
    let loaded = camera_picture(&state.borrow(), 2, (6000, 4000));
    deliver_preview(&app, loaded);
    draw_laid_out(&app, &state);
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Failed {
            generation,
            message: "unreadable".into(),
        },
    );
    draw_laid_out(&app, &state);
    assert_eq!(state.borrow().current, Some(2));
    assert_eq!(view_of(&state).0, 0.0, "fitted");
}

/// Leaving Cull for Develop, the loupe's picture stands in for the
/// develop: a zoom and a pan on it are kept when it lands.
#[test]
fn a_zoom_on_the_loupes_picture_after_cull_stays_into_develop() {
    let (app, state, worker) = developed_window(4, None);
    {
        let mut st = state.borrow_mut();
        enter_cull(&mut st, &app, 1);
        cull_select(&mut st, &app, 1);
        focus_frame(&mut st, 1, (6000, 4000), None);
        leave_cull(&mut st, &app, &worker, None);
        assert!(st.develop.held.is_some(), "the last develop is held");
        assert_eq!(standing_in(&st), Some(1), "the loupe's picture stands in");
        st.develop.image_size = (6000, 4000);
        st.develop.center = (3000.0, 2000.0);
    }
    press(&app, "z");
    app.invoke_pan(30.0, -60.0);
    let before = view_of(&state);
    assert!(before.0 > 0.0);
    land_and_lay_out(&app, &state, (6000, 4000));
    near_view(view_of(&state), before, "from Cull");
}

/// A zoom on the stand-in taken back to fit before the develop
/// lands leaves it fitted and centered.
#[test]
fn a_zoom_taken_back_to_fit_over_the_stand_in_lands_fitted() {
    let (app, state, _worker) = developed_window(4, None);
    stand_in_for(&app, &state, 1, (6000, 4000), None);
    press(&app, "z");
    assert!(view_of(&state).0 > 0.0);
    press(&app, "z");
    assert_eq!(view_of(&state).0, 0.0, "Z again fits");
    land_and_lay_out(&app, &state, (6000, 4000));
    assert_eq!(view_of(&state), (0.0, (3000.0, 2000.0)));
}

/// Arrows taken while zoomed open each frame fitted: a late develop
/// of a frame left notes nothing, and a develop that lands while
/// the frame before's picture still stands in is fitted.
#[test]
fn frames_arrowed_through_while_zoomed_open_fitted() {
    let (app, state, _worker) = developed_window(5, None);
    stand_in_for(&app, &state, 1, (6000, 4000), None);
    press(&app, "z");
    let left = state.borrow().develop.generation;
    app.invoke_select(2);
    let loaded = camera_picture(&state.borrow(), 2, (6000, 4000));
    deliver_preview(&app, loaded);
    assert_eq!(view_of(&state).0, 0.0, "2 fitted under its stand-in");
    crate::panel::deliver::deliver(&app, developed(left, (6000, 4000)));
    assert!(
        state.borrow().develop.stood_view.is_none(),
        "a late develop notes nothing"
    );
    {
        let mut st = state.borrow_mut();
        st.develop.image_size = (6000, 4000);
        st.develop.center = (3000.0, 2000.0);
    }
    press(&app, "z");
    assert!(view_of(&state).0 > 0.0);
    app.invoke_select(3);
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(&app, developed(generation, (6000, 4000)));
    draw_laid_out(&app, &state);
    assert_eq!(view_of(&state).0, 0.0, "3 opens fitted");
}

/// The whole sensor's camera picture onto a crop, under each of the
/// edit's four quarter turns, mirrored and not: the same place.
#[test]
fn the_stand_ins_place_goes_through_every_turn_and_mirror_to_a_crop() {
    for turns in 0..4i32 {
        for flip in [false, true] {
            let (app, state, _worker) = developed_window(4, None);
            app.invoke_select(1);
            app.set_geo_turns(turns);
            app.set_geo_flip(flip);
            let plane = if turns % 2 == 1 {
                (4000u32, 6000u32)
            } else {
                (6000, 4000)
            };
            {
                let mut st = state.borrow_mut();
                focus_frame(&mut st, 1, (6000, 4000), None);
                st.develop.zoom = 1.0;
                st.develop.image_size = plane;
                st.develop.center = (0.7 * plane.0 as f32, 0.3 * plane.1 as f32);
            }
            app.set_geo_has_crop(true);
            app.set_geo_crop_x(0.5);
            app.set_geo_crop_y(0.0);
            app.set_geo_crop_w(0.5);
            app.set_geo_crop_h(1.0);
            land_and_lay_out(&app, &state, (6000, 4000));
            let want = (0.2 * plane.0 as f32, 0.3 * plane.1 as f32);
            near_view(
                view_of(&state),
                (1.0, want),
                &format!("{turns} turns, mirrored {flip}"),
            );
        }
    }
}

/// A develop as the worker delivers one: a picture of `size` and
/// nothing else of interest.
fn developed(generation: u64, size: (u32, u32)) -> crate::worker::Outcome {
    developed_at(generation, size, 0)
}

/// The same, developed at `turn` quarter turns of the frame.
fn developed_at(generation: u64, size: (u32, u32), turn: u8) -> crate::worker::Outcome {
    developed_as(generation, size, turn, crate::finish::Source::SCENE)
}

/// The same, of a picture of `kind`: a raw's scene, or a picture
/// already rendered.
fn developed_as(
    generation: u64,
    size: (u32, u32),
    turn: u8,
    kind: crate::finish::Source,
) -> crate::worker::Outcome {
    crate::worker::Outcome::Developed {
        generation,
        turn,
        kind,
        image: crate::worker::Developed::Halves(Arc::new(crate::worker::Halves {
            width: size.0,
            height: size.1,
            pixels: Vec::new(),
        })),
        guide: Arc::new(crate::finish::Guide::NONE),
        planes: None,
        white: crate::worker::WhiteBase::IDENTITY,
        seconds: 0.1,
        sharpen: None,
        sources: Vec::new(),
        learned: crate::worker::LearnedReport::Off,
        fills: crate::worker::FillReport::default(),
    }
}

/// The worker's open of the frame chosen last: a raw, or a
/// picture already rendered.
fn opened(generation: u64, kind: crate::worker::SourceKind) -> crate::worker::Outcome {
    crate::worker::Outcome::Opened {
        generation,
        as_shot: (5500.0, 0.0),
        frame: None,
        profile: None,
        lens: crate::worker::LensReport::NoDatabase,
        lens_options: Vec::new(),
        kind,
        shot: Default::default(),
        taken: None,
        blend: None,
    }
}

/// A frame drawn, as far as a window without a GPU goes: the
/// render's own start, which takes up a develop that has landed,
/// and the kind the view is given for the picture on the GPU.
fn draw(app: &App, state: &Rc<Shared>) -> crate::finish::Source {
    crate::panel::startup::land_pending(&mut state.borrow_mut(), app).kind
}

/// The look the view is given: of the edit it draws the picture on
/// the GPU under, as the render reads it.
fn drawn_look(app: &App, state: &Rc<Shared>) -> f32 {
    let st = state.borrow();
    let panel = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    View::with_look(&crate::panel::startup::view_edit(&st, &panel))
        .light
        .exposure
}

/// The picture on screen is finished as what it is, whatever the
/// open frame is: a raw's develop held up while a picture file
/// opens is still drawn as a raw's scene, through its baseline and
/// its shoulder, until the picture's own develop lands in its
/// place, and the other way about. Drawn as the open frame's kind,
/// a picture already rendered would flash over a stop too bright
/// and too contrasty, and a raw's scene would go dark and flat.
#[test]
fn the_picture_on_screen_is_drawn_as_its_own_kind_until_the_next_lands() {
    use crate::finish::Source;
    use crate::worker::SourceKind;
    let files: Vec<std::path::PathBuf> = ["IMG_0000.CR3", "IMG_0001.JPG", "IMG_0002.CR3"]
        .iter()
        .map(|f| std::path::PathBuf::from("/nowhere").join(f))
        .collect();
    let app = window(3);
    let (state, _worker) = state_for(&app, files);
    let picture = || SourceKind::Picture {
        space: "sRGB".into(),
        bits: 8,
    };
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;

    // A raw opens and develops: its scene is what the view draws.
    let raw = Source::raw("Canon", "EOS R5");
    app.invoke_select(0);
    crate::panel::deliver::deliver(&app, opened(generation(&state), SourceKind::Raw));
    let developed = developed_as(generation(&state), (6000, 4000), 0, raw);
    crate::panel::deliver::deliver(&app, developed);
    assert_eq!(draw(&app, &state), raw);

    // On to a picture file: its open says what it is, to the
    // panel, and the raw's develop held on screen meanwhile is
    // still drawn as the raw's scene.
    app.invoke_select(1);
    assert!(
        state.borrow().develop.held.is_some(),
        "the raw's develop is held"
    );
    crate::panel::deliver::deliver(&app, opened(generation(&state), picture()));
    assert!(!app.get_raw_input(), "the panel's is the picture's");
    assert_eq!(draw(&app, &state), raw, "the picture on screen's");
    // Its develop delivered, the frame that takes it up draws it
    // as what it is.
    let developed = developed_as(generation(&state), (4000, 3000), 0, Source::Display);
    crate::panel::deliver::deliver(&app, developed);
    assert_eq!(draw(&app, &state), Source::Display);

    // And the other way: a raw opened over the picture's develop.
    app.invoke_select(2);
    assert!(state.borrow().develop.held.is_some());
    crate::panel::deliver::deliver(&app, opened(generation(&state), SourceKind::Raw));
    assert!(app.get_raw_input());
    assert_eq!(draw(&app, &state), Source::Display);
    let developed = developed_as(generation(&state), (6000, 4000), 0, raw);
    crate::panel::deliver::deliver(&app, developed);
    assert_eq!(draw(&app, &state), raw);
}

/// A frame chosen and left before its develop lands leaves the
/// picture on screen what it was, under that picture's own look:
/// the frame it is a develop of, as the panel last had it, and not
/// the look of the frame passed through. Its late develop is
/// dropped, and the next frame's own takes the held look away.
#[test]
fn a_frame_left_before_its_develop_keeps_the_last_picture_under_its_look() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    // Z develops, and its exposure moves on the panel after.
    app.invoke_select(0);
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    draw(&app, &state);
    app.set_exposure(1.5);
    assert_eq!(drawn_look(&app, &state), 1.5);

    // On to A, whose own look is other, and on to B before A's
    // develop lands: Z's picture is drawn under Z's look all along.
    app.invoke_select(1);
    app.set_exposure(-1.0);
    assert_eq!(drawn_look(&app, &state), 1.5);
    let late = generation(&state);
    app.invoke_select(2);
    draw(&app, &state);
    assert_eq!(drawn_look(&app, &state), 1.5, "Z's look over Z's picture");

    // A's develop, landing late, is not B's.
    crate::panel::deliver::deliver(&app, developed(late, (6000, 4000)));
    assert!(state.borrow().develop.pending.is_none());
    assert_eq!(drawn_look(&app, &state), 1.5);
    // B's own lands, and is drawn under B's panel.
    app.set_exposure(0.25);
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    draw(&app, &state);
    assert!(state.borrow().develop.held.is_none());
    assert_eq!(drawn_look(&app, &state), 0.25);
}

/// The droppers that read the picture's pixels wait for the open
/// frame's develop: the last frame's held up meanwhile is another
/// frame's, and a camera picture standing in is not the develop
/// the texture holds. The status line says so, as the Neutral
/// dropper's does.
#[test]
fn a_dropper_waits_for_the_open_frames_develop() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    app.invoke_select(0);
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    draw(&app, &state);
    app.set_picking("Curve".into());
    let wait = "curve: wait for the develop";

    // The last frame's develop held.
    app.invoke_select(1);
    app.set_status("".into());
    app.invoke_pick_pressed(400.0, 300.0);
    assert_eq!(app.get_status(), wait);
    assert!(state.borrow().develop.picking.is_none());

    // Its camera picture standing in.
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    app.set_status("".into());
    app.invoke_pick_pressed(400.0, 300.0);
    assert_eq!(app.get_status(), wait);

    // Its develop delivered, and not yet up on the GPU: the
    // texture and the size the view maps by are still the last's.
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    app.set_status("".into());
    app.invoke_pick_pressed(400.0, 300.0);
    assert_eq!(app.get_status(), wait);

    // Up: the dropper reads it (here, with no GPU to sample, it
    // finds nothing and says nothing).
    draw(&app, &state);
    app.set_status("".into());
    app.invoke_pick_pressed(400.0, 300.0);
    assert_eq!(app.get_status(), "");

    // In and out of Cull on the same frame, its loupe picture
    // standing in for the develop that asks for: nothing held, the
    // develop on the GPU the frame's own, and still the texture is
    // not what is on screen. The Neutral dropper waits too.
    app.invoke_cull_toggled();
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    app.invoke_cull_leave();
    {
        let st = state.borrow();
        assert!(st.develop.held.is_none());
        assert_eq!(standing_in(&st), Some(1));
    }
    for (dropper, said) in [
        ("Curve", wait),
        ("White", "white balance: wait for the develop"),
    ] {
        app.set_picking(dropper.into());
        app.set_status("".into());
        app.invoke_pick_pressed(400.0, 300.0);
        assert_eq!(app.get_status(), said, "{dropper}");
    }
}

/// Cull left onto another frame than the one developed, with no
/// camera picture of it to stand in: the develop still on the GPU
/// is drawn under its own frame's look, as it was saved on the way
/// into the mode, until this frame's lands.
#[test]
fn leaving_cull_onto_another_frame_keeps_the_last_picture_under_its_look() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    app.invoke_select(0);
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    draw(&app, &state);
    app.set_exposure(1.5);

    // Into Cull, on to frame 1, whose camera picture never comes,
    // and out.
    app.invoke_cull_toggled();
    app.invoke_select(1);
    app.invoke_cull_leave();
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(standing_in(&state.borrow()), None);
    assert_eq!(app.get_exposure(), 0.0, "the panel is frame 1's");
    draw(&app, &state);
    assert_eq!(
        drawn_look(&app, &state),
        1.5,
        "frame 0's look over its picture"
    );

    // Frame 1's own develop lands, under frame 1's panel.
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    draw(&app, &state);
    assert!(state.borrow().develop.held.is_none());
    assert_eq!(drawn_look(&app, &state), 0.0);

    // Back into Cull and out on the same frame: its own develop,
    // under its own panel, with nothing held.
    app.invoke_cull_toggled();
    app.invoke_cull_leave();
    assert!(state.borrow().develop.held.is_none());
}

/// A frame come back to whose camera picture is still kept from
/// the first visit stands in with it at once, as on that visit,
/// rather than leaving the last frame's develop on screen under
/// a line that says it is the camera's preview.
#[test]
fn a_frame_come_back_to_stands_in_with_its_kept_camera_picture() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    assert_eq!(standing_in(&state.borrow()), Some(1));
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    draw(&app, &state);
    assert_eq!(
        standing_in(&state.borrow()),
        None,
        "its develop in its place"
    );

    // Frame 2 develops with no camera picture of its own in time.
    app.invoke_select(2);
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    draw(&app, &state);
    assert_eq!(standing_in(&state.borrow()), None);

    // Back to 1: frame 2's develop is held, and frame 1's camera
    // picture stands in over it, not that develop.
    app.invoke_select(1);
    assert!(state.borrow().develop.held.is_some());
    assert_eq!(standing_in(&state.borrow()), Some(1));
    assert!(app.get_placeholder());
    assert!(
        app.get_placeholder_status()
            .starts_with("camera preview: the camera JPEG, 6000 \u{d7} 4000")
    );
    // Until its develop lands.
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    draw(&app, &state);
    assert_eq!(standing_in(&state.borrow()), None);
    assert!(!app.get_placeholder());
    assert_eq!(app.get_placeholder_status(), "");
}

/// A camera picture Cull on the camera's JPEG asked for alone, kept
/// for a frame with an edit, does not stand in for that frame's
/// develop: the picture of its edit is asked for, as on a first
/// visit. One kept for a frame with no edit stands in at once, as
/// any kept camera picture does.
#[test]
fn a_kept_camera_only_picture_stands_in_only_for_a_frame_with_no_edit() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    {
        let mut st = state.borrow_mut();
        let mut edit = Edit::default();
        edit.bw.enabled = true;
        st.sidecars[1].record(edit);
        assert!(crate::edited::held_edited(&st, 1));
        assert!(!crate::edited::held_edited(&st, 2));
    }
    app.invoke_select(0);
    {
        let mut st = state.borrow_mut();
        for f in [1, 2] {
            let cull::Loaded::Ok { preview, .. } = camera_picture(&st, f, (6000, 4000)) else {
                unreachable!()
            };
            let preview = Arc::new(cull::Preview {
                camera_only: true,
                ..Arc::into_inner(preview).unwrap()
            });
            st.culling.hold.as_mut().unwrap().cache.insert(f, preview);
        }
    }
    // No edit: up at once.
    app.invoke_select(2);
    assert_eq!(standing_in(&state.borrow()), Some(2));
    crate::panel::deliver::deliver(&app, developed(generation(&state), (6000, 4000)));
    draw(&app, &state);
    assert_eq!(standing_in(&state.borrow()), None);
    // An edit: its stand-in asked for, the camera's not put up.
    app.invoke_select(1);
    assert_eq!(standing_in(&state.borrow()), None);
    let asked = state.borrow().culling.prefetch.asked();
    assert!(asked.iter().any(|w| w.file == 1 && !w.camera), "{asked:?}");
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    assert_eq!(standing_in(&state.borrow()), Some(1));
}

/// The edit's picture standing in is a 2048 px copy of the frame
/// with the crop in it: drawn as the frame its develop lands as,
/// once the open has said that frame's size, so a view wider than
/// the copy fits it as it will fit the develop (issue 76). Until
/// the open says, it waits and the picture on screen stays. A
/// frame cropped smaller than the view is drawn at 1:1 of the
/// frame, never enlarged; the camera's picture, the frame's own
/// size, fits as it did. Skipped without a GPU for the stand-in's
/// frame to draw through.
#[test]
fn an_edits_stand_in_fits_the_view_as_the_develop_will() {
    use crate::panel::viewport::{drawn_picture, effective_zoom};
    let Some((device, queue)) = greycard_viewport::testing::device("edit_stand_in") else {
        return;
    };
    let app = window(4);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(4));
    // The device goes with the test, even a failing one, not with
    // the thread's last timer.
    struct Unload(Rc<Shared>);
    impl Drop for Unload {
        fn drop(&mut self) {
            if let Ok(mut st) = self.0.try_borrow_mut() {
                st.develop.renderer = None;
            }
        }
    }
    let _unload = Unload(state.clone());
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    {
        let mut st = state.borrow_mut();
        st.develop.renderer = Some(crate::render::Renderer::new(&device, &queue));
        let mut edit = Edit::default();
        edit.bw.enabled = true;
        st.sidecars[1].record(edit.clone());
        // Frame 2 cropped to the middle half each way.
        edit.geometry.crop = Some(greycard_edit::geometry::Crop {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
        });
        st.sidecars[2].record(edit);
    }
    // A frame as the window draws one: the stand-in's draw only
    // while the loupe's picture is what is up.
    let frame = |state: &Rc<Shared>| {
        let mut st = state.borrow_mut();
        if crate::panel::startup::land_pending(&mut st, &app).loupe {
            cull_frame(&mut st, &app, state);
        }
    };
    // The edit's picture of `file`: 8 by 6, its own size.
    let edit_picture = |file: usize| {
        let cull::Loaded::Ok {
            file,
            frame: key,
            size,
            preview,
        } = camera_picture(&state.borrow(), file, (8, 6))
        else {
            unreachable!()
        };
        let preview = Arc::new(cull::Preview {
            edited: true,
            ..Arc::into_inner(preview).unwrap()
        });
        cull::Loaded::Ok {
            file,
            frame: key,
            size,
            preview,
        }
    };
    // The worker's open of a raw of `size`.
    let open = |state: &Rc<Shared>, size: (usize, usize)| {
        let mut outcome = opened(generation(state), crate::worker::SourceKind::Raw);
        if let crate::worker::Outcome::Opened { frame, .. } = &mut outcome {
            *frame = Some(Arc::new(greycard_core::raw::RawFrame {
                width: size.0,
                height: size.1,
                ..crate::panel::viewport::tests::daylight_frame()
            }));
        }
        crate::panel::deliver::deliver(&app, outcome);
    };
    assert!(app.get_view_width() > 8 && app.get_view_height() > 6);
    app.invoke_select(1);
    deliver_preview(&app, edit_picture(1));
    frame(&state);
    assert_eq!(standing_in(&state.borrow()), None);
    // The open says 60 by 40: up, as that frame, fitted below 1:1.
    open(&state, (60, 40));
    frame(&state);
    {
        let st = state.borrow();
        assert_eq!(standing_in(&st), Some(1));
        assert_eq!(st.develop.image_size, (60, 40));
        assert!((effective_zoom(&st, 30, 20) - 0.5).abs() < 1e-6);
    }
    // The develop lands at the size the stand-in had, with pixels,
    // since the renderer takes it up.
    let mut developed = developed(generation(&state), (60, 40));
    if let crate::worker::Outcome::Developed { image, .. } = &mut developed {
        *image = crate::worker::Developed::Halves(Arc::new(crate::worker::Halves {
            width: 60,
            height: 40,
            pixels: vec![Default::default(); 60 * 40 * 4],
        }));
    }
    crate::panel::deliver::deliver(&app, developed);
    frame(&state);
    {
        let st = state.borrow();
        assert_eq!(standing_in(&st), None);
        assert_eq!(drawn_picture(&st).0, (60, 40));
    }
    // Frame 2's edit's picture in hand before it is chosen; its
    // open says 40 by 30, cropped to 20 by 15: smaller than the
    // view, so 1:1 of the frame and not enlarged to fill it.
    deliver_preview(&app, edit_picture(2));
    app.invoke_select(2);
    frame(&state);
    assert_eq!(standing_in(&state.borrow()), None);
    open(&state, (40, 30));
    // The line says the frame's size from the first, not the copy's.
    assert!(
        app.get_placeholder_status()
            .contains("the edit, 20 \u{d7} 15")
    );
    frame(&state);
    {
        let st = state.borrow();
        assert_eq!(standing_in(&st), Some(2));
        assert_eq!(st.develop.image_size, (20, 15));
        assert_eq!(effective_zoom(&st, 30, 20), 1.0);
        // Frame 1's open is still known, for its picture if it
        // were still on screen.
        assert_eq!(opened_size(&st, 1), Some(Some((60, 40))));
    }
    // The camera's picture smaller than the view waits for the
    // open too, and is then drawn as the develop's size, fitted
    // as the develop will be; one of another shape (4:3 of a 3:2
    // frame) keeps its own, grown to meet the develop's height.
    app.invoke_select(3);
    let loaded = camera_picture(&state.borrow(), 3, (8, 6));
    deliver_preview(&app, loaded);
    frame(&state);
    assert_ne!(standing_in(&state.borrow()), Some(3));
    open(&state, (60, 40));
    frame(&state);
    let st = state.borrow();
    assert_eq!(standing_in(&st), Some(3));
    assert_eq!(st.develop.image_size, (53, 40));
    assert!((effective_zoom(&st, 30, 20) - 0.5).abs() < 1e-6);
}

/// A frame whose camera picture is kept, chosen while another
/// frame's still stands in, stands in with its own: not with the
/// other frame's under its own line.
#[test]
fn a_kept_frame_chosen_while_another_stands_in_shows_its_own_picture() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    app.invoke_select(2);
    let loaded = camera_picture(&state.borrow(), 2, (8192, 5464));
    deliver_preview(&app, loaded);
    crate::panel::deliver::deliver(&app, developed(generation(&state), (8192, 5464)));
    draw(&app, &state);

    // Frame 1's camera picture is up and its develop on the way.
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    assert_eq!(standing_in(&state.borrow()), Some(1));
    let first = generation(&state);

    // Back to 2, kept from before: its own picture, and its line.
    app.invoke_select(2);
    assert_eq!(standing_in(&state.borrow()), Some(2));
    assert!(
        app.get_placeholder_status()
            .starts_with("camera preview: the camera JPEG, 8192 \u{d7} 5464")
    );
    // Frame 1's develop, landing late, is dropped by the generation
    // guard, and 2's camera picture stays up.
    crate::panel::deliver::deliver(&app, developed(first, (6000, 4000)));
    draw(&app, &state);
    assert_eq!(standing_in(&state.borrow()), Some(2));
}

/// The picture on screen stays there until something replaces it.
/// A frame chosen whose camera JPEG never comes — its file has
/// none, or its decode came back with nothing — is drawn from the
/// develop still on the GPU and not from the open frame's own
/// size, which a select clears. Read from that, the viewport
/// draws into a one-pixel frame: the canvas over the whole of it,
/// dark until the develop lands.
#[test]
fn a_frame_with_no_camera_picture_keeps_the_last_develop_on_screen() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(&app, developed(generation, (6000, 4000)));
    assert_eq!(
        state.borrow().develop.shown_size,
        (6000, 4000),
        "the develop delivered is the picture the viewport draws"
    );

    // On to a frame whose file has no camera JPEG in it: the
    // decode comes back with nothing and the wait is given up.
    app.invoke_select(2);
    let frame = state.borrow().key_of(2).unwrap();
    deliver_preview(
        &app,
        cull::Loaded::Failed {
            file: 2,
            frame,
            size: 1024,
            message: "no camera preview in the file".into(),
        },
    );
    let st = state.borrow();
    assert!(st.culling.placeholder.is_none(), "nothing stands in for it");
    assert_eq!(
        st.develop.source_size,
        (0, 0),
        "the open frame has no develop of its own yet"
    );
    assert_eq!(
        placeholder::drawn_source(st.develop.source_size, st.develop.shown_size),
        (6000, 4000),
        "the picture on screen is drawn at its own size, not at nothing"
    );
}

/// A second frame chosen before the first has developed shows the
/// second frame's camera picture; the first frame's stays on
/// screen until the second's is decoded, rather than the viewport
/// going blank between them.
#[test]
fn a_second_frame_chosen_first_keeps_the_picture_there_is() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    assert_eq!(standing_in(&state.borrow()), Some(1));

    app.invoke_select(2);
    {
        let st = state.borrow();
        let wait = st
            .culling
            .placeholder
            .as_ref()
            .expect("the second frame waits");
        assert_eq!(wait.file, 2);
        assert!(!wait.up());
        assert_eq!(standing_in(&st), Some(1), "the picture there is stays");
    }
    assert!(app.get_placeholder(), "a camera picture is still on screen");

    // The first frame's develop, landing late for a frame no
    // longer selected, is not this frame's picture.
    assert!(
        !state
            .borrow()
            .culling
            .placeholder
            .as_ref()
            .unwrap()
            .replaced_by(1)
    );

    let loaded = camera_picture(&state.borrow(), 2, (6000, 4000));
    deliver_preview(&app, loaded);
    {
        let st = state.borrow();
        assert!(st.culling.placeholder.as_ref().unwrap().up());
        assert_eq!(standing_in(&st), Some(2));
    }
}

/// The develop wins the race: it lands before the JPEG is
/// decoded, and the JPEG coming back afterwards must not put a
/// camera picture over the developed one.
#[test]
fn a_develop_that_lands_first_keeps_the_late_camera_picture_down() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let generation = state.borrow().develop.generation;
    assert!(develop_landed(&mut state.borrow_mut(), &app, generation));
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    let st = state.borrow();
    assert!(
        st.culling.placeholder.is_none(),
        "the late JPEG re-armed the wait"
    );
    assert_eq!(standing_in(&st), None, "the late JPEG went up over ours");
    assert!(!app.get_placeholder());
    assert!(
        st.culling.hold.as_ref().unwrap().cache.get(1).is_some(),
        "the picture is still kept for the arrow back"
    );
}

/// A develop asked for before the frame on the panel was chosen
/// is a picture of another frame: it leaves the placeholder
/// standing, whether it arrives as a picture or as a failure.
#[test]
fn an_older_develop_leaves_the_placeholder_standing() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let older = state.borrow().develop.generation;
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    app.invoke_select(2);
    let loaded = camera_picture(&state.borrow(), 2, (6000, 4000));
    deliver_preview(&app, loaded);
    assert_eq!(standing_in(&state.borrow()), Some(2));
    // The render loop's question, asked of the older develop.
    assert!(!develop_landed(&mut state.borrow_mut(), &app, older));
    // And the same develop failing rather than landing.
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Failed {
            generation: older,
            message: "late".into(),
        },
    );
    let st = state.borrow();
    assert!(st.culling.placeholder.is_some());
    assert_eq!(standing_in(&st), Some(2));
    assert!(app.get_placeholder());
}

/// A control moved while the camera's picture stands in asks for
/// a develop, which writes "developing..." into the status line.
/// The line the window shows is the placeholder's own until that
/// develop lands, so the word over the picture and the line under
/// it cannot say different things.
#[test]
fn a_slider_over_the_placeholder_keeps_it_up_and_says_so() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    let said = app.get_placeholder_status().to_string();
    assert!(said.starts_with("camera preview: "), "{said}");
    let before = state.borrow().develop.generation;
    app.set_exposure(1.25);
    app.invoke_develop_changed();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
    {
        let st = state.borrow();
        assert!(st.develop.generation > before, "no develop was asked for");
        assert!(
            st.culling.placeholder.is_some(),
            "the placeholder went down early"
        );
        assert!(
            st.culling
                .placeholder
                .as_ref()
                .unwrap()
                .replaced_by(st.develop.generation)
        );
    }
    assert!(app.get_placeholder());
    assert_eq!(app.get_placeholder_status(), said.as_str());
    assert_eq!(app.get_exposure(), 1.25, "the panel lost the slider");
    // That develop lands, and the line goes with the picture.
    let generation = state.borrow().develop.generation;
    assert!(develop_landed(&mut state.borrow_mut(), &app, generation));
    assert!(!app.get_placeholder());
    assert_eq!(app.get_placeholder_status(), "");
}

/// Undo, redo and a snapshot restored over the placeholder: each
/// re-keys a develop and leaves the camera picture standing until
/// it lands.
#[test]
fn undo_and_redo_over_the_placeholder_keep_it_up() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    for e in [1.25f32, 2.0] {
        app.set_exposure(e);
        app.invoke_develop_changed();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
    }
    app.invoke_snapshot_taken();
    app.invoke_undo();
    let undone = app.get_exposure();
    assert_eq!(standing_in(&state.borrow()), Some(1), "undo took it down");
    assert!(app.get_placeholder());
    app.invoke_redo();
    let redone = app.get_exposure();
    assert_eq!(standing_in(&state.borrow()), Some(1), "redo took it down");
    assert!(app.get_placeholder());
    assert_ne!(undone, redone, "undo and redo did not move the panel");
    app.invoke_undo();
    app.invoke_snapshot_restored(0);
    assert_eq!(standing_in(&state.borrow()), Some(1));
    assert!(app.get_placeholder());
}

/// A frame turned while the placeholder is up: the camera picture
/// is drawn the other way up from the same bytes, a develop is
/// re-keyed, and nothing of the last frame's readings comes back
/// behind it.
#[test]
fn a_turn_over_the_placeholder_keeps_it_up_and_turns_the_picture() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    state.borrow_mut().develop.bins = Some((
        greycard_viewport::render::Tap::BeforeGlobalCurves,
        (0..768).map(|i| (i % 97) as u32 + 1).collect(),
    ));
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    let before = state.borrow().develop.generation;
    let turns_before = {
        let st = state.borrow();
        thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, 1, &app, true)
    };
    app.invoke_frame_turned(1);
    let st = state.borrow();
    assert!(
        st.develop.generation > before,
        "the develop was not re-keyed"
    );
    assert_eq!(standing_in(&st), Some(1), "the turn took it down");
    assert_ne!(
        turns_before,
        thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, 1, &app, true),
        "the camera picture would be drawn the same way up"
    );
    assert!(app.get_placeholder());
    assert!(st.develop.bins.is_none(), "the last frame's bins came back");
}

/// A frame turned with its develop on screen is drawn turned on
/// the frame the key was pressed on: the develop there is read
/// through the turn and the size the edit is measured by stands
/// on end, without waiting for the develop at the new turn. A
/// second turn before that lands stacks on the first, a develop
/// asked for at the turn between is dropped when it lands late,
/// and the one at the turn on screen settles it to no difference.
#[test]
fn a_turn_is_drawn_at_once_from_the_develop_on_screen() {
    use crate::panel::viewport::drawn_picture;
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    // What the render does with a develop that has landed.
    let land = |outcome| {
        crate::panel::deliver::deliver(&app, outcome);
        let mut st = state.borrow_mut();
        st.develop.pending.take().expect("the develop was taken");
        st.develop.source_size = st.develop.shown_size;
        crate::panel::viewport::settle_source_size(&mut st);
    };
    let generation = state.borrow().develop.generation;
    land(developed(generation, (6000, 4000)));
    assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 0));

    app.invoke_frame_turned(1);
    let first = state.borrow().develop.generation;
    {
        let st = state.borrow();
        assert_eq!(st.sidecars[1].turn, 1);
        assert_eq!(
            drawn_picture(&st),
            ((4000, 6000), 1),
            "the turn waited for its develop"
        );
        assert_eq!(st.develop.source_size, (4000, 6000));
        // The edit is measured on the source standing on end.
        let frame = st.develop.edit.geometry.frame(4000.0, 6000.0, None);
        assert!(frame.size.1 > frame.size.0, "{:?}", frame.size);
    }

    app.invoke_frame_turned(1);
    assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 2));
    // The first turn's develop, late: stale, and nothing moves.
    land_if_taken(&app, &state, developed_at(first, (4000, 6000), 1));
    assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 2));

    // The second's lands, and the picture on the GPU is turned.
    let generation = state.borrow().develop.generation;
    land(developed_at(generation, (6000, 4000), 2));
    assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 0));

    // Turned under the culling loupe, which draws the camera's
    // picture: once culling is left, the develop on screen is
    // drawn turned by the same rule, measured the way up it is.
    app.invoke_cull_toggled();
    app.invoke_frame_turned(1);
    app.invoke_cull_leave();
    {
        let st = state.borrow();
        assert_eq!(drawn_picture(&st), ((4000, 6000), 1));
        assert_eq!(st.develop.source_size, (4000, 6000));
    }

    // Another frame chosen: the develop on screen is the last
    // frame's, under its own edit, and no turn of this one is
    // read into it.
    app.invoke_select(2);
    app.invoke_frame_turned(1);
    assert_eq!(drawn_picture(&state.borrow()), ((6000, 4000), 0));
}

/// A delivery that may be dropped as stale: taken, if it was not.
fn land_if_taken(app: &App, state: &Rc<Shared>, outcome: crate::worker::Outcome) {
    crate::panel::deliver::deliver(app, outcome);
    let mut st = state.borrow_mut();
    if st.develop.pending.take().is_some() {
        st.develop.source_size = st.develop.shown_size;
        crate::panel::viewport::settle_source_size(&mut st);
    }
}

/// The size the open frame is measured by follows its turn
/// however the turn came: a selection turned from the culling
/// loupe while another frame was under it, and a turn pressed
/// between a develop's delivery and the frame that takes it up.
#[test]
fn the_measured_size_follows_a_turn_however_it_came() {
    use crate::panel::viewport::drawn_picture;
    let app = window(4);
    let (state, worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let generation = state.borrow().develop.generation;
    land_if_taken(&app, &state, developed(generation, (6000, 4000)));
    assert_eq!(state.borrow().develop.source_size, (6000, 4000));

    // Culling, on to frame 2, and both turned together; then
    // back to frame 1 and out, with no camera picture of it.
    app.invoke_cull_toggled();
    app.invoke_select(2);
    crate::panel::browser::turn_frames(&mut state.borrow_mut(), &app, &worker, &[1, 2], 1);
    app.invoke_select(1);
    {
        let st = state.borrow();
        assert_eq!(st.current, Some(1));
        assert_eq!(
            st.develop.source_size,
            (4000, 6000),
            "back on frame 1 under the loupe"
        );
    }
    app.invoke_cull_leave();
    {
        let st = state.borrow();
        assert_eq!(drawn_picture(&st), ((4000, 6000), 1));
        assert_eq!(st.develop.source_size, (4000, 6000));
    }

    // Its develop delivered, and a turn before the frame that
    // takes it up: that frame measures it on end again.
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(&app, developed_at(generation, (4000, 6000), 1));
    app.invoke_frame_turned(1);
    {
        let mut st = state.borrow_mut();
        // What the render does with it.
        st.develop.pending.take().expect("the develop was taken");
        st.develop.source_size = (4000, 6000);
        crate::panel::viewport::settle_source_size(&mut st);
        assert_eq!(st.develop.source_size, (6000, 4000));
        assert_eq!(drawn_picture(&st), ((6000, 4000), 1));
    }
}

/// The culling mode entered over a placeholder takes it, and
/// leaving the mode hands one back — but only where the loupe had
/// a picture of the frame to hold. A mode left before its first
/// decode has nothing standing in, and the developed picture on
/// screen keeps the panel.
#[test]
fn culling_entered_over_a_placeholder_takes_it_and_gives_it_back() {
    for decoded in [false, true] {
        let app = window(4);
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(1);
        let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
        deliver_preview(&app, loaded);
        assert!(app.get_placeholder());
        app.invoke_cull_toggled();
        {
            let st = state.borrow();
            assert!(
                st.culling.placeholder.is_none(),
                "a wait survived into the mode"
            );
            assert!(
                st.culling.hold.is_none(),
                "the develop view's pictures survived"
            );
            assert!(st.culling.cull.is_some(), "the mode did not start");
        }
        assert!(!app.get_placeholder(), "the mode draws with the gate on");
        if decoded {
            let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
            deliver_preview(&app, loaded);
        }
        app.invoke_cull_leave();
        let st = state.borrow();
        assert!(st.culling.cull.is_none());
        assert_eq!(st.culling.placeholder.is_some(), decoded);
        assert_eq!(app.get_placeholder(), decoded);
        assert_eq!(
            app.get_placeholder_status().starts_with("camera preview: "),
            decoded,
            "the loupe's picture said {:?}",
            app.get_placeholder_status()
        );
    }
}

/// A frame with no camera JPEG in it is remembered as having
/// none: choosing it again costs no second decode, and the
/// picture on screen stays where it is either time.
#[test]
fn a_frame_with_no_camera_jpeg_is_not_asked_for_again() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let failed = {
        let st = state.borrow();
        cull::Loaded::Failed {
            file: 1,
            frame: st.key_of(1).unwrap(),
            size: 1024,
            message: "no camera preview in the file".into(),
        }
    };
    deliver_preview(&app, failed);
    app.invoke_select(2);
    app.invoke_select(1);
    let st = state.borrow();
    assert!(
        st.culling.hold.as_ref().unwrap().failed.contains_key(&1),
        "the frame is asked for again on every select"
    );
    assert!(
        st.culling.placeholder.is_none(),
        "it waits for a decode nobody sent"
    );
    assert!(!app.get_placeholder());
}

/// A picture file chosen while a placeholder is up takes it down
/// — there is no camera JPEG in a JPEG — and the pictures kept
/// are still trimmed to the window about it.
#[test]
fn a_picture_file_chosen_takes_the_placeholder_down_and_still_prunes() {
    let app = window(5);
    let mut files = folder(5);
    files[4] = std::path::PathBuf::from("/nowhere/scan.jpg");
    let (state, _worker) = state_for(&app, files);
    app.invoke_select(0);
    let loaded = camera_picture(&state.borrow(), 0, (6000, 4000));
    deliver_preview(&app, loaded);
    assert!(app.get_placeholder());
    app.invoke_select(4);
    let st = state.borrow();
    assert!(
        st.culling.placeholder.is_none(),
        "a picture file got a placeholder"
    );
    assert!(!app.get_placeholder());
    assert!(
        st.culling.hold.as_ref().unwrap().cache.get(0).is_none(),
        "the window was not trimmed on the way past"
    );
}

/// The histogram behind the curve editor goes with the scopes
/// while the camera picture stands in, and a touch of the curve
/// does not paint the last frame's back: the bins go with it.
#[test]
fn the_curve_editor_does_not_paint_the_last_frames_histogram_back() {
    fn bytes(img: &slint::Image) -> Vec<u8> {
        img.to_rgba8()
            .map(|b| b.as_bytes().to_vec())
            .unwrap_or_default()
    }
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    let last = (0..768).map(|i| (i % 97) as u32 + 1).collect::<Vec<u32>>();
    state.borrow_mut().develop.bins = Some((
        greycard_viewport::render::Tap::BeforeGlobalCurves,
        last.clone(),
    ));
    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    assert!(app.get_placeholder());
    let blank = bytes(&draw_curve(&app, None));
    assert_eq!(
        bytes(&app.get_curve_image()),
        blank,
        "the histogram was not taken off the curve"
    );
    // A point put on the curve, as the panel's own touch area
    // does: the curve is redrawn from whatever bins are in hand.
    app.invoke_curve_press(0.5, 0.5);
    assert_ne!(
        bytes(&app.get_curve_image()),
        bytes(&draw_curve(&app, Some(&last))),
        "the last frame's histogram came back behind the camera picture"
    );
}

/// A frame with no camera JPEG in it — a DNG written without one —
/// falls back to what the editor did before there were
/// placeholders: the picture on screen stays where it is until the
/// develop lands, and nothing says otherwise.
#[test]
fn a_frame_with_no_camera_jpeg_keeps_the_picture_on_screen() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(1);
    let failed = {
        let st = state.borrow();
        cull::Loaded::Failed {
            file: 1,
            frame: st.key_of(1).unwrap(),
            size: 1024,
            message: "no camera preview in the file".into(),
        }
    };
    deliver_preview(&app, failed);
    let st = state.borrow();
    assert!(
        st.culling.placeholder.is_none(),
        "nothing is waited on any more"
    );
    assert_eq!(standing_in(&st), None);
    assert!(!app.get_placeholder());
    // Asked for once: the frame is not re-decoded every frame.
    assert!(st.culling.hold.as_ref().unwrap().failed.contains_key(&1));
}

/// Nothing that belongs to the developed picture is drawn over a
/// camera picture standing in for it: a mask handle is neither
/// drawn nor pressable while one is up, and both come back when
/// the develop does.
#[test]
fn a_mask_handle_is_not_pressable_over_a_camera_picture() {
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.set_mask_kind("Linear".into());
    let grabbed = Rc::new(std::cell::Cell::new(false));
    {
        let grabbed = grabbed.clone();
        app.on_mask_grabbed(move |_, _| grabbed.set(true));
    }
    // A handle at these view pixels sits at 240 + x of the window,
    // the left bar being 240 wide.
    let press_handle = || {
        app.set_mask_handles(ModelRc::new(VecModel::from(vec![Pt {
            x: 360.0,
            y: 400.0,
        }])));
        grabbed.set(false);
        click(&app, 240.0 + 360.0, 400.0);
        grabbed.get()
    };
    assert!(press_handle(), "a handle on the developed picture");

    app.invoke_select(1);
    let loaded = camera_picture(&state.borrow(), 1, (6000, 4000));
    deliver_preview(&app, loaded);
    assert!(app.get_placeholder());
    assert!(
        !press_handle(),
        "a handle took a click over the camera's picture"
    );

    drop_placeholder(&mut state.borrow_mut(), &app);
    assert!(press_handle(), "the handle did not come back");
}

/// The frame on screen taken out of the list (moved with the
/// rejects, or deleted): the nearest frame left is chosen and
/// opens, in culling as in the loupe. The first cut left the old
/// picture up with no row chosen, since the row to select was
/// asked of `rebuild_browser`, which has no current frame then.
#[test]
fn the_frame_on_screen_dropped_from_the_list_hands_on_to_the_next() {
    let app = window(5);
    let (state, worker) = state_for(&app, folder(5));
    app.invoke_select(2);
    app.invoke_cull_toggled();
    assert!(state.borrow().culling.cull.is_some());
    assert_eq!(state.borrow().current, Some(2));
    drop_files(&mut state.borrow_mut(), &app, &worker, &[2], "gone".into());
    assert_eq!(state.borrow().files.len(), 4);
    // The select is a timer away, as a click's would be.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    slint::platform::update_timers_and_animations();
    assert_eq!(
        state.borrow().current,
        Some(2),
        "the frame after took its place"
    );
    assert_eq!(app.get_selected(), 2);
    assert_eq!(app.get_file_name(), "IMG_0003.CR3");
    assert!(state.borrow().culling.cull.is_some(), "culling was left");

    // The last frame: the one before takes over.
    app.invoke_select(3);
    drop_files(&mut state.borrow_mut(), &app, &worker, &[3], "gone".into());
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    slint::platform::update_timers_and_animations();
    assert_eq!(state.borrow().current, Some(2));
    assert_eq!(app.get_file_name(), "IMG_0003.CR3");
}

/// Run what a turn of the event loop would: the change handlers
/// and the zero-length timers, the select after a drop among them.
fn settle() {
    for _ in 0..4 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
        slint::platform::update_timers_and_animations();
    }
}

/// Row `row` whole in the strip at its scroll, the pad beside it.
fn whole_in_strip(app: &App, row: i32) -> bool {
    let layout = crate::cells::strip_layout(app);
    let scroll = app.get_strip_scroll();
    layout.reveal(scroll, app.get_strip_view(), row) == scroll
}

/// Forty frames in a strip 1200 wide, frame `selected` chosen.
fn long_strip(selected: i32) -> (App, Rc<Shared>, Rc<Worker>) {
    let app = window(40);
    app.window()
        .set_size(slint::LogicalSize::new(1200.0, 800.0));
    let (state, worker) = state_for(&app, folder(40));
    settle();
    app.invoke_select(selected);
    settle();
    (app, state, worker)
}

/// The selected frame deleted mid-strip: the strip stays where it
/// was, the frames before it where they were, and the frame that
/// takes its place is chosen where it stood (issues 83 and 84).
#[test]
fn a_frame_deleted_mid_strip_leaves_the_strip_where_it_was() {
    let (app, state, worker) = long_strip(20);
    let layout = crate::cells::strip_layout(&app);
    let (left, width) = layout.place(20).unwrap();
    let mid = left + width / 2.0 - app.get_strip_view() / 2.0;
    app.invoke_strip_hold(mid);
    settle();
    let scroll = app.get_strip_scroll();
    assert!(scroll > 0.0 && whole_in_strip(&app, 20));
    drop_files(&mut state.borrow_mut(), &app, &worker, &[20], "gone".into());
    settle();
    assert_eq!(app.get_selected(), 20, "the frame after took its place");
    assert_eq!(app.get_strip_scroll(), scroll, "the strip jumped");
    assert!(whole_in_strip(&app, 20));
    // A new list after it is laid out afresh, from the start.
    crate::cells::set_rows(&app, vec![Default::default(); 30]);
    settle();
    let fresh = crate::cells::strip_layout(&app).reveal(0.0, app.get_strip_view(), 20);
    assert_eq!(app.get_strip_scroll(), fresh);
}

/// The selected frame deleted while the wheel had taken it off
/// screen, or cut it at the edge: the strip moves the least that
/// shows the frame chosen in its place, and no further.
#[test]
fn a_frame_deleted_off_screen_brings_the_next_on_by_the_least() {
    for away in [false, true] {
        let (app, state, worker) = long_strip(20);
        let layout = crate::cells::strip_layout(&app);
        let (left, _) = layout.place(20).unwrap();
        // Cut at the left edge by ten pixels, or a screen past it.
        let to = if away { left + 1500.0 } else { left + 10.0 };
        app.invoke_strip_hold(to);
        settle();
        assert!(!whole_in_strip(&app, 20));
        drop_files(&mut state.borrow_mut(), &app, &worker, &[20], "gone".into());
        settle();
        assert_eq!(app.get_selected(), 20);
        let (left, _) = crate::cells::strip_layout(&app).place(20).unwrap();
        assert_eq!(
            app.get_strip_scroll(),
            left - crate::strip::PAD,
            "the least scroll (away: {away})"
        );
    }
}

/// The grid holds still the same way: the selected frame deleted
/// mid-sheet leaves the sheet's scroll where it was.
#[test]
fn a_frame_deleted_mid_sheet_leaves_the_grid_where_it_was() {
    let (app, state, worker) = long_strip(0);
    app.set_grid_open(true);
    settle();
    app.invoke_select(20);
    settle();
    // Down a little: the selected frame still whole.
    app.invoke_scroll_by(30.0);
    settle();
    let scroll = app.get_grid_scroll();
    assert!(scroll > 0.0);
    drop_files(&mut state.borrow_mut(), &app, &worker, &[20], "gone".into());
    settle();
    assert_eq!(app.get_selected(), 20);
    assert_eq!(app.get_grid_scroll(), scroll, "the sheet jumped");
}

/// The last frames deleted from a sheet scrolled to its end: a row
/// goes, the sheet comes back up to its new end and no further, and
/// the frame before them is chosen, whole. A new list after that is
/// laid out afresh, from the top.
#[test]
fn the_last_frames_deleted_pull_the_sheet_back_to_its_new_end() {
    let (app, state, worker) = long_strip(0);
    app.set_grid_open(true);
    settle();
    app.invoke_select(39);
    settle();
    let end = app.get_grid_scroll();
    assert!(end > 0.0);
    // Twelve frames: at least a row of the sheet, whatever its
    // columns.
    let gone: Vec<usize> = (28..40).collect();
    drop_files(&mut state.borrow_mut(), &app, &worker, &gone, "gone".into());
    settle();
    assert_eq!(app.get_selected(), 27, "the frame before them");
    let held = app.get_grid_scroll();
    assert!(0.0 < held && held < end, "{held} against {end}");
    app.invoke_scroll_by(10_000.0);
    assert_eq!(app.get_grid_scroll(), held, "held at the new end");
    // Nothing chosen and the sheet part way down: a new list puts
    // it at the top.
    app.set_selected(-1);
    app.invoke_scroll_by(-30.0);
    settle();
    assert!(app.get_grid_scroll() > 0.0);
    crate::cells::set_rows(&app, vec![Default::default(); 40]);
    settle();
    assert_eq!(app.get_grid_scroll(), 0.0);
}

#[test]
fn c_enters_culling_return_and_escape_leave_it_and_v_cycles_the_compare() {
    let app = window(11);
    let toggles = Rc::new(RefCell::new(0));
    let counted = toggles.clone();
    app.on_cull_toggled(move || *counted.borrow_mut() += 1);
    let leaves = Rc::new(RefCell::new(0));
    let counted = leaves.clone();
    app.on_cull_leave(move || *counted.borrow_mut() += 1);
    let compares = Rc::new(RefCell::new(Vec::new()));
    let seen = compares.clone();
    app.on_compare_changed(move |n| seen.borrow_mut().push(n));
    let rated = Rc::new(RefCell::new(Vec::new()));
    let seen = rated.clone();
    app.on_meta_key(move |key| {
        let known = meta::Change::from_key(&key).is_some();
        if known {
            seen.borrow_mut().push(key.to_string());
        }
        known
    });
    let answers = Rc::new(RefCell::new(Vec::new()));
    let seen = answers.clone();
    let weak = app.as_weak();
    app.on_rejects_answered(move |yes| {
        seen.borrow_mut().push(yes);
        weak.unwrap().set_rejects_open(false);
    });
    // C is the way in; out of the mode Return and V are nobody's.
    press(&app, "c");
    assert_eq!(*toggles.borrow(), 1);
    press(&app, Key::Return);
    press(&app, "v");
    assert_eq!(*leaves.borrow(), 0);
    assert!(compares.borrow().is_empty());
    // In it: V cycles two, four, one; the culling keys still
    // reach the browser; Return or Escape leaves.
    app.set_culling(true);
    press(&app, "v");
    app.set_compare(2);
    press(&app, "V");
    app.set_compare(4);
    press(&app, "v");
    assert_eq!(*compares.borrow(), vec![2, 4, 1]);
    press(&app, "3");
    press(&app, "x");
    assert_eq!(*rated.borrow(), ["3", "x"]);
    press(&app, Key::Return);
    assert_eq!(*leaves.borrow(), 1);
    press(&app, Key::Escape);
    assert_eq!(*leaves.borrow(), 2);
    // The grid over the mode: Return closes the grid, and the
    // mode stays.
    press(&app, "g");
    assert!(app.get_grid_open());
    press(&app, Key::Return);
    assert!(!app.get_grid_open());
    assert_eq!(*leaves.borrow(), 2);
    // The rejects sheet takes every key it is shown; Return is
    // its Move, once, and Escape its no.
    app.set_rejects_open(true);
    a_moment_on(&app);
    press(&app, "c");
    press(&app, "4");
    assert_eq!(*toggles.borrow(), 1);
    assert_eq!(*leaves.borrow(), 2);
    assert_eq!(rated.borrow().len(), 2);
    press(&app, Key::Return);
    assert_eq!(*answers.borrow(), vec![true]);
    assert!(!app.get_rejects_open());
    assert_eq!(*leaves.borrow(), 2);
    app.set_rejects_open(true);
    a_moment_on(&app);
    press(&app, Key::Escape);
    assert_eq!(*answers.borrow(), vec![true, false]);
    // Ctrl+C is not the mode's.
    app.set_rejects_open(false);
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    press(&app, "c");
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(*toggles.borrow(), 1);
}

/// Everything the filter does to the browser, driven through the
/// window's own callbacks rather than the functions behind them:
/// the rows it leaves, the counts it puts on the chips, and what
/// happens to the selection when a star takes the frame on screen
/// out of the list.
///
/// A row and a file are two different numbers under a filter, and
/// confusing them is a bug this crate has shipped once already,
/// so the rows are checked by name and not by count.
#[test]
fn a_filter_leaves_the_rows_it_says_it_leaves() {
    let app = window(0);
    let (state, _worker) = state_for(&app, folder(6));
    {
        let mut st = state.borrow_mut();
        // Two five-star frames, two threes, two nothing.
        for (i, stars) in [0u8, 5, 3, 0, 5, 3].into_iter().enumerate() {
            st.sidecars[i].meta.set_rating(stars);
        }
        st.sidecars[1].meta.flag = meta::Flag::Pick;
        st.sidecars[4].meta.flag = meta::Flag::Reject;
        st.sidecars[2].meta.label = meta::Label::Red;
        st.sidecars[2].meta.keywords = vec!["harbor".into()];
        st.views.shown = (0..6).collect();
        st.current = Some(2);
    }
    rebuild_browser(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_filter_total(), 6);
    assert_eq!(app.get_filter_shown(), 6);
    assert!(!app.get_filter_on(), "nothing asked of a frame yet");

    // Three stars or more: the two fives and the two threes, and
    // the chips say so before they are pressed.
    let stars = app.get_filter_stars();
    assert_eq!(stars.row_data(3).unwrap().text, "3+");
    assert_eq!(stars.row_data(3).unwrap().count, 4);
    assert_eq!(stars.row_data(5).unwrap().count, 2);
    app.invoke_filter_star_picked(3);
    assert_eq!(state.borrow().views.shown, vec![1, 2, 4, 5]);
    assert_eq!(app.get_filter_shown(), 4);
    assert!(app.get_filter_on());
    assert_eq!(app.get_thumbs().row_count(), 4);
    assert_eq!(app.get_thumbs().row_data(0).unwrap().name, "IMG_0001.CR3");

    // The flag chips read how the frames the stars left are
    // flagged — their own group set aside, the other groups as
    // they stand: of those four, one is a pick and one a reject.
    let flags = app.get_filter_flags();
    assert_eq!(flags.row_data(1).unwrap().text, "Pick");
    assert_eq!(flags.row_data(1).unwrap().count, 1);
    assert_eq!(flags.row_data(2).unwrap().count, 1);
    app.invoke_filter_flag_toggled(1, false);
    assert_eq!(state.borrow().views.shown, vec![1]);
    // Any-of: the rejects as well, and then neither again.
    app.invoke_filter_flag_toggled(2, false);
    assert_eq!(state.borrow().views.shown, vec![1, 4]);
    app.invoke_filter_flag_toggled(1, false);
    app.invoke_filter_flag_toggled(2, false);
    assert_eq!(state.borrow().views.shown, vec![1, 2, 4, 5]);
    // Ctrl+click holds the rejects out, and the chip says so; a
    // plain click lets them back.
    app.invoke_filter_flag_toggled(2, true);
    assert_eq!(state.borrow().views.shown, vec![1, 2, 5]);
    let reject = app.get_filter_flags().row_data(2).unwrap();
    assert!(reject.out && !reject.on);
    assert_eq!(reject.count, 1, "the row still reads the shoot");
    app.invoke_filter_flag_toggled(2, false);
    assert_eq!(state.borrow().views.shown, vec![1, 2, 4, 5]);
    assert!(!app.get_filter_flags().row_data(2).unwrap().out);

    // "Exactly" reads the same chip the other way.
    app.invoke_filter_exact_toggled();
    assert!(app.get_filter_exact());
    assert_eq!(state.borrow().views.shown, vec![2, 5]);
    app.invoke_filter_exact_toggled();

    // A word: a keyword one frame carries, then part of a name,
    // and both of them narrow what the stars already left.
    app.invoke_filter_text_edited("harbor".into());
    assert_eq!(state.borrow().views.shown, vec![2]);
    app.invoke_filter_text_edited("0004".into());
    assert_eq!(state.borrow().views.shown, vec![4]);

    // Clear puts the folder back, the field with it.
    app.invoke_filter_cleared();
    assert_eq!(state.borrow().views.shown, vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(app.get_filter_text(), "");
    assert!(!app.get_filter_on());
}

/// A star that takes the frame on screen out of the filtered list
/// does what a reject under "No rejects" has always done: the
/// selection goes to the nearest frame still shown and that one
/// opens. It never goes nowhere, and it never stays on a row that
/// is no longer there.
#[test]
fn a_rating_that_hides_the_shown_frame_hands_the_selection_on() {
    let app = window(0);
    let (state, _worker) = state_for(&app, folder(5));
    {
        let mut st = state.borrow_mut();
        for (i, stars) in [4u8, 4, 4, 0, 4].into_iter().enumerate() {
            st.sidecars[i].meta.set_rating(stars);
        }
        st.views.shown = (0..5).collect();
        st.current = Some(2);
        app.set_selected(2);
    }
    app.invoke_filter_star_picked(4);
    assert_eq!(state.borrow().views.shown, vec![0, 1, 2, 4]);
    assert_eq!(app.get_selected(), 2, "the frame kept its own row");

    // One star on it: it leaves the list, and file 4 — the next
    // one still shown, at row 2 now — is what opens.
    app.invoke_meta_key("1".into());
    assert_eq!(state.borrow().views.shown, vec![0, 1, 4]);
    assert_eq!(state.borrow().current, Some(4));
    assert_eq!(app.get_selected(), 2);
    assert_eq!(app.get_thumbs().row_data(2).unwrap().name, "IMG_0004.CR3");
    assert_eq!(app.get_file_name(), "IMG_0004.CR3");

    // And a star that does not hide anything leaves the selection
    // where it is, and still moves the counts.
    app.invoke_meta_key("5".into());
    assert_eq!(state.borrow().current, Some(4));
    assert_eq!(state.borrow().views.shown, vec![0, 1, 4]);
    assert_eq!(app.get_filter_stars().row_data(5).unwrap().count, 1);

    // The last frame the filter shows, rated out of it: there is
    // nothing after it, so the one before takes the selection.
    app.invoke_select(2);
    app.invoke_meta_key("0".into());
    assert_eq!(state.borrow().views.shown, vec![0, 1]);
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(app.get_selected(), 1);
}

/// A key that takes the frame out of the filtered list says so on
/// the status line, with the filter in its words and the way back,
/// and the selection goes on to the next frame shown. Without the
/// word a frame rated 0 under a one-star filter was simply gone.
/// The same key with no filter on says nothing of the kind.
#[test]
fn a_rating_under_a_hiding_filter_says_where_the_frame_went() {
    let app = window(0);
    let (state, _worker) = state_for(&app, folder(3));
    {
        let mut st = state.borrow_mut();
        for s in &mut st.sidecars[..3] {
            s.meta.set_rating(2);
        }
        st.views.shown = (0..3).collect();
        st.current = Some(1);
        app.set_selected(1);
    }
    app.invoke_filter_star_picked(1);
    assert_eq!(state.borrow().views.shown, vec![0, 1, 2]);

    app.invoke_meta_key("0".into());
    assert_eq!(state.borrow().views.shown, vec![0, 2]);
    assert_eq!(state.borrow().current, Some(2), "on to the next frame");
    assert_eq!(app.get_selected(), 1);
    assert_eq!(
        app.get_status(),
        "IMG_0001.CR3 rated 0 and hidden by the filter (1 star or more); \
         Clear the filter to see it"
    );

    // A flag key that hides the frame says it the same way.
    app.invoke_meta_key("p".into());
    app.invoke_filter_flag_toggled(1, false);
    assert_eq!(state.borrow().views.shown, vec![2]);
    app.invoke_meta_key("u".into());
    assert!(state.borrow().views.shown.is_empty());
    assert!(
        app.get_status()
            .as_str()
            .starts_with("IMG_0002.CR3 unflagged and hidden by the filter"),
        "{}",
        app.get_status()
    );

    // No filter: the same rating key hides nothing and says
    // nothing of the filter.
    app.invoke_filter_cleared();
    app.invoke_select(0);
    app.set_status("".into());
    app.invoke_meta_key("0".into());
    assert_eq!(state.borrow().sidecars[0].meta.rating, 0);
    assert_eq!(state.borrow().current, Some(0));
    assert!(
        !app.get_status().as_str().contains("filter"),
        "{}",
        app.get_status()
    );
}

/// Ctrl+Z in culling steps back the ratings, flags and labels the
/// session gave, newest first, going to each frame it changes;
/// Ctrl+Shift+Z makes them again. No develop moves, and culling
/// stays up.
#[test]
fn undo_in_culling_takes_back_the_tags_and_nothing_else() {
    use greycard_edit::meta::Flag;
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(0);
    {
        let mut st = state.borrow_mut();
        // A develop step on the frame, for undo to leave alone.
        let mut e = st.sidecars[0].current.clone();
        e.light.exposure = 1.0;
        st.sidecars[0].record(e);
        enter_cull(&mut st, &app, 1);
    }
    let develop = |st: &State| (st.sidecars[0].states(), st.sidecars[0].position());
    let before = develop(&state.borrow());
    let tags = |st: &State, i: usize| (st.sidecars[i].meta.rating, st.sidecars[i].meta.flag);

    app.invoke_meta_key("3".into());
    app.invoke_select(1);
    app.invoke_meta_key("p".into());
    assert_eq!(tags(&state.borrow(), 0), (3, Flag::None));
    assert_eq!(tags(&state.borrow(), 1), (0, Flag::Pick));

    // The pick first, on the frame on screen.
    app.invoke_undo();
    assert_eq!(tags(&state.borrow(), 1), (0, Flag::None));
    assert_eq!(state.borrow().current, Some(1));
    // Then the stars, and over to the frame that had them.
    app.invoke_undo();
    assert_eq!(tags(&state.borrow(), 0), (0, Flag::None));
    assert_eq!(state.borrow().current, Some(0));
    assert_eq!(
        app.get_thumbs().row_data(0).unwrap().rating,
        0,
        "the badge too"
    );
    // Nothing left: nothing moves.
    app.invoke_undo();
    assert_eq!(state.borrow().current, Some(0));

    app.invoke_redo();
    assert_eq!(tags(&state.borrow(), 0), (3, Flag::None));
    assert!(state.borrow().culling.cull.is_some(), "still culling");
    assert_eq!(
        develop(&state.borrow()),
        before,
        "the develop was not touched"
    );

    // A new rating drops what was left to redo.
    app.invoke_meta_key("5".into());
    app.invoke_redo();
    assert_eq!(tags(&state.borrow(), 1), (0, Flag::None));
}

/// The CULLING section's switch: off, a key leaves the selection
/// where it is; on, a rating, a flag or a label moves it on to
/// the next frame by the arrow's path and stops at the end, and
/// each key's tags land on the frame it was pressed over. Undo
/// still puts that frame back and goes to it, and the word over
/// the picture and the loupe's badge say what the frame carries.
#[test]
fn move_on_advances_after_a_key_stops_at_the_end_and_undo_goes_back() {
    use greycard_edit::meta::{Flag, Label};
    let app = window(4);
    let (state, _worker) = state_for(&app, folder(4));
    app.invoke_select(0);
    enter_cull(&mut state.borrow_mut(), &app, 1);
    assert!(!app.get_cull_move_on(), "off by default");
    assert!(!state.borrow().culling.cull_move_on);

    // Off: the key and the arrow are two presses.
    app.invoke_meta_key("2".into());
    assert_eq!(state.borrow().current, Some(0));
    assert_eq!(app.get_notice(), "2 stars");
    assert!(app.get_notice_on());
    assert_eq!(app.get_frame_rating(), 2, "the loupe's badge");

    app.set_cull_move_on(true);
    app.invoke_cull_move_on_changed();
    assert!(state.borrow().culling.cull_move_on);

    app.invoke_meta_key("3".into());
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(app.get_selected(), 1);
    assert_eq!(app.get_frame_rating(), 0, "the next frame's badge");
    assert_eq!(app.get_notice(), "3 stars");
    assert!(app.get_notice_on(), "moving on does not take the word down");
    app.invoke_meta_key("p".into());
    assert_eq!(state.borrow().current, Some(2));
    app.invoke_meta_key("6".into());
    assert_eq!(state.borrow().current, Some(3));
    assert_eq!(app.get_notice(), "Red");
    // At the end: nothing past it, and the key still lands.
    app.invoke_meta_key("x".into());
    assert_eq!(state.borrow().current, Some(3));
    assert_eq!(app.get_selected(), 3);
    assert_eq!(app.get_notice(), "Rejected");

    // Each on the frame it was pressed over, and on no other.
    let tags = |st: &State, i: usize| {
        let m = &st.sidecars[i].meta;
        (m.rating, m.flag, m.label)
    };
    {
        let st = state.borrow();
        assert_eq!(tags(&st, 0), (3, Flag::None, Label::None));
        assert_eq!(tags(&st, 1), (0, Flag::Pick, Label::None));
        assert_eq!(tags(&st, 2), (0, Flag::None, Label::Red));
        assert_eq!(tags(&st, 3), (0, Flag::Reject, Label::None));
    }
    assert_eq!(app.get_frame_flag(), Flag::Reject.code());

    // Undo: the reject off frame 3, which is on screen; then the
    // label off frame 2, and over to it.
    app.invoke_undo();
    assert_eq!(tags(&state.borrow(), 3), (0, Flag::None, Label::None));
    assert_eq!(state.borrow().current, Some(3));
    assert_eq!(app.get_frame_flag(), 0);
    assert_eq!(app.get_notice(), "Undo: Unflagged", "the step's word");
    app.invoke_undo();
    assert_eq!(tags(&state.borrow(), 2), (0, Flag::None, Label::None));
    assert_eq!(state.borrow().current, Some(2));
    assert_eq!(app.get_frame_label(), 0);
    assert_eq!(app.get_notice(), "Undo: No label");
    // Redo makes it again and stays: a redo is not a key.
    app.invoke_redo();
    assert_eq!(tags(&state.borrow(), 2), (0, Flag::None, Label::Red));
    assert_eq!(state.borrow().current, Some(2));
    assert_eq!(app.get_frame_label(), Label::Red.code());
    assert_eq!(app.get_notice(), "Redo: Red");
    // A key after the undo rates this frame and moves on again;
    // the label key on the red frame clears it, and says so.
    app.invoke_meta_key("4".into());
    assert_eq!(tags(&state.borrow(), 2), (4, Flag::None, Label::Red));
    assert_eq!(state.borrow().current, Some(3));
    app.invoke_select(2);
    app.invoke_meta_key("6".into());
    assert_eq!(tags(&state.borrow(), 2), (4, Flag::None, Label::None));
    assert_eq!(app.get_notice(), "No label");
}

/// The camera and the exposure under the file's name follow the
/// focus in culling, one compared frame to the next, from the
/// index; a frame the index lacks reads empty.
#[test]
fn the_shot_lines_follow_the_focus_in_culling() {
    use greycard_library::fixture::{A7, R5, R6, write_frame};
    let dir = crate::testing::scratch_dir("cull-shot");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dunce::canonicalize(&dir).unwrap();
    let files: Vec<PathBuf> = ["a.tif", "b.tif", "c.tif", "d.tif"]
        .iter()
        .map(|n| dir.join(n))
        .collect();
    write_frame(&files[0], &R5, 1);
    write_frame(&files[1], &A7, 2);
    write_frame(&files[2], &R6, 3);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_folder(&dir, &mut |_| {}).unwrap();
    drop(writer);
    // Written after the pass: no row for it yet.
    write_frame(&files[3], &R6, 4);
    let app = window(files.len());
    let (state, _worker) = state_for(&app, files);
    state.borrow_mut().views.index_reader =
        Some(greycard_library::Library::open_read_only(&db).unwrap());
    app.invoke_select(0);
    crate::library::refresh_ids(&mut state.borrow_mut());
    enter_cull(&mut state.borrow_mut(), &app, 2);
    assert_eq!(
        app.get_shot_camera(),
        "Canon EOS R5 \u{b7} RF35mm F1.4 L VCM"
    );
    assert_eq!(
        app.get_shot_exposure(),
        "35 mm \u{b7} f/2 \u{b7} 1/2500 s \u{b7} ISO 100"
    );
    let (a, b) = (app.get_shot_camera(), app.get_shot_exposure());
    cull_select(&mut state.borrow_mut(), &app, 1);
    assert_ne!(app.get_shot_camera(), a, "the camera followed the focus");
    assert_ne!(app.get_shot_exposure(), b, "and the exposure");
    assert!(app.get_shot_camera().contains("ILCE-7M4"));
    assert!(app.get_shot_exposure().contains("ISO 3200"));
    cull_select(&mut state.borrow_mut(), &app, 2);
    assert!(app.get_shot_exposure().contains("ISO 6400"));
    cull_select(&mut state.borrow_mut(), &app, 3);
    assert_eq!(app.get_shot_camera(), "", "not indexed yet");
    assert_eq!(app.get_shot_exposure(), "");
    // The pass reaches it while the loupe is on it: the lines fill.
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_folder(&dir, &mut |_| {}).unwrap();
    drop(writer);
    crate::library::refresh_ids(&mut state.borrow_mut());
    show_shot(&state.borrow(), &app);
    assert!(app.get_shot_exposure().contains("ISO 6400"));
    // The window's callbacks hold the state, so the reader must
    // be closed here or Windows keeps the folder locked.
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A culling key held down is one press. The window's repeats of
/// it are dropped before they reach the browser, so with move-on
/// on a held 3 rates one frame and steps once, not a run of them.
#[test]
fn a_held_culling_key_is_one_press_and_one_step() {
    let app = window(6);
    let (state, _worker) = state_for(&app, folder(6));
    app.invoke_select(0);
    enter_cull(&mut state.borrow_mut(), &app, 1);
    app.set_cull_move_on(true);
    app.invoke_cull_move_on_changed();
    let text: slint::SharedString = "3".into();
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    for _ in 0..3 {
        app.window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
    }
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text });
    let st = state.borrow();
    assert_eq!(st.current, Some(1), "one step");
    let ratings: Vec<u8> = st.sidecars.iter().map(|s| s.meta.rating).collect();
    assert_eq!(ratings, vec![3, 0, 0, 0, 0, 0], "one frame rated");
    assert_eq!(app.get_notice(), "3 stars");
}

/// Move-on stays out of two cases: a set of several frames, whose
/// arrow would collapse it, and a frame the key took out of the
/// filtered list, whose nearest is already the next.
#[test]
fn move_on_leaves_a_set_alone_and_does_not_skip_past_the_filters_nearest() {
    let app = window(5);
    let (state, _worker) = state_for(&app, folder(5));
    app.invoke_select(1);
    enter_cull(&mut state.borrow_mut(), &app, 1);
    app.set_cull_move_on(true);
    app.invoke_cull_move_on_changed();

    // Frames 1 and 2 as a set: rated together, and left there.
    app.invoke_frame_clicked(2, false, true);
    assert_eq!(
        crate::panel::browser::chosen_frames(&state.borrow()),
        vec![1, 2]
    );
    app.invoke_meta_key("4".into());
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(state.borrow().sidecars[1].meta.rating, 4);
    assert_eq!(state.borrow().sidecars[2].meta.rating, 4);
    assert_eq!(state.borrow().sidecars[3].meta.rating, 0);

    // Under "No rejects", a reject leaves the list: the nearest
    // frame shown takes the selection, once, not the one after.
    {
        let mut st = state.borrow_mut();
        st.views.filter = filter::Filter::from_name("No rejects").expect("it parses");
        rebuild_browser(&mut st, &app);
    }
    app.invoke_select(0);
    assert_eq!(state.borrow().current, Some(0));
    app.invoke_meta_key("x".into());
    assert_eq!(state.borrow().views.shown, vec![1, 2, 3, 4]);
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(app.get_selected(), 0);
}

/// Ctrl+F and / both ask for the filter's text field, and neither
/// is eaten by anything else in the window. The culling keys keep
/// their own keys, and a sheet over the window keeps all of them.
#[test]
fn ctrl_f_and_slash_ask_for_the_filter_and_nothing_else_does() {
    let app = window(11);
    let asked = Rc::new(RefCell::new(0));
    let counted = asked.clone();
    app.on_filter_asked(move || *counted.borrow_mut() += 1);
    let rated = Rc::new(RefCell::new(Vec::new()));
    let seen = rated.clone();
    app.on_meta_key(move |key| {
        let known = meta::Change::from_key(&key).is_some();
        if known {
            seen.borrow_mut().push(key.to_string());
        }
        known
    });
    press(&app, "/");
    assert_eq!(*asked.borrow(), 1);
    app.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    press(&app, "f");
    app.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(*asked.borrow(), 2);
    // A rating key is still a rating key, and it did not ask.
    press(&app, "3");
    assert_eq!(*rated.borrow(), ["3"]);
    assert_eq!(*asked.borrow(), 2);
    // Nor does a sheet over the window let it through.
    app.set_export_open(true);
    press(&app, "/");
    assert_eq!(*asked.borrow(), 2);
}

/// Where the key puts the cursor, through the handler itself and
/// not a stand-in for it: the grid when the panel has no bar, and
/// the CULLING section when it has one — unfolding it first,
/// since a folded section clips its field to nothing and the
/// focus lands nowhere. The press is answered when the bar is
/// already up and when the same press is what built it, so the
/// proof either way is a character typed afterwards landing in
/// the field rather than rating the frame.
#[test]
fn the_filter_key_opens_the_grid_or_unfolds_the_culling_section() {
    // A develop tab, no grid: the chips are only in the grid's
    // header, so that is where the key goes.
    let app = window(0);
    let (state, _worker) = state_for(&app, folder(4));
    state.borrow_mut().current = Some(0);
    rebuild_browser(&mut state.borrow_mut(), &app);
    assert!(!app.get_grid_open());
    press(&app, "/");
    assert!(app.get_grid_open(), "the key opened the grid");
    // The header is built with the grid and takes the ask on the
    // next turn of the loop, where the timer puts it.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    assert!(!app.get_filter_focus(), "the bar answered and put it back");
    press(&app, "5");
    assert_eq!(
        app.get_filter_text(),
        "5",
        "the character went to the field"
    );
    assert_eq!(
        state.borrow().sidecars[0].meta.rating,
        0,
        "and rated nothing"
    );
}

#[test]
fn the_filter_key_unfolds_a_folded_culling_section() {
    let app = window(0);
    let (state, _worker) = state_for(&app, folder(4));
    state.borrow_mut().current = Some(0);
    rebuild_browser(&mut state.borrow_mut(), &app);
    // Culling, with the section folded away by a click on its
    // header: the bar is there but clipped to nothing.
    app.set_panel_tab(CULL_TAB.into());
    app.set_collapsed_culling(true);
    press(&app, "/");
    assert!(!app.get_grid_open(), "the panel already has the chips");
    assert!(!app.get_collapsed_culling(), "so the key unfolded them");
    // The field takes the focus at once, but a section unfolds
    // over an eighth of a second and a field with no height yet
    // is not where a key lands. Past the fold, then: a hand
    // reaching for the next character is slower than that, and
    // the alternative is a fold that snaps open.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
    assert!(!app.get_filter_focus(), "the bar answered and put it back");
    press(&app, "5");
    assert_eq!(
        app.get_filter_text(),
        "5",
        "the character went to the field"
    );
    assert_eq!(
        state.borrow().sidecars[0].meta.rating,
        0,
        "and rated nothing"
    );
}

/// A shoot with a rejects folder in it: c.tif in the shoot, a.tif
/// and b.tif in its rejects folder flagged reject, a's sidecar
/// beside it with an XMP, b's under the hidden folder. The scratch
/// folder, the shoot, and the three frames in that order.
fn shoot_with_rejects(what: &str) -> (PathBuf, PathBuf, Vec<PathBuf>) {
    use greycard_library::fixture::{A7, R5, R6, write_frame};
    let dir = std::env::temp_dir().join(format!(
        "greycard-ui-back-{what}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let shoot = dir.join("shoot");
    let rejects = cull::rejects_dir(&shoot);
    std::fs::create_dir_all(&rejects).unwrap();
    let shoot = dunce::canonicalize(&shoot).unwrap();
    let rejects = cull::rejects_dir(&shoot);
    let files = vec![
        shoot.join("c.tif"),
        rejects.join("a.tif"),
        rejects.join("b.tif"),
    ];
    for (i, (f, camera)) in files.iter().zip([&R6, &R5, &A7]).enumerate() {
        write_frame(f, camera, i as u16 + 1);
    }
    let mut s = Sidecar::default();
    s.meta.flag = meta::Flag::Reject;
    s.save_in(&files[1], greycard_edit::Placement::Beside)
        .unwrap();
    greycard_edit_io::xmp::save(&files[1], &s.meta, None).unwrap();
    s.save_in(&files[2], greycard_edit::Placement::Folder)
        .unwrap();
    (dir, shoot, files)
}

/// `files` open with their sidecars read and writes on, the first
/// frame on screen.
fn opened_with_sidecars(app: &App, files: &[PathBuf]) -> (Rc<Shared>, Rc<Worker>) {
    let (state, worker) = state_for(app, files.to_vec());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        for (i, f) in files.iter().enumerate() {
            if let Some(s) = Sidecar::load(f).unwrap() {
                st.sidecars[i] = s;
            }
        }
        st.current = Some(0);
        rebuild_browser(&mut st, app);
    }
    (state, worker)
}

/// Move back is offered for a frame in a rejects folder and not for
/// one in the shoot, on the menu and in CULLING alike; asked, it
/// takes the frame home with its sidecar and XMP and the flag off,
/// and a frame whose name is taken above stays whole, said.
#[test]
fn move_back_takes_the_frame_home_unflagged_and_leaves_a_taken_name_whole() {
    let (dir, shoot, files) = shoot_with_rejects("menu");
    let rejects = cull::rejects_dir(&shoot);
    let app = window(files.len());
    let (state, worker) = opened_with_sidecars(&app, &files);
    // c, in the shoot: nothing to move back.
    app.invoke_frame_menu_asked(0);
    assert_eq!(app.get_back_count(), 0);
    assert_eq!(app.get_back_to(), "");
    // a, in the rejects folder: back to the shoot.
    app.invoke_select(1);
    assert_eq!(app.get_back_count(), 1);
    assert_eq!(app.get_back_to(), "shoot");
    app.invoke_frame_menu_asked(1);
    assert_eq!(app.get_back_count(), 1);
    // The set of all three: the two in the rejects folder.
    {
        let mut st = state.borrow_mut();
        st.views.picked = vec![0, 1, 2];
        crate::panel::browser::show_set(&mut st, &app);
    }
    assert_eq!(app.get_back_count(), 2);
    // b's name is taken in the shoot already.
    std::fs::write(shoot.join("b.tif"), b"another").unwrap();
    app.invoke_move_back_asked();

    let status = app.get_status().to_string();
    assert!(
        status.starts_with(&format!(
            "moved 1 frame and 1 sidecar back to {}",
            shoot.display()
        )),
        "{status}"
    );
    assert!(
        status.contains("1 left where it was (b.tif is in"),
        "{status}"
    );
    // a is home, with its sidecar beside and its XMP, and no longer
    // a reject.
    let home = shoot.join("a.tif");
    assert!(home.exists() && !files[1].exists());
    assert!(shoot.join("a.tif.gcd").exists() && shoot.join("a.xmp").exists());
    assert!(!rejects.join("a.tif.gcd").exists() && !rejects.join("a.xmp").exists());
    let s = Sidecar::load(&home).unwrap().unwrap();
    assert_eq!(s.meta.flag, meta::Flag::None);
    // b is whole where it was, still a reject; the file above as
    // it was.
    assert!(files[2].exists());
    let hidden = rejects
        .join(greycard_edit::SIDECAR_FOLDER)
        .join("b.tif.gcd");
    assert!(hidden.exists());
    let s = Sidecar::load(&files[2]).unwrap().unwrap();
    assert_eq!(s.meta.flag, meta::Flag::Reject);
    assert_eq!(std::fs::read(shoot.join("b.tif")).unwrap(), b"another");
    // a went out of the list, as Move rejects takes its frames out.
    let st = state.borrow();
    assert_eq!(st.files, [files[0].clone(), files[2].clone()]);
    assert_eq!(st.sidecars[1].meta.flag, meta::Flag::Reject);
    drop(st);
    // Asked again for b alone: nothing goes, and the line says so.
    app.invoke_select(1);
    app.invoke_move_back_asked();
    let status = app.get_status().to_string();
    assert!(
        status.starts_with("nothing moved back; 1 left where it was (b.tif is in"),
        "{status}"
    );
    assert!(files[2].exists());
    drop(state);
    drop(worker);
    crate::testing::remove_dir_retry(&dir);
}

/// In CULLING, the button is there for a frame in a rejects folder
/// and gone for one that is not.
#[test]
fn the_culling_section_offers_move_back_for_a_frame_in_rejects() {
    let (dir, shoot, files) = shoot_with_rejects("section");
    let app = window(files.len());
    let (state, _worker) = opened_with_sidecars(&app, &files);
    {
        let mut st = state.borrow_mut();
        // The hidden folder is the setting here: the flag's write
        // after the move settles a sidecar where the setting says,
        // as every save does.
        st.window.placement = greycard_edit::Placement::Folder;
        enter_cull(&mut st, &app, 0);
    }
    assert_eq!(app.get_panel_tab(), "Cull");
    assert_eq!(crate::testing::count_labeled(&app, "Move back to shoot"), 0);
    app.invoke_select(2);
    assert_eq!(crate::testing::count_labeled(&app, "Move back to shoot"), 1);
    app.invoke_move_back_asked();
    assert!(shoot.join("b.tif").exists());
    assert!(
        shoot
            .join(greycard_edit::SIDECAR_FOLDER)
            .join("b.tif.gcd")
            .exists(),
        "the sidecar kept its place under the hidden folder"
    );
    assert_eq!(
        crate::testing::count_labeled(&app, "Move back to shoot"),
        0,
        "{}",
        app.get_status()
    );
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// Move back of every frame in the rejects folder, which empties
/// it, and Move rejects after: each move told to the index, so the
/// rows follow with their ids, nothing is left at an old path, and
/// nothing is missing. A pass over the emptied folder takes it for
/// a drive that is away and sees no move, which is why it is told.
#[test]
fn the_rows_follow_both_moves_even_out_of_an_emptied_folder() {
    use crate::library::{Indexer, Told};
    use std::time::Duration;
    let (dir, shoot, files) = shoot_with_rejects("emptied");
    let rejects = cull::rejects_dir(&shoot);
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
    // Both folders' passes over, by a word of either kind.
    let passed = |a: &Path, b: &Path| {
        let mut seen: Vec<PathBuf> = Vec::new();
        while !(seen.iter().any(|p| p == a) && seen.iter().any(|p| p == b)) {
            match wait(&|t| matches!(t, Told::Background { .. } | Told::Skipped { .. })) {
                Told::Background { path, .. } | Told::Skipped { path, .. } => seen.push(path),
                _ => {}
            }
        }
    };
    wait(&|t| matches!(t, Told::Opened(_)));
    indexer.folders(vec![shoot.clone(), rejects.clone()], 1);
    wait(&|t| matches!(t, Told::Indexed { .. }));
    let ids: Vec<i64> = {
        let reader = greycard_library::Library::open_read_only(&db).unwrap();
        files
            .iter()
            .map(|f| reader.by_path(f).unwrap().expect("indexed").id)
            .collect()
    };

    let app = window(files.len());
    let (state, worker) = opened_with_sidecars(&app, &files);
    {
        let mut st = state.borrow_mut();
        st.views.index = Some(indexer);
        // a has an XMP, which the index reads too: written with the
        // flag, as the editor does with XMPs on.
        st.window.xmp_sidecars = true;
    }
    app.invoke_select(1);
    {
        let mut st = state.borrow_mut();
        st.views.picked = vec![1, 2];
        crate::panel::browser::show_set(&mut st, &app);
    }
    app.invoke_move_back_asked();
    let (a, b) = (shoot.join("a.tif"), shoot.join("b.tif"));
    assert!(a.exists() && b.exists(), "{}", app.get_status());
    passed(&shoot, &rejects);
    {
        let reader = greycard_library::Library::open_read_only(&db).unwrap();
        for (f, (old, id)) in [&a, &b].iter().zip(files[1..].iter().zip(&ids[1..])) {
            let row = reader.by_path(f).unwrap().expect("the row at the new path");
            assert_eq!(row.id, *id, "{}", f.display());
            assert!(!row.missing);
            assert_eq!(row.meta.flag, meta::Flag::None, "the flag's write read");
            assert!(reader.by_path(old).unwrap().is_none(), "{}", old.display());
        }
    }

    // Move rejects, the other way, the same: c out.
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.files[0], files[0]);
        st.sidecars[0].meta.flag = meta::Flag::Reject;
        crate::panel::edit::write_sidecar(&mut st, 0);
    }
    app.invoke_rejects_answered(true);
    let out = rejects.join("c.tif");
    assert!(out.exists(), "{}", app.get_status());
    passed(&shoot, &rejects);
    {
        let reader = greycard_library::Library::open_read_only(&db).unwrap();
        let row = reader.by_path(&out).unwrap().expect("c's row in rejects");
        assert_eq!(row.id, ids[0]);
        assert!(!row.missing);
        assert!(reader.by_path(&files[0]).unwrap().is_none());
    }
    let indexer = state.borrow_mut().views.index.take().unwrap();
    indexer.stop(Duration::from_secs(20));
    drop(state);
    drop(worker);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame in a rejects folder under a root that is offline is not
/// offered, and asked anyway nothing moves.
#[test]
fn move_back_leaves_a_frame_under_an_offline_root_alone() {
    let (dir, shoot, files) = shoot_with_rejects("offline");
    let app = window(files.len());
    let (state, worker) = opened_with_sidecars(&app, &files);
    app.invoke_select(1);
    assert_eq!(app.get_back_count(), 1);
    {
        let mut st = state.borrow_mut();
        st.views.library.offline.insert(shoot.clone());
        assert!(!to_move_back(&st, 1));
        crate::panel::browser::show_set(&mut st, &app);
    }
    assert_eq!(app.get_back_count(), 0);
    app.invoke_move_back_asked();
    assert_eq!(app.get_status(), "no frame chosen is in a rejects folder");
    assert!(files[1].exists() && !shoot.join("a.tif").exists());
    drop(state);
    drop(worker);
    crate::testing::remove_dir_retry(&dir);
}

/// A folder with no name in its path, "." or "..", is named by its
/// canonical form, never as the dot.
#[test]
fn a_folder_without_a_name_in_its_path_is_named_all_the_same() {
    let here = std::env::current_dir().unwrap();
    let name = here.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(folder_name(Path::new(".")), name);
    assert_eq!(folder_name(&here), name);
    let up = here
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy();
    assert_eq!(folder_name(Path::new("..")), up);
}

/// The rejects folder and the folder above are handed to the
/// indexer at once, so the index has the frame at its new path
/// without waiting for the watcher or a poll.
#[test]
fn move_back_hands_both_folders_to_the_indexer() {
    use crate::library::{Indexer, Told};
    use std::time::Duration;
    let (dir, shoot, files) = shoot_with_rejects("index");
    let rejects = cull::rejects_dir(&shoot);
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
    indexer.folders(vec![shoot.clone(), rejects.clone()], 1);
    wait(&|t| matches!(t, Told::Indexed { .. }));

    let app = window(files.len());
    let (state, worker) = opened_with_sidecars(&app, &files);
    state.borrow_mut().views.index = Some(indexer);
    app.invoke_select(1);
    app.invoke_move_back_asked();
    assert!(shoot.join("a.tif").exists());
    let mut passed: Vec<PathBuf> = Vec::new();
    while !(passed.contains(&shoot) && passed.contains(&rejects)) {
        if let Told::Background { path, .. } = wait(&|t| matches!(t, Told::Background { .. })) {
            passed.push(path);
        }
    }
    let reader = greycard_library::Library::open_read_only(&db).unwrap();
    let row = reader
        .by_path(&shoot.join("a.tif"))
        .unwrap()
        .expect("a's row at its new path");
    assert!(!row.missing);
    drop(reader);
    let indexer = state.borrow_mut().views.index.take().unwrap();
    indexer.stop(Duration::from_secs(20));
    drop(state);
    drop(worker);
    crate::testing::remove_dir_retry(&dir);
}

/// The window's rejects callbacks, each recorded by name and
/// nothing more: no state behind them, so a click that reaches one
/// moves, deletes and opens nothing.
fn recorded_rejects(app: &App) -> Rc<RefCell<Vec<String>>> {
    let got = Rc::new(RefCell::new(Vec::new()));
    let g = got.clone();
    app.on_rejects_asked(move || g.borrow_mut().push("move rejects".into()));
    let g = got.clone();
    app.on_move_back_asked(move || g.borrow_mut().push("move back".into()));
    let g = got.clone();
    app.on_delete_asked(move |w| g.borrow_mut().push(format!("delete {w}")));
    let g = got.clone();
    app.on_archive_rejects_asked(move |i| g.borrow_mut().push(format!("archive {i}")));
    got
}

/// The grid header's Rejects menu opened by a click inside its
/// button's own words, and the item `downs` steps down it (the
/// menu skips its separators) taken with Return. The menu's items
/// are not clicked: the testing backend places them in the
/// popup's own coordinates, and a click there would land on the
/// window under it instead.
fn rejects_menu_pick(app: &App, button: &str, item: &str, downs: usize) {
    let (at, size) = crate::testing::labeled(app, button);
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert_eq!(crate::testing::count_labeled(app, item), 1, "{item:?}");
    for _ in 0..downs {
        press(app, Key::DownArrow);
    }
    press(app, Key::Return);
    slint::platform::update_timers_and_animations();
}

/// The grid's header holds CULLING's rejects actions in a menu,
/// each the same window callback the section's button calls, so
/// each runs the one handler and opens the one sheet.
#[test]
fn the_grids_rejects_menu_asks_what_the_culling_section_asks() {
    let app = window(3);
    // The tall desktop window the test was written at.
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let got = recorded_rejects(&app);
    app.set_reject_count(2);
    app.set_back_count(1);
    app.set_back_to("shoot".into());
    app.set_archive_rejects_choices(ModelRc::new(VecModel::from(vec![
        slint::SharedString::from("Remove rejects from nas..."),
    ])));
    let items = [
        ("Move 2 rejects...", "move rejects"),
        ("Move back to shoot", "move back"),
        ("Delete rejects folder...", "delete rejects"),
        ("Remove rejects from nas...", "archive 0"),
    ];
    app.set_grid_open(true);
    for (downs, (item, call)) in items.iter().enumerate() {
        rejects_menu_pick(&app, "2 rejects", item, downs + 1);
        assert_eq!(got.borrow_mut().drain(..).collect::<Vec<_>>(), [*call]);
    }
    app.set_grid_open(false);
    app.set_panel_tab(CULL_TAB.into());
    for (item, call) in items {
        let (at, size) = crate::testing::labeled(&app, item);
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        assert_eq!(got.borrow_mut().drain(..).collect::<Vec<_>>(), [call]);
    }
}

/// The Move rejects button keeps no focus once its sheet is up: the
/// sheet takes the keys, so Return confirms once instead of asking
/// again, Escape closes it, and the window has the keys after.
#[test]
fn the_move_rejects_sheet_takes_the_keys_from_the_button_that_opened_it() {
    let app = window(3);
    app.set_reject_count(2);
    app.set_panel_tab(CULL_TAB.into());
    let asked = Rc::new(RefCell::new(0));
    let answers = Rc::new(RefCell::new(Vec::new()));
    let n = asked.clone();
    let weak = app.as_weak();
    app.on_rejects_asked(move || {
        *n.borrow_mut() += 1;
        weak.unwrap().set_rejects_open(true);
    });
    let seen = answers.clone();
    let weak = app.as_weak();
    app.on_rejects_answered(move |yes| {
        seen.borrow_mut().push(yes);
        weak.unwrap().set_rejects_open(false);
    });
    let open = |app: &App| {
        let (at, size) = crate::testing::labeled(app, "Move 2 rejects...");
        click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        slint::platform::update_timers_and_animations();
        assert!(app.get_rejects_open());
    };
    open(&app);
    assert_eq!(*asked.borrow(), 1);
    press(&app, Key::Return);
    assert_eq!(*asked.borrow(), 1, "Return does not ask again");
    assert_eq!(*answers.borrow(), [true]);
    assert!(!app.get_rejects_open());
    open(&app);
    press(&app, Key::Escape);
    assert_eq!(*answers.borrow(), [true, false]);
    assert!(!app.get_rejects_open());
    // The keys are the window's again: C enters the mode.
    let toggles = Rc::new(RefCell::new(0));
    let seen = toggles.clone();
    app.on_cull_toggled(move || *seen.borrow_mut() += 1);
    press(&app, "c");
    assert_eq!(*toggles.borrow(), 1);
}

/// The same for the archive's sheet, opened by its section button.
#[test]
fn the_archive_rejects_sheet_takes_the_keys_from_its_button_too() {
    let app = window(3);
    app.set_panel_tab(CULL_TAB.into());
    app.set_archive_rejects_choices(ModelRc::new(VecModel::from(vec![
        slint::SharedString::from("Remove rejects from nas..."),
    ])));
    let asked = Rc::new(RefCell::new(0));
    let answers = Rc::new(RefCell::new(Vec::new()));
    let n = asked.clone();
    let weak = app.as_weak();
    app.on_archive_rejects_asked(move |_| {
        *n.borrow_mut() += 1;
        let app = weak.unwrap();
        app.set_archive_rejects_move("Move to rejects".into());
        app.set_archive_rejects_open(true);
    });
    let seen = answers.clone();
    let weak = app.as_weak();
    app.on_archive_rejects_answered(move |a| {
        seen.borrow_mut().push(a);
        weak.unwrap().set_archive_rejects_open(false);
    });
    let open = |app: &App| {
        let (at, size) = crate::testing::labeled(app, "Remove rejects from nas...");
        click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        slint::platform::update_timers_and_animations();
        assert!(app.get_archive_rejects_open());
    };
    open(&app);
    press(&app, Key::Return);
    assert_eq!(*asked.borrow(), 1);
    assert_eq!(*answers.borrow(), [1]);
    open(&app);
    press(&app, Key::Escape);
    assert_eq!(*answers.borrow(), [1, 0]);
}

/// A sheet closed from Rust, as the `--move-rejects` driver does,
/// leaves the window its keys.
#[test]
fn a_sheet_closed_from_rust_gives_the_window_its_keys() {
    let app = window(3);
    app.set_reject_count(2);
    app.set_panel_tab(CULL_TAB.into());
    let weak = app.as_weak();
    app.on_rejects_asked(move || weak.unwrap().set_rejects_open(true));
    // As the real handler: the answer closes the sheet from Rust.
    let weak = app.as_weak();
    app.on_rejects_answered(move |_| weak.unwrap().set_rejects_open(false));
    let toggles = Rc::new(RefCell::new(0));
    let seen = toggles.clone();
    app.on_cull_toggled(move || *seen.borrow_mut() += 1);
    let (at, size) = crate::testing::labeled(&app, "Move 2 rejects...");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    a_moment_on(&app);
    assert!(app.get_rejects_open());
    app.invoke_rejects_answered(true);
    a_moment_on(&app);
    assert!(!app.get_rejects_open());
    press(&app, "c");
    assert_eq!(*toggles.borrow(), 1);
}

/// A sheet that closes in the turn another opens leaves the keys
/// with the one that opened: the closed one's handler runs after
/// the turn and finds a sheet up.
#[test]
fn a_sheet_closed_as_another_opens_leaves_it_the_keys() {
    let app = window(3);
    let answers = Rc::new(RefCell::new(Vec::new()));
    let seen = answers.clone();
    app.on_rejects_answered(move |yes| seen.borrow_mut().push(yes));
    app.set_archive_open(true);
    a_moment_on(&app);
    app.set_archive_open(false);
    app.set_rejects_open(true);
    a_moment_on(&app);
    press(&app, Key::Return);
    assert_eq!(*answers.borrow(), [true]);
}

/// The menu grays and leaves out what CULLING does: no rejects
/// grays Move rejects, none of the selection in a rejects folder
/// leaves Move back out, and Delete rejects folder is always there
/// to ask, its handler saying when there is no folder.
#[test]
fn the_grids_rejects_menu_grays_and_hides_as_the_section_does() {
    let app = window(3);
    let got = recorded_rejects(&app);
    // A name with no frame to move back: the count alone decides.
    app.set_back_to("shoot".into());
    app.set_grid_open(true);
    rejects_menu_pick(&app, "Rejects", "No rejects to move", 1);
    assert!(got.borrow().is_empty(), "a grayed item asks nothing");
    // Still open, a grayed pick closing nothing: what it holds.
    assert_eq!(crate::testing::count_labeled(&app, "Move back to shoot"), 0);
    assert_eq!(
        crate::testing::count_labeled(&app, "Delete rejects folder..."),
        1
    );
    press(&app, Key::Escape);
    slint::platform::update_timers_and_animations();
    // Down past Move rejects and its separator: Delete is next,
    // with nothing between them.
    rejects_menu_pick(&app, "Rejects", "Delete rejects folder...", 2);
    assert_eq!(
        got.borrow_mut().drain(..).collect::<Vec<_>>(),
        ["delete rejects"]
    );
}

/// A moment past the menu's 1 ms refocus, with any sheet a pick
/// brought in built first: what a person's next key meets.
fn a_moment_on(app: &App) {
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(5));
    slint::platform::update_timers_and_animations();
}

/// Delete rejects folder and Remove rejects, picked from the grid's
/// menu, open sheets that keep the focus, so Return answers them
/// as it does when they come from CULLING. The window's callbacks
/// open the sheets and record the answers, and nothing else: no
/// state behind them, nothing deleted or moved.
#[test]
fn return_answers_the_sheets_the_grids_rejects_menu_opens() {
    let app = window(3);
    let got = Rc::new(RefCell::new(Vec::<String>::new()));
    let (g, weak) = (got.clone(), app.as_weak());
    app.on_delete_asked(move |w| {
        g.borrow_mut().push(format!("delete {w}"));
        weak.unwrap().set_delete_open(true);
    });
    let (g, weak) = (got.clone(), app.as_weak());
    app.on_delete_answered(move |a| {
        g.borrow_mut().push(format!("delete answered {a}"));
        weak.unwrap().set_delete_open(false);
    });
    let (g, weak) = (got.clone(), app.as_weak());
    app.on_archive_rejects_asked(move |i| {
        g.borrow_mut().push(format!("archive {i}"));
        let app = weak.unwrap();
        app.set_archive_rejects_move("Move to rejects".into());
        app.set_archive_rejects_open(true);
    });
    let (g, weak) = (got.clone(), app.as_weak());
    app.on_archive_rejects_answered(move |a| {
        g.borrow_mut().push(format!("archive answered {a}"));
        weak.unwrap().set_archive_rejects_open(false);
    });
    let (g, weak) = (got.clone(), app.as_weak());
    app.on_rejects_asked(move || {
        g.borrow_mut().push("move rejects".into());
        weak.unwrap().set_rejects_open(true);
    });
    let (g, weak) = (got.clone(), app.as_weak());
    app.on_rejects_answered(move |yes| {
        g.borrow_mut().push(format!("move answered {yes}"));
        weak.unwrap().set_rejects_open(false);
    });
    app.set_delete_trash(true);
    app.set_reject_count(2);
    app.set_archive_rejects_choices(ModelRc::new(VecModel::from(vec![
        slint::SharedString::from("Remove rejects from nas..."),
    ])));
    let items = [
        ("Move 2 rejects...", 1, "move rejects", "move answered true"),
        (
            "Delete rejects folder...",
            2,
            "delete rejects",
            "delete answered 1",
        ),
        (
            "Remove rejects from nas...",
            3,
            "archive 0",
            "archive answered 1",
        ),
    ];
    app.set_grid_open(true);
    for (item, downs, asked, answered) in items {
        rejects_menu_pick(&app, "2 rejects", item, downs);
        a_moment_on(&app);
        press(&app, Key::Return);
        assert_eq!(
            got.borrow_mut().drain(..).collect::<Vec<_>>(),
            [asked, answered],
            "{item} from the grid"
        );
    }
    app.set_grid_open(false);
    app.set_panel_tab(CULL_TAB.into());
    for (item, _, asked, answered) in items {
        let (at, size) = crate::testing::labeled(&app, item);
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        a_moment_on(&app);
        press(&app, Key::Return);
        assert_eq!(
            got.borrow_mut().drain(..).collect::<Vec<_>>(),
            [asked, answered],
            "{item} from CULLING"
        );
    }
}

/// Two frames on disk, a edited (+1.5 EV, its sidecar written) and
/// b never touched (no sidecar), in a folder of `what`'s own.
fn an_edited_frame_and_one_untouched(what: &str) -> (PathBuf, Vec<PathBuf>) {
    use greycard_library::fixture::{A7, R5, write_frame};
    let dir = crate::testing::scratch_dir(&format!("ui-cull-quit-{what}"));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dunce::canonicalize(&dir).unwrap();
    let files = vec![dir.join("a.tif"), dir.join("b.tif")];
    write_frame(&files[0], &R5, 1);
    write_frame(&files[1], &A7, 2);
    let mut edited = Edit::default();
    edited.light.exposure = 1.5;
    let mut s = Sidecar::default();
    s.record(edited);
    s.save(&files[0]).unwrap();
    (dir, files)
}

/// The quit's saves, as `startup::main` makes them once the window
/// has closed: the panel read, then `save_at_quit`.
fn quit(state: &Rc<Shared>, app: &App) -> crate::sync::QuitWrites {
    let edit = {
        let st = state.borrow();
        read_edit(app, &st.develop.edit, st.develop.target)
    };
    crate::panel::startup::save_at_quit(&mut state.borrow_mut(), edit, crate::sync::QUIT_WRITE_WAIT)
}

/// `--cull` over an edited frame, and quit: the run developed
/// nothing, so the panel never held the frame's edit, only the
/// default; the quit records none of it on the frame, whose
/// sidecar on disk is as it was.
#[test]
fn a_quit_from_culling_at_launch_leaves_the_edited_frame_alone() {
    let (dir, files) = an_edited_frame_and_one_untouched("launch");
    let app = window(files.len());
    let (state, _worker) = opened_with_sidecars(&app, &files);
    {
        let mut st = state.borrow_mut();
        // As the window starts: nothing open yet, the mode asked for.
        st.current = None;
        st.launch.cull_at_start = Some(1);
    }
    let on_disk = std::fs::read(Sidecar::path_for(&files[0])).unwrap();
    app.invoke_select(0);
    {
        let st = state.borrow();
        assert!(st.culling.cull.is_some());
        assert_eq!(st.current, Some(0));
        assert_ne!(
            read_edit(&app, &st.develop.edit, st.develop.target),
            st.sidecars[0].current,
            "the panel is not the frame's"
        );
    }
    quit(&state, &app);
    assert_eq!(
        std::fs::read(Sidecar::path_for(&files[0])).unwrap(),
        on_disk,
        "nothing written over it"
    );
    assert_eq!(state.borrow().sidecars[0].current.light.exposure, 1.5);
    assert_eq!(state.borrow().sidecars[0].history.len(), 1);
    assert!(Sidecar::load(&files[1]).unwrap().is_none());
    crate::testing::remove_scratch(state, &dir);
}

/// Developed on an edited frame, culled on to an untouched one,
/// and quit: the panel still holds the first frame's edit, and
/// none of it reaches the second, which still has no sidecar; the
/// first frame's is as it was, the way in having found nothing new
/// on the panel to record.
#[test]
fn a_quit_from_culling_writes_no_frames_edit_onto_another() {
    let (dir, files) = an_edited_frame_and_one_untouched("moved-on");
    let app = window(files.len());
    let (state, _worker) = opened_with_sidecars(&app, &files);
    state.borrow_mut().current = None;
    let on_disk = std::fs::read(Sidecar::path_for(&files[0])).unwrap();
    // a opens in the develop view: its edit on the panel.
    app.invoke_select(0);
    assert!(state.borrow().culling.cull.is_none());
    assert_eq!(app.get_exposure(), 1.5);
    // C, then the arrow on to b.
    press(&app, "c");
    assert!(state.borrow().culling.cull.is_some(), "culling");
    press(&app, Key::RightArrow);
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(app.get_exposure(), 1.5, "the panel is still a's");
    quit(&state, &app);
    assert!(
        Sidecar::load(&files[1]).unwrap().is_none(),
        "b has no sidecar"
    );
    assert!(state.borrow().sidecars[1].history.is_empty());
    assert_eq!(state.borrow().sidecars[1].current, Edit::default());
    assert_eq!(
        std::fs::read(Sidecar::path_for(&files[0])).unwrap(),
        on_disk,
        "a's is as it was"
    );
    crate::testing::remove_scratch(state, &dir);
}

// The window's two views, Library and Loupe, and the Loupe's two
// modes, Cull and Develop; Library has no mode of its own.

/// Whether the control a screen reader knows as `label` is
/// pressed; None when there is no such control.
fn pressed(app: &App, label: &str) -> Option<bool> {
    crate::testing::count_labeled(app, label);
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(app, label)
        .next()
        .and_then(|e| e.accessible_checked())
}

fn click_labeled(app: &App, label: &str) {
    let (at, size) = crate::testing::labeled(app, label);
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

/// A click on the panel's tab `label`: the element so named in the
/// row the Cull tab is in (CULLING ends in a Develop button of its
/// own, lower down).
fn click_tab(app: &App, label: &str) {
    let (row, height) = {
        let (at, size) = crate::testing::labeled(app, "Cull mode");
        (at.y, size.height)
    };
    crate::testing::count_labeled(app, label);
    let (at, size) = i_slint_backend_testing::ElementHandle::find_by_accessible_label(app, label)
        .map(|e| (e.absolute_position(), e.size()))
        .find(|(at, size)| at.y >= row && at.y + size.height <= row + height)
        .expect("the tab is there");
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

/// A tall window over six frames, the third one open in Develop.
fn modes_window() -> (App, Rc<Shared>, Rc<Worker>) {
    let app = window(6);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = state_for(&app, folder(6));
    app.invoke_select(2);
    (app, state, worker)
}

/// The selection: the frame open, its row and the set.
fn chosen(app: &App, state: &Rc<Shared>) -> (Option<usize>, i32, Vec<usize>) {
    let st = state.borrow();
    (st.current, app.get_selected(), st.views.picked.clone())
}

/// Which state the window is in, as the controls and the state
/// both say it: Library, the Loupe in Cull from the camera's JPEG,
/// the Loupe in Cull developed, or the Loupe in Develop.
#[derive(Debug, PartialEq)]
enum Seen {
    Library,
    CullCamera,
    CullDeveloped,
    Develop,
}

fn seen(app: &App, state: &Rc<Shared>) -> Seen {
    let st = state.borrow();
    // Never both at once.
    assert!(!(st.culling.cull.is_some() && st.culling.developed.is_some()));
    assert_eq!(app.get_culling(), st.culling.cull.is_some());
    assert_eq!(app.get_cull_developed(), st.culling.developed.is_some());
    if app.get_grid_open() {
        return Seen::Library;
    }
    if st.culling.cull.is_some() {
        assert_eq!(app.get_panel_tab(), CULL_TAB);
        Seen::CullCamera
    } else if st.culling.developed.is_some() {
        assert_eq!(app.get_panel_tab(), CULL_TAB);
        Seen::CullDeveloped
    } else {
        assert_ne!(app.get_panel_tab(), CULL_TAB);
        Seen::Develop
    }
}

/// Library is the grid with the left pane, Loupe the one frame:
/// the header's own buttons are there in the one and not the
/// other, and the pane's Library | Loupe say which is on.
#[test]
fn library_shows_the_grid_and_the_left_pane_and_the_loupe_one_picture() {
    let (app, state, _worker) = modes_window();
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert_eq!(pressed(&app, "Loupe view"), Some(true));
    assert_eq!(crate::testing::count_labeled(&app, "Lightbox"), 0);
    click_labeled(&app, "Library view");
    assert_eq!(seen(&app, &state), Seen::Library);
    assert!(!app.get_left_hidden(), "the left pane is the library's");
    assert_eq!(pressed(&app, "Library view"), Some(true));
    assert_eq!(pressed(&app, "Loupe view"), Some(false));
    assert_eq!(
        crate::testing::count_labeled(&app, "Lightbox"),
        1,
        "the grid's header"
    );
    click_labeled(&app, "Loupe view");
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert_eq!(chosen(&app, &state), (Some(2), 2, vec![2]));
}

/// The Loupe comes back from Library in the mode it was left in,
/// Cull or Develop, by the pane's Loupe, the grid's Loupe, a
/// double-click on a cell, Return in the grid, G or E.
#[test]
fn the_loupe_comes_back_from_library_in_the_mode_it_was_left_in() {
    let (app, state, _worker) = modes_window();
    click_labeled(&app, "Cull mode");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    assert_eq!(pressed(&app, "Cull mode"), Some(true));
    assert_eq!(
        crate::testing::count_labeled(&app, "Develop mode"),
        0,
        "no row of its own"
    );
    assert!(state.borrow().culling.loupe_cull);
    for back in ["pane", "double-click", "return", "g", "e"] {
        click_labeled(&app, "Library view");
        assert_eq!(seen(&app, &state), Seen::Library, "{back}");
        // The old Cull toggle is gone from the grid's header, and
        // so is its Loupe button: the one "Loupe" left is the pane's
        // switch, the one way between the views.
        assert_eq!(crate::testing::count_labeled(&app, "Culling"), 0);
        assert_eq!(crate::testing::count_labeled(&app, "Loupe"), 1);
        match back {
            "pane" => click_labeled(&app, "Loupe view"),
            "double-click" => {
                // The cell's name (the test window's names, not the
                // files'), as wide as the grid's cell: not the
                // header's word for the frame, nor the strip's.
                crate::testing::count_labeled(&app, "f02.CR3");
                let (at, size) = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                    &app, "f02.CR3",
                )
                .map(|e| (e.absolute_position(), e.size()))
                .find(|(_, size)| size.width == app.get_grid_cell())
                .expect("the grid's cell");
                let (x, y) = (at.x + size.width / 2.0, at.y - 40.0);
                let position = slint::LogicalPosition::new(x, y);
                let button = slint::platform::PointerEventButton::Left;
                app.window()
                    .dispatch_event(WindowEvent::PointerMoved { position });
                for _ in 0..2 {
                    app.window()
                        .dispatch_event(WindowEvent::PointerPressed { position, button });
                    app.window()
                        .dispatch_event(WindowEvent::PointerReleased { position, button });
                }
            }
            "return" => press(&app, Key::Return),
            "g" => press(&app, "g"),
            _ => press(&app, "e"),
        }
        assert_eq!(seen(&app, &state), Seen::CullCamera, "{back}");
        assert_eq!(chosen(&app, &state), (Some(2), 2, vec![2]), "{back}");
    }
    // Develop is kept the same way.
    click_tab(&app, "Develop");
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert!(!state.borrow().culling.loupe_cull);
    click_labeled(&app, "Library view");
    click_labeled(&app, "Loupe view");
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert_eq!(chosen(&app, &state), (Some(2), 2, vec![2]));
}

/// Develop from Library is the Loupe in Develop, the edit panel
/// up, whatever mode the Loupe was left in: D from the grid.
#[test]
fn develop_from_library_lands_in_the_loupe_in_develop_with_the_edit_panel() {
    let (app, state, _worker) = modes_window();
    app.set_panel_tab("Crop".into());
    press(&app, "c");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    press(&app, "g");
    assert_eq!(seen(&app, &state), Seen::Library);
    press(&app, "d");
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert_eq!(app.get_panel_tab(), "Crop", "the tab Cull was entered from");
    assert_eq!(pressed(&app, "Cull mode"), Some(false));
    assert_eq!(chosen(&app, &state), (Some(2), 2, vec![2]));
    // From the developed Cull too.
    press(&app, "c");
    click_labeled(&app, "Developed pictures");
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    press(&app, "g");
    press(&app, "d");
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert_eq!(app.get_panel_tab(), "Crop");
}

/// D is Develop from anywhere: from Cull on either picture, from
/// Library over either, and from the lightbox, the selection
/// carried; in Develop it changes nothing. Under a sheet it is the
/// sheet's, and with the filter's field focused it is typed.
#[test]
fn d_goes_straight_to_develop_from_cull_library_and_the_lightbox() {
    let (app, state, _worker) = modes_window();
    press(&app, "d");
    assert_eq!(seen(&app, &state), Seen::Develop);
    for developed in [false, true] {
        press(&app, "c");
        let cull = if developed {
            click_labeled(&app, "Developed pictures");
            Seen::CullDeveloped
        } else {
            Seen::CullCamera
        };
        assert_eq!(seen(&app, &state), cull);
        press(&app, "d");
        assert_eq!(seen(&app, &state), Seen::Develop, "{developed}");
        press(&app, "c");
        press(&app, "g");
        assert_eq!(seen(&app, &state), Seen::Library);
        press(&app, "d");
        assert_eq!(seen(&app, &state), Seen::Develop, "{developed}");
        assert_eq!(chosen(&app, &state), (Some(2), 2, vec![2]));
    }
    press(&app, "c");
    app.invoke_lightbox_enter();
    assert!(app.get_lightbox());
    press(&app, "d");
    assert!(!app.get_lightbox());
    assert_eq!(seen(&app, &state), Seen::Develop);
    // A sheet takes it.
    press(&app, "c");
    app.set_keys_open(true);
    press(&app, "d");
    assert_eq!(seen(&app, &state), Seen::CullDeveloped, "the sheet's key");
    app.set_keys_open(false);
    // Typed into the filter, not a key of the window's.
    press(&app, "g");
    app.set_filter_focus(true);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    press(&app, "d");
    assert_eq!(seen(&app, &state), Seen::Library);
    assert_eq!(app.get_filter_text(), "d");
}

/// The Loupe's Cull | Develop: Cull draws the camera's JPEG (a step
/// develops nothing) with the CULLING section in place of the edit
/// panel; Develop develops the frame and brings the panel back.
#[test]
fn the_cull_and_develop_switch_changes_the_picture_and_the_panel() {
    let (app, state, _worker) = modes_window();
    assert_eq!(crate::testing::count_labeled(&app, "Exposure value"), 1);
    click_labeled(&app, "Cull mode");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    assert_eq!(pressed(&app, "Camera JPEG pictures"), Some(true));
    assert_eq!(pressed(&app, "Developed pictures"), Some(false));
    // The camera's picture: a step reaches the next frame without a
    // develop.
    let generation = state.borrow().develop.generation;
    press(&app, Key::RightArrow);
    assert_eq!(state.borrow().current, Some(3));
    assert_eq!(
        state.borrow().develop.generation,
        generation,
        "nothing developed"
    );
    assert!(!app.get_busy());
    click_tab(&app, "Develop");
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert_eq!(app.get_panel_tab(), "Develop");
    assert!(
        state.borrow().develop.generation > generation,
        "the frame develops"
    );
    assert_eq!(chosen(&app, &state), (Some(3), 3, vec![3]));
    // C is the same switch in the Loupe.
    press(&app, "c");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    press(&app, "c");
    assert_eq!(seen(&app, &state), Seen::Develop);
}

/// Cull's Developed switch: the developed picture, each frame
/// developed as it is reached, still with the CULLING section and
/// no edit panel, the key's word over the picture, Move on and
/// culling's own undo; Camera JPEG goes back. The switch is kept
/// for the next Cull.
#[test]
fn the_developed_switch_inside_cull_shows_the_developed_picture_with_no_edit_panel() {
    let (app, state, _worker) = modes_window();
    click_labeled(&app, "Cull mode");
    click_labeled(&app, "Developed pictures");
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    assert!(state.borrow().culling.show_developed);
    assert!(state.borrow().culling.loupe_cull);
    assert_eq!(pressed(&app, "Developed pictures"), Some(true));
    assert_eq!(pressed(&app, "Cull mode"), Some(true));
    assert_eq!(
        crate::testing::count_labeled(&app, "Exposure value"),
        0,
        "no edit panel"
    );
    assert_eq!(chosen(&app, &state), (Some(2), 2, vec![2]));
    // A step develops the next frame: the develop view's own path.
    let generation = state.borrow().develop.generation;
    press(&app, Key::RightArrow);
    assert_eq!(state.borrow().current, Some(3));
    assert!(state.borrow().develop.generation > generation, "developed");
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    // The culling keys: the word, and Move on.
    app.set_cull_move_on(true);
    app.invoke_cull_move_on_changed();
    app.invoke_meta_key("4".into());
    assert_eq!(state.borrow().sidecars[3].meta.rating, 4);
    assert_eq!(app.get_notice(), "4 stars");
    assert!(app.get_notice_on());
    assert_eq!(state.borrow().current, Some(4), "moved on");
    // Culling's undo: the rating, and back to its frame; the mode
    // stays.
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[3].meta.rating, 0);
    assert_eq!(state.borrow().current, Some(3));
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    // Camera JPEG, and back.
    click_labeled(&app, "Camera JPEG pictures");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    assert!(!state.borrow().culling.show_developed);
    click_labeled(&app, "Developed pictures");
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    // Develop, then Cull again: developed, as the switch was left.
    click_tab(&app, "Develop");
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert_eq!(app.get_panel_tab(), "Develop");
    assert!(!app.get_notice_on(), "the word goes with the mode");
    click_labeled(&app, "Cull mode");
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    // A tab of the edit is the way out, as it is from Cull on the
    // camera's JPEG.
    app.invoke_tab_changed("Crop".into());
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert_eq!(app.get_panel_tab(), "Crop");
    assert_eq!(chosen(&app, &state), (Some(3), 3, vec![3]));
}

/// In Library the culling keys are culling's whatever the Loupe's
/// mode: a rating lands, and with the switch on moves on along the
/// grid, by culling's rules (one frame chosen, not past the end);
/// with the switch off it stays. Over Develop, so no Cull is on.
#[test]
fn a_rating_in_library_works_and_moves_on_with_no_toggle() {
    let (app, state, _worker) = modes_window();
    click_labeled(&app, "Library view");
    assert_eq!(seen(&app, &state), Seen::Library);
    assert!(state.borrow().culling.cull.is_none());
    press(&app, "3");
    assert_eq!(state.borrow().sidecars[2].meta.rating, 3);
    assert_eq!(state.borrow().current, Some(2), "the switch is off");
    app.set_cull_move_on(true);
    app.invoke_cull_move_on_changed();
    press(&app, "4");
    assert_eq!(state.borrow().sidecars[2].meta.rating, 4);
    assert_eq!(state.borrow().current, Some(3), "moved on");
    assert_eq!(app.get_selected(), 3);
    press(&app, "p");
    assert_eq!(state.borrow().current, Some(4));
    // A set of two is rated together and left where it is.
    app.invoke_frame_clicked(5, true, false);
    press(&app, "2");
    assert_eq!(state.borrow().sidecars[4].meta.rating, 2);
    assert_eq!(state.borrow().sidecars[5].meta.rating, 2);
    assert_eq!(state.borrow().current, Some(4));
    // At the end, nothing past it.
    press(&app, Key::Escape);
    app.invoke_select(5);
    press(&app, "1");
    assert_eq!(state.borrow().current, Some(5));
    assert_eq!(seen(&app, &state), Seen::Library, "still the grid");
    // The lightbox is a view apart: no Move on there over Develop.
    app.invoke_select(0);
    app.invoke_lightbox_enter();
    press(&app, "5");
    assert_eq!(state.borrow().sidecars[0].meta.rating, 5);
    assert_eq!(state.borrow().current, Some(0));
}

/// A rating in the Loupe's Cull moves on, from the button's way in.
#[test]
fn a_rating_in_the_loupes_cull_moves_on() {
    let (app, state, _worker) = modes_window();
    app.set_cull_move_on(true);
    app.invoke_cull_move_on_changed();
    click_labeled(&app, "Cull mode");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    press(&app, "5");
    assert_eq!(state.borrow().sidecars[2].meta.rating, 5);
    assert_eq!(state.borrow().current, Some(3));
    assert_eq!(app.get_notice(), "5 stars");
    // In Develop it does not: the switch is culling's.
    click_tab(&app, "Develop");
    press(&app, "1");
    assert_eq!(state.borrow().sidecars[3].meta.rating, 1);
    assert_eq!(state.borrow().current, Some(3));
}

/// The Loupe's mode and Cull's switch are kept in the settings, and
/// the next run's first frame opens in them; not for a capture, and
/// not over a `--cull` that asked for its own.
#[test]
fn the_loupes_mode_is_kept_across_a_settings_round_trip() {
    let kept = crate::settings::Settings {
        loupe_cull: true,
        cull_developed: true,
        ..Default::default()
    };
    let text = serde_json::to_string(&kept).unwrap();
    let read: crate::settings::Settings = serde_json::from_str(&text).unwrap();
    assert!(read.loupe_cull && read.cull_developed);
    let old: crate::settings::Settings = serde_json::from_str("{}").unwrap();
    assert!(!old.loupe_cull && !old.cull_developed, "Develop by default");

    // Cull on the camera's JPEG: the first frame culls.
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(3));
    take_remembered_mode(&mut state.borrow_mut(), true, false);
    assert_eq!(state.borrow().launch.cull_at_start, Some(1));
    app.invoke_select(0);
    assert_eq!(seen(&app, &state), Seen::CullCamera);

    // Developed: the first frame opens under the CULLING section.
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(3));
    take_remembered_mode(
        &mut state.borrow_mut(),
        read.loupe_cull,
        read.cull_developed,
    );
    assert!(state.borrow().launch.developed_at_start);
    assert_eq!(state.borrow().launch.cull_at_start, None);
    app.invoke_select(0);
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    assert_eq!(state.borrow().current, Some(0));

    // Develop, a run the command line starts and a `--cull` leave
    // the start alone.
    for (cull, scripted, flag) in [
        (false, false, None),
        (true, true, None),
        (true, false, Some(2)),
    ] {
        let app = window(3);
        let (state, _worker) = state_for(&app, folder(3));
        {
            let mut st = state.borrow_mut();
            st.launch.scripted = scripted;
            st.launch.cull_at_start = flag;
            take_remembered_mode(&mut st, cull, true);
            assert!(!st.launch.developed_at_start, "{cull} {scripted} {flag:?}");
            assert_eq!(st.launch.cull_at_start, flag, "{cull} {scripted} {flag:?}");
        }
    }
}

/// The keys: G is a round trip, Library from the Loupe and back to
/// the Loupe in its mode; E the Loupe in its mode; D Develop; C Cull
/// from Library and Cull or Develop in turn in the Loupe; Return and
/// Escape in Cull are Develop. The selection and the set ride
/// through every one.
#[test]
fn g_e_and_c_switch_the_views_and_the_modes_and_carry_the_selection() {
    let (app, state, _worker) = modes_window();
    app.invoke_frame_clicked(3, true, false);
    let set = chosen(&app, &state);
    assert_eq!(set.2.len(), 2, "a set of two");
    let steps: [(&str, Seen); 14] = [
        ("g", Seen::Library),
        ("g", Seen::Develop),
        ("g", Seen::Library),
        ("e", Seen::Develop),
        ("c", Seen::CullCamera),
        ("g", Seen::Library),
        ("g", Seen::CullCamera),
        ("g", Seen::Library),
        ("e", Seen::CullCamera),
        ("g", Seen::Library),
        ("d", Seen::Develop),
        ("g", Seen::Library),
        ("c", Seen::CullCamera),
        ("c", Seen::Develop),
    ];
    for (key, then) in steps {
        press(&app, key);
        assert_eq!(seen(&app, &state), then, "{key}");
        assert_eq!(chosen(&app, &state), set, "{key}: the selection carries");
    }
    // Return in Cull is Develop, from either picture.
    press(&app, "c");
    press(&app, Key::Return);
    assert_eq!(seen(&app, &state), Seen::Develop);
    press(&app, "c");
    click_labeled(&app, "Developed pictures");
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    assert_eq!(chosen(&app, &state), set);
    // Escape lets go of the set first, then leaves Cull.
    press(&app, Key::Escape);
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    press(&app, Key::Escape);
    assert_eq!(seen(&app, &state), Seen::Develop);
    // E in the Loupe stays in the Loupe.
    press(&app, "e");
    assert_eq!(seen(&app, &state), Seen::Develop);
}

/// The command line owns the start of a capture or a timing run:
/// the mode the last session left is not taken for it (a timing run
/// opened in Cull would wait for a develop that never comes), and
/// nothing it does to the mode is kept.
#[test]
fn a_timing_run_or_a_capture_opens_in_no_remembered_mode() {
    use clap::Parser;
    let parse = |args: &[&str]| {
        let mut all = vec!["greycard-ui"];
        all.extend_from_slice(args);
        crate::Cli::try_parse_from(all).unwrap()
    };
    let runs: [&[&str]; 9] = [
        &["--snapshot", "a.png", "x"],
        &["--screenshot", "a.png", "x"],
        &["--time-sharpen", "3", "x"],
        &["--time-clarity", "3", "x"],
        &["--time-dehaze", "3", "x"],
        &["--time-texture", "3", "x"],
        &["--time-cull", "3", "x"],
        &["--time-select", "3", "x"],
        &["--time-scroll", "3", "x"],
    ];
    for args in runs {
        assert!(
            crate::panel::startup::scripted_start(&parse(args)),
            "{args:?}"
        );
    }
    assert!(!crate::panel::startup::scripted_start(&parse(&["x"])));
    assert!(!crate::panel::startup::scripted_start(&parse(&[
        "--cull", "x"
    ])));

    let app = window(3);
    let (state, _worker) = state_for(&app, folder(3));
    state.borrow_mut().launch.scripted = true;
    take_remembered_mode(&mut state.borrow_mut(), true, false);
    assert_eq!(state.borrow().launch.cull_at_start, None);
    assert!(!state.borrow().culling.loupe_cull);
    app.invoke_select(0);
    assert_eq!(seen(&app, &state), Seen::Develop, "the timing run develops");
}

/// `--cull-developed` is refused beside every flag that asks for
/// Cull on the camera's JPEG, as `--cull` and `--cull-compare` are.
#[test]
fn cull_developed_is_refused_beside_the_camera_culls_flags() {
    use clap::Parser;
    let parse = |args: &[&str]| {
        let mut all = vec!["greycard-ui", "--cull-developed"];
        all.extend_from_slice(args);
        crate::Cli::try_parse_from(all)
    };
    assert!(parse(&["x"]).is_ok());
    let others: [&[&str]; 7] = [
        &["--cull"],
        &["--cull-compare", "2"],
        &["--cull-key", "3"],
        &["--ask-rejects"],
        &["--move-rejects"],
        &["--cull-develop"],
        &["--time-cull", "3"],
    ];
    for other in others {
        let mut args = other.to_vec();
        args.push("x");
        assert!(parse(&args).is_err(), "{other:?}");
    }
}

/// With Developed on, a Cull a compare put up on the camera's JPEG
/// (Compare versions and Compare stack enter it directly) holds
/// through a click on the lit Cull tab, and through G and then C:
/// neither ends the compare nor develops.
#[test]
fn a_compare_on_the_camera_jpeg_holds_through_cull_asked_again() {
    let (app, state, _worker) = modes_window();
    click_labeled(&app, "Cull mode");
    click_labeled(&app, "Developed pictures");
    assert!(state.borrow().culling.show_developed);
    click_tab(&app, "Develop");
    assert_eq!(seen(&app, &state), Seen::Develop);
    // As Compare stack puts it up.
    enter_cull(&mut state.borrow_mut(), &app, 2);
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    let generation = state.borrow().develop.generation;
    click_labeled(&app, "Cull mode");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    assert_eq!(app.get_compare(), 2, "the compare stands");
    press(&app, "g");
    press(&app, "c");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    assert_eq!(app.get_compare(), 2);
    assert_eq!(
        state.borrow().develop.generation,
        generation,
        "nothing developed"
    );
}

/// C in Cull leaves it even with no frame open, as after a folder
/// with no pictures in it.
#[test]
fn c_leaves_cull_with_no_frame_open() {
    let (app, state, _worker) = modes_window();
    press(&app, "c");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    state.borrow_mut().current = None;
    press(&app, "c");
    assert_eq!(seen(&app, &state), Seen::Develop);
    // And with no frame, C has nothing to cull.
    press(&app, "c");
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert_eq!(app.get_status(), "nothing to cull: open a folder");
}

/// A preset applied in Cull moves the edit, so the white As shot's
/// switch replaced before Cull is not given back after it.
#[test]
fn a_preset_in_cull_forgets_what_as_shot_replaced() {
    use greycard_edit::preset::{Entry, Preset, Section};
    let (app, state, _worker) = modes_window();
    let mut edit = Edit::default();
    edit.light.exposure = 0.5;
    state.borrow_mut().develop.presets = vec![Entry {
        path: PathBuf::new(),
        preset: Preset::from_edit("Bright", &edit, &[Section::Light]),
    }];
    click_labeled(&app, "Cull mode");
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    {
        let mut st = state.borrow_mut();
        let key = st.current.and_then(|c| st.key_of(c)).expect("a frame");
        st.develop.before_as_shot = Some((key, (4000.0, 0.01)));
    }
    app.invoke_preset_applied(0);
    assert_eq!(seen(&app, &state), Seen::Develop);
    assert!(state.borrow().develop.before_as_shot.is_none());
    app.set_as_shot(false);
    app.invoke_as_shot_toggled();
    assert_ne!((app.get_temperature(), app.get_tint()), (4000.0, 0.01));
}

/// An edit made in Cull drawn developed (a preset clicked) leaves to
/// Develop as one made in Cull on the camera's JPEG does: the panel
/// shows the edit, and Ctrl+Z takes back the preset, not a rating.
#[test]
fn an_edit_in_developed_cull_leaves_to_develop() {
    use greycard_edit::preset::{Entry, Preset, Section};
    let (app, state, _worker) = modes_window();
    let mut edit = Edit::default();
    edit.light.exposure = 0.5;
    state.borrow_mut().develop.presets = vec![Entry {
        path: PathBuf::new(),
        preset: Preset::from_edit("Bright", &edit, &[Section::Light]),
    }];
    click_labeled(&app, "Cull mode");
    click_labeled(&app, "Developed pictures");
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    app.invoke_meta_key("3".into());
    assert_eq!(state.borrow().sidecars[2].meta.rating, 3);
    app.invoke_preset_applied(0);
    assert_eq!(seen(&app, &state), Seen::Develop, "the edit is the way out");
    assert_eq!(app.get_panel_tab(), "Develop");
    assert_eq!(state.borrow().sidecars[2].current.light.exposure, 0.5);
    assert_eq!(app.get_exposure(), 0.5, "on the panel");
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[2].current.light.exposure, 0.0);
    assert_eq!(
        state.borrow().sidecars[2].meta.rating,
        3,
        "the rating stays"
    );
    // The switch is still on for the next Cull.
    click_labeled(&app, "Cull mode");
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
}

/// Two frames whose edits are black and white, with the pictures of
/// those edits kept (as once a develop has landed and been saved),
/// over camera pictures in color: Cull on the camera's JPEG shows the
/// camera's picture, entered from Develop and switched to from
/// Developed Cull alike, and the develop view's stand-in, which is
/// the edit's, never goes up in its place. The other way, Developed
/// stands each frame in with its edit's picture until its develop
/// lands, not with the camera picture the mode had decoded.
#[test]
fn cull_on_the_camera_jpeg_shows_the_camera_s_picture_of_an_edited_frame() {
    use crate::versions::Key as Frame;
    let dir = crate::testing::scratch_dir("ui-cull-camera-of-edited");
    let files = vec![dir.join("a.png"), dir.join("b.png")];
    // Each of its own content, a in red.
    for (i, f) in files.iter().enumerate() {
        image::RgbImage::from_pixel(600, 400, image::Rgb([200, 40 + i as u8, 30]))
            .save(f)
            .unwrap();
    }
    let mut edit = Edit::for_picture();
    edit.bw.enabled = true;
    for f in &files {
        greycard_edit::Sidecar {
            line: greycard_edit::Line::of(edit.clone()),
            ..greycard_edit::Sidecar::default()
        }
        .save(f)
        .unwrap();
    }
    // The edit's pictures of both, grey, kept as a develop keeps them.
    let cache: crate::worker::ThumbCache = Arc::new(std::sync::Mutex::new(Some(
        greycard_library::thumbs::Thumbs::at(
            dir.join("thumbs"),
            greycard_library::thumbs::DEFAULT_CAP,
        )
        .with_previews(
            crate::edited_pictures::BIG,
            greycard_library::thumbs::DEFAULT_PREVIEW_CAP,
        ),
    )));
    let edited = Arc::new(crate::edited_pictures::Edited::new(cache.clone()));
    edited.note_all([
        (Frame::file(files[0].clone()), true, None, None),
        (Frame::file(files[1].clone()), true, None, None),
    ]);
    let grey = |size: u32| {
        let (w, h) = (size, size * 2 / 3);
        greycard_library::thumbs::Thumb {
            width: w,
            height: h,
            rgb: vec![128; (w * h * 3) as usize],
        }
    };
    let made = crate::edited_pictures::Made {
        big: grey(crate::edited_pictures::BIG),
        thumbs: crate::grid::MADE.iter().map(|&s| (s, grey(s))).collect(),
    };
    for f in &files {
        let hash = greycard_library::hash_file(f).unwrap();
        edited
            .store_reduced(&hash, edited.key_of(f, &edit), &made)
            .unwrap();
    }
    let previews = Arc::new(crate::previews::Previews::new(cache).with_edited(edited));

    let app = window(files.len());
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = opened_with_sidecars(&app, &files);
    {
        let mut st = state.borrow_mut();
        st.current = None;
        st.culling.prefetch.set_previews(previews.clone());
    }
    let generation = |state: &Rc<Shared>| state.borrow().develop.generation;
    let picture = || crate::worker::SourceKind::Picture {
        space: "sRGB".into(),
        bits: 8,
    };
    let develop = |app: &App| {
        crate::panel::deliver::deliver(app, opened(generation(&state), picture()));
        let developed = developed_as(
            generation(&state),
            (600, 400),
            0,
            crate::finish::Source::Display,
        );
        crate::panel::deliver::deliver(app, developed);
        draw(app, &state);
    };
    // The decode threads' work, here: every picture of `file` the
    // loupe asked for, delivered.
    let decode_asked = |app: &App, file: usize| {
        let asked = state.borrow().culling.prefetch.asked();
        let wants: Vec<_> = asked.into_iter().filter(|w| w.file == file).collect();
        assert!(!wants.is_empty(), "{file}'s picture asked for");
        for want in wants {
            let got = std::cell::RefCell::new(Vec::new());
            cull::fetch(&want, Some(&previews), &|l| got.borrow_mut().push(l));
            for loaded in got.into_inner() {
                deliver_preview(app, loaded);
            }
        }
    };
    // What the loupe draws of a: where it came from, and its red
    // against its blue.
    let shown = |state: &Rc<Shared>| {
        let st = state.borrow();
        let cull = st.culling.cull.as_ref().expect("Cull on the camera's JPEG");
        let p = cull.cache.best(0).expect("a picture of a");
        let px = p.rgba.as_chunks::<4>().0;
        let mean = |k: usize| px.iter().map(|q| q[k] as f64).sum::<f64>() / px.len() as f64;
        (p.origin(), mean(0) - mean(2))
    };

    // a opens in Develop and its develop lands.
    app.invoke_select(0);
    assert_eq!(seen(&app, &state), Seen::Develop);
    develop(&app);
    // Cull, on the camera's JPEG, b decoded ahead.
    app.invoke_cull_asked();
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    decode_asked(&app, 0);
    decode_asked(&app, 1);
    let (from, warmth) = shown(&state);
    assert_eq!(from, cull::Origin::Camera, "the camera's picture");
    assert!(warmth > 100.0, "in color: {warmth:.0}");

    // Developed: until a's develop lands it stands in with the
    // picture of its edit, the camera's up only until that is read,
    // and b's camera picture is not kept to stand in with.
    let stand_in = |state: &Rc<Shared>, file: usize| {
        let st = state.borrow();
        assert_eq!(standing_in(&st), Some(file), "a stand-in up");
        let hold = st.culling.hold.as_ref().expect("the hold");
        hold.cache.get(file).map(|p| p.origin())
    };
    app.invoke_cull_source_asked(true);
    assert_eq!(seen(&app, &state), Seen::CullDeveloped);
    assert_eq!(stand_in(&state, 0), Some(cull::Origin::Camera));
    assert!(
        state
            .borrow()
            .culling
            .hold
            .as_ref()
            .unwrap()
            .cache
            .get(1)
            .is_none(),
        "b's camera picture let go"
    );
    decode_asked(&app, 0);
    assert_eq!(stand_in(&state, 0), Some(cull::Origin::Edit));
    develop(&app);

    // Camera JPEG again.
    app.invoke_cull_source_asked(false);
    assert_eq!(seen(&app, &state), Seen::CullCamera);
    decode_asked(&app, 0);
    let (from, warmth) = shown(&state);
    assert_eq!(from, cull::Origin::Camera, "the camera's picture again");
    assert!(warmth > 100.0, "in color: {warmth:.0}");

    // The develop view's stand-in for a is the edit's picture, and
    // one still on its way when Cull came does not go up there.
    let stand_in = cull::Want::of_file(0, files[0].clone(), 1024);
    let got = std::cell::RefCell::new(Vec::new());
    cull::fetch(&stand_in, Some(&previews), &|l| got.borrow_mut().push(l));
    let got = got.into_inner();
    assert!(
        matches!(&got[..], [cull::Loaded::Ok { preview, .. }] if preview.edited),
        "the stand-in is the edit's"
    );
    for loaded in got {
        deliver_preview(&app, loaded);
    }
    assert_eq!(shown(&state).0, cull::Origin::Camera, "still the camera's");
    crate::testing::remove_scratch(state, &dir);
}

/// Into culling with the repair tool in hand: the tool is put down
/// and its hint, with its Esc, goes off the status line.
#[test]
fn culling_entered_with_a_repair_in_hand_drops_its_hint() {
    let app = window(1);
    let (state, _worker) = state_for(&app, folder(1));
    app.invoke_select(0);
    app.invoke_retouch_toggled("Clone".into());
    assert_eq!(app.get_status(), crate::panel::retouch::retouch_hint());
    enter_cull(&mut state.borrow_mut(), &app, 1);
    assert_eq!(app.get_retouch_mode(), "");
    assert_eq!(app.get_status(), "");
}

/// Move back of the frame on screen: its file moves out of the list
/// (and back again in the bookkeeping), and the frame chosen after
/// it starts a visit of its own, with no earlier path, no open again
/// due, and no open but its own.
#[test]
fn move_back_of_the_frame_on_screen_leaves_no_move_behind() {
    let (dir, _shoot, files) = shoot_with_rejects("on-screen");
    let app = window(files.len());
    let (state, worker) = opened_with_sidecars(&app, &files);
    app.invoke_select(1);
    app.invoke_move_back_asked();
    {
        let st = state.borrow();
        assert!(!st.files.contains(&files[1]));
        assert!(st.develop.moved_from.is_none());
        assert!(!st.develop.reopen_due);
        let on_screen = st.current.and_then(|c| st.path_at(c)).cloned();
        if let Some((_, (sent, _))) = &st.develop.open_sent {
            assert_eq!(Some(sent), on_screen.as_ref());
        }
    }
    drop(state);
    drop(worker);
    crate::testing::remove_dir_retry(&dir);
}

/// A camera picture of a frame in the loupe's cache, `source` its
/// size as the file's tag turns it, with its focus point told. Out
/// of Cull, it is the picture standing in for the frame's develop.
pub(crate) fn focus_frame(
    st: &mut State,
    file: usize,
    source: (u32, u32),
    point: Option<(
        greycard_core::raw::FocusPoint,
        greycard_core::raw::Orientation,
    )>,
) {
    let preview = Arc::new(cull::Preview::new(8, 6, vec![0; 192], source, false));
    if st.culling.cull.is_none() {
        let generation = st.develop.generation;
        let hold = st
            .culling
            .hold
            .get_or_insert_with(|| Cull::new("".into(), 2048));
        hold.cache.insert(file, preview);
        st.culling.placeholder = Some(placeholder::Wait::in_hand(file, generation));
        crate::focus_points::tell(&st.path_of(file).clone(), point);
        return;
    }
    let cull = st.culling.cull.as_mut().expect("culling");
    cull.cache.insert(file, preview);
    crate::focus_points::tell(&st.path_of(file).clone(), point);
}

fn near(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5
}

/// In Cull, from the camera's JPEG: Space and Z from fit go to each
/// frame's own point as the frames are stepped through, through the
/// file's tag, the frame's quarter turns, and its mirror, and never
/// through the edit's crop or straightening, which the camera's
/// picture is not drawn by; a frame with no point zooms about the
/// middle as before.
#[test]
fn the_zoom_key_in_cull_goes_to_each_frames_own_point() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let app = window(5);
    let names = (0..5).map(|i| format!("/nowhere/focus-cull/IMG_{i:04}.CR3").into());
    let (state, _worker) = state_for(&app, names.collect());
    app.invoke_select(0);
    {
        let mut st = state.borrow_mut();
        enter_cull(&mut st, &app, 1);
    }
    // Right of and above the middle of the sensor.
    let point = FocusPoint {
        area: [0.7, 0.2, 0.1, 0.1],
        aspect: 1.5,
    };
    {
        let mut st = state.borrow_mut();
        // A landscape frame; a portrait one, turned by its tag; one
        // the user turned a quarter; one mirrored and cropped and
        // straightened in its edit; and one with no point.
        focus_frame(&mut st, 0, (6000, 4000), Some((point, Orientation::Normal)));
        focus_frame(
            &mut st,
            1,
            (4000, 6000),
            Some((point, Orientation::Rotate270)),
        );
        focus_frame(&mut st, 2, (6000, 4000), Some((point, Orientation::Normal)));
        st.sidecars[2].turn = 1;
        focus_frame(&mut st, 3, (6000, 4000), Some((point, Orientation::Normal)));
        let g = &mut st.sidecars[3].line.current.geometry;
        g.flip = true;
        g.angle = 4.0;
        g.crop = Some(greycard_edit::geometry::Crop {
            x: 0.5,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        });
        focus_frame(&mut st, 4, (6000, 4000), None);
    }
    let expected = [
        Some((4500.0, 1000.0)),
        // (u, v) to (v, 1 - u) on the 4000 by 6000 picture.
        Some((1000.0, 1500.0)),
        // A quarter clockwise on screen: (u, v) to (1 - v, u).
        Some((3000.0, 4500.0)),
        // Mirrored left to right, crop and angle not counted.
        Some((1500.0, 1000.0)),
        None,
    ];
    for level in [1.0, 3.0] {
        for (file, want) in expected.iter().enumerate() {
            app.invoke_select(file as i32);
            let middle = {
                let mut st = state.borrow_mut();
                st.develop.zoom = 0.0;
                st.develop.center = (1234.0, 567.0);
                st.develop.center
            };
            app.invoke_zoom_key(level);
            let st = state.borrow();
            assert_eq!(st.develop.zoom, level, "frame {file}");
            let at = st.develop.center;
            match want {
                Some(w) => assert!(near(at, *w), "frame {file} at {level}: {at:?}"),
                None => assert!(near(at, middle), "frame {file}: {at:?}"),
            }
        }
    }
}

/// Waits for the head read of `path` off the window's thread.
fn read_landed(path: &Path) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while crate::focus_points::known(path).is_none() {
        assert!(
            std::time::Instant::now() < until,
            "the head read never came"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// A Cull window on `names` written in a scratch folder, each with
/// its bytes, the loupe on the first frame.
fn cull_on_files(
    what: &str,
    files: &[&[u8]],
) -> (App, Rc<Shared>, Rc<crate::worker::Worker>, PathBuf) {
    let dir = crate::testing::scratch_dir(what);
    let names: Vec<PathBuf> = files
        .iter()
        .enumerate()
        .map(|(i, bytes)| {
            let path = dir.join(format!("IMG_{i:04}.CR3"));
            std::fs::write(&path, bytes).unwrap();
            path
        })
        .collect();
    let app = window(files.len());
    let (state, worker) = state_for(&app, names);
    app.invoke_select(0);
    enter_cull(&mut state.borrow_mut(), &app, 1);
    (app, state, worker, dir)
}

const SEED: &[u8] = include_bytes!("../../../../../fuzz/corpus/focus_point/seed-cr3-two-in-focus");

/// In Cull, a frame whose picture came from the local previews has
/// its file's head read when the key is pressed, off the window's
/// thread; the point coming in takes the key's untouched zoom there,
/// and leaves a panned one be. The seed's point, turned by its tag
/// (Rotate270), is at (2400, 3900) of the 4000 by 6000 picture.
#[test]
fn a_point_read_late_in_cull_takes_an_untouched_key_zoom_there() {
    let (app, state, _worker, dir) = cull_on_files("focus-late", &[SEED, SEED]);
    for (file, panned) in [(0, false), (1, true)] {
        app.invoke_select(file as i32);
        let path = {
            let mut st = state.borrow_mut();
            let preview = Arc::new(cull::Preview::new(8, 6, vec![0; 192], (4000, 6000), false));
            st.culling
                .cull
                .as_mut()
                .unwrap()
                .cache
                .insert(file, preview);
            st.develop.image_size = (4000, 6000);
            st.develop.zoom = 0.0;
            st.develop.center = (2000.0, 3000.0);
            st.path_of(file).clone()
        };
        assert!(crate::focus_points::known(&path).is_none());
        app.invoke_zoom_key(1.0);
        assert_eq!(state.borrow().develop.center, (2000.0, 3000.0));
        assert!(state.borrow().develop.focus_pending.is_some());
        if panned {
            state.borrow_mut().develop.center = (1000.0, 3000.0);
        }
        read_landed(&path);
        let mut st = state.borrow_mut();
        crate::panel::viewport::focus_arrived(&mut st, &app);
        let want = if panned {
            (1000.0, 3000.0)
        } else {
            (2400.0, 3900.0)
        };
        assert!(near(st.develop.center, want), "{:?}", st.develop.center);
        assert!(st.develop.focus_pending.is_none());
    }
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// A head read that finds no point answers the press: the wait is
/// dropped and the view stays about the middle. Leaving Cull drops
/// a wait made in it, and a compare view never waits.
#[test]
fn a_wait_in_cull_ends_with_its_answer_a_mode_switch_or_never_starts() {
    let not_a_raw: &[u8] = b"not a raw at all";
    let (app, state, _worker, dir) = cull_on_files("focus-answered", &[not_a_raw, SEED, SEED]);
    let pending = || state.borrow().develop.focus_pending.clone();
    let at_fit = |file: usize, in_hand: bool| {
        let mut st = state.borrow_mut();
        if in_hand {
            let preview = Arc::new(cull::Preview::new(8, 6, vec![0; 192], (6000, 4000), false));
            st.culling
                .cull
                .as_mut()
                .unwrap()
                .cache
                .insert(file, preview);
        }
        st.develop.image_size = (6000, 4000);
        st.develop.zoom = 0.0;
        st.develop.center = (3000.0, 2000.0);
        st.path_of(file).clone()
    };

    // No point in the file: answered, dropped, about the middle.
    let path = at_fit(0, true);
    app.invoke_zoom_key(1.0);
    assert!(pending().is_some());
    read_landed(&path);
    {
        let mut st = state.borrow_mut();
        crate::panel::viewport::focus_arrived(&mut st, &app);
        assert!(st.develop.focus_pending.is_none());
        assert_eq!(st.develop.center, (3000.0, 2000.0));
    }
    // Pressed again, the answer known: no wait at all.
    state.borrow_mut().develop.zoom = 0.0;
    app.invoke_zoom_key(1.0);
    assert!(pending().is_none());

    // A wait made in Cull, its picture not in hand, and Cull left.
    app.invoke_select(1);
    at_fit(1, false);
    app.invoke_zoom_key(1.0);
    assert!(pending().is_some_and(|p| p.culling));
    {
        let mut st = state.borrow_mut();
        let left = st.culling.cull.take();
        crate::panel::viewport::focus_arrived(&mut st, &app);
        assert!(st.develop.focus_pending.is_none(), "dropped on leaving");
        st.culling.cull = left;
    }

    // A compare view never waits.
    app.invoke_select(2);
    at_fit(2, false);
    state.borrow_mut().culling.cull.as_mut().unwrap().compare = 2;
    app.invoke_zoom_key(1.0);
    assert!(pending().is_none());
    state.borrow_mut().culling.cull.as_mut().unwrap().compare = 1;
    // Let the reads started here land before the folder goes.
    read_landed(&state.borrow().path_of(1).clone());
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// The camera's cut: a frame the camera framed comes cut to its
/// frame, and the point is taken into the cut; one outside it is
/// none.
#[test]
fn the_point_in_cull_is_taken_into_the_cameras_cut() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let app = window(2);
    let names = (0..2).map(|i| format!("/nowhere/focus-cut/P{i:04}.ORF").into());
    let (state, _worker) = state_for(&app, names.collect());
    app.invoke_select(0);
    let mut st = state.borrow_mut();
    enter_cull(&mut st, &app, 1);
    let path = st.path_of(0).clone();
    let area = greycard_core::raw::Rect {
        x: 0,
        y: 0,
        width: 4000,
        height: 3000,
    };
    // The middle 3:2 band of a 4:3 sensor.
    let frame = greycard_core::raw::CameraFrame {
        area,
        frame: greycard_core::raw::Rect {
            x: 0,
            y: 166,
            width: 4000,
            height: 2667,
        },
        orientation: Orientation::Normal,
    };
    crate::edited_pictures::note_decoded(&path, Some(frame));
    let mut preview = cull::Preview::new(8, 6, vec![0; 192], (4000, 2667), false);
    preview.cut = true;
    st.culling
        .cull
        .as_mut()
        .unwrap()
        .cache
        .insert(0, Arc::new(preview));
    let at = |st: &State, area: [f32; 4]| {
        // Of the 4:3 sensor, which the cut makes 3:2.
        let point = FocusPoint {
            area,
            aspect: 4.0 / 3.0,
        };
        crate::focus_points::tell(&path, Some((point, Orientation::Normal)));
        loupe_focus_center(st, &app)
    };
    // At three quarters down the sensor: (2250 - 166) of the cut.
    let c = at(&st, [0.45, 0.7, 0.1, 0.1]).unwrap();
    assert!(near(c, (2000.0, 2084.0)), "{c:?}");
    assert_eq!(at(&st, [0.45, 0.0, 0.1, 0.02]), None, "above the cut");
}

/// A camera JPEG beside the raw of another shape than the picture
/// the point is of (shot at 16:9, 1:1, or 4:3 beside a 3:2 point) is
/// not the one the fractions are of: no point. One of the point's
/// shape, turned by the tag or within 1%, takes it.
#[test]
fn a_paired_jpeg_of_another_shape_has_no_point() {
    use greycard_core::raw::{FocusPoint, Orientation};
    let app = window(2);
    let names = (0..2).map(|i| format!("/nowhere/focus-shape/IMG_{i:04}.CR3").into());
    let (state, _worker) = state_for(&app, names.collect());
    app.invoke_select(0);
    let mut st = state.borrow_mut();
    enter_cull(&mut st, &app, 1);
    let path = st.path_of(0).clone();
    let point = FocusPoint {
        area: [0.45, 0.45, 0.1, 0.1],
        aspect: 1.5,
    };
    let at = |st: &mut State, source: (u32, u32), tag: Orientation| {
        let mut preview = cull::Preview::new(8, 6, vec![0; 192], source, false);
        preview.companion = true;
        st.culling
            .cull
            .as_mut()
            .unwrap()
            .cache
            .insert(0, Arc::new(preview));
        crate::focus_points::tell(&path, Some((point, tag)));
        loupe_focus_center(st, &app)
    };
    for source in [(6000, 3376), (4000, 4000), (5328, 4000)] {
        assert_eq!(at(&mut st, source, Orientation::Normal), None, "{source:?}");
    }
    assert_eq!(at(&mut st, (6000, 4000), Orientation::Rotate90), None);
    let c = at(&mut st, (6000, 4000), Orientation::Normal).unwrap();
    assert!(near(c, (3000.0, 2000.0)), "{c:?}");
    assert!(at(&mut st, (6000, 3990), Orientation::Normal).is_some());
    assert!(at(&mut st, (4000, 6000), Orientation::Rotate270).is_some());
}
