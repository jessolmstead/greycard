//! The learned highlight fill's switch in LIGHT, by its element id:
//! what it reads and writes, its history, the pending word and each way
//! it goes, and the switch standing down on a frame the fill does not
//! take, its setting kept. The switch is drawn only with
//! `GREYCARD_HIGHLIGHT_FILL=1` at the start; these tests set the
//! property it is read into, never the process's environment, which
//! the tests share.

use std::rc::Rc;
use std::sync::Arc;

use greycard_core::raw::{CfaPattern, RawFrame, Samples, SensorLayout};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model};

use super::*;
use crate::testing::{by_id, click, folder, land_develop, state_for, toggle_parts, window};
use crate::worker::{FillReport, LearnedReport, LensReport, Outcome, SourceKind};

const SWITCH: &str = "LightSection::highlight-fill-switch";

/// The words the status line says while the fill is to come.
const PENDING: &str = "developing... the highlight fill is at work";

/// A window over `n` frames with frame 0 open, LIGHT unfolded, and the
/// switch drawn.
fn open(n: usize) -> (App, Rc<Shared>, Rc<Worker>) {
    let app = window(n);
    app.set_highlight_fill_shown(true);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, worker) = state_for(&app, folder(n));
    app.invoke_select(0);
    app.set_collapsed_light(false);
    (app, state, worker)
}

/// The switch, scrolled into the panel's view.
fn switch(app: &App) -> ElementHandle {
    by_id(app, SWITCH);
    ElementHandle::find_by_element_id(app, SWITCH)
        .next()
        .expect("the highlight fill's switch")
}

/// A click on the switch's knob, and the develop it asks let go.
fn flip(app: &App) {
    let (at, size) = toggle_parts(&switch(app)).switch;
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(DEBOUNCE_MS + 50));
}

fn takes_a_hand(app: &App) -> bool {
    switch(app)
        .accessible_enabled()
        .expect("the switch says whether it takes a hand")
}

fn save(app: &App, state: &Rc<Shared>) {
    let mut st = state.borrow_mut();
    let edit = read_edit(app, &st.develop.edit, st.develop.target);
    save_edit(&mut st, edit);
    crate::panel::history::show_history(&st, app);
}

fn current(state: &Rc<Shared>, i: usize) -> Edit {
    state.borrow().line(i).unwrap().current().clone()
}

fn history_rows(app: &App) -> Vec<String> {
    app.get_history_rows()
        .iter()
        .map(|r| r.name.to_string())
        .collect()
}

/// The open of the frame chosen, a raw `frame`, said by the worker.
fn opened(state: &Rc<Shared>, frame: RawFrame) -> Outcome {
    let profile = greycard_core::color::profile_from_frame(&frame).expect("its profile");
    Outcome::Opened {
        generation: state.borrow().develop.generation,
        as_shot: (5000.0, 0.0),
        frame: Some(Arc::new(frame)),
        profile: Some(Box::new(profile)),
        lens: LensReport::NoLensName,
        lens_options: Vec::new(),
        kind: SourceKind::Raw,
        shot: Default::default(),
        taken: None,
        blend: None,
    }
}

fn bayer() -> RawFrame {
    crate::panel::viewport::tests::daylight_frame()
}

fn xtrans() -> RawFrame {
    RawFrame {
        width: 6,
        height: 6,
        layout: SensorLayout::Cfa(CfaPattern::xtrans()),
        samples: Samples::U16(vec![500; 36]),
        ..bayer()
    }
}

fn pending() -> FillReport {
    FillReport {
        highlight_pending: true,
        ..FillReport::default()
    }
}

/// The switch, clicked by its id, turns the fill on in the edit at this
/// build's version and off again, each a step of the history named as
/// a switch; an undo and a redo move the switch with the edit. The
/// switch is a switch to a screen reader, named by its label.
#[test]
fn the_switch_turns_the_fill_on_and_off_and_is_undone() {
    let (app, state, _worker) = open(1);
    // A new edit is off, and reads back as itself: its key and so its
    // picture are what they were before the switch.
    assert!(!app.get_highlight_fill());
    let read = read_edit(&app, &Edit::default(), None);
    assert_eq!(read, Edit::default());
    assert_eq!(
        read.develop_key_with(&[], None),
        Edit::default().develop_key_with(&[], None)
    );
    let s = switch(&app);
    assert_eq!(
        s.accessible_role(),
        Some(i_slint_backend_testing::AccessibleRole::Switch)
    );
    assert_eq!(
        s.accessible_label().map(|l| l.to_string()),
        Some(crate::tr!("Highlight fill"))
    );
    assert_eq!(s.accessible_checked(), Some(false));
    assert!(takes_a_hand(&app));

    flip(&app);
    assert!(app.get_highlight_fill());
    assert_eq!(switch(&app).accessible_checked(), Some(true));
    assert_eq!(
        state.borrow().develop.edit.highlight_fill.as_deref(),
        greycard_edit::key::HIGHLIGHT_FILLS.last().copied()
    );
    save(&app, &state);
    assert_eq!(current(&state, 0).highlight_fill.as_deref(), Some("hl-2"));
    assert_eq!(history_rows(&app)[0], "Highlight fill on");

    flip(&app);
    assert!(!app.get_highlight_fill());
    save(&app, &state);
    assert_eq!(current(&state, 0).highlight_fill, None);
    assert_eq!(history_rows(&app)[0], "Highlight fill off");

    app.invoke_undo();
    assert!(app.get_highlight_fill(), "undone: on again");
    assert_eq!(current(&state, 0).highlight_fill.as_deref(), Some("hl-2"));
    app.invoke_undo();
    assert!(!app.get_highlight_fill());
    assert_eq!(current(&state, 0).highlight_fill, None);
    app.invoke_redo();
    assert!(app.get_highlight_fill());
}

/// An edit naming a version this build lacks shows the switch on, and
/// keeps its version through the panel; turned off and on, it takes
/// this build's.
#[test]
fn a_version_this_build_lacks_is_kept_until_the_switch_is_turned() {
    let (app, state, _worker) = open(2);
    let newer = Edit {
        highlight_fill: Some("hl-9".into()),
        ..Edit::default()
    };
    state.borrow_mut().sidecars[1].record(newer.clone());
    app.invoke_select(1);
    assert!(app.get_highlight_fill());
    {
        let st = state.borrow();
        assert_eq!(read_edit(&app, &st.develop.edit, None), newer);
    }
    flip(&app);
    flip(&app);
    assert_eq!(
        state.borrow().develop.edit.highlight_fill.as_deref(),
        Some("hl-2")
    );
}

/// An edit carrying either of this build's versions shows the switch
/// on and keeps its version through the panel: hl-1 stays hl-1.
#[test]
fn either_version_reads_as_on_and_is_kept() {
    for name in greycard_edit::key::HIGHLIGHT_FILLS {
        let (app, state, _worker) = open(2);
        let edit = Edit {
            highlight_fill: Some((*name).into()),
            ..Edit::default()
        };
        state.borrow_mut().sidecars[1].record(edit.clone());
        app.invoke_select(1);
        assert!(app.get_highlight_fill(), "{name}");
        let st = state.borrow();
        assert_eq!(read_edit(&app, &st.develop.edit, None), edit, "{name}");
    }
}

/// The develop that comes without the fill says so on the status line,
/// with no bar or spinner, until the filled picture lands.
#[test]
fn the_pending_word_goes_when_the_filled_picture_lands() {
    let (app, state, _worker) = open(1);
    flip(&app);
    let status = land_develop(&app, &state, LearnedReport::Off, pending());
    assert_eq!(status, PENDING);
    assert!(state.borrow().develop.highlight_pending);
    assert!(!app.get_busy(), "no spinner for it");
    assert_eq!(app.get_fill_fraction(), -1.0, "no bar for it");
    // The fill's round, of the same generation.
    let status = land_develop(&app, &state, LearnedReport::Off, FillReport::default());
    assert_eq!(status, "");
    assert!(!state.borrow().develop.highlight_pending);
}

/// The word follows what else the develop says, and goes on its own.
#[test]
fn the_pending_word_follows_what_the_develop_went_without() {
    let (app, state, _worker) = open(1);
    let status = land_develop(
        &app,
        &state,
        LearnedReport::Unavailable("not here"),
        pending(),
    );
    assert_eq!(status, format!("no denoise: not here; {PENDING}"));
}

/// The switch turned off while the fill is to come: the develop asked
/// lands without the fill and with nothing to wait on, and the word
/// goes with it.
#[test]
fn the_pending_word_goes_when_the_fill_is_turned_off() {
    let (app, state, _worker) = open(1);
    flip(&app);
    let asked = state.borrow().develop.generation;
    assert_eq!(
        land_develop(&app, &state, LearnedReport::Off, pending()),
        PENDING
    );
    flip(&app);
    assert!(state.borrow().develop.generation > asked, "a develop asked");
    assert_eq!(app.get_status(), "developing...");
    let status = land_develop(&app, &state, LearnedReport::Off, FillReport::default());
    assert_eq!(status, "");
    assert!(!state.borrow().develop.highlight_pending);
}

/// Another frame opened while the fill is to come: its open says the
/// last frame's fill is not its own.
#[test]
fn the_pending_word_goes_with_another_frame() {
    let (app, state, _worker) = open(2);
    land_develop(&app, &state, LearnedReport::Off, pending());
    assert!(state.borrow().develop.highlight_pending);
    app.invoke_select(1);
    crate::panel::deliver::deliver(&app, opened(&state, bayer()));
    assert!(!state.borrow().develop.highlight_pending);
    let status = land_develop(&app, &state, LearnedReport::Off, FillReport::default());
    assert_eq!(status, "");
}

/// The fill's round failed: nothing is coming, and the status line
/// says the failure.
#[test]
fn the_pending_word_goes_with_a_failure() {
    let (app, state, _worker) = open(1);
    land_develop(&app, &state, LearnedReport::Off, pending());
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(
        &app,
        Outcome::Failed {
            generation,
            message: "no memory".into(),
        },
    );
    assert!(!state.borrow().develop.highlight_pending);
    assert_eq!(app.get_status(), "failed: no memory");
    assert!(!app.get_busy());
}

/// The Original over a picture still waiting on its fill keeps no
/// copy of it to put back: the edit develops again on the way back.
#[test]
fn the_original_keeps_no_picture_still_waiting_on_its_fill() {
    let (app, state, _worker) = open(1);
    land_develop(&app, &state, LearnedReport::Off, FillReport::default());
    {
        let mut st = state.borrow_mut();
        st.develop.pending = None;
        st.develop.drawn_generation = st.develop.generation;
        assert!(crate::panel::original::edit_picture_whole(&st));
    }
    land_develop(&app, &state, LearnedReport::Off, pending());
    let mut st = state.borrow_mut();
    st.develop.pending = None;
    assert!(!crate::panel::original::edit_picture_whole(&st));
}

/// On an X-Trans raw the switch stands down, its setting kept: a fill
/// pasted on before the frame is open reads back on, through another
/// change, and the frame's sidecar keeps it; a Bayer raw opened after
/// takes a hand again.
#[test]
fn the_switch_stands_down_on_a_frame_the_fill_does_not_take() {
    let (app, state, _worker) = open(2);
    crate::panel::deliver::deliver(&app, opened(&state, bayer()));
    assert!(takes_a_hand(&app));
    flip(&app);
    save(&app, &state);
    app.invoke_copy_asked();
    app.invoke_select(1);
    app.invoke_paste_asked();
    app.invoke_sync_applied();
    assert_eq!(current(&state, 1).highlight_fill.as_deref(), Some("hl-2"));
    crate::panel::deliver::deliver(&app, opened(&state, xtrans()));
    assert!(!takes_a_hand(&app), "greyed on X-Trans");
    assert!(app.get_highlight_fill(), "and still on");
    assert_eq!(switch(&app).accessible_checked(), Some(true));
    app.set_exposure(0.3);
    save(&app, &state);
    let kept = current(&state, 1);
    assert_eq!(kept.light.exposure, 0.3);
    assert_eq!(kept.highlight_fill.as_deref(), Some("hl-2"));

    app.invoke_select(0);
    crate::panel::deliver::deliver(&app, opened(&state, bayer()));
    assert!(takes_a_hand(&app));
}

/// In Cull the switch reaches the frame the selection is on, whatever
/// the frame open before was, and goes over its own edit as the one
/// change: an X-Trans frame's switch stood down, and the frame culled
/// to keeps its own exposure and takes the fill.
#[test]
fn the_switch_in_cull_lays_the_fill_over_the_frame_alone() {
    let (app, state, _worker) = open(2);
    let own = Edit {
        light: greycard_edit::Light {
            exposure: 0.7,
            ..Default::default()
        },
        ..Edit::default()
    };
    state.borrow_mut().sidecars[1].record(own);
    crate::panel::deliver::deliver(&app, opened(&state, xtrans()));
    assert!(!takes_a_hand(&app));
    app.invoke_cull_toggled();
    assert!(state.borrow().culling.cull.is_some());
    app.invoke_select(1);
    // Develop's sections, reached from Cull.
    app.set_panel_tab("Develop".into());
    app.invoke_tab_changed("Develop".into());
    app.set_collapsed_light(false);
    assert!(takes_a_hand(&app), "free in Cull");
    flip(&app);
    assert!(
        state.borrow().culling.cull.is_none(),
        "the control left Cull"
    );
    let mut st = state.borrow_mut();
    let edit = read_edit(&app, &st.develop.edit, st.develop.target);
    save_edit(&mut st, edit);
    let frame = &st.line(1).unwrap().current();
    assert_eq!(frame.light.exposure, 0.7);
    assert_eq!(frame.highlight_fill.as_deref(), Some("hl-2"));
    assert_eq!(st.line(0).unwrap().current().highlight_fill, None);
}

/// A virtual copy keeps its own switch: on for the copy, the file's
/// own line off, each shown as its own when chosen.
#[test]
fn a_virtual_copy_keeps_its_own_fill() {
    let (app, state, worker) = open(1);
    let id = {
        let mut st = state.borrow_mut();
        let id = st.sidecars[0].new_version(None, Some("Filled")).unwrap();
        crate::versions::settle(&mut st, &app, &worker);
        id
    };
    app.invoke_select(1);
    assert!(!app.get_highlight_fill());
    flip(&app);
    save(&app, &state);
    {
        let st = state.borrow();
        let line = &st.sidecars[0].version(&id).unwrap().line;
        assert_eq!(line.current().highlight_fill.as_deref(), Some("hl-2"));
        assert_eq!(st.sidecars[0].current().highlight_fill, None);
    }
    app.invoke_select(0);
    assert!(!app.get_highlight_fill(), "the file's own");
    app.invoke_select(1);
    assert!(app.get_highlight_fill(), "the copy's");
}

/// The switch is drawn in LIGHT but is the Demosaic section's: Light's
/// own switch turned off, Light's header reset, and a copy of Light
/// alone leave the fill as it is; Demosaic's reset takes it off.
#[test]
fn light_s_switch_reset_and_copy_leave_the_fill_alone() {
    use crate::panel::section_reset::{reset_section, section_at_reset};
    use greycard_edit::preset::{Choice, Preset, Section};
    let (app, state, _worker) = open(1);
    flip(&app);
    app.set_exposure(0.5);
    save(&app, &state);
    // Light's switch off: the fill stays on, and on the picture.
    app.set_light_enabled(false);
    app.invoke_view_changed();
    save(&app, &state);
    let now = current(&state, 0);
    assert!(!now.light.enabled);
    assert_eq!(now.highlight_fill.as_deref(), Some("hl-2"));
    assert_eq!(switch(&app).accessible_checked(), Some(true));
    // Light's reset, through its header and as the table has it.
    let (at, size) = crate::testing::header_reset(&app, "LightSection");
    click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    let reset = current(&state, 0);
    assert_eq!(reset.light, Edit::default().light);
    assert_eq!(reset.highlight_fill.as_deref(), Some("hl-2"));
    assert!(app.get_highlight_fill());
    let original = Edit::default();
    assert!(section_at_reset(&reset, &original, None, "light"));
    let mut e = reset.clone();
    assert!(reset_section(&mut e, &original, None, "demosaic"));
    assert_eq!(e.highlight_fill, None, "Demosaic's reset takes it");
    // Light ticked alone in a copy or a preset: no fill goes.
    let light = Preset::between_frames(&reset, &Choice::whole(&[Section::Light]));
    assert_eq!(light.applied(&Edit::default()).highlight_fill, None);
    let demosaic = Preset::between_frames(&reset, &Choice::whole(&[Section::Demosaic]));
    assert_eq!(
        demosaic.applied(&Edit::default()).highlight_fill.as_deref(),
        Some("hl-2")
    );
}

/// A mask's Light has no switch of its own, and a mask's Light reset
/// leaves the picture's fill.
#[test]
fn a_mask_s_light_has_no_switch_and_its_reset_leaves_the_fill() {
    use crate::panel::section_reset::reset_section;
    let mut edit = Edit {
        highlight_fill: Some("hl-1".into()),
        ..Edit::default()
    };
    edit.adjustments.push(greycard_edit::Adjustment {
        id: 1,
        name: "M1".into(),
        ..Default::default()
    });
    edit.adjustments[0].look.light.exposure = 0.7;
    let app = window(1);
    app.set_highlight_fill_shown(true);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = state_for(&app, folder(1));
    state.borrow_mut().sidecars[0].record(edit.clone());
    app.invoke_select(0);
    app.set_collapsed_light(false);
    assert!(app.get_highlight_fill());
    app.set_panel_tab("Masks".into());
    app.invoke_tab_changed("Masks".into());
    app.invoke_target_changed(1);
    app.set_collapsed_light(false);
    crate::testing::settle(&app);
    assert!(
        ElementHandle::find_by_element_id(&app, SWITCH)
            .next()
            .is_none(),
        "a mask's Light shows no switch"
    );
    let mut e = edit.clone();
    assert!(reset_section(&mut e, &Edit::default(), Some(0), "light"));
    assert_eq!(e.adjustments[0].look.light, Default::default());
    assert_eq!(e.highlight_fill.as_deref(), Some("hl-1"));
    let st = state.borrow();
    let read = read_edit(&app, &st.develop.edit, st.develop.target);
    assert_eq!(read.highlight_fill.as_deref(), Some("hl-1"));
}

/// The regional three, the switch, then the hairline, then the
/// clipping pair: the hairline sits under the switch, with a little
/// room about its line.
#[test]
fn the_hairline_parts_the_regional_three_from_the_clipping_pair() {
    let (app, _state, _worker) = open(1);
    by_id(&app, SWITCH);
    let bounds = |id: &str| {
        crate::testing::bounds_of(
            &ElementHandle::find_by_element_id(&app, id)
                .next()
                .unwrap_or_else(|| panic!("{id}")),
        )
    };
    let (fill_at, fill_size) = bounds(SWITCH);
    let (line_at, line_size) = bounds("LightSection::light-hairline");
    assert!(
        line_at.y >= fill_at.y + fill_size.height,
        "under the switch"
    );
    assert!(line_size.height > 1.0, "room about the line");
}

/// The switch shows the open frame's kind, never the last frame's: an
/// X-Trans frame greys it, the next frame picked frees it until its
/// open says, and an open that fails leaves it free; a develop that
/// fails on a frame whose open said keeps what it said.
#[test]
fn the_greyed_state_follows_the_frame_chosen() {
    let (app, state, _worker) = open(2);
    crate::panel::deliver::deliver(&app, opened(&state, xtrans()));
    assert!(!takes_a_hand(&app));
    let failed = |app: &App| {
        let generation = state.borrow().develop.generation;
        crate::panel::deliver::deliver(
            app,
            Outcome::Failed {
                generation,
                message: "unreadable".into(),
            },
        );
    };
    failed(&app);
    assert!(!takes_a_hand(&app), "its open said");
    app.invoke_select(1);
    assert!(takes_a_hand(&app), "not the last frame's");
    failed(&app);
    assert!(takes_a_hand(&app), "a failed open leaves it free");
    crate::panel::deliver::deliver(&app, opened(&state, xtrans()));
    assert!(!takes_a_hand(&app));
}

/// Left for the grid, the words go, the round waiting for the editor;
/// back, they come back with the frame. Into Cull, they go too.
#[test]
fn the_pending_word_waits_with_the_editor() {
    let (app, state, worker) = open(1);
    assert_eq!(
        land_develop(&app, &state, LearnedReport::Off, pending()),
        PENDING
    );
    crate::edited::note_editing(&mut state.borrow_mut(), &app, &worker, false);
    assert_eq!(app.get_status(), PENDING, "still editing");
    crate::edited::note_editing(&mut state.borrow_mut(), &app, &worker, true);
    assert_eq!(app.get_status(), "", "the grid: the round waits");
    // A first round that lands under the grid says nothing either.
    app.set_grid_open(true);
    assert_eq!(
        land_develop(&app, &state, LearnedReport::Off, pending()),
        ""
    );
    app.set_grid_open(false);
    crate::edited::note_editing(&mut state.borrow_mut(), &app, &worker, false);
    assert_eq!(app.get_status(), PENDING, "back on the frame");
    // What else the line says is kept either way.
    app.set_status(format!("Portra onto 2 frames; {PENDING}").into());
    crate::edited::note_editing(&mut state.borrow_mut(), &app, &worker, true);
    assert_eq!(app.get_status(), "Portra onto 2 frames");
    crate::edited::note_editing(&mut state.borrow_mut(), &app, &worker, false);
    assert_eq!(app.get_status(), format!("Portra onto 2 frames; {PENDING}"));
    app.invoke_cull_toggled();
    assert!(state.borrow().culling.cull.is_some());
    assert!(!app.get_status().contains(PENDING), "{}", app.get_status());
}

/// Without `GREYCARD_HIGHLIGHT_FILL=1` the switch is not drawn: no
/// element by its id, nothing a screen reader reaches by its name. An
/// edit that carries the fill all the same (a sidecar from a build that
/// drew it, a paste, a sync) reads back on, saves on, and a paste of it
/// carries it to another frame.
#[test]
fn hidden_the_switch_is_absent_and_a_carried_fill_stays_on() {
    let app = window(2);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    assert!(!app.get_highlight_fill_shown(), "hidden by default");
    let (state, _worker) = state_for(&app, folder(2));
    let filled = Edit {
        highlight_fill: Some("hl-1".into()),
        ..Edit::default()
    };
    state.borrow_mut().sidecars[0].record(filled.clone());
    app.invoke_select(0);
    app.set_collapsed_light(false);
    crate::testing::settle(&app);
    by_id(&app, "LightSection::light-hairline");
    assert!(
        ElementHandle::find_by_element_id(&app, SWITCH)
            .next()
            .is_none(),
        "no switch by its id"
    );
    assert!(
        crate::testing::spoken(&app, "Highlight fill").is_empty(),
        "nothing named for it"
    );
    assert!(app.get_highlight_fill(), "the edit's fill, shown on");
    {
        let st = state.borrow();
        assert_eq!(read_edit(&app, &st.develop.edit, None), filled);
    }
    app.set_exposure(0.3);
    save(&app, &state);
    let kept = current(&state, 0);
    assert_eq!(kept.light.exposure, 0.3);
    assert_eq!(kept.highlight_fill.as_deref(), Some("hl-1"));
    app.invoke_copy_asked();
    app.invoke_select(1);
    app.invoke_paste_asked();
    app.invoke_sync_applied();
    assert_eq!(current(&state, 1).highlight_fill.as_deref(), Some("hl-1"));
}

/// Hidden, the switch takes no room: the hairline sits under Shadows
/// as close as the switch sits drawn, as tall as it is drawn, and Light
/// is shorter by the switch's row and its spacing alone.
#[test]
fn hidden_the_switch_takes_no_room_in_light() {
    let (app, _state, _worker) = open(1);
    // Read without scrolling between, so each pair shares the panel's
    // offset.
    let bounds = |id: &str| {
        crate::testing::bounds_of(
            &ElementHandle::find_by_element_id(&app, id)
                .next()
                .unwrap_or_else(|| panic!("{id}")),
        )
    };
    let exposure = "LightSection::exposure";
    let hairline = "LightSection::light-hairline";
    by_id(&app, hairline);
    let (top_on, _) = bounds(exposure);
    let (fill_at, _) = bounds(SWITCH);
    let (line_on, line_size_on) = bounds(hairline);
    let row = line_on.y - fill_at.y;
    assert!(row > 0.0);
    app.set_highlight_fill_shown(false);
    crate::testing::settle(&app);
    assert!(
        ElementHandle::find_by_element_id(&app, SWITCH)
            .next()
            .is_none()
    );
    by_id(&app, hairline);
    let (top_off, _) = bounds(exposure);
    let (line_off, line_size_off) = bounds(hairline);
    assert_eq!(line_size_off, line_size_on, "the hairline as it was");
    let drop = (line_on.y - top_on.y) - (line_off.y - top_off.y);
    assert!(
        (drop - row).abs() < 0.5,
        "shorter by the switch's row ({row}) alone, not {drop}"
    );
    assert!(
        ((line_off.y - top_off.y) - (fill_at.y - top_on.y)).abs() < 0.5,
        "the hairline where the switch was"
    );
}
