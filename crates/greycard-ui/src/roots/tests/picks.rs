//! Tests of picks and culling while reads are out, and of the keys
//! pressed meanwhile.

use super::*;

/// A slider moved over a pick whose sidecar is still being read is
/// refused and said: the stand-in takes nothing, so the read's
/// landing loses nothing and the sidecar on disk is as it was. So
/// is one moved after the read was given up on.
#[test]
fn an_edit_during_a_picks_read_is_refused_and_said() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-pick-edit");
    let on_disk = std::fs::read(Sidecar::path_for(&files[1])).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    let moved = {
        let mut e = state.borrow().sidecars[1].current.clone();
        e.light.exposure += 1.0;
        e
    };
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.views.pick_pending, Some(files[1].clone()));
        assert!(crate::panel::edit::save_edit(&mut st, moved.clone()));
        assert_eq!(
            crate::rows::not_kept(&st, 1).as_deref(),
            Some("y.tif's sidecar is still being read: the change is not kept")
        );
        assert!(st.sidecars[1].history.is_empty(), "nothing on the stand-in");
        // Unmoved, nothing to say.
        let stand_in = st.sidecars[1].current.clone();
        assert!(!crate::panel::edit::save_edit(&mut st, stand_in));
    }
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert!(st.from_row[1].read);
        assert_eq!(st.sidecars[1].history.len(), 1, "the sidecar's own");
        assert_eq!(st.sidecars[1].current.geometry.turns, 1);
        assert_eq!(
            st.develop.edit, st.sidecars[1].current,
            "the panel is the sidecar's"
        );
    }
    assert_eq!(
        std::fs::read(Sidecar::path_for(&files[1])).unwrap(),
        on_disk,
        "nothing written over it"
    );
    // A pick whose read was given up on stands in still: refused,
    // in other words.
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 2, false);
        st.views.pick_pending = None;
        assert!(crate::panel::edit::save_edit(&mut st, moved));
        assert_eq!(
            crate::rows::not_kept(&st, 2).as_deref(),
            Some("z.tif's sidecar did not come in: the change is not kept")
        );
        assert!(st.sidecars[2].history.is_empty());
    }
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// `--show-mask` reaches the first file when the index stands it
/// in: the open waits for its sidecar (`pick_pending`), the frame
/// is current by the time the read lands and opens it, and that
/// open is still the first.
#[test]
fn the_command_lines_mask_reaches_a_first_file_read_after_its_pick() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-pick-show-mask");
    let mut s = Sidecar::load(&files[1]).unwrap().unwrap();
    let mut e = s.current.clone();
    e.adjustments.push(greycard_edit::Adjustment::default());
    s.record(e);
    s.save(&files[1]).unwrap();
    state.borrow_mut().launch.show_mask = Some(0);
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().current, None, "nothing opened at the start");
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert_eq!(state.borrow().views.pick_pending, Some(files[1].clone()));
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.current, Some(1));
        assert_eq!(
            st.develop.edit.adjustments.len(),
            1,
            "the sidecar's own edit"
        );
        assert_eq!(st.develop.target, Some(0), "the mask asked for is chosen");
        assert_eq!(st.launch.show_mask, None, "and taken");
    }
    assert!(app.get_show_mask());
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A pick in culling whose sidecar is slow to come: the mode's own
/// line takes the status over meanwhile, and the bar still stops
/// when the read lands.
#[test]
fn a_pick_in_culling_stops_the_bar_when_its_read_lands() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-pick-cull-bar");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::cull::enter_cull(&mut state.borrow_mut(), &app, 1);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert_eq!(state.borrow().views.pick_pending, Some(files[1].clone()));
    assert!(app.get_busy());
    app.set_status("y.tif: waiting for its preview".into());
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    assert_eq!(state.borrow().current, Some(1));
    assert!(state.borrow().views.pick_pending.is_none());
    assert!(!app.get_busy(), "the bar stops with the read");
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// An export pressed on a frame whose sidecar is being read, the
/// user moving on to another before it lands, exports the pressed
/// frame, as a set of one under its own edit, and not the frame on
/// screen when the read came in.
#[test]
fn an_export_is_of_the_frame_it_was_pressed_on() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-export-source");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    crate::rows::land_pending(&state, &app, &worker);
    assert!(state.borrow().from_row[2].read);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert_eq!(state.borrow().views.pick_pending, Some(files[1].clone()));
    app.invoke_export();
    assert_eq!(
        state.borrow().views.loads.whats(),
        ["open of y.tif", "export"]
    );
    // z is loaded: open at once, the export still waiting.
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    assert_eq!(state.borrow().current, Some(2));
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(st.current, Some(2), "the export opened nothing");
    // A set of one, into the folder the sheet shows for it.
    let set = st.work.exporting.clone().expect("the set");
    assert_eq!(set.total, 1);
    assert_eq!(set.folder.as_deref(), files[1].parent());
    set.cancel();
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// An export asked for before culling, its request waiting on a
/// frame of the set that stands in, and landing in culling on that
/// frame: the panel is no frame's there, so each frame of the set
/// takes its own sidecar's edit, and the one now current is not
/// given the panel's (the frame the mode was entered from).
/// Through Add to queue, whose entry says what each frame took.
#[test]
fn an_export_landing_in_culling_takes_each_frames_own_edit() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-export-cull");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    // z open, its panel at 2.0.
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(state.borrow().current, Some(2));
    app.set_exposure(2.0);
    // y and z chosen, y standing in still: the queue's request
    // waits on its read.
    {
        let mut st = state.borrow_mut();
        st.views.picked = vec![1, 2];
        assert!(!st.from_row[1].read);
    }
    state.borrow_mut().work.export_last_folder = Some(dir.join("out"));
    app.invoke_export_queue_add();
    assert!(state.borrow().work.export_queue.is_empty(), "waiting");
    // Into culling, on to y; then the read lands.
    {
        let mut st = state.borrow_mut();
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        crate::panel::cull::cull_select(&mut st, &app, 1);
        assert_eq!(st.current, Some(1));
    }
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(st.work.export_queue.len(), 1);
    let got: Vec<(PathBuf, f32, u8)> = st.work.export_queue[0]
        .frames
        .iter()
        .map(|f| {
            (
                f.source.clone(),
                f.edit.light.exposure,
                f.edit.geometry.turns,
            )
        })
        .collect();
    let y = &st.sidecars[1].current;
    assert_eq!(
        got,
        vec![
            (files[1].clone(), y.light.exposure, 1),
            (files[2].clone(), 2.0, 0),
        ],
        "y under its own edit, z under the one saved on the way in"
    );
    assert_ne!(y.light.exposure, 2.0);
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A sidecar the index says holds a develop, and that cannot be
/// read when its frame is opened, or is not there, is not replaced
/// by the default: the frame stands in still, the status line says
/// why, and neither a slider nor a key writes over the file.
#[test]
fn a_failed_read_keeps_the_stand_in() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-failed-read");
    let gcd = Sidecar::path_for(&files[1]);
    std::fs::write(&gcd, b"{ not a sidecar").unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.current, Some(1));
        assert!(st.views.pick_pending.is_none());
        assert!(!st.from_row[1].read, "stands in still");
        assert!(st.from_row[1].unread.is_some());
        assert!(
            app.get_status()
                .starts_with("y.tif's sidecar could not be read"),
            "{}",
            app.get_status()
        );
        assert_eq!(st.sidecars[1].meta.keywords, ["Harbor"], "the row's meta");
        let mut moved = st.sidecars[1].current.clone();
        moved.light.exposure += 1.0;
        assert!(crate::panel::edit::save_edit(&mut st, moved));
        assert_eq!(
            crate::rows::not_kept(&st, 1).as_deref(),
            Some("y.tif's sidecar could not be read: the change is not kept")
        );
    }
    // A key reads it again, fails again, and writes nothing.
    crate::panel::browser::meta_on_selection(&state, &app, &worker, meta::Change::Rating(5));
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(std::fs::read(&gcd).unwrap(), b"{ not a sidecar");
    assert!(!state.borrow().from_row[1].read);
    // Not there at all, though the row says there is a develop:
    // the same.
    std::fs::remove_file(&gcd).unwrap();
    {
        let mut st = state.borrow_mut();
        st.from_row[1].unread = None;
        assert!(!crate::rows::load_frame(&mut st, &app, 1));
        assert!(!st.from_row[1].read);
        assert!(
            app.get_status().contains("was not found"),
            "{}",
            app.get_status()
        );
    }
    assert!(!gcd.exists(), "nothing written in its place");
    // A frame the row says has no develop and no sidecar is read as
    // the default, as it always was.
    {
        let mut st = state.borrow_mut();
        assert!(crate::rows::load_frame(&mut st, &app, 2));
        assert!(st.from_row[2].read);
    }
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A pick whose read was started under a list a view's read has
/// since moved on from (the generation bumped) is not opened on the
/// stand-in as if offline: its sidecar is asked for again, and the
/// frame opens on it.
#[test]
fn a_pick_whose_read_was_discarded_is_read_again() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-pick-again");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.views.pick_pending, Some(files[1].clone()));
        // A view's read asked for meanwhile.
        st.views.view_generation += 1;
    }
    assert_eq!(
        crate::rows::land_pending(&state, &app, &worker),
        2,
        "discarded, then read again"
    );
    let st = state.borrow();
    assert_eq!(st.current, Some(1));
    assert!(st.views.pick_pending.is_none());
    assert!(st.from_row[1].read);
    assert_eq!(st.sidecars[1].history.len(), 1);
    assert_eq!(st.develop.edit, st.sidecars[1].current);
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A pick whose read is given up on, then brought in by a key's
/// read, is opened on its own sidecar: the panel takes the frame's
/// edit (its quarter turn), and the next slider moves from it
/// rather than writing the stand-in's default over it.
#[test]
fn a_frame_loaded_after_the_give_up_takes_its_own_edit_onto_the_panel() {
    use crate::panel::edit::{read_edit, save_edit};
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-late-load");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    {
        let mut st = state.borrow_mut();
        let mut e = read_edit(&app, &st.develop.edit, st.develop.target);
        e.light.exposure = 0.8;
        assert!(save_edit(&mut st, e), "refused while the read is out");
    }
    let since = state.borrow().views.loads.since.expect("a read out");
    crate::rows::bar_tick(&state, &app, &worker, since + Duration::from_secs(120));
    assert!(state.borrow().views.pick_pending.is_none(), "given up on");
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(2)
    ));
    crate::rows::land_pending(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.current, Some(1));
        assert!(st.from_row[1].read);
        assert!(st.views.panel_stand_in.is_none());
        assert_eq!(st.develop.edit.geometry.turns, 1, "the frame's own edit");
        let mut e = read_edit(&app, &st.develop.edit, st.develop.target);
        assert_eq!(e.geometry.turns, 1);
        e.light.exposure = 0.8;
        assert!(!save_edit(&mut st, e), "kept");
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(on_disk.current.geometry.turns, 1);
    assert_eq!(on_disk.current.light.exposure, 0.8);
    assert_eq!(on_disk.history.len(), 2, "its own step, then the slider");
    assert_eq!(on_disk.meta.rating, 2);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A preset pressed on a frame whose read was given up on loads the
/// frame there and then: the panel takes its own edit first, and the
/// preset goes over that, with no stand-in's default recorded.
#[test]
fn a_preset_on_a_given_up_pick_goes_over_its_own_edit() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-late-preset");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    let since = state.borrow().views.loads.since.expect("a read out");
    crate::rows::bar_tick(&state, &app, &worker, since + Duration::from_secs(120));
    let bright = {
        let mut e = greycard_edit::Edit::for_picture();
        e.light.exposure = 1.0;
        greycard_edit::preset::Preset::from_edit(
            "Bright",
            &e,
            &[greycard_edit::preset::Section::Light],
        )
    };
    crate::panel::sync::apply_preset_to(
        &mut state.borrow_mut(),
        &app,
        &worker,
        &bright,
        &[1],
        &[],
        |_, _| None,
    );
    {
        let st = state.borrow();
        assert!(st.from_row[1].read);
        assert!(st.views.panel_stand_in.is_none());
        assert_eq!(
            st.sidecars[1].current.geometry.turns, 1,
            "its own turn kept"
        );
        assert_eq!(st.sidecars[1].current.light.exposure, 1.0);
        assert_eq!(
            st.sidecars[1].history.len(),
            2,
            "its own step, then the preset"
        );
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(on_disk.current.geometry.turns, 1);
    assert_eq!(on_disk.history.len(), 2);
    crate::rows::land_pending(&state, &app, &worker);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Export on a frame on screen whose panel is the stand-in's (its
/// sidecar could not be read) is refused and said: nothing is sent
/// under the default edit while the worker holds another frame's
/// picture.
#[test]
fn an_export_of_a_stand_in_on_screen_is_refused() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-export-stand-in");
    std::fs::write(Sidecar::path_for(&files[1]), b"{ not a sidecar").unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(
        state.borrow().views.panel_stand_in.as_deref(),
        Some(files[1].as_path())
    );
    app.invoke_export();
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(
        app.get_status(),
        "y.tif's sidecar could not be read: nothing is exported"
    );
    assert!(!state.borrow().work.export_choosing);
    assert!(!app.get_busy());
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Auto on a frame on screen whose panel is the stand-in's (its
/// sidecar could not be read) is refused and said, as a preset is:
/// the stand-in's default edit with Auto over it is never recorded
/// as the frame's state, even with the stand-in's develop up.
#[test]
fn auto_light_on_a_stand_in_on_screen_is_refused() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-auto-stand-in");
    std::fs::write(Sidecar::path_for(&files[1]), b"{ not a sidecar").unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    crate::rows::land_pending(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.views.panel_stand_in.as_deref(), Some(files[1].as_path()));
        // The stand-in's develop up, as a raw's, so only the guard
        // stands between the press and a recorded state.
        let turn = crate::panel::edit::current_turn(&st);
        st.develop.shown_turn = Some((1, turn));
        st.develop.drawn_kind = crate::finish::Source::SCENE;
    }
    let grid: Vec<[f32; 3]> = (0..4000)
        .map(|i| [0.18 * 2f32.powf(-6.0 + 4.0 * i as f32 / 4000.0); 3])
        .collect();
    crate::panel::viewport::AUTO_WHITE_GRID.with(|g| *g.borrow_mut() = Some((grid, 40)));
    app.invoke_light_auto();
    crate::panel::viewport::AUTO_WHITE_GRID.with(|g| g.borrow_mut().take());
    assert_eq!(
        app.get_status(),
        "y.tif's sidecar could not be read: the change is not kept"
    );
    assert!(state.borrow().sidecars[1].history.is_empty());
    assert_eq!(
        std::fs::read(Sidecar::path_for(&files[1])).unwrap(),
        b"{ not a sidecar"
    );
    drop(writer);
    crate::testing::remove_scratch(state, &dir);
}

/// The frame on screen followed to its new name keeps its panel's
/// stand-in flag.
#[test]
fn following_the_frame_on_screen_moves_the_stand_in_flag() {
    let app = window(0);
    let files = crate::testing::folder(2);
    let (state, _worker) = state_for(&app, files.clone());
    let to = files[0].with_file_name("renamed.CR3");
    let mut st = state.borrow_mut();
    st.current = Some(0);
    st.views.panel_stand_in = Some(files[0].clone());
    let seen = OnScreen {
        path: Some(files[0].clone()),
        there: false,
        moved_to: Some(to.clone()),
    };
    assert!(follow_current(&mut st, &seen));
    assert_eq!(st.files[0], to);
    assert_eq!(st.views.panel_stand_in, Some(to));
}

/// Two roots on a window with the library open, sidecars on: `a`
/// with x (rated, flagged reject) and `b` with z (flagged reject,
/// three stars), both indexed, then `b` renamed away so it is
/// offline, and the view of the roots opened and landed: x is
/// file 0, z file 1, both standing in from their rows.
#[allow(clippy::type_complexity)]
fn offline_view(
    what: &str,
) -> (
    PathBuf,
    PathBuf,
    Vec<PathBuf>,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let x = frames(&a, &["x.tif"]).remove(0);
    let z = frames(&b, &["z.tif"]).remove(0);
    let mut s = Sidecar::default();
    s.meta.rating = 2;
    s.meta.flag = meta::Flag::Reject;
    s.save(&x).unwrap();
    let mut s = Sidecar::default();
    s.meta.rating = 3;
    s.meta.flag = meta::Flag::Reject;
    s.save(&z).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.index_path = Some(db);
        st.window.write_sidecars = true;
        st.views.library.roots.add(&a).unwrap();
        st.views.library.roots.add(&b).unwrap();
    }
    let away = dir.join("b-away");
    crate::testing::rename_away(&b, &away);
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.files, [x.clone(), z.clone()]);
        assert!(st.views.library.offline.contains(&b));
        assert!(st.from_row.iter().all(|r| !r.read));
    }
    (dir, away, vec![x, z], writer, app, state, worker)
}

/// Moving the rejects leaves a root that is offline alone: nothing
/// is made where it was (a folder made on a mount point would
/// have the next pass mark the drive's frames missing), its
/// frames stay in the list, and its rows stay as they were. The
/// online root's reject goes out as ever. A delete of the
/// selection leaves an offline frame out the same way.
#[test]
fn the_rejects_move_and_a_delete_leave_an_offline_root_alone() {
    let (dir, away, files, mut writer, app, state, worker) = offline_view("offline-rejects");
    let (a, b) = (dir.join("a"), dir.join("b"));
    let (x, z) = (files[0].clone(), files[1].clone());
    {
        let mut st = state.borrow_mut();
        assert!(crate::panel::cull::to_move_out(&st, 0));
        assert!(
            !crate::panel::cull::to_move_out(&st, 1),
            "a reject under an offline root is not one to move"
        );
        crate::panel::cull::move_rejects(&mut st, &app, &worker);
        assert_eq!(
            st.files,
            std::slice::from_ref(&z),
            "x went out; z stays, untouched"
        );
    }
    assert!(!b.exists(), "nothing made where the root was");
    assert!(
        !away.join("rejects").exists(),
        "nothing made on the drive either"
    );
    assert!(away.join("z.tif").exists());
    assert!(!x.exists() && a.join("rejects").join("x.tif").exists());
    // The pass over the online root marks nothing of the other's.
    writer.index_folder(&a, &mut |_| {}).unwrap();
    let row = writer.by_path(&z).unwrap().unwrap();
    assert!(!row.missing, "z's row as it was");
    assert_eq!(writer.count_under(&b).unwrap(), 1);
    // A delete of a set with the offline frame in it leaves it out.
    {
        let mut st = state.borrow_mut();
        st.current = Some(0);
        st.views.picked = vec![0];
        app.set_status("".into());
        crate::panel::delete::ask_delete(&mut st, &app, crate::panel::delete::Which::Selection);
        assert!(st.sheets.delete_asked.is_none(), "nothing to ask about");
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
    }
    assert!(away.join("z.tif").exists());
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A turn on a frame standing in reads its sidecar first and
/// writes the turn into it, and the thumbnail follows; a turn on
/// a frame under an offline root is refused with a word, and
/// nothing changes anywhere.
#[test]
fn a_turn_on_a_view_frame_reads_its_sidecar_then_writes_it() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-turn");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let mut st = state.borrow_mut();
        assert!(!st.from_row[0].read);
        crate::panel::browser::turn_frames(&mut st, &app, &worker, &[0], 1);
        assert!(st.from_row[0].read, "read for the turn");
        assert_eq!(st.sidecars[0].turn, 1);
        assert_eq!(st.sidecars[0].meta.rating, 3, "its own meta kept");
        assert_eq!(crate::panel::browser::thumb_turns(&st, &app, 0), (3, false));
        assert_eq!(disk_reads_under(&dir), 1);
    }
    let on_disk = Sidecar::load(&files[0]).unwrap().unwrap();
    assert_eq!(on_disk.turn, 1, "written");
    assert_eq!(on_disk.meta.rating, 3);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    let (dir, away, _files, writer, app, state, worker) = offline_view("offline-turn");
    let before = std::fs::read(away.join("z.tif.gcd")).unwrap();
    {
        let mut st = state.borrow_mut();
        app.set_status("".into());
        crate::panel::browser::turn_frames(&mut st, &app, &worker, &[1], 1);
        assert!(!st.from_row[1].read, "nothing read");
        assert_eq!(st.sidecars[1].turn, 0, "nothing turned in memory either");
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
    }
    assert_eq!(std::fs::read(away.join("z.tif.gcd")).unwrap(), before);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame under an offline root opened in the loupe shows the
/// default edit that stands in for it, not the last frame's, and
/// is no source: a copy, a sync and a preset from it are refused
/// with a word, in the loupe and in culling alike, and the other
/// frames' sidecars are left as they were.
#[test]
fn an_offline_frame_is_no_source_for_a_copy_a_sync_or_a_preset() {
    use greycard_edit::preset::{Preset, Section};
    let (dir, _away, files, writer, app, state, worker) = offline_view("offline-source");
    let x = files[0].clone();
    let x_before;
    {
        // x on screen first, with an exposure of its own on the
        // panel, so the panel holds something that is not z's.
        crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
        crate::rows::land_pending(&state, &app, &worker);
        let mut st = state.borrow_mut();
        assert_eq!(st.current, Some(0));
        assert!(st.from_row[0].read);
        st.develop.edit.light.exposure = 0.7;
        app.set_exposure(0.7);
        // Then z, under the offline root. Leaving x saves the panel's
        // exposure into x, as leaving any frame does: one step.
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.current, Some(1));
        assert!(!st.from_row[1].read);
        assert_eq!(st.sidecars[0].history.len(), 1);
        assert_eq!(st.sidecars[0].current.light.exposure, 0.7);
        x_before = std::fs::read(Sidecar::path_for(&x)).unwrap();
        // The frames are pictures: a picture's default stands in.
        assert_eq!(
            st.develop.edit,
            greycard_edit::Edit::for_picture(),
            "the stand-in's default, not x's"
        );
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
        // Copy: refused.
        app.set_status("".into());
        crate::panel::sync::copy_settings(&mut st, &app);
        assert!(st.sheets.clipboard.is_none());
        assert!(
            app.get_status().contains("nothing to copy"),
            "{}",
            app.get_status()
        );
        // Sync onto x: refused, x untouched.
        st.views.picked = vec![0, 1];
        let synced = crate::panel::sync::sync_selection(
            &mut st,
            &app,
            Section::ALL,
            &[],
            crate::panel::sync::probe,
        );
        assert!(synced.moved.is_empty());
        assert_eq!(st.sidecars[0].history.len(), 1, "x got no step");
        // A preset: refused.
        let mut e = greycard_edit::Edit::default();
        e.light.exposure = 1.5;
        let preset = Preset::from_edit("bright", &e, Section::ALL);
        crate::panel::sync::apply_preset(
            &mut st,
            &app,
            &worker,
            &preset,
            &[],
            crate::panel::sync::probe,
        );
        assert_eq!(st.sidecars[0].history.len(), 1);
        assert_eq!(st.sidecars[1].current, greycard_edit::Edit::for_picture());
        assert!(
            app.get_status().contains("is offline"),
            "{}",
            app.get_status()
        );
        // In culling the sidecar is the truth, and a stand-in is
        // not one to sync from.
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        crate::panel::cull::cull_select(&mut st, &app, 1);
        assert_eq!(st.current, Some(1));
        let synced = crate::panel::sync::sync_selection(
            &mut st,
            &app,
            Section::ALL,
            &[],
            crate::panel::sync::probe,
        );
        assert!(synced.moved.is_empty());
    }
    assert_eq!(
        std::fs::read(Sidecar::path_for(&x)).unwrap(),
        x_before,
        "x's sidecar as it was"
    );
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Culling's compare tiles and the loupe's stand-in read a frame's
/// turns from its row while its sidecar stands in.
#[test]
fn the_culling_tiles_turn_a_frame_by_its_row() {
    let (dir, _files, writer, app, state, worker) = rated_under_a_root("rows-tiles");
    open_view(&state, &app, &worker, View::Roots(None));
    assert_eq!(land_all(&state, &app, &worker), 1);
    let st = state.borrow();
    assert!(!st.from_row[1].read);
    assert_eq!(
        crate::panel::cull::thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, 1, &app, false),
        (1, false),
        "y's edit turns it once, says the row"
    );
    assert_eq!(
        crate::panel::cull::thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, 1, &app, true),
        (1, false),
        "and not the panel, which is not y's"
    );
    assert_eq!(
        crate::panel::cull::thumb_turns_of(&st.sidecars, &st.from_row, &st.cells, 0, &app, false),
        (0, false)
    );
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A rating another tool wrote to an XMP beside the frame, with no
/// `.gcd` at all, is in the row and so in a list built from the
/// rows: the badge and the filter see it before the frame is ever
/// opened, as they did when every sidecar was read.
#[test]
fn an_xmp_s_rating_shows_in_a_list_built_from_the_rows() {
    use greycard_edit::xmp;
    let dir = scratch("rows-xmp");
    let files = frames(&dir, &["a.tif", "b.tif"]);
    let three = Meta {
        rating: 3,
        ..Meta::default()
    };
    std::fs::write(xmp::short_path(&files[0]), xmp::fresh(&three, None)).unwrap();
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_folder(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.views.index_path = Some(db.clone());
    }
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Files(files.clone()),
        Select::Row(0),
    );
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let mut st = state.borrow_mut();
        assert!(st.from_row.iter().all(|r| !r.read), "from the rows");
        assert_eq!(
            st.sidecars[0].meta.rating, 3,
            "the XMP's rating, through the row"
        );
        assert_eq!(st.sidecars[1].meta.rating, 0);
        assert_eq!(app.get_thumbs().row_data(0).unwrap().rating, 3);
        st.views.filter = filter::Filter {
            text: "rating>=3".into(),
            ..filter::Filter::default()
        };
        rebuild_browser(&mut st, &app);
        assert_eq!(st.views.shown, [0], "the filter sees it");
        // As the loader gives it, on opening.
        let (loaded, _) = crate::panel::browser::load_sidecar(&files[0], true);
        assert_eq!(loaded.meta, st.sidecars[0].meta);
    }
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// `n` bare frames under one root, indexed, the view of the roots
/// opened and landed on a window with the library open and sidecars
/// on: every frame standing in from its row.
#[allow(clippy::type_complexity)]
fn many_under_a_root(
    what: &str,
    n: usize,
) -> (
    PathBuf,
    Vec<PathBuf>,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let a = dir.join("a");
    std::fs::create_dir_all(&a).unwrap();
    let names: Vec<String> = (0..n).map(|i| format!("f{i:02}.tif")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let files = frames(&a, &names);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        // By name over the folders: the frames are told apart by their
        // place in the list this built, in that order.
        st.views.sort = crate::sort::Kept::by_name();
        st.views.index_path = Some(db);
        st.window.write_sidecars = true;
        st.views.library.roots.add(&a).unwrap();
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files, files);
    (dir, files, writer, app, state, worker)
}

/// The frame's whole sidecar in memory after its pick landed.
fn open_and_land(state: &Rc<RefCell<State>>, app: &App, worker: &Rc<Worker>, row: i32) {
    crate::panel::browser::open_row(&mut state.borrow_mut(), app, worker, row, false);
    crate::rows::land_pending(state, app, worker);
}

/// An arrow onto a frame whose sidecar is on its way, then a key:
/// the frame is current from the arrow, so the key is a request on
/// it alone, queued behind its open, and applied once it is open.
/// The frame that was current before is left byte for byte as it
/// was; nothing of the key reaches it.
#[test]
fn a_key_during_a_pick_s_read_reaches_the_pick_alone() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-arrow-key");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    open_and_land(&state, &app, &worker, 0);
    let x_before = std::fs::read(Sidecar::path_for(&files[0])).unwrap();
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.current, Some(0));
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.current, Some(1), "current from the pick");
        assert_eq!(st.views.picked, [1]);
        assert!(!st.from_row[1].read);
    }
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(5)
    ));
    {
        let st = state.borrow();
        assert_eq!(
            st.sidecars[1].meta.rating, 0,
            "not yet: queued behind the open"
        );
        assert_eq!(st.sidecars[0].meta.rating, 3, "x untouched");
        assert_eq!(st.views.loads.queue.len(), 2, "the open, then the key");
    }
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_eq!(st.current, Some(1));
        assert!(st.from_row[1].read);
        assert!(st.views.pick_pending.is_none());
        assert_eq!(st.sidecars[1].meta.rating, 5, "the key, after the open");
        assert_eq!(st.sidecars[0].meta.rating, 3);
        assert!(st.views.loads.queue.is_empty());
    }
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 5);
    assert_eq!(
        std::fs::read(Sidecar::path_for(&files[0])).unwrap(),
        x_before,
        "the frame before the pick, as it was"
    );
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A preset pressed on fifty frames whose sidecars are still to be
/// read, then a click on a loaded frame: the preset lands on the
/// fifty it was pressed on, not on the frame on screen by then.
#[test]
fn a_preset_lands_on_the_frames_it_was_pressed_on() {
    use greycard_edit::preset::{Preset, Section};
    let (dir, _files, writer, app, state, worker) = many_under_a_root("rows-preset-set", 52);
    // Frame 51 loaded, then frame 0 on screen with 0..50 chosen.
    open_and_land(&state, &app, &worker, 51);
    open_and_land(&state, &app, &worker, 0);
    state.borrow_mut().views.picked = (0..50).collect();
    let mut e = greycard_edit::Edit::for_picture();
    e.light.exposure = 1.5;
    let preset = Preset::from_edit("bright", &e, Section::ALL);
    crate::panel::sync::preset_pressed(&state, &app, &worker, preset);
    assert!(state.borrow().views.loads.reading(), "49 sidecars to read");
    // Meanwhile: the loaded frame clicked, which opens at once.
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 51, false);
    assert_eq!(state.borrow().current, Some(51));
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    let st = state.borrow();
    for f in 0..50 {
        assert_eq!(st.sidecars[f].history.len(), 1, "frame {f} took the preset");
        assert_eq!(st.sidecars[f].current.light.exposure, 1.5, "frame {f}");
    }
    assert_eq!(st.sidecars[50].history.len(), 0, "not chosen");
    assert_eq!(
        st.sidecars[51].history.len(),
        0,
        "on screen by then, not pressed on"
    );
    assert_eq!(st.current, Some(51));
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Two keys during one read both apply, in order.
#[test]
fn two_keys_during_one_read_both_apply_in_order() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-two-keys");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    for change in [
        meta::Change::Rating(4),
        meta::Change::Flag(meta::Flag::Pick),
        meta::Change::Rating(2),
    ] {
        assert!(crate::panel::browser::meta_on_selection(
            &state, &app, &worker, change
        ));
    }
    assert_eq!(
        state.borrow().views.loads.queue.len(),
        4,
        "the open and three keys, in order"
    );
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert!(st.views.loads.queue.is_empty());
        assert_eq!(st.sidecars[1].meta.rating, 2, "the last rating stands");
        assert_eq!(st.sidecars[1].meta.flag, meta::Flag::Pick);
        assert_eq!(
            st.sidecars[1].meta.keywords,
            ["Harbor"],
            "its own meta under them"
        );
        assert_eq!(st.sidecars[0].meta.rating, 3);
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(
        (on_disk.meta.rating, on_disk.meta.flag),
        (2, meta::Flag::Pick)
    );
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A click on a frame whose sidecar is to be read, then on a loaded
/// one before it lands: the loaded one opens and stays open; the
/// first's sidecar comes in silently, and nothing jumps back.
#[test]
fn a_pick_moved_on_from_lands_silently() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-moved-on");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    open_and_land(&state, &app, &worker, 0);
    {
        let mut st = state.borrow_mut();
        crate::panel::browser::open_row(&mut st, &app, &worker, 1, false);
        assert_eq!(st.current, Some(1));
        assert_eq!(st.views.pick_pending, Some(files[1].clone()));
        // Back to x, loaded: opens at once, and the pick is over.
        crate::panel::browser::open_row(&mut st, &app, &worker, 0, false);
        assert_eq!(st.current, Some(0));
        assert!(st.views.pick_pending.is_none());
        assert!(
            !app.get_status().contains("reading"),
            "{}",
            app.get_status()
        );
    }
    let status = app.get_status();
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    let st = state.borrow();
    assert_eq!(st.current, Some(0), "no jump back");
    assert!(st.from_row[1].read, "y's sidecar in memory all the same");
    assert_eq!(st.sidecars[1].history.len(), 1, "y's own");
    assert_eq!(app.get_status(), status, "nothing said");
    assert!(st.views.loads.queue.is_empty());
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A read dropped with its list takes the bar down with it, and a
/// frames' read never touches the roots view's read: the two keep
/// their own state.
#[test]
fn a_dropped_read_clears_the_bar() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-dropped");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    let since = state.borrow().views.loads.since.expect("a read out");
    assert!(state.borrow().views.loads.timer.running());
    crate::rows::bar_tick(&state, &app, &worker, since + Duration::from_millis(300));
    assert!(app.get_loading_shown(), "the frames' bar, after the moment");
    assert_eq!(app.get_loading_line(), "Reading sidecars… 0 of 1");
    let waited = since + LOADING_WAITING_AFTER + Duration::from_millis(500);
    crate::rows::bar_tick(&state, &app, &worker, waited);
    assert_eq!(app.get_loading_line(), "Waiting on a…");
    assert!(!state.borrow().views.library.loading, "not the view's read");
    // The list replaced: the request goes, the bar with it.
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Files(vec![files[0].clone()]),
        Select::Row(0),
    );
    // The new list lands, and the requests for the old go with it.
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files, std::slice::from_ref(&files[0]));
    {
        let st = state.borrow();
        assert!(st.views.loads.queue.is_empty());
        assert!(st.views.loads.since.is_none());
        assert!(!st.views.loads.timer.running());
        assert!(st.views.pick_pending.is_none());
    }
    assert!(!app.get_loading_shown());
    // The dropped request's read lands on nothing, quietly.
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    assert!(!app.get_loading_shown());
    assert!(state.borrow().views.loads.queue.is_empty());
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A purple label, and no label, from the menu on a frame whose
/// sidecar is still to be read: the change itself rides with the
/// request, so every label reaches the frame.
#[test]
fn a_purple_label_and_no_label_reach_an_unloaded_frame() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-purple");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Label(meta::Label::Purple)
    ));
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(state.borrow().sidecars[2].meta.label, meta::Label::Purple);
    assert_eq!(
        Sidecar::load(&files[2]).unwrap().unwrap().meta.label,
        meta::Label::Purple
    );
    // Another frame, still to be read, and no label asked of it: a
    // change all the same, applied when it is in.
    {
        let mut st = state.borrow_mut();
        st.sidecars[1].meta.label = meta::Label::Red;
        st.sidecars[1].save(&files[1]).unwrap();
        // Standing in again, as a list from the rows would have it.
        st.from_row[1].read = false;
    }
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Label(meta::Label::None)
    ));
    crate::rows::land_pending(&state, &app, &worker);
    assert_eq!(state.borrow().sidecars[1].meta.label, meta::Label::None);
    assert_eq!(
        Sidecar::load(&files[1]).unwrap().unwrap().meta.label,
        meta::Label::None
    );
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// An export record for a frame the list holds as a stand-in (the
/// list replaced while the export ran) reaches the disk: the
/// sidecar is read and written as for a frame the window let go
/// of, never the stand-in.
#[test]
fn an_export_record_reaches_a_frame_that_became_a_stand_in() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-export");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    let edit = Sidecar::load(&files[1]).unwrap().unwrap().line.current;
    {
        let mut st = state.borrow_mut();
        assert!(!st.from_row[1].read, "a stand-in");
        crate::panel::history::record_export(
            &mut st,
            &app,
            &files[1],
            &edit,
            None,
            &dir.join("y.jpg"),
            None,
        );
        assert!(
            !st.from_row[1].read,
            "still a stand-in; nothing recorded on it"
        );
        assert!(st.sidecars[1].current_exports.is_empty());
    }
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(on_disk.current_exports.len(), 1);
    assert!(on_disk.current_exports[0].file.ends_with("y.jpg"));
    assert_eq!(on_disk.history.len(), 1, "its develop kept");
    drop(worker);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Culling with move-on, the frames' sidecars still to be read: each
/// key reaches the frame the user saw when pressing it, and the pick
/// moves on at the press, so a run of keys rates a run of frames and
/// ends where the user expects. Stepped when the key landed, every
/// key pressed during the first frame's read went to that frame.
#[test]
fn culling_s_move_on_steps_at_the_press_and_each_key_finds_its_frame() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-move-on");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        st.culling.cull_move_on = true;
    }
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
    assert_eq!(state.borrow().current, Some(0));
    for change in [meta::Change::Rating(4), meta::Change::Rating(2)] {
        assert!(crate::panel::browser::meta_on_selection(
            &state, &app, &worker, change
        ));
    }
    assert_eq!(state.borrow().current, Some(2), "moved on at each press");
    crate::rows::land_pending(&state, &app, &worker);
    {
        let st = state.borrow();
        assert_eq!(st.sidecars[0].meta.rating, 4, "x got the first key");
        assert_eq!(st.sidecars[1].meta.rating, 2, "y the second");
        assert_eq!(st.sidecars[2].meta.rating, 0);
        assert_eq!(st.current, Some(2));
        assert!(st.views.loads.queue.is_empty());
    }
    assert_eq!(Sidecar::load(&files[0]).unwrap().unwrap().meta.rating, 4);
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 2);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    // Ten keys on ten unread frames.
    let (dir, files, writer, app, state, worker) = many_under_a_root("rows-move-on-ten", 12);
    {
        let mut st = state.borrow_mut();
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        st.culling.cull_move_on = true;
    }
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 0, false);
    for k in 0..10u8 {
        let stars = k % 5 + 1;
        assert!(crate::panel::browser::meta_on_selection(
            &state,
            &app,
            &worker,
            meta::Change::Rating(stars)
        ));
    }
    assert_eq!(state.borrow().current, Some(10));
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    for k in 0..10u8 {
        let f = usize::from(k);
        assert_eq!(st.sidecars[f].meta.rating, k % 5 + 1, "frame {f}");
        assert_eq!(
            Sidecar::load(&files[f]).unwrap().unwrap().meta.rating,
            k % 5 + 1
        );
    }
    assert_eq!(st.sidecars[10].meta.rating, 0);
    assert_eq!(st.current, Some(10));
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A read started under one list lands nothing in the next: the
/// path may have been written meanwhile, and the older bytes would
/// be saved over the newer by the next key. The write survives, and
/// a read of the new list brings it.
#[test]
fn a_read_from_an_older_list_fills_no_stand_in_of_the_new() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-late-read");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    // The list replaced while y's read is out: the same files, from
    // their rows again.
    open_listing(
        &state,
        &app,
        &worker,
        Listing::Files(files.clone()),
        Select::Row(0),
    );
    land_all(&state, &app, &worker);
    assert!(!state.borrow().from_row[1].read);
    // y written meanwhile.
    let mut s = Sidecar::load(&files[1]).unwrap().unwrap();
    s.meta.rating = 5;
    s.save(&files[1]).unwrap();
    // The old read lands: nothing taken.
    assert_eq!(crate::rows::land_pending(&state, &app, &worker), 1);
    assert!(
        !state.borrow().from_row[1].read,
        "the older read filled nothing"
    );
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 5);
    // A key on y now: its own read, of the newer bytes, then the key.
    state.borrow_mut().current = Some(1);
    state.borrow_mut().views.picked = vec![1];
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::Pick)
    ));
    crate::rows::land_pending(&state, &app, &worker);
    let on_disk = Sidecar::load(&files[1]).unwrap().unwrap();
    assert_eq!(
        (on_disk.meta.rating, on_disk.meta.flag),
        (5, meta::Flag::Pick)
    );
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Giving up on a read names what it dropped, and a dropped pick is
/// let go: not busy, no pick pending, the stand-in shown as an
/// offline frame is.
#[test]
fn giving_up_names_what_was_dropped_and_lets_a_pick_go() {
    let (dir, _files, writer, app, state, worker) = rated_under_a_root("rows-give-up");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    for change in [meta::Change::Rating(4), meta::Change::Rating(3)] {
        assert!(crate::panel::browser::meta_on_selection(
            &state, &app, &worker, change
        ));
    }
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::Pick)
    ));
    assert!(app.get_busy());
    let since = state.borrow().views.loads.since.unwrap();
    crate::rows::bar_tick(
        &state,
        &app,
        &worker,
        since + Duration::from_secs(61) + Duration::from_secs(11),
    );
    {
        let st = state.borrow();
        assert_eq!(
            app.get_status(),
            "the open of y.tif, 2 rating keys and the flag key dropped: the sidecars did not come in"
        );
        assert!(st.views.pick_pending.is_none());
        assert!(!app.get_busy());
        assert!(st.views.loads.queue.is_empty());
        assert!(st.views.loads.since.is_none());
        assert_eq!(
            st.current,
            Some(1),
            "the frame stays shown, its stand-in on the panel"
        );
        assert_eq!(st.sidecars[1].meta.rating, 0, "nothing applied");
    }
    assert!(!app.get_loading_shown());
    // The read that was given up on lands on the same list: the
    // sidecar comes in silently, and nothing runs.
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    assert!(st.from_row[1].read);
    assert_eq!(st.sidecars[1].meta.rating, 0);
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A frame a request named that is gone from the list by the time
/// the request runs takes nothing of it, and the status line says
/// so; the others take it.
#[test]
fn a_frame_gone_before_its_request_ran_is_said() {
    let (dir, files, writer, app, state, worker) = rated_under_a_root("rows-gone");
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    open_and_land(&state, &app, &worker, 0);
    state.borrow_mut().views.picked = vec![0, 1];
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(5)
    ));
    // y moves before its read lands.
    state.borrow_mut().files[1] = dir.join("a").join("elsewhere.tif");
    crate::rows::land_pending(&state, &app, &worker);
    let st = state.borrow();
    assert_eq!(st.sidecars[0].meta.rating, 5, "x took it");
    assert_eq!(st.sidecars[1].meta.rating, 0);
    assert_eq!(
        app.get_status(),
        "y.tif moved or is gone; the rating key was not applied to it"
    );
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 0);
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Three picked frames under a root, indexed, the view opened and
/// landed, culling on with move-on and the Picks filter up: what a
/// key that unflags one of them starts from.
#[allow(clippy::type_complexity)]
fn three_picks_culling(
    what: &str,
) -> (
    PathBuf,
    Vec<PathBuf>,
    greycard_library::Library,
    App,
    Rc<RefCell<State>>,
    Rc<Worker>,
) {
    let dir = scratch(what);
    let a = dir.join("a");
    std::fs::create_dir_all(&a).unwrap();
    let files = frames(&a, &["f00.tif", "f01.tif", "f02.tif"]);
    for f in &files {
        let mut s = Sidecar::default();
        s.meta.flag = meta::Flag::Pick;
        s.save(f).unwrap();
    }
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        // By name over the folders: the frames are told apart by their
        // place in the list this built, in that order.
        st.views.sort = crate::sort::Kept::by_name();
        st.views.index_path = Some(db);
        st.window.write_sidecars = true;
        st.views.library.roots.add(&a).unwrap();
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    {
        let mut st = state.borrow_mut();
        assert_eq!(st.files, files);
        st.views.filter = filter::Filter::from_name("Picks").unwrap();
        rebuild_browser(&mut st, &app);
        assert_eq!(st.views.shown, [0, 1, 2]);
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        st.culling.cull_move_on = true;
    }
    (dir, files, writer, app, state, worker)
}

/// With move-on and the Picks filter, an unflag key on a frame ends
/// on the same frame whether the key applied at once or waited for
/// its read: in the middle of the list the next frame (the step),
/// and on the last frame the nearest one shown (§123's rule), which
/// the landing may choose there since the step had nowhere to go.
#[test]
fn a_queued_key_on_the_last_frame_ends_where_the_key_applied_now_would() {
    // At once, the frames loaded: last row.
    let (dir, _files, writer, app, state, worker) = three_picks_culling("end-now");
    for row in 0..3 {
        open_and_land(&state, &app, &worker, row);
    }
    assert_eq!(state.borrow().current, Some(2));
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::None)
    ));
    {
        let st = state.borrow();
        assert_eq!(st.views.shown, [0, 1], "f02 left the list");
        assert_eq!(st.current, Some(1), "the nearest shown, once");
    }
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    // Queued behind its read: last row.
    let (dir, _files, writer, app, state, worker) = three_picks_culling("end-queued");
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 2, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::None)
    ));
    assert_eq!(state.borrow().current, Some(2), "nowhere to step");
    crate::rows::land_pending(&state, &app, &worker);
    {
        let st = state.borrow();
        assert_eq!(st.views.shown, [0, 1]);
        assert_eq!(
            st.current,
            Some(1),
            "the same frame as the key applied at once"
        );
        assert!(st.from_row[1].read, "opened");
        assert!(st.views.pick_pending.is_none());
    }
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    // At once: the middle row.
    let (dir, _files, writer, app, state, worker) = three_picks_culling("middle-now");
    for row in 0..3 {
        open_and_land(&state, &app, &worker, row);
    }
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::None)
    ));
    {
        let st = state.borrow();
        assert_eq!(st.views.shown, [0, 2]);
        assert_eq!(st.current, Some(2), "the next frame, once");
    }
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);

    // Queued: the middle row.
    let (dir, _files, writer, app, state, worker) = three_picks_culling("middle-queued");
    crate::panel::browser::open_row(&mut state.borrow_mut(), &app, &worker, 1, false);
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Flag(meta::Flag::None)
    ));
    assert_eq!(state.borrow().current, Some(2), "stepped at the press");
    crate::rows::land_pending(&state, &app, &worker);
    {
        let st = state.borrow();
        assert_eq!(st.views.shown, [0, 2]);
        assert_eq!(st.current, Some(2), "and stays");
    }
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// Culling's undo is a request too, queued behind the keys still
/// waiting on their reads: it takes back the last key the user
/// pressed, not the last that landed.
#[test]
fn culling_s_undo_takes_back_the_last_key_pressed_not_the_last_landed() {
    let (dir, files, writer, app, state, worker) = many_under_a_root("rows-undo", 3);
    open_and_land(&state, &app, &worker, 0);
    {
        let mut st = state.borrow_mut();
        crate::panel::cull::enter_cull(&mut st, &app, 1);
        st.culling.cull_move_on = true;
    }
    // 5 on f00, loaded: at once, and on to f01, whose read goes out.
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(5)
    ));
    {
        let st = state.borrow();
        assert_eq!(st.sidecars[0].meta.rating, 5);
        assert_eq!(st.current, Some(1));
        assert!(!st.from_row[1].read);
    }
    // 1 on f01: queued behind its read. Then undo: queued behind
    // that.
    assert!(crate::panel::browser::meta_on_selection(
        &state,
        &app,
        &worker,
        meta::Change::Rating(1)
    ));
    app.invoke_undo();
    {
        let st = state.borrow();
        assert_eq!(
            st.sidecars[0].meta.rating, 5,
            "not taken back ahead of its turn"
        );
        // The 1 moved the pick on to f02 at its press, as a key does.
        assert_eq!(
            st.views.loads.whats(),
            ["open of f01.tif", "rating key", "open of f02.tif", "undo"]
        );
    }
    crate::rows::land_pending(&state, &app, &worker);
    {
        let st = state.borrow();
        assert_eq!(
            st.sidecars[1].meta.rating, 0,
            "the 1 applied, then taken back"
        );
        assert_eq!(st.sidecars[0].meta.rating, 5, "the 5 stands");
        assert_eq!(st.current, Some(1), "over to the frame put back, not f00");
        assert!(st.views.loads.queue.is_empty());
        assert!(
            !app.get_status().starts_with("reading"),
            "the abandoned pick's line is gone: {}",
            app.get_status()
        );
    }
    assert_eq!(Sidecar::load(&files[0]).unwrap().unwrap().meta.rating, 5);
    assert_eq!(Sidecar::load(&files[1]).unwrap().unwrap().meta.rating, 0);
    // Redo brings the 1 back, at once now.
    app.invoke_redo();
    assert_eq!(state.borrow().sidecars[1].meta.rating, 1);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// A burst of passes reported while a read is out: one read is out
/// at a time, the burst asks for one more after it, and that one is
/// merged once.
#[test]
fn a_burst_of_reports_is_one_read_and_one_merge() {
    let dir = scratch("burst");
    let root = dir.join("root");
    let days: Vec<PathBuf> = (0..6).map(|i| root.join(format!("day{i}"))).collect();
    for d in &days {
        std::fs::create_dir_all(d).unwrap();
        frames(d, &["a.tif"]);
    }
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&root, &mut |_| {}).unwrap();
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.library.roots.add(&root).unwrap();
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert_eq!(state.borrow().files.len(), 6);

    // A file copied into the first folder and its pass said: a read
    // goes out. Then one into each of the others, each pass said
    // while that read is out.
    frames(&days[0], &["b.tif"]);
    let report = writer.index_folder(&days[0], &mut |_| {}).unwrap();
    assert!(background_done(
        &state, &app, &days[0], &report, false, false
    ));
    assert_eq!(sent(), 1, "the first report sends a read");
    for d in &days[1..] {
        frames(d, &["b.tif"]);
        let report = writer.index_folder(d, &mut |_| {}).unwrap();
        assert!(background_done(&state, &app, d, &report, false, false));
    }
    assert_eq!(sent(), 1, "the rest wait for it");
    assert!(state.borrow().views.library.stale);

    // The read out lands and what it saw is merged; the burst's one
    // read after it goes out, and is merged once.
    state.borrow_mut().views.library.painted = None;
    let look = SENT.with(|s| s.borrow_mut().pop()).unwrap();
    land(&state, &app, &worker, look.run());
    assert!(state.borrow().views.library.painted.is_some(), "merged");
    assert_eq!(state.borrow().files.len(), 7);
    assert_eq!(sent(), 1, "one more read for the burst");
    state.borrow_mut().views.library.painted = None;
    assert_eq!(land_all(&state, &app, &worker), 1, "and none after it");
    let st = state.borrow();
    assert!(st.views.library.painted.is_some(), "merged");
    assert_eq!(st.files.len(), 12);
    assert!(!st.views.library.merging && !st.views.library.stale);
    drop(st);
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}

/// M1: a capture of an empty all-roots view ends, failed, with a
/// status saying so, rather than wait for a picture that will not
/// come: the only root offline, or a root with nothing in it once
/// the launch pass is done.
#[test]
fn a_capture_of_an_empty_view_ends() {
    let dir = scratch("empty-view");
    let (gone, empty) = (dir.join("unplugged"), dir.join("empty"));
    std::fs::create_dir_all(&gone).unwrap();
    std::fs::create_dir_all(&empty).unwrap();
    let db = dir.join("index").join("library.sqlite");
    drop(greycard_library::Library::open(&db).unwrap());
    let app = window(0);
    let (state, worker) = state_for(&app, Vec::new());
    {
        let mut st = state.borrow_mut();
        st.window.batch = true;
        st.views.library.awaiting = true;
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.library.roots.add(&gone).unwrap();
    }
    std::fs::remove_dir(&gone).unwrap();
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    {
        let st = state.borrow();
        assert!(st.launch.failed, "the capture ends");
        assert!(!st.views.library.awaiting);
    }
    assert!(app.get_status().contains("offline"), "{}", app.get_status());

    // A fresh library over an empty root: the view waits for the
    // launch pass, and ends when the pass brings nothing.
    {
        let mut st = state.borrow_mut();
        st.launch.failed = false;
        st.views.library.awaiting = true;
        st.views.library.roots = Roots::default();
        st.views.library.roots.add(&empty).unwrap();
        st.views.library.launch_left = 1;
    }
    open_view(&state, &app, &worker, View::Roots(None));
    land_all(&state, &app, &worker);
    assert!(!state.borrow().launch.failed, "the pass is still to come");
    let report = greycard_library::Report::default();
    background_done(&state, &app, &empty, &report, true, false);
    assert!(state.borrow().launch.failed, "nothing came of the pass");
    assert!(app.get_status().contains("nothing"), "{}", app.get_status());
    state.borrow_mut().views.index_reader = None;
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// M2: a pass that found one file changed asks for that file's
/// picture again, and no other under the root.
#[test]
fn a_changed_file_asks_for_its_own_picture_only() {
    let dir = scratch("changed-rows");
    let files = frames(&dir, &["a.tif", "b.tif", "c.tif"]);
    let app = window(3);
    let (state, _worker) = state_for(&app, files.clone());
    let report = greycard_library::Report {
        changed: 1,
        changed_files: vec![greycard_library::key_path(&files[1])],
        ..greycard_library::Report::default()
    };
    assert_eq!(changed_rows(&state.borrow(), &report), [1]);
    assert!(changed_rows(&state.borrow(), &greycard_library::Report::default()).is_empty());
    drop(state);
    crate::testing::remove_dir_retry(&dir);
}

/// M3: the frame on screen, the last file in its folder, deleted.
/// The folder is left empty, which the index takes for a drive not
/// mounted and marks nothing; the window still lets the frame go,
/// and opens the nearest, rather than keep a frame whose next save
/// would write a sidecar beside nothing.
#[test]
fn the_last_frame_of_a_folder_deleted_goes_from_the_window() {
    let dir = scratch("last-gone");
    let (a, b) = (dir.join("a"), dir.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    frames(&a, &["x.tif", "y.tif"]);
    let lone = frames(&b, &["z.tif"]).remove(0);
    let db = dir.join("index").join("library.sqlite");
    let mut writer = greycard_library::Library::open(&db).unwrap();
    writer.index_tree(&dir, &mut |_| {}).unwrap();
    let listed = writer.paths_under(std::slice::from_ref(&dir)).unwrap();
    let app = window(3);
    let (state, worker) = state_for(&app, listed.clone());
    {
        let mut st = state.borrow_mut();
        st.views.index_reader = Some(greycard_library::Library::open_read_only(&db).unwrap());
        st.views.library.roots.add(&dir).unwrap();
        st.views.view = View::Roots(None);
        crate::library::refresh_ids(&mut st);
        st.current = listed.iter().position(|p| *p == lone);
        rebuild_browser(&mut st, &app);
    }
    std::fs::remove_file(&lone).unwrap();
    let report = writer.index_folder(&b, &mut |_| {}).unwrap();
    assert_eq!(report.missing, 0, "the index keeps the row: {report:?}");
    assert!(!report.unavailable.is_empty());
    background_done(&state, &app, &b, &report, false, false);
    assert_eq!(land_all(&state, &app, &worker), 1);
    {
        let st = state.borrow();
        assert_ne!(st.current.and_then(|c| st.files.get(c)), Some(&lone));
        assert!(!st.files.contains(&lone), "{:?}", st.files);
    }
    assert!(!lone.with_extension("tif.gcd").exists());
    state.borrow_mut().views.index_reader = None;
    drop(state);
    drop(writer);
    crate::testing::remove_dir_retry(&dir);
}
