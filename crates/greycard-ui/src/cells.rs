//! The browser's rows, and the cells the strip and the grid draw of
//! them.
//!
//! The window's `thumbs` holds every row of the browser's list, a
//! folder of twenty thousand included: the header reads the current
//! frame's name there, and Rust sets a row's picture, badges and
//! place in the selection there. The strip and the grid do not repeat
//! over it. Each repeats over a [`Cells`], a window of the rows on
//! screen and a margin round them, which is all Slint instantiates:
//! a cell is an item tree of a dozen elements, and a frame over one
//! for every row cost 45 to 400 ms on a folder that size.
//!
//! A window's cells are slots, and a row always takes the slot its
//! number falls on modulo the slots. A scroll of one row makes cells
//! afresh in the slots of the rows that left, for the rows that came
//! in, and every other cell keeps its row, its bindings and the
//! texture the renderer made of its picture. The window's slots change only when
//! the view wants more of them, or fewer than half: a range whose
//! length wobbles by a row as it scrolls keeps its cells.
//!
//! A change to a row reaches the view that has a cell for it as a
//! change to that one cell. Both views read the one list, so there is
//! nothing to keep in step: [`set_rows`] puts a new list in, and each
//! view's window is held to its length until the view says what it
//! shows.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::{Model, ModelNotify, ModelRc, ModelTracker};

use crate::{App, Thumb, ThumbCell};

/// The views that draw the rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    Strip = 0,
    Grid = 1,
}

/// A view's window over the rows: the first row it has a cell for,
/// and how many cells it has.
#[derive(Default)]
struct Window {
    first: Cell<usize>,
    slots: Cell<usize>,
    notify: ModelNotify,
}

impl Window {
    /// The slot row `row` is in, when the window has one for it.
    fn slot_of(&self, row: usize) -> Option<usize> {
        let (first, slots) = (self.first.get(), self.slots.get());
        (slots > 0 && (first..first + slots).contains(&row)).then(|| row % slots)
    }

    /// The row in slot `slot`.
    fn row_at(&self, slot: usize) -> usize {
        let (first, slots) = (self.first.get(), self.slots.get());
        first + (slot + slots - first % slots) % slots
    }

    /// Held to a list of `count` rows: no more slots than rows, and
    /// none past the last.
    fn fit(&self, count: usize) {
        let slots = self.slots.get().min(count);
        self.slots.set(slots);
        self.first.set(self.first.get().min(count - slots));
    }
}

/// What the rows and the two views share.
#[derive(Default)]
struct Shared {
    rows: RefCell<Vec<Thumb>>,
    views: [Window; 2],
}

/// Every row of the browser's list: the window's `thumbs`.
pub(crate) struct Rows {
    shared: Rc<Shared>,
    notify: ModelNotify,
}

impl Model for Rows {
    type Data = Thumb;

    fn row_count(&self) -> usize {
        self.shared.rows.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<Thumb> {
        self.shared.rows.borrow().get(row).cloned()
    }

    fn set_row_data(&self, row: usize, data: Thumb) {
        match self.shared.rows.borrow_mut().get_mut(row) {
            Some(r) => *r = data,
            None => return,
        }
        self.notify.row_changed(row);
        for view in &self.shared.views {
            if let Some(slot) = view.slot_of(row) {
                view.notify.row_changed(slot);
            }
        }
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A view's cells: the window's `strip-cells` or `grid-cells`.
pub(crate) struct Cells {
    shared: Rc<Shared>,
    view: View,
}

impl Cells {
    fn window(&self) -> &Window {
        &self.shared.views[self.view as usize]
    }
}

impl Model for Cells {
    type Data = ThumbCell;

    fn row_count(&self) -> usize {
        self.window().slots.get()
    }

    fn row_data(&self, slot: usize) -> Option<ThumbCell> {
        let window = self.window();
        if slot >= window.slots.get() {
            return None;
        }
        let row = window.row_at(slot);
        let thumb = self.shared.rows.borrow().get(row)?.clone();
        Some(ThumbCell {
            row: row as i32,
            thumb,
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.window().notify
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The rows and views on `app`, put there the first time they are
/// asked for.
fn shared(app: &App) -> Rc<Shared> {
    if let Some(cells) = app.get_grid_cells().as_any().downcast_ref::<Cells>() {
        return cells.shared.clone();
    }
    let shared = Rc::new(Shared::default());
    app.set_strip_cells(ModelRc::new(Cells {
        shared: shared.clone(),
        view: View::Strip,
    }));
    app.set_grid_cells(ModelRc::new(Cells {
        shared: shared.clone(),
        view: View::Grid,
    }));
    shared
}

/// Put `thumbs` on the window as the browser's rows, in place of what
/// it had. The views keep their windows, held to the new list's
/// length, and have every cell made again; each says what it shows
/// next as it lays the new list out.
pub(crate) fn set_rows(app: &App, thumbs: Vec<Thumb>) {
    let shared = shared(app);
    let count = thumbs.len();
    *shared.rows.borrow_mut() = thumbs;
    for view in &shared.views {
        view.fit(count);
        view.notify.reset();
    }
    // A model of its own each time: the window re-flows when `thumbs`
    // changes, not when the rows under it do.
    app.set_thumbs(ModelRc::new(Rows {
        shared,
        notify: ModelNotify::default(),
    }));
}

/// Give `view` cells for `wanted` rows from `first`, held to the list.
/// Says whether any cell changed rows.
pub(crate) fn show(app: &App, view: View, first: usize, wanted: usize) -> bool {
    let shared = shared(app);
    let count = shared.rows.borrow().len();
    let window = &shared.views[view as usize];
    let had = window.slots.get();
    let slots = if wanted > had || wanted * 2 < had {
        wanted
    } else {
        had
    }
    .min(count);
    let first = first.min(count - slots);
    if slots != had {
        window.first.set(first);
        window.slots.set(slots);
        window.notify.reset();
        return true;
    }
    let was = window.first.get();
    if first == was {
        return false;
    }
    window.first.set(first);
    // The rows come into the window take the slots of those gone, each
    // as a cell made afresh: a cell kept and handed another row would
    // carry over what the old row left in it, its background fading
    // from the old row's (the current frame's ground on a row that is
    // not it), the pointer's hover, a press waiting for its release.
    // A slot removed and added again is a new instance in its place;
    // the others are handed their own data again, which changes
    // nothing. A move past the whole window makes every cell afresh.
    if first.abs_diff(was) >= slots {
        window.notify.reset();
        return true;
    }
    for row in first..first + slots {
        if !(was..was + slots).contains(&row) {
            window.notify.row_removed(row % slots, 1);
            window.notify.row_added(row % slots, 1);
        }
    }
    true
}

/// The frames the strip has cells for past those it says it shows,
/// either side. Its window moves when the strip reports, which it
/// does after its move rather than with it, so the margin is what
/// the frame between is drawn from on a fast drag.
pub(crate) const STRIP_MARGIN: usize = 3;

/// The strip's window when it shows rows `first` to `last`: the first
/// row it has a cell for and how many.
pub(crate) fn strip_window(first: usize, last: usize) -> (usize, usize) {
    (
        first.saturating_sub(STRIP_MARGIN),
        last.saturating_sub(first) + 1 + 2 * STRIP_MARGIN,
    )
}

/// The rows `view` has cells for, first and how many.
pub(crate) fn window(app: &App, view: View) -> (usize, usize) {
    let shared = shared(app);
    let window = &shared.views[view as usize];
    (window.first.get(), window.slots.get())
}

/// Whether either view has a cell for `row`.
pub(crate) fn has_cell(app: &App, row: usize) -> bool {
    shared(app)
        .views
        .iter()
        .any(|view| view.slot_of(row).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(count: usize) -> Vec<Thumb> {
        (0..count)
            .map(|i| Thumb {
                name: format!("f{i}").into(),
                ..Default::default()
            })
            .collect()
    }

    fn app() -> App {
        thread_local! {
            static BACKEND: () = i_slint_backend_testing::init_no_event_loop();
        }
        BACKEND.with(|()| ());
        App::new().expect("the window builds")
    }

    /// The rows each slot holds, in slot order.
    fn slots(app: &App, view: View) -> Vec<i32> {
        let cells = match view {
            View::Strip => app.get_strip_cells(),
            View::Grid => app.get_grid_cells(),
        };
        cells.iter().map(|c| c.row).collect()
    }

    #[test]
    fn a_row_takes_the_slot_its_number_falls_on() {
        let app = app();
        set_rows(&app, rows(100));
        assert!(show(&app, View::Grid, 10, 8));
        assert_eq!(
            slots(&app, View::Grid),
            vec![16, 17, 10, 11, 12, 13, 14, 15]
        );
        // One row on: 18 and 19 take the slots 10 and 11 had, and the
        // rest keep theirs.
        assert!(show(&app, View::Grid, 12, 8));
        assert_eq!(
            slots(&app, View::Grid),
            vec![16, 17, 18, 19, 12, 13, 14, 15]
        );
        assert!(!show(&app, View::Grid, 12, 8), "the same window again");
        // Near the end the window is held to the list.
        show(&app, View::Grid, 97, 8);
        assert_eq!(window(&app, View::Grid), (92, 8));
        // The strip's window is its own.
        assert_eq!(window(&app, View::Strip), (0, 0));
    }

    #[test]
    fn a_window_keeps_its_slots_through_a_wobble_and_follows_a_real_change() {
        let app = app();
        set_rows(&app, rows(100));
        show(&app, View::Strip, 0, 12);
        show(&app, View::Strip, 1, 11);
        assert_eq!(window(&app, View::Strip), (1, 12));
        show(&app, View::Strip, 1, 13);
        assert_eq!(window(&app, View::Strip), (1, 13));
        show(&app, View::Strip, 1, 7);
        assert_eq!(window(&app, View::Strip), (1, 13), "half is kept");
        show(&app, View::Strip, 1, 6);
        assert_eq!(window(&app, View::Strip), (1, 6));
        // No more slots than rows.
        show(&app, View::Strip, 0, 500);
        assert_eq!(window(&app, View::Strip), (0, 100));
    }

    #[test]
    fn a_labeled_rows_color_and_chosen_flag_reach_its_cell_after_a_repoint() {
        let app = app();
        set_rows(&app, rows(100));
        show(&app, View::Grid, 10, 8);
        let model = app.get_thumbs();
        let mut labeled = model.row_data(19).unwrap();
        labeled.label = 3;
        labeled.chosen = true;
        model.set_row_data(19, labeled);
        // The window has no cell for row 19 yet.
        assert!(!has_cell(&app, 19));
        // A scroll of two rows re-points the slots that held 10 and
        // 11, bringing 18 and 19 in.
        assert!(show(&app, View::Grid, 12, 8));
        let cells = app.get_grid_cells();
        let cell = cells.row_data(19 % 8).unwrap();
        assert_eq!(
            (cell.row, cell.thumb.label, cell.thumb.chosen),
            (19, 3, true)
        );
        // A row that carries no label reaches its cell with none.
        let plain = cells.row_data(12 % 8).unwrap();
        assert_eq!((plain.thumb.label, plain.thumb.chosen), (0, false));
    }

    #[test]
    fn a_change_to_a_row_reaches_its_cell_and_a_new_list_is_held_to() {
        let app = app();
        set_rows(&app, rows(100));
        show(&app, View::Grid, 40, 10);
        let model = app.get_thumbs();
        let mut t = model.row_data(45).unwrap();
        t.rating = 3;
        model.set_row_data(45, t);
        let cells = app.get_grid_cells();
        let cell = cells.row_data(45 % 10).unwrap();
        assert_eq!((cell.row, cell.thumb.rating), (45, 3));
        assert!(has_cell(&app, 45));
        assert!(!has_cell(&app, 50));
        // A shorter list: the window is held to it.
        set_rows(&app, rows(6));
        assert_eq!(window(&app, View::Grid), (0, 6));
        assert_eq!(app.get_thumbs().row_count(), 6);
        let mut seen = slots(&app, View::Grid);
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2, 3, 4, 5]);
        set_rows(&app, Vec::new());
        assert_eq!(window(&app, View::Grid), (0, 0));
        assert!(!show(&app, View::Grid, 3, 10));
    }
}
