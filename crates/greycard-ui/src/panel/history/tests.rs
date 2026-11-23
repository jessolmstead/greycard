use super::*;
use crate::worker::Outcome;
use greycard_edit::Sidecar;
use std::path::PathBuf;
use std::sync::Arc;

/// A frame exported as a set of one, as the window sends every
/// export: what lands is that frame's `Done::Exported`.
fn landed(source: &std::path::Path, edit: &Edit, path: PathBuf, preset: Option<String>) -> Outcome {
    let set = crate::queue::Set::new(
        1,
        None,
        crate::export::Settings::default(),
        crate::export::OnExists::Increment,
    )
    .with_preset(preset);
    Outcome::SetFrameDone {
        set: Arc::new(set),
        index: 0,
        source: source.to_path_buf(),
        edit: edit.clone(),
        version: None,
        done: crate::queue::Done::Exported {
            path,
            seconds: 1.0,
            note: None,
            left_out: Vec::new(),
        },
    }
}

/// The frame's edit position, its rating, and its flag.
fn edit_and_marks(st: &State, i: usize) -> (usize, u8, greycard_edit::meta::Flag) {
    let s = &st.sidecars[i];
    (s.position(), s.meta.rating, s.meta.flag)
}

/// Issue 115: a frame rejected in Develop under a filter that
/// hides the rejects is gone from the strip, and Ctrl+Z takes the
/// reject back, brings the frame back, and goes to it, without
/// touching any edit. Ctrl+Shift+Z rejects it again.
#[test]
fn undo_in_develop_takes_back_a_mark_and_brings_the_frame_back() {
    use greycard_edit::meta::Flag;
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(0);
    // The rejects hidden.
    app.invoke_filter_flag_toggled(2, true);
    assert_eq!(state.borrow().views.shown, vec![0, 1, 2]);

    app.invoke_meta_key("x".into());
    assert_eq!(state.borrow().views.shown, vec![1, 2]);
    assert_eq!(state.borrow().current, Some(1), "on to the next frame");

    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[0].meta.flag, Flag::None);
    assert_eq!(state.borrow().views.shown, vec![0, 1, 2]);
    assert_eq!(state.borrow().current, Some(0), "back on the frame");
    assert_eq!(app.get_status(), "Undo: Unflagged");
    for i in 0..3 {
        assert_eq!(state.borrow().sidecars[i].states(), 1, "no edit moved");
    }

    app.invoke_redo();
    assert_eq!(state.borrow().sidecars[0].meta.flag, Flag::Reject);
    assert_eq!(state.borrow().views.shown, vec![1, 2]);
    assert_eq!(state.borrow().current, Some(1));
    assert_eq!(app.get_status(), "Redo: Rejected");
}

/// Last done first: an edit and then a mark undo the mark and
/// then the edit; a mark and then an edit undo the edit and leave
/// the mark. Redo makes them again the way they were undone, and
/// the walk goes back and forth the same each time.
#[test]
fn undo_in_develop_takes_marks_and_edits_back_in_the_order_they_were_done() {
    use greycard_edit::meta::Flag;
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let now = |state: &Rc<Shared>| edit_and_marks(&state.borrow(), 0);

    // An edit, then a mark.
    app.set_exposure(0.5);
    save(&app, &state);
    app.invoke_meta_key("3".into());
    assert_eq!(now(&state), (1, 3, Flag::None));
    for _ in 0..2 {
        app.invoke_undo();
        assert_eq!(now(&state), (1, 0, Flag::None), "the mark first");
        app.invoke_undo();
        assert_eq!(now(&state), (0, 0, Flag::None), "then the edit");
        assert_eq!(app.get_exposure(), 0.0);
        app.invoke_undo();
        assert_eq!(now(&state), (0, 0, Flag::None), "nothing left");
        app.invoke_redo();
        assert_eq!(now(&state), (1, 0, Flag::None), "the edit again first");
        assert_eq!(app.get_exposure(), 0.5);
        app.invoke_redo();
        assert_eq!(now(&state), (1, 3, Flag::None), "then the mark");
        app.invoke_redo();
        assert_eq!(now(&state), (1, 3, Flag::None), "nothing left");
    }

    // A mark, then an edit: the edit first, the mark kept.
    app.invoke_meta_key("p".into());
    app.set_exposure(1.0);
    save(&app, &state);
    assert_eq!(now(&state), (2, 3, Flag::Pick));
    app.invoke_undo();
    assert_eq!(now(&state), (1, 3, Flag::Pick), "the edit, the mark left");
    assert_eq!(app.get_exposure(), 0.5);
    app.invoke_undo();
    assert_eq!(now(&state), (1, 3, Flag::None), "then the mark");
    app.invoke_undo();
    assert_eq!(now(&state), (1, 0, Flag::None), "the older mark");
    app.invoke_redo();
    app.invoke_redo();
    assert_eq!(now(&state), (1, 3, Flag::Pick), "the marks again, in order");
    app.invoke_redo();
    assert_eq!(now(&state), (2, 3, Flag::Pick), "and the edit last");

    // The panel's edit not yet saved is the newest thing: it is
    // what an undo takes back, and the mark stays.
    app.set_exposure(2.0);
    app.invoke_undo();
    assert_eq!(now(&state), (2, 3, Flag::Pick));
    assert_eq!(app.get_exposure(), 1.0);
}

/// An edit from before the session is older than any mark made in
/// it.
#[test]
fn an_edit_from_before_the_session_is_undone_after_the_marks() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    {
        // A step the sidecar was read with.
        let mut st = state.borrow_mut();
        let mut e = st.sidecars[0].current().clone();
        e.light.exposure = 0.5;
        st.sidecars[0].record(e);
        st.sidecars[0].current_order = greycard_edit::Order::BEFORE;
    }
    app.invoke_select(0);
    app.invoke_meta_key("2".into());
    app.invoke_undo();
    assert_eq!(edit_and_marks(&state.borrow(), 0).0, 1, "the mark first");
    assert_eq!(state.borrow().sidecars[0].meta.rating, 0);
    app.invoke_undo();
    assert_eq!(edit_and_marks(&state.borrow(), 0).0, 0, "then the edit");
}

/// The library's grid: a mark made there is undone there, with a
/// frame current or none, and the frame the filter hid comes back
/// and is selected.
#[test]
fn undo_in_the_library_takes_back_a_mark() {
    use greycard_edit::meta::Flag;
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(1);
    app.set_grid_open(true);
    app.invoke_filter_flag_toggled(2, true);
    app.invoke_meta_key("x".into());
    assert_eq!(state.borrow().views.shown, vec![0, 2]);
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[1].meta.flag, Flag::None);
    assert_eq!(state.borrow().views.shown, vec![0, 1, 2]);
    assert_eq!(state.borrow().current, Some(1));

    // No frame current: the marks still step.
    app.invoke_redo();
    assert_eq!(state.borrow().sidecars[1].meta.flag, Flag::Reject);
    state.borrow_mut().current = None;
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[1].meta.flag, Flag::None);
    assert_eq!(state.borrow().views.shown, vec![0, 1, 2]);
}

/// The export queue's view refuses an undo, a mark's as an edit's.
#[test]
fn the_queue_view_refuses_a_mark_s_undo() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    app.invoke_meta_key("4".into());
    assert!(marks_first(&state.borrow(), &app, true).is_some());
    state.borrow_mut().views.view = crate::roots::View::Queue;
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[0].meta.rating, 4);
}

/// The panel's exposure, the frame's edit position, and its marks.
type Seen = (f32, usize, u8, greycard_edit::meta::Flag);

/// An undo or a redo, pressed.
type Press = fn(&App);

fn seen(app: &App, state: &Rc<Shared>) -> Seen {
    let (p, r, f) = edit_and_marks(&state.borrow(), 0);
    (app.get_exposure(), p, r, f)
}

/// The review's case A: a mark redone after an edit made since
/// its undo is what the next Ctrl+Z takes back, not the edit.
#[test]
fn undo_takes_back_a_mark_just_redone() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    app.invoke_meta_key("3".into());
    app.invoke_undo();
    app.set_exposure(0.5);
    save(&app, &state);
    app.invoke_redo();
    assert_eq!(state.borrow().sidecars[0].meta.rating, 3, "the mark again");
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[0].meta.rating, 0, "the redo undone");
    assert_eq!(app.get_exposure(), 0.5, "the edit left");
}

/// Case B: an edit redone after a mark made since its undo is
/// what the next Ctrl+Z takes back; and the mirror, a mark undone
/// after an edit was is what Ctrl+Shift+Z makes again.
#[test]
fn undo_takes_back_an_edit_just_redone_and_redo_the_mark_just_undone() {
    use greycard_edit::meta::Flag;
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    app.set_exposure(0.5);
    save(&app, &state);
    app.set_exposure(1.0);
    save(&app, &state);
    app.invoke_undo();
    app.invoke_meta_key("3".into());
    app.invoke_redo();
    assert_eq!(
        seen(&app, &state),
        (1.0, 2, 3, Flag::None),
        "the edit again"
    );
    app.invoke_undo();
    assert_eq!(
        seen(&app, &state),
        (0.5, 1, 3, Flag::None),
        "the redo undone"
    );

    // The mirror: the mark undone after the edit was.
    app.invoke_undo();
    assert_eq!(seen(&app, &state), (0.5, 1, 0, Flag::None), "the mark");
    app.invoke_redo();
    assert_eq!(
        seen(&app, &state),
        (0.5, 1, 3, Flag::None),
        "the mark again"
    );
    app.invoke_redo();
    assert_eq!(seen(&app, &state), (1.0, 2, 3, Flag::None), "then the edit");
}

/// A longer walk, undos, redos, marks, and edits interleaved: a
/// Ctrl+Z always takes back what the Ctrl+Shift+Z before it did,
/// and a Ctrl+Shift+Z what the Ctrl+Z before it did.
#[test]
fn each_undo_and_redo_is_taken_back_by_the_other() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let mut exposure = 0.0;
    let mut moved = 0;
    for op in [
        "e", "3", "u", "e", "r", "p", "u", "u", "r", "e", "u", "x", "r", "r", "u", "5", "r", "u",
        "u", "u", "e", "r", "r", "r", "u", "1", "u", "u", "r", "e", "r", "u", "u", "u", "u", "u",
        "r", "r", "r", "r",
    ] {
        let before = seen(&app, &state);
        match op {
            "e" => {
                exposure += 0.25;
                app.set_exposure(exposure);
                save(&app, &state);
            }
            "u" | "r" => {
                let back = op == "u";
                let (go, again): (Press, Press) = if back {
                    (App::invoke_undo, App::invoke_redo)
                } else {
                    (App::invoke_redo, App::invoke_undo)
                };
                go(&app);
                let after = seen(&app, &state);
                if after != before {
                    moved += 1;
                    again(&app);
                    assert_eq!(seen(&app, &state), before, "{op} taken back");
                    go(&app);
                    assert_eq!(seen(&app, &state), after, "{op} made again");
                }
            }
            key => {
                app.invoke_meta_key(key.into());
            }
        }
    }
    assert!(moved > 20, "the walk moved: {moved}");
}

/// The history panel's buttons step the edit alone, as before
/// marks were undone out of Cull; the keys take the mark first.
#[test]
fn the_history_buttons_step_the_edit_alone() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    app.set_exposure(0.5);
    save(&app, &state);
    app.invoke_meta_key("3".into());
    app.invoke_undo_edit();
    assert_eq!(edit_and_marks(&state.borrow(), 0).0, 0, "the edit undone");
    assert_eq!(state.borrow().sidecars[0].meta.rating, 3, "the mark kept");
    app.invoke_redo_edit();
    assert_eq!(edit_and_marks(&state.borrow(), 0).0, 1);
    // The keys: the button's redo, the last thing done, then the
    // mark.
    app.invoke_undo();
    assert_eq!(edit_and_marks(&state.borrow(), 0).0, 0);
    assert_eq!(state.borrow().sidecars[0].meta.rating, 3);
    app.invoke_undo();
    assert_eq!(
        state.borrow().sidecars[0].meta.rating,
        0,
        "the key: the mark"
    );
    // The button's redo: the edit, not the mark undone last.
    app.invoke_redo_edit();
    assert_eq!(state.borrow().sidecars[0].meta.rating, 0, "the button: not");
    assert_eq!(edit_and_marks(&state.borrow(), 0).0, 1);
}

/// A mark's undo goes to the frame it put back at once: the next
/// Ctrl+Z is that frame's, never the one the view just left.
#[test]
fn the_undo_after_a_mark_s_steps_the_frame_it_went_to() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    for i in [1, 0] {
        app.invoke_select(i);
        app.set_exposure(0.5);
        save(&app, &state);
    }
    app.invoke_filter_flag_toggled(2, true);
    app.invoke_meta_key("x".into());
    assert_eq!(state.borrow().current, Some(1));
    app.invoke_undo();
    assert_eq!(state.borrow().current, Some(0), "moved before it returns");
    app.invoke_undo();
    let st = state.borrow();
    assert_eq!(st.sidecars[0].position(), 0, "the frame gone to");
    assert_eq!(st.sidecars[1].position(), 1, "the frame left untouched");
}

/// A mark's undo that moves to another frame with a tool in hand or
/// the crop open does what a click on the strip does: the same
/// open, the tool put down, the crop kept for the frame it lands on.
#[test]
fn a_mark_s_undo_with_a_tool_or_the_crop_up_moves_as_a_click_does() {
    // Frame 0 rejected out of the list, frame 1 on screen with a
    // brush in hand or the crop open; `go` then moves off it.
    let after = |crop: bool, go: &dyn Fn(&App)| {
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        app.invoke_select(1);
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        save(&app, &state);
        app.invoke_select(0);
        app.invoke_filter_flag_toggled(2, true);
        app.invoke_meta_key("x".into());
        assert_eq!(state.borrow().current, Some(1));
        if crop {
            app.set_crop_mode(true);
            app.invoke_crop_toggled();
        } else {
            app.invoke_target_changed(1);
            app.invoke_add_shape("Brush".into(), "Add".into());
            assert!(state.borrow().develop.placing.is_some());
        }
        let states = state.borrow().sidecars[1].states();
        go(&app);
        let st = state.borrow();
        assert_ne!(st.current, Some(1), "moved off");
        assert_eq!(st.sidecars[1].states(), states, "nothing recorded");
        (
            st.develop.placing.is_some(),
            app.get_placing().to_string(),
            app.get_crop_mode(),
            st.develop.crop_start_due,
        )
    };
    for crop in [false, true] {
        let undone = after(crop, &|app| app.invoke_undo());
        let clicked = after(crop, &|app| app.invoke_frame_clicked(1, false, false));
        assert_eq!(undone, clicked, "crop {crop}");
        assert!(!undone.0, "the tool down");
    }
}

/// The ratings of every file.
fn ratings(state: &Rc<Shared>) -> Vec<u8> {
    state
        .borrow()
        .sidecars
        .iter()
        .map(|s| s.meta.rating)
        .collect()
}

/// In Develop undo and redo reach only the open frame's marks: a
/// mark on another frame is passed over and stays, the open
/// frame's edits and marks interleave by when they were done, and
/// the window never leaves the frame. Cull's walk after keeps the
/// passed-over marks, in their order.
#[test]
fn undo_in_develop_reaches_only_the_open_frame_s_marks() {
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(0);
    app.invoke_meta_key("3".into());
    app.invoke_select(1);
    app.set_exposure(0.5);
    save(&app, &state);
    app.invoke_meta_key("4".into());
    app.invoke_select(2);
    app.invoke_meta_key("5".into());
    app.invoke_select(1);
    let position = |state: &Rc<Shared>| state.borrow().sidecars[1].position();

    app.invoke_undo();
    assert_eq!(
        ratings(&state),
        [3, 0, 5],
        "the open frame's mark, past 2's"
    );
    assert_eq!(position(&state), 1);
    app.invoke_undo();
    assert_eq!(position(&state), 0, "then its edit");
    assert_eq!(app.get_exposure(), 0.0);
    app.invoke_undo();
    assert_eq!(ratings(&state), [3, 0, 5], "never frame 0's");
    assert_eq!(state.borrow().current, Some(1), "never off the frame");

    app.invoke_redo();
    assert_eq!(position(&state), 1, "the edit, undone last");
    assert_eq!(ratings(&state), [3, 0, 5]);
    app.invoke_redo();
    assert_eq!(ratings(&state), [3, 4, 5], "then the mark");
    app.invoke_redo();
    assert_eq!(ratings(&state), [3, 4, 5], "nothing of another frame's");
    assert_eq!(state.borrow().current, Some(1));

    // Cull walks every frame's marks, the redone one first, then
    // those passed over in the order they were made.
    app.invoke_cull_toggled();
    assert!(crate::panel::cull::cull_mode(&state.borrow()));
    app.invoke_undo();
    assert_eq!(ratings(&state), [3, 0, 5]);
    app.invoke_undo();
    assert_eq!(ratings(&state), [3, 0, 0]);
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 0, 0]);
    app.invoke_redo();
    app.invoke_redo();
    app.invoke_redo();
    assert_eq!(ratings(&state), [3, 4, 5]);
}

/// The tester's case with a mark made before it on another frame:
/// the reject that took the window off its frame is undone from
/// the frame the window went to, and the window goes back; the
/// older mark is not reached. The landed frame's own steps go
/// first when they were done after the reject.
#[test]
fn the_mark_that_moved_the_window_is_undone_and_no_older_one() {
    use greycard_edit::meta::Flag;
    let flag = |state: &Rc<Shared>| state.borrow().sidecars[0].meta.flag;
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(2);
    app.invoke_meta_key("2".into());
    app.invoke_select(0);
    app.invoke_filter_flag_toggled(2, true);
    app.invoke_meta_key("x".into());
    assert_eq!(state.borrow().current, Some(1));

    // A mark and an edit on the frame landed on, after the reject.
    app.invoke_meta_key("4".into());
    app.set_exposure(0.5);
    save(&app, &state);
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[1].position(), 0, "the edit");
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 0, 2], "its mark");
    app.invoke_undo();
    assert_eq!(flag(&state), Flag::None, "then the reject");
    assert_eq!(state.borrow().current, Some(0), "back on its frame");
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 0, 2], "frame 2's not reached");
    assert_eq!(state.borrow().current, Some(0));

    // Moved away from where the reject landed, and back: the reject
    // is frame 0's alone again.
    app.invoke_redo();
    assert_eq!(flag(&state), Flag::Reject);
    assert_eq!(state.borrow().current, Some(1));
    // Rows: frame 0 hidden, so row 1 is frame 2 and row 0 frame 1.
    app.invoke_select(1);
    assert_eq!(state.borrow().current, Some(2));
    app.invoke_select(0);
    assert_eq!(state.borrow().current, Some(1));
    app.invoke_undo();
    assert_eq!(flag(&state), Flag::Reject, "not reached once left");
    assert_eq!(state.borrow().current, Some(1));
}

/// A mark that hides the open frame on its undo carries the window
/// off it as a key does, and the redo, from where it landed,
/// brings the frame back.
#[test]
fn a_redo_follows_an_undo_that_moved_the_window() {
    use greycard_edit::meta::Flag;
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(1);
    app.invoke_meta_key("p".into());
    app.invoke_select(0);
    app.invoke_meta_key("p".into());
    // The picks alone shown.
    app.invoke_filter_flag_toggled(1, false);
    assert_eq!(state.borrow().views.shown, vec![0, 1]);
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[0].meta.flag, Flag::None);
    assert_eq!(state.borrow().current, Some(1), "carried off");
    app.invoke_redo();
    assert_eq!(state.borrow().sidecars[0].meta.flag, Flag::Pick);
    assert_eq!(state.borrow().current, Some(0), "back on it");
    // And the undo again, from there.
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[0].meta.flag, Flag::None);
    assert_eq!(state.borrow().sidecars[1].meta.flag, Flag::Pick);
}

/// Every mark undone in Cull, then every one redone: `n` presses
/// each, the ratings after each walk.
fn cull_walk(app: &App, state: &Rc<Shared>, n: usize) -> (Vec<u8>, Vec<u8>) {
    if !crate::panel::cull::cull_mode(&state.borrow()) {
        app.invoke_cull_toggled();
    }
    for _ in 0..n {
        app.invoke_undo();
    }
    let undone = ratings(state);
    for _ in 0..n {
        app.invoke_redo();
    }
    (undone, ratings(state))
}

/// A mark over a selection in Develop counts as the open frame's,
/// as it covers it, and goes whole. Once a later mark passed over
/// names another frame of it, only the open frame's share goes; the
/// next Ctrl+Z moves on to the frame's older mark, and Cull takes
/// back the rest in order, every frame ending right both ways.
#[test]
fn a_mark_over_a_selection_goes_whole_or_the_open_frame_s_share() {
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(1);
    app.invoke_meta_key("1".into());
    app.invoke_select(0);
    app.invoke_step(1, true);
    assert_eq!(
        crate::panel::browser::chosen_frames(&state.borrow()),
        [0, 1]
    );
    app.invoke_meta_key("3".into());
    assert_eq!(ratings(&state), [3, 3, 0]);
    app.invoke_select(2);
    app.invoke_select(1);
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 1, 0], "both frames");
    app.invoke_redo();
    assert_eq!(ratings(&state), [3, 3, 0], "both again");

    app.invoke_select(0);
    app.invoke_meta_key("5".into());
    app.invoke_select(1);
    app.invoke_undo();
    assert_eq!(ratings(&state), [5, 1, 0], "frame 1's share");
    assert_eq!(state.borrow().current, Some(1));
    app.invoke_undo();
    assert_eq!(ratings(&state), [5, 0, 0], "then its older mark");
    app.invoke_redo();
    app.invoke_redo();
    assert_eq!(ratings(&state), [5, 3, 0], "and back, in order");
    app.invoke_undo();
    assert_eq!(ratings(&state), [5, 1, 0]);
    assert_eq!(cull_walk(&app, &state, 6), (vec![0, 0, 0], vec![5, 3, 0]));
}

/// The review's case: a frame of a selection's mark put back to the
/// same value by later marks is still theirs to undo first. Develop
/// takes the open frame's share; Cull's undo of everything ends at
/// the session's start and its redo at the end, both ways.
#[test]
fn a_frame_back_at_the_same_value_still_blocks_the_selection_s_mark() {
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(0);
    app.invoke_step(1, true);
    app.invoke_meta_key("5".into());
    assert_eq!(ratings(&state), [5, 5, 0]);
    app.invoke_select(1);
    app.invoke_meta_key("0".into());
    app.invoke_meta_key("5".into());
    assert_eq!(ratings(&state), [5, 5, 0]);
    app.invoke_select(0);
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 5, 0], "frame 0's share alone");
    assert_eq!(cull_walk(&app, &state, 5), (vec![0, 0, 0], vec![5, 5, 0]));
    assert_eq!(cull_walk(&app, &state, 5), (vec![0, 0, 0], vec![5, 5, 0]));
}

/// Redo the same way: a selection's mark undone whole, then a mark
/// on the other frame undone after it, is redone from the open frame
/// for its share alone, and Cull's walks end right.
#[test]
fn a_blocked_redo_makes_again_the_open_frame_s_share() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(1);
    app.invoke_meta_key("2".into());
    app.invoke_select(0);
    app.invoke_step(1, true);
    app.invoke_meta_key("5".into());
    app.invoke_select(0);
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 2], "whole");
    app.invoke_select(1);
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 0]);
    app.invoke_select(0);
    app.invoke_redo();
    assert_eq!(ratings(&state), [5, 0], "frame 0's share");
    app.invoke_redo();
    assert_eq!(ratings(&state), [5, 0], "nothing more of frame 0's");
    assert_eq!(cull_walk(&app, &state, 4), (vec![0, 0], vec![5, 5]));
}

/// The window left where a carry landed by Cull's own step to a
/// frame (not a click), and come back: the carry is over, and the
/// reject is not reached from there.
#[test]
fn a_carry_ends_when_cull_steps_off_the_frame() {
    use greycard_edit::meta::Flag;
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(0);
    app.invoke_filter_flag_toggled(2, true);
    app.invoke_meta_key("x".into());
    assert_eq!(state.borrow().current, Some(1));
    app.invoke_cull_toggled();
    crate::panel::cull::cull_select(&mut state.borrow_mut(), &app, 2);
    crate::panel::cull::cull_select(&mut state.borrow_mut(), &app, 1);
    app.invoke_cull_toggled();
    assert!(!crate::panel::cull::cull_mode(&state.borrow()));
    assert_eq!(state.borrow().current, Some(1));
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[0].meta.flag, Flag::Reject);
}

/// A reject over a selection that hid both frames carries the window
/// to the third. Undone whole there when nothing since names its
/// frames; with a later mark on one, the other's share alone, the
/// window back on it; with later marks on both, passed over. Cull's
/// walks end right after each.
#[test]
fn a_carried_selection_reject_goes_whole_in_a_share_or_not_at_all() {
    use greycard_edit::meta::{Change, Flag};
    let flags = |state: &Rc<Shared>| -> Vec<Flag> {
        state
            .borrow()
            .sidecars
            .iter()
            .map(|s| s.meta.flag)
            .collect()
    };
    let (none, reject) = (Flag::None, Flag::Reject);
    for later in [&[][..], &[1], &[0, 1]] {
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        app.invoke_select(0);
        app.invoke_step(1, true);
        app.invoke_filter_flag_toggled(2, true);
        app.invoke_meta_key("x".into());
        assert_eq!(state.borrow().current, Some(2));
        for &f in later {
            crate::panel::browser::set_meta(&mut state.borrow_mut(), &app, &[f], Change::Rating(4));
        }
        assert_eq!(state.borrow().current, Some(2));
        app.invoke_undo();
        let (want, on) = match later.len() {
            0 => (vec![none, none, none], Some(0)),
            1 => (vec![none, reject, none], Some(0)),
            _ => (vec![reject, reject, none], Some(2)),
        };
        assert_eq!(flags(&state), want, "{later:?}");
        assert_eq!(state.borrow().current, on, "{later:?}");
        let top = ratings(&state);
        assert_eq!(cull_walk(&app, &state, 6).0, [0, 0, 0], "{later:?}");
        assert_eq!(ratings(&state), top, "{later:?}");
        assert_eq!(flags(&state), [reject, reject, none], "{later:?}");
    }
}

/// A reject in the library's grid that hid its frame carries the
/// window the same way: Develop opened next on the frame it landed
/// on undoes it.
#[test]
fn a_reject_in_the_grid_that_moved_the_window_is_undone_in_develop() {
    use greycard_edit::meta::Flag;
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    app.invoke_select(0);
    app.set_grid_open(true);
    app.invoke_filter_flag_toggled(2, true);
    app.invoke_meta_key("x".into());
    assert_eq!(state.borrow().current, Some(1));
    app.set_grid_open(false);
    app.invoke_undo();
    assert_eq!(state.borrow().sidecars[0].meta.flag, Flag::None);
    assert_eq!(state.borrow().current, Some(0));
}

/// A reject in Cull that hid its frame and moved the window on is
/// the landed frame's in Develop too: leaving Cull there, Ctrl+Z
/// brings the frame back. Moved on to another frame first, it does
/// not.
#[test]
fn a_reject_in_cull_that_moved_the_window_is_undone_in_develop() {
    use greycard_edit::meta::Flag;
    for moved_on in [false, true] {
        let app = crate::testing::window(3);
        let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        app.invoke_select(0);
        app.invoke_filter_flag_toggled(2, true);
        app.invoke_cull_toggled();
        app.invoke_meta_key("x".into());
        assert_eq!(state.borrow().current, Some(1), "{moved_on}");
        if moved_on {
            // Rows: frame 0 hidden, so row 1 is frame 2.
            app.invoke_select(1);
            assert_eq!(state.borrow().current, Some(2));
        }
        app.invoke_cull_toggled();
        assert!(!crate::panel::cull::cull_mode(&state.borrow()));
        app.invoke_undo();
        let st = state.borrow();
        if moved_on {
            assert_eq!(st.sidecars[0].meta.flag, Flag::Reject, "not reached");
            assert_eq!(st.current, Some(2));
        } else {
            assert_eq!(st.sidecars[0].meta.flag, Flag::None, "brought back");
            assert_eq!(st.current, Some(0));
        }
    }
}

/// A version is its file's: a mark is the file's, so from a
/// version's cell undo reaches the mark made on the file's own,
/// and not another file's.
#[test]
fn a_version_reaches_its_file_s_marks() {
    let app = crate::testing::window(2);
    let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    {
        let mut st = state.borrow_mut();
        st.sidecars[0].new_version(None, Some("Warm")).unwrap();
        crate::versions::settle(&mut st, &app, &worker);
        assert_eq!(st.cells.len(), 3);
        assert_eq!(st.file_of(1), 0, "cell 1 the version");
    }
    app.invoke_select(2);
    app.invoke_meta_key("2".into());
    app.invoke_select(0);
    app.invoke_meta_key("3".into());
    app.invoke_select(2);
    app.invoke_select(1);
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 2], "the file's mark");
    assert_eq!(state.borrow().current, Some(1));
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 2], "not the other file's");
}

/// The library's grid reaches a mark on any frame, as before.
#[test]
fn the_library_reaches_a_mark_on_another_frame() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.set_grid_open(true);
    app.invoke_select(0);
    app.invoke_meta_key("3".into());
    app.invoke_select(1);
    app.invoke_undo();
    assert_eq!(ratings(&state), [0, 0]);
    assert_eq!(state.borrow().current, Some(0), "gone to it");
}

#[test]
fn a_moment_reads_as_a_date() {
    assert_eq!(date_of(0), "1970-01-01 00:00 UTC");
    assert_eq!(date_of(951_782_400), "2000-02-29 00:00 UTC");
    assert_eq!(date_of(1_789_000_000), "2026-09-10 00:26 UTC");
}

/// An export's moment in local time: the date and time on hover,
/// the time alone on its row when it was today, the date when not.
#[test]
fn an_export_s_moment_reads_in_local_time() {
    let at = 1_789_000_000;
    let whole = local_date_of(at);
    assert_eq!(whole.len(), "2026-09-10 00:26".len(), "{whole}");
    assert!(!whole.contains("UTC"));
    assert_eq!(row_time(at, at), whole[11..]);
    assert_eq!(row_time(at, at + 3 * 86_400), whole[..10]);
}

/// The panel's edit made a state, as the save timer (or the
/// export button) makes it.
fn save(app: &App, state: &Rc<Shared>) {
    let mut st = state.borrow_mut();
    let edit = read_edit(app, &st.develop.edit, st.develop.target);
    crate::panel::edit::save_edit(&mut st, edit);
    show_history(&st, app);
}

fn rows(app: &App) -> Vec<(String, bool)> {
    let rows = app.get_history_rows();
    (0..rows.row_count())
        .map(|r| {
            let row = rows.row_data(r).unwrap();
            (row.name.to_string(), row.exported)
        })
        .collect()
}

/// Which rows are drawn as undone, newest first.
fn undone(app: &App) -> Vec<bool> {
    let rows = app.get_history_rows();
    (0..rows.row_count())
        .map(|r| rows.row_data(r).unwrap().undone)
        .collect()
}

/// The two marks a frame's cell wears, as the strip and the grid
/// are given them.
fn marks(app: &App, row: usize) -> (bool, bool) {
    let t = app.get_thumbs().row_data(row).unwrap();
    (t.edited, t.exported)
}

/// A saved edit puts the pencil on the frame's cell, a reset takes
/// it off again whatever history it leaves, and a frame never
/// touched wears nothing.
#[test]
fn a_saved_edit_shows_its_mark_and_a_reset_hides_it() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    assert_eq!(marks(&app, 0), (false, false));
    assert_eq!(crate::testing::count_id(&app, "ThumbMarks::edited-mark"), 0);

    app.set_exposure(0.5);
    save(&app, &state);
    assert_eq!(marks(&app, 0), (true, false));
    assert_eq!(marks(&app, 1), (false, false), "the other frame is bare");
    assert_eq!(
        crate::testing::count_id(&app, "ThumbMarks::edited-mark"),
        1,
        "drawn once, on the one cell"
    );
    assert_eq!(
        crate::testing::count_id(&app, "ThumbMarks::exported-mark"),
        0
    );

    // Reset: the panel back to the default is a state of its own,
    // and the frame is no longer edited.
    app.set_exposure(0.0);
    save(&app, &state);
    assert!(!state.borrow().sidecars[0].history.is_empty());
    assert_eq!(marks(&app, 0), (false, false));
    assert_eq!(crate::testing::count_id(&app, "ThumbMarks::edited-mark"), 0);
}

/// An export landing puts its mark on the cell, beside the pencil
/// when the frame is edited as well, and one that failed puts
/// none.
#[test]
fn an_export_landing_shows_its_mark() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    let source = state.borrow().files[0].clone();
    let set = Arc::new(crate::queue::Set::new(
        1,
        None,
        crate::export::Settings::default(),
        crate::export::OnExists::Increment,
    ));
    crate::panel::deliver::deliver(
        &app,
        Outcome::SetFrameDone {
            set,
            index: 0,
            source: source.clone(),
            edit: Edit::default(),
            version: None,
            done: crate::queue::Done::Failed {
                message: "disk full".into(),
            },
        },
    );
    assert_eq!(marks(&app, 0), (false, false));

    // An unedited frame exported: the export mark alone.
    let plain = state.borrow().sidecars[0].current().clone();
    crate::panel::deliver::deliver(
        &app,
        landed(&source, &plain, PathBuf::from("/out/IMG_0000.jpg"), None),
    );
    assert_eq!(marks(&app, 0), (false, true));
    assert_eq!(marks(&app, 1), (false, false));
    assert_eq!(
        crate::testing::count_id(&app, "ThumbMarks::exported-mark"),
        1
    );
    assert_eq!(crate::testing::count_id(&app, "ThumbMarks::edited-mark"), 0);

    // Edited as well: both, together.
    app.set_exposure(0.5);
    save(&app, &state);
    assert_eq!(marks(&app, 0), (true, true));
    assert_eq!(
        crate::testing::count_id(&app, "ThumbMarks::exported-mark"),
        1
    );
    assert_eq!(crate::testing::count_id(&app, "ThumbMarks::edited-mark"), 1);
}

/// The export mark is a fact about the frame: an undo back past the
/// exported state leaves it, and so do more steps than the history
/// keeps.
#[test]
fn the_export_mark_outlives_the_state_it_was_made_from() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let source = state.borrow().files[0].clone();
    app.set_exposure(0.5);
    save(&app, &state);
    let sent = state.borrow().sidecars[0].current().clone();
    crate::panel::deliver::deliver(
        &app,
        landed(&source, &sent, PathBuf::from("/out/IMG_0000.jpg"), None),
    );
    assert_eq!(marks(&app, 0), (true, true));

    // Back past the exported state: the pencil goes, the export stays.
    {
        let st = &mut *state.borrow_mut();
        assert!(st.sidecars[0].undo());
        show_history(st, &app);
    }
    assert_eq!(marks(&app, 0), (false, true));

    // More steps than the history keeps: the record is off it, the
    // mark is not.
    for k in 0..51 {
        app.set_exposure(1.0 + k as f32 / 100.0);
        save(&app, &state);
    }
    assert!(
        state.borrow().sidecars[0]
            .history
            .iter()
            .all(|s| s.exports.is_empty())
    );
    assert_eq!(marks(&app, 0), (true, true));
}

/// An export that finishes after the panel moved on is recorded on
/// the state it was written from, as a row of its own marked as
/// one; a click on it goes back to that look; undo walks the
/// changes as before; a failed export records nothing.
#[test]
fn an_export_is_a_row_on_the_state_it_was_written_from() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    let source = state.borrow().files[0].clone();

    // Exported at +0.5, and the panel moved on to +1.0 while the
    // file was written.
    app.set_exposure(0.5);
    save(&app, &state);
    let sent = state.borrow().sidecars[0].current().clone();
    assert_eq!(sent.light.exposure, 0.5);
    app.set_exposure(1.0);
    save(&app, &state);
    let states = state.borrow().sidecars[0].states();
    crate::panel::deliver::deliver(
        &app,
        landed(
            &source,
            &sent,
            PathBuf::from("/out/IMG_0000.jpg"),
            Some("Web".into()),
        ),
    );
    {
        let st = state.borrow();
        let s = &st.sidecars[0];
        assert_eq!(s.states(), states, "no state for the export");
        assert_eq!(s.current().light.exposure, 1.0, "the panel's moved on");
        assert!(s.current_exports.is_empty());
        let on = s.exports(1);
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].file, "/out/IMG_0000.jpg");
        assert_eq!(on[0].preset.as_deref(), Some("Web"));
        assert!(on[0].at > 0);
    }
    assert_eq!(
        rows(&app),
        [
            ("Exposure +1.00".to_string(), false),
            ("IMG_0000.jpg · Web".to_string(), true),
            ("Exposure +0.50".to_string(), false),
            ("Original".to_string(), false),
        ]
    );
    assert_eq!(app.get_history_current(), 0);
    assert_eq!(undone(&app), [false; 4]);
    assert!(!app.get_history_rows().row_data(1).unwrap().time.is_empty());

    // A set's frame that failed, was skipped or was canceled:
    // nothing.
    let set = Arc::new(crate::queue::Set::new(
        1,
        None,
        crate::export::Settings::default(),
        crate::export::OnExists::Increment,
    ));
    for done in [
        crate::queue::Done::Failed {
            message: "no".into(),
        },
        crate::queue::Done::Skipped {
            path: PathBuf::from("/out/IMG_0000.jpg"),
        },
        crate::queue::Done::Canceled,
    ] {
        crate::panel::deliver::deliver(
            &app,
            Outcome::SetFrameDone {
                set: set.clone(),
                index: 0,
                source: source.clone(),
                edit: sent.clone(),
                version: None,
                done,
            },
        );
    }
    assert_eq!(rows(&app).len(), 4);
    assert_eq!(state.borrow().sidecars[0].exports(1).len(), 1);

    // Hovered, the viewport shows the look that went out.
    app.invoke_history_hovered(1);
    assert_eq!(state.borrow().develop.peek.as_ref(), Some(&sent));
    assert!(app.get_status().contains("/out/IMG_0000.jpg, exported"));
    app.invoke_history_hovered(-1);

    // Clicked: that state is current, the panel and all.
    app.invoke_history_clicked(1);
    assert_eq!(*state.borrow().sidecars[0].current(), sent);
    assert_eq!(app.get_exposure(), 0.5);
    assert_eq!(app.get_history_current(), 2, "the state's row, lit");
    assert_eq!(state.borrow().sidecars[0].current_exports.len(), 1);
    // The current state's export above it reads as live; only the
    // step undone above them is muted.
    assert_eq!(undone(&app), [true, false, false, false]);

    // One undo is one change: back to the original, not a step
    // for the export; and a redo brings the record back with its
    // state.
    app.invoke_undo();
    assert_eq!(*state.borrow().sidecars[0].current(), Edit::default());
    // The export's state undone: its row is muted with it.
    assert_eq!(undone(&app), [true, true, true, false]);
    app.invoke_redo();
    assert_eq!(*state.borrow().sidecars[0].current(), sent);
    assert_eq!(state.borrow().sidecars[0].current_exports.len(), 1);
    app.invoke_redo();
    assert_eq!(state.borrow().sidecars[0].current().light.exposure, 1.0);
    assert!(rows(&app)[1].1);
}

/// A frame the window let go of while its file was written (another
/// folder opened): its sidecar is read from the disk, noted, and
/// written back through the same save, counted; a later export of
/// a state it no longer has writes nothing.
#[test]
fn a_frame_no_longer_shown_is_noted_on_the_disk() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/scratch/export-record-{}",
        std::process::id()
    ));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("IMG_0100.CR3");
    let mut on_disk = Sidecar::default();
    let mut edit = Edit::default();
    edit.light.exposure = 0.75;
    on_disk.record(edit.clone());
    on_disk.save(&raw).unwrap();

    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    state.borrow_mut().window.write_sidecars = true;
    let exported = |edit: &Edit| landed(&raw, edit, dir.join("IMG_0100.jpg"), None);
    crate::panel::deliver::deliver(&app, exported(&edit));
    let back = Sidecar::load_as_file(&raw).unwrap().unwrap();
    assert_eq!(back.saved, 2);
    assert_eq!(*back.current(), edit);
    assert_eq!(back.current_exports.len(), 1);
    assert_eq!(back.current_exports[0].label(), "IMG_0100.jpg");
    // The window's own frames are untouched.
    assert_eq!(state.borrow().sidecars[0], Sidecar::default());

    crate::panel::deliver::deliver(&app, exported(&Edit::for_picture()));
    assert_eq!(Sidecar::load_as_file(&raw).unwrap().unwrap(), back);
    crate::testing::remove_dir_retry(&dir);
}

fn scratch(what: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/scratch/export-{what}-{}",
        std::process::id()
    ));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn exposed(ev: f32) -> Edit {
    let mut e = Edit::default();
    e.light.exposure = ev;
    e
}

/// A shoot of `frames` raws in `dir`, the first with a sidecar at
/// 0.5 EV carrying one export record, opened in a window as a run
/// with `--exposure 1.0` (and `--develop-temperature 4000`, when
/// `temperature`) opens it: the window, the state, the first raw,
/// and its sidecar's bytes on disk.
fn launched(
    dir: &Path,
    frames: usize,
    temperature: bool,
) -> (App, Rc<Shared>, Rc<Worker>, PathBuf, Vec<u8>) {
    let raws: Vec<PathBuf> = (0..frames)
        .map(|i| dir.join(format!("IMG_02{i:02}.CR3")))
        .collect();
    let mut kept = Sidecar::default();
    kept.record(exposed(0.5));
    kept.record_export(
        &exposed(0.5),
        Exported {
            file: "/out/earlier.jpg".into(),
            preset: None,
            at: 1,
        },
    );
    kept.save(&raws[0]).unwrap();
    let bytes = std::fs::read(Sidecar::path_for(&raws[0])).unwrap();
    let app = crate::testing::window(frames);
    let (state, worker) = crate::testing::state_for(&app, raws.clone());
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.sidecars[0] = Sidecar::load_as_file(&raws[0]).unwrap().unwrap();
        st.launch.overrides_at_start = Some((
            0,
            crate::panel::startup::Overrides {
                temperature: temperature.then_some(4000.0),
                exposure: Some(1.0),
                display_curve: None,
            },
        ));
    }
    app.invoke_select(0);
    (app, state, worker, raws[0].clone(), bytes)
}

fn on_disk(raw: &Path) -> Sidecar {
    Sidecar::load_as_file(raw).unwrap().unwrap()
}

/// `--exposure` on a batch export: the command line's value is the
/// panel's, never the sidecar's. The file is written at it, but it
/// is no state of the frame's, so nothing is recorded, and the
/// run's closing save finds nothing to save: the sidecar is left
/// byte for byte as it was, its 0.5 EV state and that state's
/// record with it. The first move ends that, one-shot: 1.0, then
/// 1.5, then 1.0 again is on disk at 1.0.
#[test]
fn a_command_line_exposure_leaves_the_sidecar_as_it_was() {
    let dir = scratch("override");
    let half = exposed(0.5);
    let (app, state, _worker, raw, bytes) = launched(&dir, 1, true);
    state.borrow_mut().window.batch = true;
    let rendered = state.borrow().develop.edit.clone();
    assert_eq!(rendered.light.exposure, 1.0, "the panel's");
    assert_eq!(
        *state.borrow().sidecars[0].current(),
        half,
        "not the sidecar's"
    );

    // The batch export lands, and the run closes with the save it
    // always makes of the open file.
    crate::panel::deliver::deliver(
        &app,
        landed(&raw, &rendered, dir.join("IMG_0200.jpg"), None),
    );
    {
        let mut st = state.borrow_mut();
        let edit = read_edit(&app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(&mut st, edit);
    }
    assert!(
        std::fs::read(Sidecar::path_for(&raw)).unwrap() == bytes,
        "rewritten"
    );
    assert_eq!(state.borrow().sidecars[0].states(), 2);
    assert_eq!(state.borrow().sidecars[0].exports(1).len(), 1);

    // Moved on the panel, the edit is a state as any would be, and
    // the overrides are done with: moved back to exactly their
    // values, that is a state too, and it is what is on disk.
    app.set_exposure(1.5);
    save(&app, &state);
    assert_eq!(state.borrow().sidecars[0].states(), 3);
    assert_eq!(on_disk(&raw).current().light.exposure, 1.5);
    app.set_exposure(1.0);
    save(&app, &state);
    assert_eq!(state.borrow().sidecars[0].states(), 4);
    assert_eq!(state.borrow().sidecars[0].current().light.exposure, 1.0);
    assert_eq!(on_disk(&raw).current().light.exposure, 1.0);
    crate::testing::remove_dir_retry(&dir);
}

/// Left and come back to, a frame opened under `--exposure` is its
/// sidecar's again: nothing of the override's was recorded on the
/// way out, and on the way back an edit that happens to be the
/// override's values is saved like any other.
#[test]
fn a_frame_left_and_come_back_to_is_its_sidecar_s_again() {
    let dir = scratch("override-return");
    let (app, state, _worker, raw, bytes) = launched(&dir, 2, false);
    app.invoke_select(1);
    assert!(std::fs::read(Sidecar::path_for(&raw)).unwrap() == bytes);
    app.invoke_select(0);
    assert_eq!(app.get_exposure(), 0.5, "the sidecar's, not the override's");
    assert_eq!(state.borrow().develop.edit, exposed(0.5));
    app.set_exposure(1.0);
    save(&app, &state);
    assert_eq!(state.borrow().sidecars[0].states(), 3);
    assert_eq!(*on_disk(&raw).current(), exposed(1.0));
    crate::testing::remove_dir_retry(&dir);
}

/// Undo as the first thing done after an `--exposure` launch goes
/// back from the sidecar's own current state: the override is not
/// recorded on the way, so the redo stack the sidecar carried is
/// still there to walk, and redo lands on the sidecar's states.
#[test]
fn a_first_undo_under_an_override_records_nothing_and_keeps_the_redo() {
    let dir = scratch("override-undo");
    let (app, state, _worker, _raw, _) = launched(&dir, 1, false);
    // The sidecar as a session left it: 0.75 undone, back at 0.5,
    // and the launch's override on the panel at 1.0.
    {
        let mut st = state.borrow_mut();
        let s = &mut st.sidecars[0];
        s.record(exposed(0.75));
        s.undo();
        assert_eq!(*s.current(), exposed(0.5));
        assert_eq!(s.redo.len(), 1);
    }
    assert_eq!(app.get_exposure(), 1.0);

    app.invoke_undo();
    {
        let st = state.borrow();
        let s = &st.sidecars[0];
        assert_eq!(*s.current(), Edit::default());
        assert_eq!(s.states(), 3, "no state for the override");
        assert!(
            (0..s.states()).all(|i| s.state(i) != Some(&exposed(1.0))),
            "the override is no state"
        );
        assert_eq!(s.redo.len(), 2, "the carried redo kept");
        assert!(st.launch.overridden.is_none(), "done with");
    }
    assert_eq!(app.get_exposure(), 0.0);
    app.invoke_redo();
    assert_eq!(*state.borrow().sidecars[0].current(), exposed(0.5));
    app.invoke_redo();
    assert_eq!(*state.borrow().sidecars[0].current(), exposed(0.75));
    crate::testing::remove_dir_retry(&dir);
}

/// A frame never opened, exported in a set: the worker seeds its
/// learned blend from the ISO, as a first open would, and the
/// sidecar its export creates carries that seed as the frame's
/// current state, not a step; the next load reads it back as the
/// seed, not the plain default.
#[test]
fn an_export_of_a_frame_never_opened_keeps_its_iso_seed() {
    let seed = greycard_edit::Noise::blend_for_iso(Some(200));
    assert!(seed < 1.0);
    let mut rendered = Edit::default();
    rendered.noise.learned_strength = seed;

    // In the window's list, waiting on its seed.
    let dir = scratch("seed");
    let (listed, gone) = (dir.join("IMG_0300.CR3"), dir.join("IMG_0301.CR3"));
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, vec![listed.clone()]);
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        assert!(crate::rows::wants_seed(&st, 0), "a raw never developed");
    }
    let set = Arc::new(crate::queue::Set::new(
        2,
        Some(dir.clone()),
        crate::export::Settings::default(),
        crate::export::OnExists::Increment,
    ));
    for (i, source) in [&listed, &gone].into_iter().enumerate() {
        crate::panel::deliver::deliver(
            &app,
            Outcome::SetFrameDone {
                set: set.clone(),
                index: i,
                source: source.clone(),
                edit: rendered.clone(),
                version: None,
                done: crate::queue::Done::Exported {
                    path: dir.join(format!("{i}.jpg")),
                    seconds: 1.0,
                    note: None,
                    left_out: Vec::new(),
                },
            },
        );
    }
    assert!(!crate::rows::wants_seed(&state.borrow(), 0), "taken");
    // Both on disk, the one the window had and the one it had not:
    // the seed as current, no step, the record on it.
    for raw in [&listed, &gone] {
        let back = Sidecar::load_as_file(raw).unwrap().unwrap();
        assert_eq!(*back.current(), rendered, "{}", raw.display());
        assert!(back.history.is_empty());
        assert_eq!(back.current_exports.len(), 1);
        let (loaded, seed_again) = crate::panel::browser::load_sidecar(raw, true);
        assert_eq!(loaded.current().noise.learned_strength, seed);
        assert!(!seed_again, "seeded already");
    }
    crate::testing::remove_dir_retry(&dir);
}

/// A set records one row a frame, each on that frame's own state
/// under the set's preset; a frame that failed records nothing.
#[test]
fn a_set_records_a_row_on_each_frame_it_wrote() {
    let app = crate::testing::window(3);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(3));
    let mut edits = Vec::new();
    {
        let mut st = state.borrow_mut();
        for (i, ev) in [0.25, -0.5, 1.5].into_iter().enumerate() {
            let mut e = Edit::default();
            e.light.exposure = ev;
            st.sidecars[i].record(e.clone());
            edits.push(e);
        }
    }
    let set = Arc::new(
        crate::queue::Set::new(
            3,
            Some(PathBuf::from("/out")),
            crate::export::Settings::default(),
            crate::export::OnExists::Increment,
        )
        .with_preset(Some("Print".into())),
    );
    let files = state.borrow().files.clone();
    for (i, done) in [
        crate::queue::Done::Exported {
            path: PathBuf::from("/out/IMG_0000.jpg"),
            seconds: 1.0,
            note: None,
            left_out: Vec::new(),
        },
        crate::queue::Done::Failed {
            message: "no".into(),
        },
        crate::queue::Done::Exported {
            path: PathBuf::from("/out/IMG_0002.jpg"),
            seconds: 1.0,
            note: None,
            left_out: Vec::new(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        crate::panel::deliver::deliver(
            &app,
            Outcome::SetFrameDone {
                set: set.clone(),
                index: i,
                source: files[i].clone(),
                edit: edits[i].clone(),
                version: None,
                done,
            },
        );
    }
    let st = state.borrow();
    for (i, want) in [Some("/out/IMG_0000.jpg"), None, Some("/out/IMG_0002.jpg")]
        .into_iter()
        .enumerate()
    {
        let s = &st.sidecars[i];
        assert_eq!(s.states(), 2, "frame {i}");
        match want {
            Some(file) => {
                assert_eq!(s.current_exports.len(), 1, "frame {i}");
                assert_eq!(s.current_exports[0].file, file);
                assert_eq!(s.current_exports[0].preset.as_deref(), Some("Print"));
                assert_eq!(
                    s.current_exports[0].label(),
                    format!("{} · Print", &file[5..])
                );
            }
            None => assert!(s.current_exports.is_empty(), "frame {i}"),
        }
    }
}

/// A develop landing with `sources` for the state developing now.
fn land(app: &App, state: &Rc<Shared>, sources: Vec<greycard_edit::retouch::Patch>) {
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(
        app,
        crate::worker::Outcome::Developed {
            generation,
            turn: 0,
            kind: crate::finish::Source::SCENE,
            image: crate::worker::Developed::Halves(std::sync::Arc::new(crate::worker::Halves {
                width: 60,
                height: 40,
                pixels: Vec::new(),
            })),
            guide: std::sync::Arc::new(crate::finish::Guide::NONE),
            planes: None,
            white: crate::worker::WhiteBase::IDENTITY,
            seconds: 0.1,
            sharpen: None,
            sources,
            learned: crate::worker::LearnedReport::Off,
            fills: crate::worker::FillReport::default(),
        },
    );
}

/// Issue 7, with the slow develop's order: a clone spot placed,
/// the panel's save firing before the develop lands, then the
/// develop landing with the source the engine chose. One undo
/// takes the spot away, and stays undone when that state's own
/// develop lands; a redo brings the spot back with its source.
#[test]
fn one_undo_takes_a_clone_spot_away_when_its_source_lands_after_the_save() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    assert_eq!(state.borrow().current, Some(0));

    app.invoke_retouch_toggled("Clone".into());
    app.invoke_place_pressed(10.0, 10.0, false);
    app.invoke_place_released();
    let placed = {
        let st = state.borrow();
        assert_eq!(st.develop.edit.retouch.patches.len(), 1);
        assert!(st.develop.edit.retouch.patches[0].source.is_none());
        st.develop.edit.retouch.patches[0].clone()
    };
    let spots = |state: &Rc<Shared>| {
        state.borrow().sidecars[0]
            .current()
            .retouch
            .patches
            .iter()
            .map(|p| p.source)
            .collect::<Vec<_>>()
    };

    // The save timer fires first: the spot is recorded sourceless.
    {
        let mut st = state.borrow_mut();
        let edit = crate::panel::edit::read_edit(&app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(&mut st, edit);
    }
    let before = state.borrow().sidecars[0].states();
    assert_eq!(spots(&state), vec![None]);

    // The develop lands with the source: it completes that state.
    let source = [0.05, -0.02];
    let developed = greycard_edit::retouch::Patch {
        source: Some(source),
        ..placed
    };
    land(&app, &state, vec![developed]);
    assert_eq!(state.borrow().sidecars[0].states(), before, "no new step");
    assert_eq!(spots(&state), vec![Some(source)]);
    assert_eq!(
        state.borrow().develop.edit.retouch.patches[0].source,
        Some(source)
    );

    // One undo, and the spot is gone.
    app.invoke_undo();
    assert_eq!(spots(&state), vec![]);
    assert!(state.borrow().develop.edit.retouch.patches.is_empty());
    // That state's develop lands, and nothing comes back.
    land(&app, &state, Vec::new());
    assert_eq!(spots(&state), vec![]);
    assert_eq!(state.borrow().sidecars[0].redo.len(), 1, "the redo stays");

    // A redo brings the spot back with its source, and its
    // develop chooses nothing.
    app.invoke_redo();
    assert_eq!(spots(&state), vec![Some(source)]);
    land(&app, &state, Vec::new());
    assert_eq!(state.borrow().sidecars[0].states(), before);
    app.invoke_undo();
    assert_eq!(spots(&state), vec![]);
}

/// A spot deleted while its develop is in flight, and another
/// placed elsewhere that takes its id: the develop landing with
/// the first spot's source gives the second nothing. Likewise a
/// sourceless spot resized while its develop is in flight.
#[test]
fn a_source_lands_only_on_the_patch_it_was_chosen_for() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    app.invoke_select(0);
    app.invoke_retouch_toggled("Clone".into());
    app.invoke_place_pressed(10.0, 10.0, false);
    app.invoke_place_released();
    let first = state.borrow().develop.edit.retouch.patches[0].clone();
    let developed = greycard_edit::retouch::Patch {
        source: Some([0.05, -0.02]),
        ..first.clone()
    };

    // The save records the first spot; then it is deleted, and
    // another placed elsewhere, before the develop lands.
    {
        let mut st = state.borrow_mut();
        let edit = crate::panel::edit::read_edit(&app, &st.develop.edit, st.develop.target);
        crate::panel::edit::save_edit(&mut st, edit);
    }
    app.set_patch(0);
    app.invoke_delete_patch();
    assert!(state.borrow().develop.edit.retouch.patches.is_empty());
    app.invoke_place_pressed(10.0, 10.0, false);
    app.invoke_place_released();
    {
        let mut st = state.borrow_mut();
        // Wherever the test window puts it, somewhere else.
        st.develop.edit.retouch.patches[0].points = vec![[0.8, 0.2]];
        let second = &st.develop.edit.retouch.patches[0];
        assert_eq!(second.id, first.id, "the id is reused");
        assert_ne!(second.points, first.points);
    }
    let states = state.borrow().sidecars[0].states();
    land(&app, &state, vec![developed.clone()]);
    assert!(
        state.borrow().develop.edit.retouch.patches[0]
            .source
            .is_none()
    );
    // The sidecar's state with the first spot is completed, with
    // that spot's own source, and nothing is recorded.
    assert_eq!(state.borrow().sidecars[0].states(), states);
    assert_eq!(
        state.borrow().sidecars[0].current().retouch.patches,
        vec![developed.clone()]
    );

    // Resized while in flight: the same id and place, but not the
    // spot the develop chose for.
    {
        let mut st = state.borrow_mut();
        st.develop.edit.retouch.patches[0].points = first.points.clone();
        st.develop.edit.retouch.patches[0].radius = first.radius * 2.0;
    }
    land(&app, &state, vec![developed]);
    assert!(
        state.borrow().develop.edit.retouch.patches[0]
            .source
            .is_none()
    );
}

/// The rests a slider's timers wait out: the develop's debounce
/// and the panel's save, fired as the event loop fires them.
fn rest() {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
        crate::panel::edit::SAVE_MS + 1,
    ));
    slint::platform::update_timers_and_animations();
}

/// Click the history's row for the state at `index`.
fn click_state(app: &App, state: &Rc<Shared>, index: usize) {
    let row = state.borrow().sidecars[0]
        .row_of_state(index)
        .expect("a row for every state");
    app.invoke_history_clicked(row as i32);
}

/// The global exposure of every state, oldest first, and where
/// the current one sits.
fn walk(state: &Rc<Shared>) -> (Vec<f32>, usize) {
    let st = state.borrow();
    let s = &st.sidecars[0];
    let all = (0..s.states())
        .map(|i| s.state(i).unwrap().light.exposure)
        .collect();
    (all, s.position())
}

/// A click back in the history with a mask chosen, over states
/// that differ only in what the finish draws (the global look
/// here): nothing recorded by whatever comes after it, and the
/// rows above it still there to click forward to. The panel's
/// edit kept the newest state's global look and masks behind the
/// chosen mask's look, since that jump needed no develop, so the
/// next read of the panel was news to the history and its save
/// cut the rows above.
#[test]
fn a_click_back_keeps_the_rows_above_it_through_what_follows() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    app.invoke_select(0);
    app.set_panel_tab("Masks".into());
    app.invoke_new_mask();
    app.invoke_target_changed(1);
    save(&app, &state);
    // Four global steps, each saved as the save timer saves it.
    app.invoke_target_changed(0);
    for ev in [0.2, 0.4, 0.6, 0.8] {
        app.set_exposure(ev);
        app.invoke_view_changed();
        rest();
    }
    let steps = vec![0.0, 0.0, 0.2, 0.4, 0.6, 0.8];
    assert_eq!(walk(&state), (steps.clone(), 5));
    // The mask chosen again, as it was when the rows were clicked.
    app.invoke_target_changed(1);

    // Two back, and the panel reads as that state.
    click_state(&app, &state, 3);
    assert_eq!(walk(&state), (steps.clone(), 3));
    let panel_is_current = |state: &Rc<Shared>| {
        let st = state.borrow();
        read_edit(&app, &st.develop.edit, st.develop.target) == st.sidecars[0].current().clone()
    };
    assert!(
        panel_is_current(&state),
        "the panel reads as the row clicked"
    );
    assert_eq!(undone(&app), [true, true, false, false, false, false]);

    // A develop landing, the timers running out, and nothing moves.
    land(&app, &state, Vec::new());
    rest();
    save(&app, &state);
    assert_eq!(walk(&state), (steps.clone(), 3));

    // Away to the other frame and back.
    app.invoke_select(1);
    app.invoke_select(0);
    rest();
    assert_eq!(walk(&state), (steps.clone(), 3));

    // Forward a row, then straight on to the newest.
    click_state(&app, &state, 4);
    assert_eq!(walk(&state), (steps.clone(), 4));
    assert!(panel_is_current(&state));
    click_state(&app, &state, 5);
    rest();
    assert_eq!(walk(&state), (steps.clone(), 5));

    // Back past the mask's making, two clicks in a row with the
    // timers running out after: the mask goes with the state, and
    // comes back with the one forward.
    click_state(&app, &state, 2);
    click_state(&app, &state, 0);
    rest();
    assert_eq!(walk(&state), (steps.clone(), 0));
    assert!(state.borrow().develop.edit.adjustments.is_empty());
    assert!(panel_is_current(&state));
    click_state(&app, &state, 3);
    rest();
    assert_eq!(walk(&state), (steps.clone(), 3));
    assert_eq!(state.borrow().develop.edit.adjustments.len(), 1);
    assert!(panel_is_current(&state));

    // An undo and a redo go the same way.
    app.invoke_undo();
    rest();
    assert_eq!(walk(&state), (steps.clone(), 2));
    assert!(panel_is_current(&state));
    app.invoke_redo();
    rest();
    assert_eq!(walk(&state), (steps, 3));
}

/// A snapshot just taken waits for its name. A click anywhere
/// else ends the naming and keeps what was typed, and from then on
/// a click on the row restores it. Left in naming mode, the row
/// took every later click as a name to edit and nothing could
/// restore it.
#[test]
fn a_click_away_from_a_new_snapshot_s_name_ends_the_naming() {
    use crate::testing;

    let app = testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = testing::state_for(&app, testing::folder(1));
    app.invoke_select(0);

    app.invoke_snapshot_taken();
    assert_eq!(
        app.get_snapshot_editing(),
        0,
        "the new row waits for its name"
    );
    testing::type_text(&app, "Keep");
    // The pointer crossing the picture does not end the naming:
    // the picture takes the keys on hover otherwise.
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(750.0, 450.0),
        });
    assert_eq!(app.get_snapshot_editing(), 0, "a hover leaves the naming");
    // A click on the picture, nowhere near the row, ends it.
    testing::click(&app, 750.0, 450.0);
    assert_eq!(app.get_snapshot_editing(), -1, "the naming ended");
    assert_eq!(state.borrow().sidecars[0].snapshots[0].name, "Keep");

    // The row is a button again: a click restores, once the
    // double-click's window has passed.
    let (at, size) = snapshot_part(&app, 0, "SnapshotsSection::snapshot-row");
    testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert_eq!(
        app.get_snapshot_editing(),
        -1,
        "a single click does not rename"
    );
    past_double_click();
    assert!(
        app.get_status()
            .as_str()
            .contains("Keep is the current state"),
        "{}",
        app.get_status()
    );
}

/// The time a click on a snapshot's row waits before it restores:
/// past the double-click's window, fired as the event loop fires
/// it.
fn past_double_click() {
    slint::platform::update_timers_and_animations();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(600));
    slint::platform::update_timers_and_animations();
}

/// A snapshot's row says what a click does, in its icons: the
/// restore icon restores at once, the rename icon opens the name
/// and restores nothing, and a double-click on the row renames
/// without its first click restoring on the way, which it did
/// when the row restored on every click.
#[test]
fn a_snapshot_s_icons_restore_and_rename_and_a_double_click_only_renames() {
    use crate::testing;
    use slint::platform::Key;

    let app = testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = testing::state_for(&app, testing::folder(1));
    app.invoke_select(0);
    app.invoke_snapshot_taken();
    testing::type_text(&app, "Base");
    testing::press(&app, Key::Return);
    assert_eq!(app.get_snapshot_editing(), -1);
    assert_eq!(state.borrow().sidecars[0].snapshots[0].name, "Base");
    // A step past the snapshot, so a restore has somewhere to go.
    app.set_exposure(1.0);
    app.invoke_view_changed();
    rest();
    let exposure = |state: &Rc<Shared>| state.borrow().sidecars[0].current().light.exposure;
    assert_eq!(exposure(&state), 1.0);
    let middle = |(at, size): (slint::LogicalPosition, slint::LogicalSize)| {
        (at.x + size.width / 2.0, at.y + size.height / 2.0)
    };

    // The pencil: its words on hover, then the name open for
    // typing, and nothing restored, then or later.
    let (x, y) = middle(snapshot_part(&app, 0, "SnapshotsSection::rename"));
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(x, y),
        });
    // The row's hover is a changed handler's, run with the loop.
    slint::platform::update_timers_and_animations();
    assert!(
        app.get_status().as_str().contains("click to rename it"),
        "{}",
        app.get_status()
    );
    testing::click(&app, x, y);
    assert_eq!(app.get_snapshot_editing(), 0, "the name is open");
    past_double_click();
    assert_eq!(exposure(&state), 1.0, "the rename icon restores nothing");
    testing::press(&app, Key::Escape);
    assert_eq!(app.get_snapshot_editing(), -1);

    // A double-click on the row: the name, and no restore.
    let (x, y) = middle(snapshot_part(&app, 0, "SnapshotsSection::snapshot-row"));
    testing::click(&app, x, y);
    testing::click(&app, x, y);
    assert_eq!(app.get_snapshot_editing(), 0, "a double-click renames");
    past_double_click();
    assert_eq!(exposure(&state), 1.0, "and its first click did not restore");
    testing::press(&app, Key::Escape);
    assert_eq!(app.get_snapshot_editing(), -1);

    // The restore icon: back to the snapshot at once.
    // The pointer comes to it first, as a hand does, and its
    // words are up before the press.
    let (x, y) = middle(snapshot_part(&app, 0, "SnapshotsSection::restore"));
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(x, y),
        });
    slint::platform::update_timers_and_animations();
    assert!(
        app.get_status().as_str().contains("click to restore it"),
        "{}",
        app.get_status()
    );
    // Resting on the icon, its name comes up at the pointer too;
    // the press takes it down.
    assert_eq!(app.get_tip_text(), "", "no hover text before the rest");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(800));
    slint::platform::update_timers_and_animations();
    assert_eq!(
        app.get_tip_text(),
        "Restore",
        "the icon's word after a rest"
    );
    testing::click(&app, x, y);
    assert_eq!(app.get_tip_text(), "", "the press took the hover text down");
    assert_eq!(exposure(&state), 0.0, "restored");
    assert_eq!(app.get_snapshot_editing(), -1, "and not renamed");
    // The hover's words stand over the status while the pointer
    // stays on the row; away from it, the restore's are there.
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(750.0, 450.0),
        });
    slint::platform::update_timers_and_animations();
    assert!(
        app.get_status().as_str().contains("Base restored"),
        "{}",
        app.get_status()
    );
}

/// The part `part` (its row, its restore or rename icon) of the
/// snapshot `i` in the list, each keyed by the snapshot's place,
/// scrolled into the panel's view.
fn snapshot_part(app: &App, i: usize, part: &str) -> (slint::LogicalPosition, slint::LogicalSize) {
    crate::testing::by_key(app, part, i)
}

/// A frame with a snapshot named `name`, the naming over.
fn named_snapshot(app: &App, name: &str) {
    app.invoke_snapshot_taken();
    crate::testing::type_text(app, name);
    crate::testing::press(app, slint::platform::Key::Return);
    assert_eq!(app.get_snapshot_editing(), -1);
}

/// A restore still waiting out the double-click's window is for
/// the frame its row was clicked on: another frame chosen in the
/// meantime drops it, rather than restoring the new frame's
/// snapshot of that number. Left alone, the wait ends in the
/// restore.
#[test]
fn a_waiting_restore_is_dropped_when_the_frame_changes() {
    use crate::testing;
    use std::cell::Cell;

    let app = testing::window(2);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (_state, _worker) = testing::state_for(&app, testing::folder(2));
    app.invoke_select(0);
    named_snapshot(&app, "Base");
    let restores = Rc::new(Cell::new(0));
    {
        let restores = restores.clone();
        app.on_snapshot_restored(move |_| restores.set(restores.get() + 1));
    }
    let (at, size) = snapshot_part(&app, 0, "SnapshotsSection::snapshot-row");
    let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);

    testing::click(&app, x, y);
    past_double_click();
    assert_eq!(restores.get(), 1, "a click alone restores");

    // Past the double-click's window, so the next click is a
    // first click again.
    past_double_click();
    testing::click(&app, x, y);
    app.invoke_select(1);
    past_double_click();
    past_double_click();
    assert_eq!(restores.get(), 1, "nothing restored on either frame");
}

/// The pencil on one row while another row's new name is being
/// typed leaves the typing alone: opening the other name would
/// take the text box away without ending its naming, and the
/// typed name with it.
#[test]
fn the_pencil_leaves_a_name_being_typed_on_another_row() {
    use crate::testing;

    let app = testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = testing::state_for(&app, testing::folder(1));
    app.invoke_select(0);
    named_snapshot(&app, "Base");
    app.invoke_snapshot_taken();
    assert_eq!(app.get_snapshot_editing(), 1);
    testing::type_text(&app, "Two");

    let (at, size) = snapshot_part(&app, 0, "SnapshotsSection::rename");
    testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert_ne!(app.get_snapshot_editing(), 0, "the other name stays shut");
    if app.get_snapshot_editing() == 1 {
        testing::press(&app, slint::platform::Key::Return);
    }
    assert_eq!(app.get_snapshot_editing(), -1);
    let names: Vec<String> = state.borrow().sidecars[0]
        .snapshots
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names, ["Base", "Two"], "the typed name kept");
}

/// An undone state whose masks wait on a shape nothing can measure
/// (§724), its row clicked over a panel moved on the state before it:
/// refused, nothing recorded, the state still there to go to and as
/// written. With the panel unmoved the click goes to it, as written,
/// and the frame is read-only on it.
#[test]
fn a_waiting_undone_row_clicked_over_a_moved_panel_is_refused() {
    use i_slint_backend_testing::ElementHandle;
    let app = crate::testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 1200.0));
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(1));
    state.borrow_mut().sidecars[0] = crate::testing::waiting_sidecar();
    let written = state.borrow().sidecars[0].current_written().into_owned();
    app.invoke_select(0);
    app.set_panel_tab("Develop".into());
    let click = |index: usize| {
        let row = state.borrow().sidecars[0]
            .row_of_state(index)
            .expect("a row for every state");
        // A pointer move first, so the rows just drawn are there.
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        ElementHandle::find_by_element_id(&app, "HistorySection::history-row")
            .nth(row)
            .expect("the row is drawn")
            .invoke_accessible_default_action();
    };
    // Back to the state before it, which has nothing placed.
    click(0);
    assert_eq!(state.borrow().sidecars[0].position(), 0);
    assert!(!state.borrow().sidecars[0].current_waits());
    // The panel moved on it, and the waiting state's row clicked.
    app.set_exposure(0.7);
    click(1);
    {
        let st = state.borrow();
        let s = &st.sidecars[0];
        assert_eq!(s.position(), 0, "not gone to");
        assert!(s.history.is_empty(), "nothing recorded");
        assert_eq!(s.redo.len(), 1, "still there to go to");
        assert!(s.waits_at(1));
    }
    // The panel unmoved: gone to, as written, and read-only.
    app.set_exposure(0.0);
    click(1);
    let st = state.borrow();
    let s = &st.sidecars[0];
    assert_eq!(s.position(), 1);
    assert!(s.current_waits());
    assert_eq!(s.current_written().into_owned(), written);
}
