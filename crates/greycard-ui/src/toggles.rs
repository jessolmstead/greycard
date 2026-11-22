//! The `Toggle`'s layout: the switch beside its label, or at the
//! row's right edge where a use asks; a label too long for the row cut
//! so the switch stays in it; and a click flipping it only on the
//! label, the gap, and the switch. Tried on a bench of our own, one
//! switch whose row we size and label as no panel does, and on the
//! editor's own rows.

use slint::ComponentHandle;
use slint::{LogicalPosition, LogicalSize};

use crate::testing::{Bounds, ToggleParts, click, press};

// No edition, as the editor's own build sets none: the bench compiles
// `controls.slint` under the same language behavior as the editor.
#[allow(deprecated)]
mod bench_ui {
    slint::slint! {
        import { Toggle, Tips } from "../../greycard-slint/ui/controls.slint";
        export component Bench inherits Window {
            width: 400px;
            height: 60px;
            in property <string> label;
            in property <length> row-width: 300px;
            in property <bool> at-end;
            in-out property <bool> on;
            in property <bool> enabled: true;
            in property <string> tip;
            // The hover text up, from the controls' own global.
            out property <string> tip-up: Tips.text;
            out property <length> label-x: t.label-x;
            out property <length> label-width: t.label-width;
            out property <length> switch-x: t.switch-x;
            out property <length> target-x: t.target-x;
            out property <length> target-width: t.target-width;
            // What holds the keys behind the row, as the window's own scope
            // does: focused at the start, and whether it still is.
            out property <bool> behind-focused: behind.has-focus;
            forward-focus: behind;
            behind := FocusScope { }
            t := Toggle {
                x: 20px;
                y: 10px;
                width: root.row-width;
                label: root.label;
                switch-at-end: root.at-end;
                enabled: root.enabled;
                tip: root.tip;
                checked <=> root.on;
            }
        }
    }
}
use bench_ui::Bench;

/// The switch's width, the gap after a label (`Theme.space-2`), and
/// the row's place and height on the bench.
const SWITCH: f32 = 34.0;
const GAP: f32 = 8.0;
const ROW_X: f32 = 20.0;
const ROW_Y: f32 = 10.0;
const ROW_H: f32 = 28.0;

const LONG: &str = "Traiter un JPEG à côté de son raw comme une image distincte";

fn bench(label: &str, at_end: bool) -> Bench {
    crate::testing::backend();
    let b = Bench::new().expect("the bench builds");
    b.set_label(label.into());
    b.set_at_end(at_end);
    b.show().expect("the bench shows");
    b
}

fn bounds(x: f32, width: f32) -> Bounds {
    (
        LogicalPosition::new(ROW_X + x, ROW_Y),
        LogicalSize::new(width, ROW_H),
    )
}

fn parts(b: &Bench) -> ToggleParts {
    ToggleParts {
        row: bounds(0.0, b.get_row_width()),
        label: bounds(b.get_label_x(), b.get_label_width()),
        switch: bounds(b.get_switch_x(), SWITCH),
        target: bounds(b.get_target_x(), b.get_target_width()),
    }
}

fn left(b: Bounds) -> f32 {
    b.0.x
}

fn right(b: Bounds) -> f32 {
    b.0.x + b.1.width
}

fn middle_y(b: Bounds) -> f32 {
    b.0.y + b.1.height / 2.0
}

fn click_at(b: &Bench, x: f32) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let position = LogicalPosition::new(x, ROW_Y + ROW_H / 2.0);
    let button = PointerEventButton::Left;
    for event in [
        WindowEvent::PointerMoved { position },
        WindowEvent::PointerPressed { position, button },
        WindowEvent::PointerReleased { position, button },
    ] {
        b.window().dispatch_event(event);
    }
}

/// The switch sits a gap after its label at the row's left, not at
/// its right edge.
#[test]
fn the_switch_sits_beside_its_label() {
    let b = bench("As shot", false);
    let p = parts(&b);
    assert_eq!(left(p.label), left(p.row));
    assert!(p.label.1.width > 20.0, "the label has its width: {p:?}");
    assert_eq!(left(p.switch), right(p.label) + GAP, "{p:?}");
    assert!(right(p.switch) < right(p.row) - 100.0, "{p:?}");
}

/// With no label the switch is where it always was, at the row's
/// right edge with no gap before it, whichever placement is asked
/// for, and the target is the switch alone.
#[test]
fn an_unlabeled_switch_keeps_the_right_edge() {
    for at_end in [false, true] {
        let b = bench("", at_end);
        let p = parts(&b);
        assert_eq!(right(p.switch), right(p.row), "{p:?}");
        assert_eq!(p.label.1.width, 0.0);
        assert_eq!(
            (left(p.target), right(p.target)),
            (left(p.switch), right(p.switch))
        );
        assert!(b.get_behind_focused(), "the keys start behind the row");
        click_at(&b, left(p.row) + 10.0);
        assert!(!b.get_on(), "the row's empty left is not the switch");
        assert!(b.get_behind_focused(), "nor does it take the keys");
        click_at(&b, left(p.switch) + 4.0);
        assert!(b.get_on());
    }
}

/// A label longer than the row is cut short of the switch and the
/// switch stays inside the row, at any row width down to the switch's
/// own, beside its label or at the edge.
#[test]
fn a_long_label_never_pushes_the_switch_out() {
    for at_end in [false, true] {
        let b = bench(LONG, at_end);
        for row in [300.0, 160.0, 60.0, 42.0, SWITCH] {
            b.set_row_width(row);
            let p = parts(&b);
            let says = format!("at the end {at_end}, a row of {row}: {p:?}");
            assert!(left(p.label) >= left(p.row), "{says}");
            if p.label.1.width > 0.0 {
                assert!(right(p.label) + GAP <= left(p.switch) + 0.01, "{says}");
            }
            assert!(left(p.switch) >= left(p.row), "{says}");
            // A row narrower than the switch cannot hold it; it starts
            // at the row's left all the same.
            if row >= SWITCH {
                assert!(right(p.switch) <= right(p.row) + 0.01, "{says}");
                // Cut, the label fills what the switch leaves.
                assert_eq!(right(p.switch), right(p.row), "{says}");
            }
            assert!(right(p.target) <= right(p.switch) + 0.01, "{says}");
        }
        // Only as wide as the switch: the switch fills the row, the
        // label has no room.
        b.set_row_width(SWITCH);
        let p = parts(&b);
        assert_eq!(
            (left(p.switch), right(p.switch)),
            (left(p.row), right(p.row))
        );
        assert_eq!(p.label.1.width, 0.0);
    }
}

/// At the edge, the switch keeps the row's right and the label sits a
/// gap left of it; the target runs from the label to the switch.
#[test]
fn at_the_end_the_label_sits_against_the_switch() {
    let b = bench("As shot", true);
    let p = parts(&b);
    assert_eq!(right(p.switch), right(p.row), "{p:?}");
    assert_eq!(right(p.label) + GAP, left(p.switch), "{p:?}");
    assert!(left(p.label) > left(p.row) + 100.0, "{p:?}");
    assert_eq!(
        (left(p.target), right(p.target)),
        (left(p.label), right(p.switch))
    );
    assert!(b.get_behind_focused(), "the keys start behind the row");
    click_at(&b, left(p.row) + 10.0);
    assert!(!b.get_on(), "the empty run left of the label");
    assert!(b.get_behind_focused(), "nor does it take the keys");
    click_at(&b, left(p.label) + 4.0);
    assert!(b.get_on(), "the label flips it");
}

/// A click on the label, the gap, or the switch flips it; one in the
/// row's empty run past the switch does not.
#[test]
fn only_the_label_the_gap_and_the_switch_flip_it() {
    let b = bench("As shot", false);
    let p = parts(&b);
    assert_eq!(
        (left(p.target), right(p.target)),
        (left(p.label), right(p.switch))
    );
    click_at(&b, right(p.switch) + 10.0);
    click_at(&b, right(p.row) - 12.0);
    assert!(!b.get_on(), "the empty run is not the switch");
    assert!(b.get_behind_focused(), "nor does it take the keys");
    click_at(&b, left(p.label) + 4.0);
    assert!(b.get_on(), "the label flips it");
    click_at(&b, right(p.label) + GAP / 2.0);
    assert!(!b.get_on(), "the gap flips it");
    click_at(&b, left(p.switch) + SWITCH / 2.0);
    assert!(b.get_on(), "the switch flips it");
}

/// In the editor: As shot keeps its switch at the White balance row's
/// right edge, the words against it, and a click there flips it; the
/// left pane's With subfolders has its switch beside the words, and a
/// click at the row's right edge leaves it be. A click on either row's
/// empty run does not take the keys either: Space after it does not
/// flip the switch.
#[test]
fn the_editors_switches_sit_where_their_rows_ask() {
    let app = crate::testing::window(1);
    app.set_panel_tab("Develop".into());
    app.set_collapsed_wb(false);
    let shot = crate::testing::toggle_by_id(&app, "WhiteBalanceSection::as-shot-toggle");
    assert_eq!(right(shot.switch), right(shot.row), "{shot:?}");
    assert_eq!(right(shot.label) + GAP, left(shot.switch), "{shot:?}");
    let was = app.get_as_shot();
    click(&app, left(shot.row) + 6.0, middle_y(shot.row));
    assert_eq!(app.get_as_shot(), was, "the row's empty left");
    press(&app, " ");
    assert_eq!(app.get_as_shot(), was, "Space after the row's empty left");

    app.set_folder_tree_shown(true);
    let sub = crate::testing::toggle_by_id(&app, "FoldersSection::subfolders-toggle");
    assert_eq!(left(sub.switch), right(sub.label) + GAP, "{sub:?}");
    assert!(right(sub.switch) < right(sub.row) - 40.0, "{sub:?}");
    let was_sub = app.get_folder_tree_subfolders();
    click(&app, right(sub.row) - 6.0, middle_y(sub.row));
    assert_eq!(
        app.get_folder_tree_subfolders(),
        was_sub,
        "the row's empty right"
    );
    press(&app, " ");
    assert_eq!(
        app.get_folder_tree_subfolders(),
        was_sub,
        "Space after the row's empty right"
    );

    crate::testing::click_switch_by_id(&app, "WhiteBalanceSection::as-shot-toggle");
    assert_eq!(app.get_as_shot(), !was);
    let sub = crate::testing::toggle_by_id(&app, "FoldersSection::subfolders-toggle");
    let was = was_sub;
    click(&app, left(sub.label) + 4.0, middle_y(sub.row));
    assert_eq!(app.get_folder_tree_subfolders(), !was, "the words");
}

/// The pointer moved onto `x` on the row and left to rest there.
fn rest_at(b: &Bench, x: f32) {
    use slint::platform::WindowEvent;
    let position = LogicalPosition::new(x, ROW_Y + ROW_H / 2.0);
    b.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    slint::platform::update_timers_and_animations();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(800));
    slint::platform::update_timers_and_animations();
}

/// A greyed switch with a tip says it at the pointer after a rest, on
/// the label and on the switch, and a click there still flips nothing
/// and takes no keys; one with no tip says nothing; and a tip goes
/// when the pointer leaves.
#[test]
fn a_greyed_switch_says_its_tip_and_still_flips_nothing() {
    const WHY: &str = "Not in the lens profile.";
    let b = bench("Vignetting", false);
    b.set_enabled(false);
    let p = parts(&b);
    rest_at(&b, left(p.label) + 4.0);
    assert_eq!(b.get_tip_up(), "", "no tip, nothing said");
    rest_at(&b, right(p.row) - 4.0);

    b.set_tip(WHY.into());
    rest_at(&b, left(p.switch) + 4.0);
    assert_eq!(b.get_tip_up(), WHY);
    rest_at(&b, right(p.row) - 4.0);
    assert_eq!(b.get_tip_up(), "", "gone on leaving");
    rest_at(&b, left(p.label) + 4.0);
    assert_eq!(b.get_tip_up(), WHY);
    click_at(&b, left(p.label) + 4.0);
    click_at(&b, left(p.switch) + 4.0);
    assert!(!b.get_on(), "greyed, it flips nothing");
    assert!(b.get_behind_focused(), "nor takes the keys");

    // Back in hand, the tip taken away: a click flips it again.
    b.set_tip("".into());
    b.set_enabled(true);
    click_at(&b, left(p.switch) + 4.0);
    assert!(b.get_on());
}

/// A screen reader's press on a section's header switch is a click:
/// LENS's flips and asks for a develop, and says on or off as it
/// comes to; the picture's COLOR MIXER, taken over by a black and
/// white, is greyed and flips nothing and asks nothing.
#[test]
fn a_screen_reader_presses_a_section_switch_as_a_click_does() {
    use i_slint_backend_testing::AccessibleRole;
    use std::cell::Cell;
    use std::rc::Rc;
    let app = crate::testing::window(1);
    app.set_panel_tab("Develop".into());
    let asked = Rc::new(Cell::new(0));
    app.on_develop_changed({
        let asked = asked.clone();
        move || asked.set(asked.get() + 1)
    });
    app.on_view_changed({
        let asked = asked.clone();
        move || asked.set(asked.get() + 1)
    });
    let switch = |section: &str| {
        crate::testing::section_bounds(&app, section, "Section::header-switch");
        let e = crate::testing::section_part(&app, section, "Section::header-switch")
            .unwrap_or_else(|| panic!("no switch in {section}"));
        assert_eq!(e.accessible_role(), Some(AccessibleRole::Switch));
        assert_eq!(e.accessible_checkable(), Some(true));
        e
    };

    app.set_lens_enabled(true);
    let lens = switch("LensSection");
    assert_eq!(lens.accessible_checked(), Some(true));
    lens.invoke_accessible_default_action();
    assert!(!app.get_lens_enabled(), "the press flips it");
    assert_eq!(asked.get(), 1, "and asks for a develop");
    assert_eq!(lens.accessible_checked(), Some(false), "and says so");
    // The press took the keys, as a click does: Space flips it back.
    press(&app, " ");
    assert!(app.get_lens_enabled(), "Space after the press");
    assert_eq!(asked.get(), 2);

    app.set_mixer_enabled(true);
    app.set_bw_enabled(true);
    let mixer = switch("MixerSection");
    assert_eq!(mixer.accessible_enabled(), Some(false));
    mixer.invoke_accessible_default_action();
    assert!(app.get_mixer_enabled(), "greyed: no flip");
    assert_eq!(asked.get(), 2, "greyed: no ask");
}
