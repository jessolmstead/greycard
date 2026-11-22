//! The left pane's lists' weights in sharing its height, kept between
//! runs by section name. The sharing itself is the pane's layout's
//! (`share-folders` in app.slint); this only reads the weights out for
//! the settings file and puts them back at the start.

use std::collections::BTreeMap;

use crate::App;

/// A sharing list, by the name the settings keep its weight under,
/// with the panel's property for it.
type Share = (&'static str, fn(&App) -> f32, fn(&App, f32));

const SHARES: &[Share] = &[
    ("folders", App::get_share_folders, App::set_share_folders),
    (
        "collections",
        App::get_share_collections,
        App::set_share_collections,
    ),
    ("presets", App::get_share_presets, App::set_share_presets),
    (
        "snapshots",
        App::get_share_snapshots,
        App::set_share_snapshots,
    ),
    ("history", App::get_share_history, App::set_share_history),
];

/// The weights the last run left; a list not named, or named with a
/// weight that is no number or not above nothing, weighs one.
pub(crate) fn show(app: &App, kept: &BTreeMap<String, f32>) {
    for (name, _, set) in SHARES {
        let w = kept
            .get(*name)
            .copied()
            .filter(|w| w.is_finite() && *w > 0.0)
            .unwrap_or(1.0);
        set(app, w);
    }
}

/// The weights now, by name: only those not one, so a file whose
/// lists were never dragged says nothing of them.
pub(crate) fn read(app: &App) -> BTreeMap<String, f32> {
    SHARES
        .iter()
        .map(|(name, get, _)| (name.to_string(), get(app)))
        .filter(|(_, w)| *w != 1.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use slint::{ComponentHandle, ModelRc, VecModel};

    use super::*;
    use crate::testing::{click, drag, settle, window};
    use crate::{CollectionChip, FolderRow, HistoryRow, PresetRow};

    const SECTIONS: [&str; 4] = [
        "folders-section",
        "presets-section",
        "snapshots-section",
        "history-section",
    ];

    /// The Loupe's left pane with FOLDERS, PRESETS, SNAPSHOTS, and
    /// HISTORY open, each with `rows` rows (PRESETS `presets`).
    fn pane(rows: usize, presets: usize) -> App {
        pane_folded(rows, presets, false)
    }

    /// [`pane`], PRESETS folded from the start when `folded`, as a
    /// run whose settings fold it opens.
    fn pane_folded(rows: usize, presets: usize, folded: bool) -> App {
        let app = window(3);
        app.set_collapsed_presets(folded);
        let folders: Vec<FolderRow> = (0..rows)
            .map(|i| FolderRow {
                name: format!("folder {i}").into(),
                path: format!("/nowhere/{i}").into(),
                count: 3,
                ..Default::default()
            })
            .collect();
        app.set_folder_tree(ModelRc::new(VecModel::from(folders)));
        app.set_folder_tree_shown(true);
        let names: Vec<slint::SharedString> =
            (0..presets).map(|i| format!("preset {i}").into()).collect();
        let preset_rows: Vec<PresetRow> = (0..presets)
            .map(|i| PresetRow {
                text: format!("preset {i}").into(),
                index: i as i32,
                ..Default::default()
            })
            .collect();
        app.set_preset_names(ModelRc::new(VecModel::from(names)));
        app.set_preset_list_rows(ModelRc::new(VecModel::from(preset_rows)));
        let snapshots: Vec<slint::SharedString> =
            (0..rows).map(|i| format!("snapshot {i}").into()).collect();
        app.set_snapshot_names(ModelRc::new(VecModel::from(snapshots)));
        let history: Vec<HistoryRow> = (0..rows)
            .map(|i| HistoryRow {
                name: format!("step {i}").into(),
                ..Default::default()
            })
            .collect();
        app.set_history_rows(ModelRc::new(VecModel::from(history)));
        settle(&app);
        app
    }

    fn sized(app: &App, height: f32) {
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, height));
        settle(app);
        settle(app);
    }

    fn wheel(app: &App, dy: f32) {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(8.0, 200.0),
                delta_x: 0.0,
                delta_y: dy,
            });
        settle(app);
    }

    /// A section's top and height in the window, the pane scrolled
    /// down to it if need be (what is scrolled out of sight is not
    /// found) and back to its top after.
    fn section(app: &App, id: &str) -> (f32, f32) {
        use i_slint_backend_testing::ElementHandle;
        let find = || {
            ElementHandle::find_by_element_id(app, &format!("App::{id}"))
                .next()
                .map(|e| (e.absolute_position().y, e.size().height))
        };
        let mut found = find();
        let mut steps = 0;
        while found.is_none() && steps < 40 {
            wheel(app, -60.0);
            found = find();
            steps += 1;
        }
        if steps > 0 {
            wheel(app, 10_000.0);
        }
        found.unwrap_or_else(|| panic!("{id} in the left pane"))
    }

    fn heights(app: &App) -> Vec<f32> {
        SECTIONS.iter().map(|id| section(app, id).1).collect()
    }

    /// Each section's height past its floor, the window 1600 high:
    /// against the same window 500 high, where every list is at its
    /// floor and the pane scrolls.
    fn growth(app: &App) -> Vec<f32> {
        sized(app, 500.0);
        let floors = heights(app);
        sized(app, 1600.0);
        heights(app)
            .iter()
            .zip(&floors)
            .map(|(h, f)| h - f)
            .collect()
    }

    fn near(a: f32, b: f32, said: &str) {
        assert!((a - b).abs() <= 2.0, "{said}: {a} against {b}");
    }

    /// The middle of the share edge above `id`'s section, in the left
    /// pane: over the 8 px of padding at the foot of the box above.
    fn gap_above(app: &App, id: &str) -> (f32, f32) {
        let (top, _) = section(app, id);
        (100.0, top - 4.0)
    }

    /// A click anywhere in a header's row folds its section, the top
    /// 8 px above the title too, whether the share edge above it is
    /// live or dead: the edge lies over the box above, not the header.
    /// And where it is live, a drag on it still moves room.
    #[test]
    fn a_click_at_a_headers_top_folds_it_under_a_live_or_dead_edge() {
        for (rows, live) in [(60, true), (5, false)] {
            let app = pane(rows, rows);
            sized(&app, 1600.0);
            let (x, y) = gap_above(&app, "snapshots-section");
            for dy in [0.0, 1.0] {
                app.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(x, y + dy),
                    });
                tick();
            }
            assert_eq!(app.invoke_share_live(2, 3), live, "the edge live: {live}");
            for dy in [1.0, 4.0, 7.0] {
                let (top, _) = section(&app, "snapshots-section");
                click(&app, 100.0, top + dy);
                settle(&app);
                assert!(
                    app.get_collapsed_snapshots(),
                    "SNAPSHOTS folded by a click {dy} px into its header, edge live {live}"
                );
                app.set_collapsed_snapshots(false);
                settle(&app);
                settle(&app);
            }
            assert!(read(&app).is_empty(), "no share moved: {:?}", read(&app));
            if live {
                let before = heights(&app);
                let (x, y) = gap_above(&app, "snapshots-section");
                drag(&app, &[(x, y), (x, y + 20.0), (x, y + 40.0)]);
                settle(&app);
                let after = heights(&app);
                near(after[1], before[1] + 40.0, "PRESETS took the drag");
                near(after[2], before[2] - 40.0, "SNAPSHOTS gave it");
            }
        }
    }

    #[test]
    fn the_open_lists_share_the_pane_by_weight() {
        let app = pane(60, 60);
        let even = growth(&app);
        assert!(even[0] > 40.0, "a tall window grows the lists: {even:?}");
        for (g, id) in even.iter().zip(SECTIONS) {
            near(*g, even[0], id);
        }
        // Twice the weight, twice the room past the floor.
        app.set_share_presets(2.0);
        let g = growth(&app);
        near(g[1], 2.0 * g[0], "PRESETS at two");
        near(g[2], g[0], "SNAPSHOTS at one");
        near(g[3], g[0], "HISTORY at one");
        // And the pane used as before: what PRESETS took, the others gave.
        near(g.iter().sum(), even.iter().sum(), "the room shared");
    }

    #[test]
    fn a_folded_list_gives_its_room_to_the_others() {
        let app = pane(60, 60);
        let open = growth(&app);
        app.set_collapsed_snapshots(true);
        app.set_collapsed_history(true);
        settle(&app);
        let g = growth(&app);
        near(g[0], g[1], "FOLDERS and PRESETS share alike");
        assert!(
            g[0] > open[0] + 40.0,
            "FOLDERS took SNAPSHOTS' and HISTORY's room: {g:?} against {open:?}"
        );
    }

    #[test]
    fn a_short_list_gives_back_what_it_does_not_need() {
        let long = pane(60, 60);
        let shared = growth(&long);
        // Five presets: their whole list, and the rest to the others.
        let app = pane(60, 5);
        let g = growth(&app);
        assert!(g[1] < shared[1], "PRESETS at its list: {g:?}");
        near(g[0], g[2], "FOLDERS and SNAPSHOTS");
        near(g[2], g[3], "SNAPSHOTS and HISTORY");
        assert!(
            g[0] > shared[0] + 20.0,
            "PRESETS' spare went round: {g:?} against {shared:?}"
        );
    }

    #[test]
    fn a_drag_moves_room_and_its_weights_are_kept() {
        let app = pane(60, 60);
        sized(&app, 1600.0);
        let before = heights(&app);
        let (x, y) = gap_above(&app, "snapshots-section");
        drag(&app, &[(x, y), (x, y + 30.0), (x, y + 60.0)]);
        settle(&app);
        let after = heights(&app);
        near(after[1], before[1] + 60.0, "PRESETS took the drag");
        near(after[2], before[2] - 60.0, "SNAPSHOTS gave it");
        near(after[0], before[0], "FOLDERS kept its own");
        near(after[3], before[3], "HISTORY kept its own");
        // The division holds as the window changes: the two keep
        // their ratio past their floors.
        let g = growth(&app);
        let (wp, ws) = (app.get_share_presets(), app.get_share_snapshots());
        assert!(wp > 1.0 && ws < 1.0, "the weights moved: {wp} {ws}");
        near(g[1] / g[0], wp, "PRESETS by its weight");

        // Kept by name, and back at the next start.
        let kept = crate::panel::startup::remember(&app).shares;
        assert_eq!(kept.len(), 2, "only the dragged two: {kept:?}");
        assert_eq!(kept.get("presets"), Some(&wp));
        assert_eq!(kept.get("snapshots"), Some(&ws));
        let next = window(3);
        show(&next, &kept);
        assert_eq!(next.get_share_presets(), wp);
        assert_eq!(next.get_share_snapshots(), ws);
        assert_eq!(next.get_share_history(), 1.0);
        assert_eq!(read(&next), kept);
    }

    #[test]
    fn a_double_click_on_a_gap_evens_its_two() {
        let app = pane(60, 60);
        sized(&app, 1600.0);
        app.set_share_snapshots(3.0);
        app.set_share_history(1.0);
        settle(&app);
        let (x, y) = gap_above(&app, "history-section");
        click(&app, x, y);
        click(&app, x, y);
        settle(&app);
        assert_eq!(app.get_share_snapshots(), 2.0);
        assert_eq!(app.get_share_history(), 2.0);
        assert_eq!(app.get_share_presets(), 1.0);
    }

    #[test]
    fn a_short_window_scrolls_the_pane_with_every_list_at_its_floor() {
        use i_slint_backend_testing::ElementHandle;
        let app = pane(60, 60);
        sized(&app, 500.0);
        let floors = heights(&app);
        sized(&app, 600.0);
        // Still the floors: 600 is short of them too.
        for ((h, f), id) in heights(&app).iter().zip(&floors).zip(SECTIONS) {
            near(*h, *f, id);
        }
        // HISTORY is past the foot, out of sight, and the wheel over
        // the pane brings it in.
        let history = || ElementHandle::find_by_element_id(&app, "App::history-section").count();
        assert_eq!(history(), 0, "HISTORY past the pane's foot");
        for _ in 0..40 {
            wheel(&app, -60.0);
            if history() > 0 {
                return;
            }
        }
        panic!("the wheel never brought HISTORY in");
    }

    #[test]
    fn the_librarys_folders_and_collections_share_alike() {
        let app = pane(60, 0);
        let chips: Vec<CollectionChip> = (0..60)
            .map(|i| CollectionChip {
                id: format!("c{i}").into(),
                name: format!("collection {i}").into(),
                ..Default::default()
            })
            .collect();
        app.set_collections(ModelRc::new(VecModel::from(chips)));
        app.set_grid_open(true);
        settle(&app);
        let ids = ["folders-section", "collections-section"];
        let measure = |app: &App| -> Vec<f32> { ids.iter().map(|id| section(app, id).1).collect() };
        sized(&app, 500.0);
        let floors = measure(&app);
        sized(&app, 1600.0);
        let tall = measure(&app);
        let (gf, gc) = (tall[0] - floors[0], tall[1] - floors[1]);
        assert!(gf > 100.0, "FOLDERS grew: {gf}");
        near(gf, gc, "COLLECTIONS as FOLDERS");
        // Their gap is a drag too.
        let (x, y) = gap_above(&app, "collections-section");
        drag(&app, &[(x, y), (x, y - 40.0)]);
        settle(&app);
        let moved = measure(&app);
        near(moved[0], tall[0] - 40.0, "FOLDERS gave");
        near(moved[1], tall[1] + 40.0, "COLLECTIONS took");
    }

    /// Each section's height at the floors, the window 500 high, and
    /// the pane back at 1600.
    fn floors(app: &App) -> Vec<f32> {
        sized(app, 500.0);
        let f = heights(app);
        sized(app, 1600.0);
        f
    }

    /// The fold's clock moved on by `ms`, and the frame drawn.
    fn after(app: &App, ms: u64) {
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
        slint::platform::update_timers_and_animations();
    }

    #[test]
    fn a_grown_list_folds_from_its_share_and_opens_back_to_it() {
        let app = pane(60, 60);
        let floor = floors(&app);
        let open = heights(&app);
        assert!(
            open[1] > floor[1] + 60.0,
            "PRESETS grown: {open:?} {floor:?}"
        );
        let neighbors = |h: &[f32]| h[0] + h[2] + h[3];

        app.set_collapsed_presets(true);
        after(&app, 0);
        after(&app, 10);
        let mid = heights(&app);
        assert!(
            mid[1] < open[1] && mid[1] > floor[1] + 20.0,
            "folding from its share, not its floor: {} from {} (floor {})",
            mid[1],
            open[1],
            floor[1]
        );
        // What it gave, the others took, and no more: no jump.
        near(
            neighbors(&mid) - neighbors(&open),
            open[1] - mid[1],
            "the neighbors grew by what PRESETS gave",
        );
        settle(&app);
        settle(&app);
        let folded = heights(&app);
        assert!(folded[1] < floor[1], "folded: {folded:?}");

        app.set_collapsed_presets(false);
        after(&app, 0);
        after(&app, 10);
        let opening = heights(&app);
        assert!(
            opening[1] > folded[1] && opening[1] < open[1],
            "opening: {} between {} and {}",
            opening[1],
            folded[1],
            open[1]
        );
        near(
            neighbors(&folded) - neighbors(&opening),
            opening[1] - folded[1],
            "the neighbors gave what PRESETS took",
        );
        settle(&app);
        settle(&app);
        let back = heights(&app);
        for ((b, o), id) in back.iter().zip(&open).zip(SECTIONS) {
            near(*b, *o, id);
        }
    }

    /// PRESETS opened, frame by frame: its height at each step of the
    /// fold's 120 ms, the last at its end.
    fn opening(app: &App) -> Vec<f32> {
        app.set_collapsed_presets(false);
        after(app, 0);
        (0..12)
            .map(|_| {
                after(app, 10);
                heights(app)[1]
            })
            .collect()
    }

    /// The last steps of an opening are small and its end is where the
    /// pane settles: no step at the hand-over to the layout.
    fn lands_without_a_step(app: &App, steps: &[f32], said: &str) {
        let n = steps.len();
        assert!(
            (steps[n - 1] - steps[n - 2]).abs() < 10.0,
            "{said}: a step at the end, {steps:?}"
        );
        settle(app);
        settle(app);
        near(heights(app)[1], steps[n - 1], said);
        assert!(
            (heights(app)[1] - steps[n - 1]).abs() <= 1.0,
            "{said}: {steps:?}"
        );
    }

    #[test]
    fn an_opening_lands_on_the_share_the_pane_has_now() {
        // Folded on a short window, opened on a tall one: the share it
        // opens to is not the one it folded from.
        let app = pane(60, 60);
        sized(&app, 900.0);
        app.set_collapsed_presets(true);
        settle(&app);
        settle(&app);
        sized(&app, 1600.0);
        let steps = opening(&app);
        lands_without_a_step(&app, &steps, "reopened after a resize");

        // Folded from the start, never open, opened on a tall window:
        // it lands on its share, not its floor.
        let share = {
            let open = pane(60, 60);
            sized(&open, 1600.0);
            heights(&open)[1]
        };
        let app = pane_folded(60, 60, true);
        sized(&app, 1600.0);
        let steps = opening(&app);
        lands_without_a_step(&app, &steps, "first opened");
        near(steps[steps.len() - 1], share, "its share");
    }

    /// The hand shows on a gap only where a drag can move something:
    /// short lists, all at their ceilings, leave the gap tried dead.
    #[test]
    fn a_gap_that_can_move_nothing_shows_no_hand() {
        let hover = |app: &App| {
            let (x, y) = gap_above(app, "snapshots-section");
            for dy in [0.0, 1.0] {
                app.window()
                    .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(x, y + dy),
                    });
                slint::platform::update_timers_and_animations();
            }
        };
        let short = pane(5, 5);
        sized(&short, 1600.0);
        hover(&short);
        assert!(!short.invoke_share_live(2, 3), "short lists, a dead gap");
        let long = pane(60, 60);
        sized(&long, 1600.0);
        hover(&long);
        assert!(long.invoke_share_live(2, 3), "long lists, a live gap");
        // Five presets, at their ceiling, over a long SNAPSHOTS: live,
        // as PRESETS can still give what it has past its floor.
        let mixed = pane(60, 5);
        sized(&mixed, 1600.0);
        hover(&mixed);
        assert!(
            mixed.invoke_share_live(2, 3),
            "a short PRESETS can still give"
        );
    }

    /// A pointer resting in a gap sees the hand turn as the lists
    /// change under it: each move in the gap asks them afresh. FOLDERS
    /// short, at its ceiling, keeps the gap below it still as PRESETS
    /// changes.
    #[test]
    fn a_resting_pointer_sees_the_gap_turn() {
        let app = pane(60, 60);
        let folders: Vec<FolderRow> = (0..5)
            .map(|i| FolderRow {
                name: format!("folder {i}").into(),
                path: format!("/nowhere/{i}").into(),
                count: 3,
                ..Default::default()
            })
            .collect();
        app.set_folder_tree(ModelRc::new(VecModel::from(folders)));
        sized(&app, 1600.0);
        let presets = |n: usize| {
            let rows: Vec<PresetRow> = (0..n)
                .map(|i| PresetRow {
                    text: format!("preset {i}").into(),
                    index: i as i32,
                    ..Default::default()
                })
                .collect();
            app.set_preset_list_rows(ModelRc::new(VecModel::from(rows)));
            tick();
        };
        let (x, y) = gap_above(&app, "presets-section");
        let rest = |dy: f32| {
            app.window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(x, y + dy),
                });
            tick();
        };
        let still = |said: &str| {
            let (gx, gy) = gap_above(&app, "presets-section");
            near(gx, x, said);
            near(gy, y, said);
        };
        rest(0.0);
        assert!(app.invoke_share_live(0, 2), "sixty presets, a live gap");
        // Five presets, at their ceiling: nowhere for FOLDERS' room.
        presets(5);
        still("the gap stayed under the pointer");
        assert!(app.invoke_share_live(0, 2), "not asked yet");
        rest(1.0);
        assert!(!app.invoke_share_live(0, 2), "a move asked: dead");
        // And back.
        presets(60);
        still("the gap stayed under the pointer");
        rest(0.0);
        assert!(app.invoke_share_live(0, 2), "a move asked: live again");
    }

    /// The share edges lie over the padding at a box's foot, never over
    /// what the box holds nor the header below: a press low on TAKE
    /// takes a snapshot, one low on the last folder row picks that
    /// folder, and one on the title under an edge folds its section.
    #[test]
    fn a_press_at_a_sections_foot_is_the_sections() {
        use i_slint_backend_testing::ElementHandle;
        use std::cell::Cell;
        use std::rc::Rc;
        let app = pane(3, 3);
        app.set_selected(0);
        sized(&app, 1600.0);
        // A row of the pane by its key: a folder's path.
        let found = |id: &str, key: &str| {
            let e = crate::testing::keyed(&app, id, key)
                .unwrap_or_else(|| panic!("{key} in the left pane"));
            (e.absolute_position(), e.size())
        };
        let taken = Rc::new(Cell::new(0));
        let t = taken.clone();
        app.on_snapshot_taken(move || t.set(t.get() + 1));
        let (at, size) = {
            let e = ElementHandle::find_by_element_id(&app, "SnapshotsSection::take-button")
                .next()
                .expect("TAKE in the left pane");
            (e.absolute_position(), e.size())
        };
        for dy in [2.0, size.height / 2.0, size.height - 2.0] {
            let before = taken.get();
            click(&app, at.x + size.width / 2.0, at.y + dy);
            settle(&app);
            assert_eq!(taken.get(), before + 1, "TAKE {dy} px down its {size:?}");
        }
        let picked = Rc::new(Cell::new(-1));
        let p = picked.clone();
        app.on_folder_tree_picked(move |i| p.set(i));
        let (at, size) = found("FoldersSection::folder-line", "/nowhere/2");
        for dy in [2.0, size.height / 2.0, size.height - 2.0] {
            picked.set(-1);
            click(&app, at.x + size.width / 2.0, at.y + dy);
            settle(&app);
            assert_eq!(picked.get(), 2, "the last folder {dy} px down its {size:?}");
        }
        // Its title's words, inside the fold that says them.
        let fold = crate::testing::section_part(&app, "SnapshotsSection", "Section::row")
            .expect("SNAPSHOTS in the left pane");
        let (top, foot) = (
            fold.absolute_position().y,
            fold.absolute_position().y + fold.size().height,
        );
        let e = ElementHandle::find_by_element_id(&app, "Section::title-words")
            .find(|t| {
                let y = t.absolute_position().y + t.size().height / 2.0;
                y >= top && y <= foot
            })
            .expect("SNAPSHOTS's words");
        let (at, size) = (e.absolute_position(), e.size());
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        settle(&app);
        assert!(
            app.get_collapsed_snapshots(),
            "SNAPSHOTS folded by its title"
        );
    }

    /// Library's pane with FOLDERS and COLLECTIONS open, `rows` rows
    /// each, in `order`.
    fn library_pane(rows: usize, order: &[&str]) -> App {
        let app = pane(rows, 0);
        let chips: Vec<CollectionChip> = (0..rows)
            .map(|i| CollectionChip {
                id: format!("c{i}").into(),
                name: format!("collection {i}").into(),
                ..Default::default()
            })
            .collect();
        app.set_collections(ModelRc::new(VecModel::from(chips)));
        app.set_grid_open(true);
        let order: Vec<slint::SharedString> = order.iter().map(|s| (*s).into()).collect();
        app.set_left_order(ModelRc::new(VecModel::from(order)));
        settle(&app);
        sized(&app, 1600.0);
        app
    }

    /// Library's first order, and the queue between COLLECTIONS and
    /// FOLDERS, where no edge lies between the two.
    const LIBRARY_ORDERS: [&[&str]; 2] = [
        &[
            "roots",
            "folders",
            "collections",
            "queue",
            "metadata",
            "presets",
            "snapshots",
            "history",
        ],
        &[
            "roots",
            "collections",
            "queue",
            "folders",
            "metadata",
            "presets",
            "snapshots",
            "history",
        ],
    ];

    /// FOLDERS straight under COLLECTIONS: COLLECTIONS' slot ends in its
    /// box, so the edge over its foot moves room and a double-click
    /// there evens the two.
    #[test]
    fn folders_under_the_collections_have_their_edge() {
        let order = [
            "roots",
            "collections",
            "folders",
            "queue",
            "metadata",
            "presets",
            "snapshots",
            "history",
        ];
        let app = library_pane(60, &order);
        let ids = ["collections-section", "folders-section"];
        let measure = |app: &App| -> Vec<f32> { ids.iter().map(|id| section(app, id).1).collect() };
        let before = measure(&app);
        let (x, y) = gap_above(&app, "folders-section");
        drag(&app, &[(x, y), (x, y + 20.0), (x, y + 40.0)]);
        settle(&app);
        let moved = measure(&app);
        near(moved[0], before[0] + 40.0, "COLLECTIONS took");
        near(moved[1], before[1] - 40.0, "FOLDERS gave");
        assert!(app.get_share_collections() > 1.0 && app.get_share_folders() < 1.0);
        let (x, y) = gap_above(&app, "folders-section");
        click(&app, x, y);
        click(&app, x, y);
        settle(&app);
        assert_eq!(
            app.get_share_collections(),
            app.get_share_folders(),
            "evened"
        );
    }

    /// The Export queue's header takes every press down its row, in the
    /// first order and between COLLECTIONS and FOLDERS: no share edge
    /// lies over it, nor over any row or header, only over the foot
    /// padding of an open list's box.
    #[test]
    fn every_y_of_the_queue_header_opens_its_view() {
        use i_slint_backend_testing::ElementHandle;
        use std::cell::Cell;
        use std::rc::Rc;
        for order in LIBRARY_ORDERS {
            let app = library_pane(60, order);
            let opened = Rc::new(Cell::new(0));
            let o = opened.clone();
            app.on_queue_view_picked(move || o.set(o.get() + 1));
            let e = ElementHandle::find_by_element_type_name(&app, "QueueHeader")
                .find(|e| e.size().height > 0.0)
                .expect("the queue's header in Library");
            let (at, size) = (e.absolute_position(), e.size());
            assert!(size.height >= 30.0, "a header's height: {size:?}");
            let mut dy = 0.5;
            while dy < size.height {
                let before = opened.get();
                click(&app, at.x + size.width / 2.0, at.y + dy);
                settle(&app);
                assert_eq!(
                    opened.get(),
                    before + 1,
                    "a press {dy} px down the queue's {size:?} in {order:?}"
                );
                dy += 1.0;
            }
            assert!(read(&app).is_empty(), "no share moved: {:?}", read(&app));
        }
    }

    /// Library's lists too: a press low on the last folder row and on
    /// the last collection row is the row's, whichever is above.
    #[test]
    fn a_press_at_a_library_lists_foot_is_the_lists() {
        use std::cell::Cell;
        use std::rc::Rc;
        for order in LIBRARY_ORDERS {
            let app = library_pane(3, order);
            let folder = Rc::new(Cell::new(-1));
            let f = folder.clone();
            app.on_folder_tree_picked(move |i| f.set(i));
            let collection = Rc::new(Cell::new(String::new()));
            let c = collection.clone();
            app.on_collection_picked(move |id| c.set(id.to_string()));
            // A row of the pane by its key: a folder's path, a
            // collection's id.
            let found = |id: &str, key: &str| {
                let e = crate::testing::keyed(&app, id, key)
                    .unwrap_or_else(|| panic!("{key} in the left pane"));
                (e.absolute_position(), e.size())
            };
            for dy_from_foot in [2.0, 1.0] {
                folder.set(-1);
                let (at, size) = found("FoldersSection::folder-line", "/nowhere/2");
                click(
                    &app,
                    at.x + size.width / 2.0,
                    at.y + size.height - dy_from_foot,
                );
                settle(&app);
                assert_eq!(folder.get(), 2, "the last folder in {order:?}");
                collection.set(String::new());
                let (at, size) = found("CollectionsSection::collection-row", "c2");
                click(
                    &app,
                    at.x + size.width / 2.0,
                    at.y + size.height - dy_from_foot,
                );
                settle(&app);
                assert_eq!(collection.take(), "c2", "the last collection in {order:?}");
            }
        }
    }

    /// Timers and change handlers a few frames on, unlike `settle`,
    /// which also moves the pointer to the corner.
    fn tick() {
        for _ in 0..5 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
            slint::platform::update_timers_and_animations();
        }
    }

    #[test]
    fn a_drag_that_moves_nothing_writes_nothing() {
        // Every list short of a tall window's share: all at their
        // ceilings, so the gap has nowhere to go.
        let app = pane(5, 5);
        sized(&app, 1600.0);
        let before = heights(&app);
        let (x, y) = gap_above(&app, "snapshots-section");
        drag(&app, &[(x, y), (x, y + 20.0), (x, y - 20.0), (x, y + 40.0)]);
        settle(&app);
        assert!(read(&app).is_empty(), "{:?}", read(&app));
        for ((a, b), id) in heights(&app).iter().zip(&before).zip(SECTIONS) {
            near(*a, *b, id);
        }
        // And a press with a jiggle back to where it was, on lists
        // that could move.
        let app = pane(60, 60);
        sized(&app, 1600.0);
        let (x, y) = gap_above(&app, "snapshots-section");
        drag(&app, &[(x, y), (x, y + 3.0), (x, y)]);
        settle(&app);
        assert!(read(&app).is_empty(), "{:?}", read(&app));
    }

    #[test]
    fn a_drag_stops_at_a_ceiling_and_at_a_floor() {
        // Five presets, at their whole list: a drag down would grow
        // them past it, and moves nothing.
        let app = pane(60, 5);
        sized(&app, 1600.0);
        let before = heights(&app);
        let (x, y) = gap_above(&app, "snapshots-section");
        drag(&app, &[(x, y), (x, y + 40.0)]);
        settle(&app);
        near(heights(&app)[1], before[1], "PRESETS at its ceiling");
        assert!(read(&app).is_empty(), "{:?}", read(&app));
        // Up, it gives SNAPSHOTS the room.
        drag(&app, &[(x, y), (x, y - 20.0)]);
        settle(&app);
        let up = heights(&app);
        near(up[1], before[1] - 20.0, "PRESETS gave");
        near(up[2], before[2] + 20.0, "SNAPSHOTS took");

        // Long lists, dragged far past PRESETS's floor: held there.
        let app = pane(60, 60);
        let floor = floors(&app);
        let before = heights(&app);
        let (x, y) = gap_above(&app, "snapshots-section");
        drag(&app, &[(x, y), (x, y - 2000.0)]);
        settle(&app);
        let after = heights(&app);
        near(after[1], floor[1], "PRESETS at its floor");
        near(
            after[2],
            before[2] + (before[1] - floor[1]),
            "SNAPSHOTS took all PRESETS had past it",
        );
        assert_eq!(app.get_share_presets(), 0.01);
    }

    #[test]
    fn weights_not_numbers_read_as_one() {
        let app = window(1);
        let kept: BTreeMap<String, f32> = [
            ("presets".to_string(), f32::NAN),
            ("history".to_string(), -2.0),
            ("folders".to_string(), 0.25),
        ]
        .into();
        show(&app, &kept);
        assert_eq!(app.get_share_presets(), 1.0);
        assert_eq!(app.get_share_history(), 1.0);
        assert_eq!(app.get_share_folders(), 0.25);
        assert_eq!(read(&app), [("folders".to_string(), 0.25)].into());
    }
}
