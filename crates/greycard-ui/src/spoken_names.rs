//! Every control a screen reader reaches has a name to say: on each
//! panel tab, in Cull, the grid and the lightbox, and in every sheet,
//! we walk each button, check, switch, choice, slider, and field the
//! window draws, and one with nothing to say fails. A button, a check,
//! a switch, a choice's option, a tab, and a list's item may be named
//! by the words drawn in it, as a screen reader names them; a row of
//! choices, a slider, and a field must carry their own. A slider says
//! its value too, and every choice sits in a named group.
//!
//! A control a screen reader cannot find at all is no gap to this
//! walk, so we also pin how many buttons, switches, and groups each
//! place has at least: a role taken away fails there.

use std::collections::{BTreeMap, HashSet};
use std::ops::ControlFlow;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::testing::{folder, settle, state_for, window};
use crate::{App, FilterChip, HistoryRow, PresetRow};

/// The roles a hand works: what a screen reader offers to press, turn,
/// move, or type into.
const INTERACTIVE: [AccessibleRole; 11] = [
    AccessibleRole::Button,
    AccessibleRole::Checkbox,
    AccessibleRole::Combobox,
    AccessibleRole::Slider,
    AccessibleRole::Spinbox,
    AccessibleRole::Tab,
    AccessibleRole::TextInput,
    AccessibleRole::Switch,
    AccessibleRole::ListItem,
    AccessibleRole::RadioButton,
    AccessibleRole::RadioGroup,
];

/// The roles a screen reader names by the words drawn inside them
/// when they carry no label of their own.
const NAMED_BY_CONTENTS: [AccessibleRole; 6] = [
    AccessibleRole::Button,
    AccessibleRole::Checkbox,
    AccessibleRole::Switch,
    AccessibleRole::RadioButton,
    AccessibleRole::Tab,
    AccessibleRole::ListItem,
];

/// The roles we count per place, in this order in [`LEAST`].
const COUNTED: [AccessibleRole; 3] = [
    AccessibleRole::Button,
    AccessibleRole::Switch,
    AccessibleRole::RadioGroup,
];

/// The fewest buttons, switches, and radio groups each place has with
/// the walk's fixture, as counted when we pinned them; a role taken
/// off a shared component (a section's fold, a filter chip, a mask's
/// switch, a row of choices' group) drops a count under its pin. Raise
/// a pin when a place gains controls; lower one only for a control
/// taken out on purpose.
const LEAST: [(&str, [usize; 3]); 34] = [
    ("Cull", [53, 27, 13]),
    ("the Crop tab", [51, 1, 2]),
    ("the Develop tab", [73, 27, 14]),
    ("the Lightroom sheet", [75, 27, 14]),
    ("the Masks tab", [77, 15, 6]),
    ("the RawTherapee sheet", [75, 27, 14]),
    ("the Retouch tab", [41, 1, 2]),
    ("the archive rejects sheet", [74, 27, 14]),
    ("the archive sheet", [75, 27, 14]),
    ("the collection sheet", [75, 27, 14]),
    ("the delete sheet", [75, 27, 14]),
    ("the export sheet", [79, 28, 24]),
    ("the grid", [68, 27, 14]),
    ("the import sheet", [78, 28, 15]),
    ("the keys sheet", [74, 27, 14]),
    ("the lightbox", [63, 27, 14]),
    ("the look remove sheet", [75, 27, 14]),
    ("the look rename sheet", [75, 27, 14]),
    ("the match sheet", [75, 28, 15]),
    ("the model sheet", [75, 27, 14]),
    ("the overwrite sheet", [76, 27, 14]),
    ("the preset group sheet", [75, 27, 14]),
    ("the preset report", [74, 27, 14]),
    ("the preset sheet", [77, 27, 14]),
    ("the rejects sheet", [75, 27, 14]),
    ("the root sheet", [75, 27, 14]),
    ("the settings sheet, About", [78, 28, 14]),
    ("the settings sheet, Editing", [75, 28, 22]),
    ("the settings sheet, Interface", [75, 28, 17]),
    ("the settings sheet, Keys", [114, 27, 14]),
    ("the settings sheet, Library", [74, 31, 17]),
    ("the settings sheet, Models", [75, 27, 14]),
    ("the sync sheet", [77, 27, 14]),
    ("the time shift sheet", [76, 27, 15]),
];

/// Controls left unnamed on purpose, by element id, each with why.
const UNNAMED_ON_PURPOSE: [(&str, &str); 0] = [];

fn spoken(words: Option<slint::SharedString>) -> bool {
    words.is_some_and(|w| !w.trim().is_empty())
}

/// Whether `e` has words a screen reader can say for it.
fn named(e: &ElementHandle, role: AccessibleRole) -> bool {
    if spoken(e.accessible_label()) {
        return true;
    }
    NAMED_BY_CONTENTS.contains(&role)
        && e.query_descendants()
            .match_accessible_role(AccessibleRole::Text)
            .match_predicate(|t| spoken(t.accessible_label()))
            .find_first()
            .is_some()
}

/// What `e` is, without where: the same control in two places (the
/// panel behind a sheet, or moved by Cull's section) is one gap.
fn what(e: &ElementHandle, role: AccessibleRole) -> String {
    let inside: Vec<String> = e
        .query_descendants()
        .match_predicate(|t| spoken(t.accessible_label()))
        .find_all()
        .iter()
        .filter_map(|t| t.accessible_label().map(|l| l.to_string()))
        .take(6)
        .collect();
    format!(
        "{role:?} {} ({}), value {:?}, inside {inside:?}",
        e.id().unwrap_or_default(),
        e.type_name().unwrap_or_default(),
        e.accessible_value().unwrap_or_default(),
    )
}

/// A box in the window: left, top, width, height.
type Bounds = (f32, f32, f32, f32);

fn box_of(e: &ElementHandle) -> Bounds {
    let (at, size) = (e.absolute_position(), e.size());
    (at.x, at.y, size.width, size.height)
}

/// Whether `group` holds `option`: round it (a row's or a grid's box),
/// or, for a row whose word is its group, on its line and left of it.
fn holds(group: Bounds, option: Bounds) -> bool {
    let (cx, cy) = (option.0 + option.2 / 2.0, option.1 + option.3 / 2.0);
    let round =
        cx >= group.0 && cx <= group.0 + group.2 && cy >= group.1 && cy <= group.1 + group.3;
    let beside = cy >= group.1 && cy <= group.1 + group.3 && group.0 + group.2 <= option.0 + 1.0;
    round || beside
}

/// What a screen reader finds wrong in the window as it is, each a
/// key (what it is) and a line (with where), and how many of the
/// [`COUNTED`] roles it finds.
struct Survey {
    gaps: Vec<(String, String)>,
    counts: [usize; 3],
}

fn survey(app: &App) -> Survey {
    let mut gaps = Vec::new();
    let mut counts = [0; 3];
    let mut groups = Vec::new();
    let mut buttons = Vec::new();
    let mut options = Vec::new();
    let mut texts = Vec::new();
    let mut labeled = Vec::new();
    app.root_element().visit_descendants(|e| {
        let Some(role) = e.accessible_role() else {
            return ControlFlow::<()>::Continue(());
        };
        if let Some(k) = COUNTED.iter().position(|r| *r == role) {
            counts[k] += 1;
        }
        if role == AccessibleRole::Text
            && let Some(words) = e.accessible_label().filter(|w| !w.trim().is_empty())
        {
            texts.push((words.to_lowercase(), box_of(&e)));
        }
        if !INTERACTIVE.contains(&role) {
            return ControlFlow::Continue(());
        }
        if let Some(words) = e.accessible_label().filter(|w| !w.trim().is_empty()) {
            let who = format!(
                "{role:?} {} ({})",
                e.id().unwrap_or_default(),
                e.type_name().unwrap_or_default()
            );
            labeled.push((words.to_lowercase(), box_of(&e), who));
        }
        match role {
            AccessibleRole::RadioGroup => groups.push(box_of(&e)),
            AccessibleRole::Button => buttons.push(box_of(&e)),
            AccessibleRole::RadioButton => options.push((box_of(&e), what(&e, role))),
            _ => {}
        }
        let id = e.id().unwrap_or_default();
        if UNNAMED_ON_PURPOSE.iter().any(|(i, _)| *i == id.as_str()) {
            return ControlFlow::Continue(());
        }
        let (x, y, w, h) = box_of(&e);
        let at = format!(" at {x:.0},{y:.0} {w:.0}x{h:.0}");
        if !named(&e, role) {
            let key = format!("no name: {}", what(&e, role));
            gaps.push((key.clone(), key + &at));
        } else if role == AccessibleRole::Slider && !spoken(e.accessible_value()) {
            let key = format!("no value: {}", what(&e, role));
            gaps.push((key.clone(), key + &at));
        }
        ControlFlow::Continue(())
    });
    // A choice with no group round it or beside it is said with
    // nothing to tell what it chooses; one inside a button (the Cull
    // button's one option) is the button's.
    for (option, what) in options {
        let grouped = groups.iter().any(|g| holds(*g, option));
        let in_button = buttons.iter().any(|b| {
            let (cx, cy) = (option.0 + option.2 / 2.0, option.1 + option.3 / 2.0);
            cx >= b.0 && cx <= b.0 + b.2 && cy >= b.1 && cy <= b.1 + b.3
        });
        if !grouped && !in_button {
            let key = format!("no group: {what}");
            let at = format!(" at {:.0},{:.0}", option.0, option.1);
            gaps.push((key.clone(), key + &at));
        }
    }
    // A label a screen reader reaches as words of its own, inside the
    // control it names or beside it on its line, is said twice: once
    // as the control's name and once as the text.
    const BESIDE: f32 = 24.0;
    for (words, text) in &texts {
        let cy = text.1 + text.3 / 2.0;
        for (name, control, who) in &labeled {
            let on_line = cy >= control.1 && cy <= control.1 + control.3;
            let near =
                text.0 <= control.0 + control.2 + BESIDE && text.0 + text.2 >= control.0 - BESIDE;
            if name == words && on_line && near {
                let key = format!("said twice: {words:?}, the name of {who}");
                let at = format!(" at {:.0},{:.0}", text.0, text.1);
                gaps.push((key.clone(), key + &at));
            }
        }
    }
    gaps.sort();
    gaps.dedup();
    Survey { gaps, counts }
}

/// Every section open, as tall a window as the panel needs to show
/// them all at once: a walk sees only what is drawn in view.
fn open_every_section(app: &App) {
    app.set_collapsed_metadata(false);
    app.set_collapsed_collections(false);
    app.set_collapsed_folders(false);
    app.set_collapsed_folders_library(false);
    app.set_collapsed_folders_loupe(false);
    app.set_collapsed_roots(false);
    app.set_collapsed_culling(false);
    app.set_collapsed_navigator(false);
    app.set_collapsed_adjustments(false);
    app.set_collapsed_mask(false);
    app.set_collapsed_wb(false);
    app.set_collapsed_light(false);
    app.set_collapsed_curves(false);
    app.set_collapsed_mixer(false);
    app.set_collapsed_color(false);
    app.set_collapsed_bw(false);
    app.set_collapsed_grading(false);
    app.set_collapsed_tint(false);
    app.set_collapsed_noise(false);
    app.set_collapsed_lens(false);
    app.set_collapsed_detail(false);
    app.set_collapsed_output_sharpen(false);
    app.set_collapsed_sharpen(false);
    app.set_collapsed_vignette(false);
    app.set_collapsed_grain(false);
    app.set_collapsed_proof(false);
    app.set_collapsed_monitor(false);
    app.set_collapsed_crop(false);
    app.set_collapsed_rotate(false);
    app.set_collapsed_perspective(false);
    app.set_collapsed_demosaic(false);
    app.set_collapsed_camera(false);
    app.set_collapsed_look(false);
    app.set_collapsed_retouch(false);
    app.set_collapsed_presets(false);
    app.set_collapsed_snapshots(false);
    app.set_collapsed_history(false);
}

fn strings(words: &[&str]) -> ModelRc<slint::SharedString> {
    ModelRc::new(VecModel::from(
        words
            .iter()
            .map(|w| (*w).into())
            .collect::<Vec<slint::SharedString>>(),
    ))
}

fn flags(on: &[bool]) -> ModelRc<bool> {
    ModelRc::new(VecModel::from(on.to_vec()))
}

/// The lists' rows a frame brings: two masks, the first chosen with two
/// shapes, a snapshot, history steps, a group of presets, and a
/// retouch patch, so their rows are walked too; and an edit, so
/// History's reset is.
fn fill_the_lists(app: &App) {
    app.set_adjustment_names(strings(&["Global", "Mask 1", "Mask 2"]));
    app.set_adjustment_on(flags(&[true, false]));
    app.set_target(1);
    app.set_component_names(strings(&["Radial 1", "Linear 2"]));
    app.set_component_on(flags(&[true, false]));
    app.set_shape_invert(flags(&[false, true]));
    app.set_component(0);
    app.set_snapshot_names(strings(&["Before"]));
    app.set_history_rows(ModelRc::new(VecModel::from(vec![
        HistoryRow {
            name: "Open".into(),
            ..Default::default()
        },
        HistoryRow {
            name: "Exposure".into(),
            ..Default::default()
        },
    ])));
    app.set_preset_names(strings(&["Warm"]));
    app.set_preset_list_rows(ModelRc::new(VecModel::from(vec![
        PresetRow {
            header: true,
            text: "Looks".into(),
            group: "Looks".into(),
            ..Default::default()
        },
        PresetRow {
            text: "Warm".into(),
            group: "Looks".into(),
            index: 0,
            ..Default::default()
        },
    ])));
    app.set_patch_names(strings(&["Patch 1"]));
    app.set_patch(0);
    // An edited frame: History offers its Reset to Original.
    app.set_history_resettable(true);
}

/// The fixture's rows, by id, with how many each tab must draw: a list
/// we filled but the window never drew would leave its rows unwalked.
const FIXTURE_ROWS: [(&str, &str, usize); 6] = [
    ("Develop", "PresetsSection::preset-row", 2),
    ("Develop", "HistorySection::history-row", 2),
    ("Develop", "SnapshotsSection::snapshot-row", 1),
    ("Masks", "AdjustmentsList::row", 3),
    ("Masks", "AdjustmentsSection::shape-row", 2),
    ("Retouch", "RetouchSection::prow", 1),
];

/// How many elements with `id` the window draws now.
fn drawn(app: &App, id: &str) -> usize {
    let mut n = 0;
    app.root_element().visit_descendants(|e| {
        if e.id().is_some_and(|i| i == id) {
            n += 1;
        }
        ControlFlow::<()>::Continue(())
    });
    n
}

/// The fixture's rows a tab draws fewer of than it should.
fn missing_rows(app: &App, tab: &str) -> Vec<String> {
    FIXTURE_ROWS
        .iter()
        .filter(|(t, _, _)| *t == tab)
        .filter_map(|(_, id, least)| {
            let n = drawn(app, id);
            (n < *least).then(|| format!("the {tab} tab: {n} {id} drawn, the fixture has {least}"))
        })
        .collect()
}

/// The window's every place: each panel tab, Cull, the grid, the
/// lightbox, and each sheet in turn, by the name a failure gives it,
/// and what a screen reader finds in each.
fn walk() -> Vec<(String, Survey)> {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1600.0, 9000.0));
    let (_state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    open_every_section(&app);
    fill_the_lists(&app);
    settle(&app);
    let mut places = Vec::new();
    let mut look = |name: &str, app: &App| {
        settle(app);
        places.push((name.to_string(), survey(app)));
    };
    let mut unwalked = Vec::new();
    for tab in ["Develop", "Crop", "Masks", "Retouch"] {
        app.set_panel_tab(tab.into());
        look(&format!("the {tab} tab"), &app);
        unwalked.extend(missing_rows(&app, tab));
    }
    assert!(
        unwalked.is_empty(),
        "rows the fixture fills that the walk never saw:\n{}",
        unwalked.join("\n")
    );
    app.set_panel_tab("Develop".into());
    app.set_culling(true);
    look("Cull", &app);
    app.set_culling(false);
    app.set_grid_open(true);
    look("the grid", &app);
    app.set_lightbox(true);
    look("the lightbox", &app);
    app.set_lightbox(false);
    app.set_grid_open(false);
    // What the sheets' callers set before they open them, which names
    // their fields; and the export's watermark and border shown.
    app.set_time_shift_title("Shift the time".into());
    app.set_look_rename_title("Rename the look".into());
    app.set_collection_sheet_title("New collection".into());
    app.set_archive_confirm("Archive".into());
    app.set_export_mark("Text".into());
    app.set_export_border(true);
    settle(&app);
    type Sheet = (&'static str, fn(&App, bool));
    let sheets: [Sheet; 22] = [
        ("the sync sheet", App::set_sync_open),
        ("the collection sheet", App::set_collection_sheet_open),
        ("the archive sheet", App::set_archive_open),
        ("the archive rejects sheet", App::set_archive_rejects_open),
        ("the root sheet", App::set_root_sheet_open),
        ("the rejects sheet", App::set_rejects_open),
        ("the delete sheet", App::set_delete_open),
        ("the preset group sheet", App::set_preset_group_sheet_open),
        ("the preset report", App::set_preset_report_open),
        ("the preset sheet", App::set_preset_open),
        ("the look remove sheet", App::set_look_remove_open),
        ("the look rename sheet", App::set_look_rename_open),
        ("the time shift sheet", App::set_time_shift_open),
        ("the overwrite sheet", App::set_overwrite_open),
        ("the match sheet", App::set_match_open),
        ("the model sheet", App::set_fetch_open),
        ("the export sheet", App::set_export_open),
        ("the import sheet", App::set_import_open),
        ("the Lightroom sheet", App::set_lr_open),
        ("the RawTherapee sheet", App::set_rt_open),
        ("the keys sheet", App::set_keys_open),
        ("the settings sheet", App::set_settings_open),
    ];
    for (name, open) in sheets {
        open(&app, true);
        if name == "the settings sheet" {
            for category in ["Interface", "Library", "Editing", "Models", "Keys", "About"] {
                app.set_settings_category(category.into());
                look(&format!("{name}, {category}"), &app);
            }
        } else {
            look(name, &app);
        }
        open(&app, false);
        settle(&app);
    }
    places
}

#[test]
fn every_control_a_screen_reader_reaches_has_a_name() {
    let places = walk();
    // A gap behind a sheet is the place's under it, said once: where
    // it was first found.
    let mut seen = HashSet::new();
    let mut failures: Vec<String> = places
        .iter()
        .flat_map(|(place, s)| {
            s.gaps
                .iter()
                .filter(|(key, _)| seen.insert(key.clone()))
                .map(move |(_, line)| format!("{place}: {line}"))
                .collect::<Vec<_>>()
        })
        .collect();
    let counted: BTreeMap<&str, [usize; 3]> =
        places.iter().map(|(p, s)| (p.as_str(), s.counts)).collect();
    for (place, least) in LEAST {
        let had = counted
            .get(place)
            .unwrap_or_else(|| panic!("no place {place:?} walked"));
        for k in 0..3 {
            if had[k] < least[k] {
                failures.push(format!(
                    "{place}: {} {:?} nodes, at least {} pinned",
                    had[k], COUNTED[k], least[k]
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} things a screen reader would miss:\n{}\ncounts (Button, Switch, RadioGroup): {counted:?}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(
        LEAST.len(),
        places.len(),
        "a pin for every place: {counted:?}"
    );
}

/// The elements a screen reader knows as `label` in the role `role`.
fn found(app: &App, role: AccessibleRole, label: &str) -> Vec<ElementHandle> {
    settle(app);
    ElementHandle::find_by_accessible_label(app, label)
        .filter(|e| e.accessible_role() == Some(role))
        .collect()
}

/// A filter chip held out (Ctrl+click) hides its matches: it says
/// "excluded", not the "not checked" of a chip merely off, and one on
/// or off still says which.
#[test]
fn a_chip_held_out_says_excluded() {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1600.0, 2000.0));
    let (_state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    app.set_grid_open(true);
    settle(&app);
    // A chip held out and one ticked, as a Ctrl+click and a click leave
    // them.
    let chip = |text: &str, on: bool, out: bool| FilterChip {
        text: text.into(),
        count: 1,
        on,
        out,
        code: 0,
    };
    let (out, on) = ("Picked", "Rejected");
    app.set_filter_flags(ModelRc::new(VecModel::from(vec![
        chip(out, false, true),
        chip(on, true, false),
    ])));
    let held = found(&app, AccessibleRole::Button, out);
    assert!(!held.is_empty(), "the held-out chip {out:?}");
    for chip in held {
        assert_eq!(chip.accessible_description().as_deref(), Some("excluded"));
        assert_eq!(chip.accessible_checkable(), Some(false), "{out:?}");
    }
    let ticked = found(&app, AccessibleRole::Button, on);
    assert!(!ticked.is_empty(), "the chip {on:?}");
    for chip in ticked {
        assert_eq!(chip.accessible_description().as_deref(), Some(""));
        assert_eq!(chip.accessible_checkable(), Some(true));
        assert_eq!(chip.accessible_checked(), Some(true));
    }
}

/// A frame's cell in the strip and on the grid is an item a screen
/// reader names by its file, says chosen or not, and presses: the
/// press picks the frame as a click does.
#[test]
fn a_screen_reader_presses_a_frame_in_the_strip_and_the_grid() {
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1600.0, 1000.0));
    let (_state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    settle(&app);
    // The thumbs' names, as `window` gives them.
    let names = ["f00.CR3", "f01.CR3", "f02.CR3"];
    // The cells of `place` (the strip stays under the grid) named `name`.
    let cells = |place: &str, name: &str| -> Vec<ElementHandle> {
        found(&app, AccessibleRole::ListItem, name)
            .into_iter()
            .filter(|e| e.id().is_some_and(|i| i == format!("{place}::cell")))
            .collect()
    };
    let press = |place: &str, i: usize| {
        let name = names[i];
        let before = cells(place, name);
        assert_eq!(before.len(), 1, "{place}: one cell {name:?}");
        assert_eq!(before[0].accessible_item_selected(), Some(false), "{place}");
        before[0].invoke_accessible_default_action();
        settle(&app);
        assert_eq!(
            app.get_selected(),
            i as i32,
            "{place}: the press picks {name:?}"
        );
        let after = cells(place, name);
        assert_eq!(after[0].accessible_item_selected(), Some(true), "{place}");
    };
    press("Filmstrip", 1);
    app.set_grid_open(true);
    press("GridSheet", 2);
}

/// A mask's row, a shape's invert, and the soft proof's badge are
/// buttons a screen reader presses, each as a click does: the row
/// chooses its mask, the invert flips its shape, the badge ends the
/// proof. A row's press while a menu is up is nothing.
#[test]
fn a_screen_reader_presses_a_mask_an_invert_and_the_proof_badge() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let app = window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1600.0, 2000.0));
    fill_the_lists(&app);
    app.set_panel_tab("Masks".into());
    app.set_collapsed_adjustments(false);
    let heard = Rc::new(RefCell::new(Vec::new()));
    app.on_target_changed({
        let heard = heard.clone();
        move |i| heard.borrow_mut().push(("target", i))
    });
    app.on_invert_shape_toggled({
        let heard = heard.clone();
        move |i| heard.borrow_mut().push(("invert", i))
    });
    let button = |id: &str, nth: usize| {
        settle(&app);
        let e = ElementHandle::find_by_element_id(&app, id)
            .nth(nth)
            .unwrap_or_else(|| panic!("no {id} {nth}"));
        assert_eq!(e.accessible_role(), Some(AccessibleRole::Button), "{id}");
        e
    };

    // While a menu is up a row's press is nothing, as a click then
    // only closes the menu.
    app.set_menu_up(true);
    button("AdjustmentsList::mask-item", 1).invoke_accessible_default_action();
    app.set_menu_up(false);
    // Global's row is the first; Mask 2's the third.
    button("AdjustmentsList::mask-item", 2).invoke_accessible_default_action();
    // The second shape is inverted, the first not; each invert says so.
    let inverts = || -> Vec<Option<bool>> {
        settle(&app);
        ElementHandle::find_by_element_id(&app, "AdjustmentsSection::invert-button")
            .map(|e| e.accessible_checked())
            .collect()
    };
    assert_eq!(inverts(), [Some(false), Some(true)]);
    button("AdjustmentsSection::invert-button", 1).invoke_accessible_default_action();
    // The press took the keys, as a click does: Space asks again.
    crate::testing::press(&app, " ");
    assert_eq!(
        *heard.borrow(),
        [("target", 2), ("invert", 1), ("invert", 1)]
    );
    app.set_shape_invert(flags(&[true, false]));
    assert_eq!(inverts(), [Some(true), Some(false)], "and follows it");

    app.set_proofing(true);
    button("Viewport::proof-pill", 0).invoke_accessible_default_action();
    assert!(!app.get_proofing(), "the badge's press ends the proof");
}

/// The words of every item of the menu up now, and how many items have
/// none: Slint draws a menu's items itself, each its words in a text.
fn menu_items(app: &App) -> (Vec<String>, usize) {
    settle(app);
    let mut words = Vec::new();
    let mut unnamed = 0;
    for item in ElementHandle::find_by_element_type_name(app, "MenuItemBase") {
        // A separator draws no press area and says nothing, rightly.
        let pressable = item
            .query_descendants()
            .match_id("MenuItemBase::touch-area")
            .find_first()
            .is_some();
        if !pressable {
            continue;
        }
        let label = item
            .query_descendants()
            .match_id("MenuItemBase::label")
            .find_first()
            .and_then(|t| t.accessible_label())
            .filter(|l| !l.trim().is_empty());
        match label {
            Some(l) => words.push(l.to_string()),
            None => unnamed += 1,
        }
    }
    (words, unnamed)
}

fn center(e: &ElementHandle) -> (f32, f32) {
    let (x, y, w, h) = box_of(e);
    (x + w / 2.0, y + h / 2.0)
}

/// The window's menus, each opened as a hand opens it: every item has
/// words to say, and whatever else the open menu brings with a role
/// has its name, as the walk asks of the window.
///
/// Slint 1.19 gives a menu's items no role of their own (its
/// `MenuItemBase` sets none, and `AccessibleRole` has no menu item),
/// so a screen reader reads them as text; we cannot give them one
/// from here.
#[test]
fn every_menu_item_has_a_name() {
    use slint::platform::{Key, PointerEventButton};
    let app = window(3);
    app.window()
        .set_size(slint::LogicalSize::new(1600.0, 2000.0));
    let (_state, _worker) = state_for(&app, folder(3));
    app.invoke_select(0);
    fill_the_lists(&app);
    settle(&app);
    let right_click = |app: &App, at: (f32, f32)| {
        crate::testing::click_with(app, PointerEventButton::Right, at.0, at.1);
    };
    let by_id = |app: &App, id: &str, nth: usize| {
        settle(app);
        let e = ElementHandle::find_by_element_id(app, id)
            .nth(nth)
            .unwrap_or_else(|| panic!("no {id}"));
        center(&e)
    };
    type Open = Box<dyn Fn(&App)>;
    let menus: Vec<(&str, usize, Open)> = vec![
        (
            "the zoom menu",
            8,
            Box::new(|app: &App| app.set_zoom_menu_shown(true)),
        ),
        (
            "a frame's menu in the strip",
            4,
            Box::new(move |app: &App| right_click(app, crate::testing::strip_cell(app, 0))),
        ),
        (
            "a preset's menu",
            2,
            Box::new(move |app: &App| {
                app.set_panel_tab("Develop".into());
                app.set_collapsed_presets(false);
                // The group's heading is the first; a preset's own is
                // the second.
                right_click(app, by_id(app, "PresetsSection::preset-row", 1));
            }),
        ),
        (
            "a mask's menu",
            2,
            Box::new(move |app: &App| {
                app.set_panel_tab("Masks".into());
                app.set_collapsed_adjustments(false);
                // Global has no menu; a mask's is the second row's.
                right_click(app, by_id(app, "AdjustmentsList::row", 1));
            }),
        ),
        (
            "the help menu",
            2,
            Box::new(|app: &App| {
                settle(app);
                let help = ElementHandle::find_by_accessible_label(app, "Help menu")
                    .next()
                    .expect("the Help button");
                let (x, y) = center(&help);
                crate::testing::click(app, x, y);
            }),
        ),
    ];
    let mut failures = Vec::new();
    for (name, least, open) in menus {
        open(&app);
        let (words, unnamed) = menu_items(&app);
        if words.len() + unnamed < least {
            failures.push(format!(
                "{name}: {} items, at least {least}",
                words.len() + unnamed
            ));
        }
        if unnamed > 0 {
            failures.push(format!(
                "{name}: {unnamed} items say nothing, beside {words:?}"
            ));
        }
        for (_, line) in survey(&app).gaps {
            failures.push(format!("{name}: {line}"));
        }
        crate::testing::press(&app, Key::Escape);
        settle(&app);
        assert!(menu_items(&app).0.is_empty(), "{name} put away");
    }
    assert!(
        failures.is_empty(),
        "menus a screen reader would miss in:\n{}",
        failures.join("\n")
    );
}
