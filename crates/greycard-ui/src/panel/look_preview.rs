//! A look previewed under the pointer: resting on a row of the LOOK
//! list shows the picture with that look, at the Strength set, until the
//! pointer leaves; a click keeps it, through the list's own path.
//!
//! The viewport applies the look at draw time from the table it hands
//! the renderer, so a preview is that table swapped and nothing else: no
//! develop, no history step, no sidecar written, and the edit is never
//! touched. The table is read on a thread through the same resolution
//! and the same cache the chosen look's is (`LookLut::look_under_in`),
//! so a click after a rest finds it read already; it is shown when it
//! lands only if the pointer is still on its row.
//!
//! The preview ends when the pointer has left the list for as long as
//! a rest takes, so a sweep from row to row goes from one look to the
//! next without the kept one between; and at once on a click, a frame
//! change, a Strength or display curve change, the list's rows put up
//! again, the list taken off the screen, or anything the draw finds the
//! preview no longer fits (`drawn`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use greycard_core::lut::Look;
use greycard_edit::DisplayCurve;
use greycard_edit::look::{LookLut, LutChoice};

use crate::*;

/// How long the pointer rests on a row before its look is shown, and
/// off the list before the kept one is back.
pub(crate) const REST: std::time::Duration = std::time::Duration::from_millis(150);

/// The hover and its preview, kept on the state. In a cell, since the
/// list's rows are put up again (`show_looks`) where the state is only
/// read, and a row put up again reports no leave.
#[derive(Default)]
pub(crate) struct Hover {
    /// The look whose row the pointer is on, by name; `None` off the
    /// list.
    pub(crate) under: Option<String>,
    /// The rest before `under` is acted on.
    pub(crate) rest: slint::Timer,
    /// The look being shown, or asked for, in place of the kept one.
    pub(crate) preview: Option<Preview>,
    /// Counts the asks, so a table that lands for an ask since replaced
    /// is dropped.
    pub(crate) generation: u64,
    /// The ask still wanted, for the reading thread: one that is not
    /// skips its read rather than wait on the look cache behind another.
    wanted: Arc<AtomicU64>,
}

/// Where a preview's own table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Read {
    /// Being read.
    Out,
    /// Landed and on show.
    Shown,
    /// Landed with the pointer off its row, and dropped: a return to the
    /// row asks again.
    Dropped,
}

/// One look previewed: what it was asked for and, once read, its table.
#[derive(Debug, Clone)]
pub(crate) struct Preview {
    /// The look's name, as its row carries it.
    pub(crate) name: String,
    /// The frame, the Strength and the display curve it was asked
    /// under: a change of any of them ends it.
    pub(crate) frame: Option<usize>,
    pub(crate) strength: f32,
    pub(crate) curve: DisplayCurve,
    pub(crate) generation: u64,
    pub(crate) read: Read,
    /// What the viewport draws through while the preview stands: the
    /// look read for this row once it has landed, until then the last
    /// row's, so a sweep never flashes the kept look between two; `None`
    /// while nothing has landed. The inner `None` is no look, which is
    /// what None's row, and a table fitted under another display curve,
    /// show.
    pub(crate) shown: Option<Option<Look>>,
}

/// The pointer came onto a look's row (`on`) or left it. Acted on after
/// a rest; a leave reported for a row the pointer has already moved off
/// to another is nothing.
fn hovered(st: &mut State, name: &str, on: bool) {
    let app_weak = st.window.app.clone();
    let hover = st.develop.look_hover.get_mut();
    if on {
        hover.under = Some(name.to_string());
    } else if hover.under.as_deref() == Some(name) {
        hover.under = None;
    } else {
        return;
    }
    let Some(state) = STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    hover
        .rest
        .start(slint::TimerMode::SingleShot, REST, move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            rested(&mut state.borrow_mut(), &app);
        });
}

/// The pointer has rested: the row under it shown, or with none, the
/// kept look back.
fn rested(st: &mut State, app: &App) {
    let Some(name) = st.develop.look_hover.get_mut().under.clone() else {
        end(st, app);
        return;
    };
    // The chosen look's own row is the kept look; culling's panel is not
    // the frame's, and a pick still reading its sidecar has none yet.
    if name == app.get_look_name().as_str()
        || st.current.is_none()
        || st.culling.cull.is_some()
        || st.views.pick_pending.is_some()
    {
        end(st, app);
        return;
    }
    // This row's table on show or on its way: nothing to ask. One
    // dropped while the pointer was off the row is asked for again.
    if st
        .develop
        .look_hover
        .get_mut()
        .preview
        .as_ref()
        .is_some_and(|p| p.name == name && p.read != Read::Dropped)
    {
        return;
    }
    ask(st, app, name);
}

/// Ask for `name`'s table at the panel's Strength under its display
/// curve, on a thread.
fn ask(st: &mut State, app: &App, name: String) {
    let panel = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    let want = LookLut {
        lut: LutChoice::from_name(&name),
        strength: panel.look_lut.strength,
    };
    let current = st.current;
    let hover = st.develop.look_hover.get_mut();
    hover.generation += 1;
    let generation = hover.generation;
    hover.wanted.store(generation, Ordering::Relaxed);
    // The last row's look stays on show until this one lands, if it was
    // asked under the same frame, Strength and curve.
    let shown = hover
        .preview
        .take()
        .filter(|p| {
            p.frame == current && p.strength == want.strength && p.curve == panel.display_curve
        })
        .and_then(|p| p.shown);
    hover.preview = Some(Preview {
        name,
        frame: current,
        strength: want.strength,
        curve: panel.display_curve,
        generation,
        read: Read::Out,
        shown,
    });
    // None's row has nothing to read.
    if want.lut.is_none() {
        landed(st, app, generation, None);
        return;
    }
    let wanted = hover.wanted.clone();
    let store = crate::panel::look_remove::store();
    load(app, generation, wanted, store, want, panel.display_curve);
}

/// The look a row names, as the viewport would draw it once kept: the
/// chosen look's resolution and its cache, through the gate, so a table
/// fitted under another curve is no look here either.
fn resolve(store: Option<PathBuf>, want: &LookLut, curve: DisplayCurve) -> Option<Look> {
    want.look_under_in(&store?, curve)
}

/// Read the table on a thread of its own and land it. An ask replaced
/// or ended before the thread gets to it reads nothing: the look cache
/// holds its lock through a read, and a sweep down the list would
/// otherwise queue a read a row behind it.
#[cfg(not(test))]
fn load(
    app: &App,
    generation: u64,
    wanted: Arc<AtomicU64>,
    store: Option<PathBuf>,
    want: LookLut,
    curve: DisplayCurve,
) {
    let app_weak = app.as_weak();
    let spawned = std::thread::Builder::new()
        .name("greycard look preview".into())
        .spawn(move || {
            if wanted.load(Ordering::Relaxed) != generation {
                return;
            }
            let look = resolve(store, &want, curve);
            let _ = slint::invoke_from_event_loop(move || {
                let (Some(app), Some(state)) =
                    (app_weak.upgrade(), STATE.with(|s| s.borrow().clone()))
                else {
                    return;
                };
                landed(&mut state.borrow_mut(), &app, generation, look);
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("look preview: {e}");
    }
}

#[cfg(test)]
thread_local! {
    /// A test's reads, resolved and waiting for it to land them
    /// ([`tests::arrive`]), so it can say when they come.
    static READ: std::cell::RefCell<Vec<(u64, Option<Look>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// A test reads at once, if the ask is still wanted, and lands when it
/// says.
#[cfg(test)]
fn load(
    _app: &App,
    generation: u64,
    wanted: Arc<AtomicU64>,
    store: Option<PathBuf>,
    want: LookLut,
    curve: DisplayCurve,
) {
    if wanted.load(Ordering::Relaxed) != generation {
        return;
    }
    let look = resolve(store, &want, curve);
    READ.with(|r| r.borrow_mut().push((generation, look)));
}

/// A table read for a preview, landed: shown if it is still the one
/// asked for and the pointer is still on its row, dropped otherwise.
fn landed(st: &mut State, app: &App, generation: u64, look: Option<Look>) {
    let hover = st.develop.look_hover.get_mut();
    let under = hover.under.clone();
    let Some(preview) = hover.preview.as_mut() else {
        return;
    };
    if preview.generation != generation {
        return;
    }
    if under.as_deref() != Some(preview.name.as_str()) {
        preview.read = Read::Dropped;
        return;
    }
    preview.read = Read::Shown;
    preview.shown = Some(look);
    app.window().request_redraw();
}

/// The preview over, the kept look back on the next draw. The row under
/// the pointer is still known, so a leave reported later is matched.
pub(crate) fn end(st: &State, app: &App) {
    let mut hover = st.develop.look_hover.borrow_mut();
    hover.rest.stop();
    hover.wanted.store(0, Ordering::Relaxed);
    if hover.preview.take().is_some() {
        app.window().request_redraw();
    }
}

/// The preview over and the pointer taken to be off the list: its rows
/// were put up again, or the list was taken off the screen, and neither
/// reports a leave for the row the pointer was on. The next move over a
/// row reports it afresh.
pub(crate) fn forget(st: &State, app: &App) {
    end(st, app);
    st.develop.look_hover.borrow_mut().under = None;
}

/// A panel change: a Strength or a display curve other than the
/// preview's ends it.
pub(crate) fn panel_moved(st: &State, app: &App) {
    let moved = st
        .develop
        .look_hover
        .borrow()
        .preview
        .as_ref()
        .is_some_and(|p| {
            let curve = DisplayCurve::from_name(app.get_display_curve().as_str())
                .unwrap_or(st.develop.edit.display_curve);
            app.get_look_strength() != p.strength || curve != p.curve
        });
    if moved {
        end(st, app);
    }
}

/// Whether the preview still fits what the viewport draws: the frame it
/// was asked on, with nothing held or peeked over it, at its Strength
/// and display curve, its section open and on show.
fn fits(preview: &Preview, st: &State, app: &App, edit: &Edit) -> bool {
    preview.frame == st.current
        && st.develop.held.is_none()
        && st.develop.peek.is_none()
        && st.culling.cull.is_none()
        && st.views.pick_pending.is_none()
        && edit.look_lut.strength == preview.strength
        && edit.display_curve == preview.curve
        && app.get_panel_tab() == "Develop"
        && !app.get_collapsed_look()
        && !app.get_right_hidden()
}

/// The look the viewport draws `edit` through: the preview's while one
/// has landed and still fits, else the kept one. A preview that no
/// longer fits is ended here, so none outlives what it was asked for.
pub(crate) fn drawn(st: &State, app: &App, edit: &Edit) -> Option<Look> {
    let unfit = st
        .develop
        .look_hover
        .borrow()
        .preview
        .as_ref()
        .is_some_and(|p| !fits(p, st, app, edit));
    if unfit {
        end(st, app);
    }
    let hover = st.develop.look_hover.borrow();
    match hover.preview.as_ref().and_then(|p| p.shown.as_ref()) {
        Some(shown) => shown.clone(),
        None => st.develop.look.clone(),
    }
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    {
        let state = state.clone();
        app.on_look_hovered(move |name, on| {
            hovered(&mut state.borrow_mut(), name.as_str(), on);
        });
    }
    // The panel hidden, another tab, the section folded: the list went
    // with the pointer on it.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_look_list_gone(move || {
            // Raised by a property the window's own code may set while
            // it holds the state: the draw ends the preview then.
            if let (Some(app), Ok(st)) = (app_weak.upgrade(), state.try_borrow()) {
                forget(&st, &app);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::assets::show_looks;
    use crate::panel::edit::read_edit;
    use crate::testing::{LOOK_STORE, click, state_for, window};
    use slint::platform::WindowEvent;
    use std::path::Path;
    use std::time::Duration;

    /// The reads asked for so far, landed.
    fn arrive(state: &Rc<RefCell<State>>, app: &App) {
        let read: Vec<_> = READ.with(|r| r.borrow_mut().drain(..).collect());
        for (generation, look) in read {
            landed(&mut state.borrow_mut(), app, generation, look);
        }
    }

    /// Time passing, `ms` of it, and the timers due in it run.
    fn wait(ms: u64) {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ms));
        slint::platform::update_timers_and_animations();
    }

    /// A rest's worth, and a little.
    fn rest() {
        wait(REST.as_millis() as u64 + 20);
    }

    /// An identity table of `size` nodes an axis, so two looks are told
    /// apart by their size.
    fn table(dir: &Path, file: &str, size: usize) {
        let mut text = format!("LUT_3D_SIZE {size}\n");
        let step = 1.0 / (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let (r, g, b) = (r as f32 * step, g as f32 * step, b as f32 * step);
                    text.push_str(&format!("{r} {g} {b}\n"));
                }
            }
        }
        std::fs::write(dir.join(file), text).unwrap();
    }

    struct Setup {
        app: App,
        state: Rc<RefCell<State>>,
        dir: PathBuf,
    }

    /// A window on three frames, the first open with Mono kept at 60%,
    /// the look folder holding Neon (two nodes) and Mono (three), and the
    /// kept look resolved as the draw resolves it.
    fn setup(what: &str) -> Setup {
        let dir = crate::testing::scratch_dir(&format!("look-preview-{what}"));
        let store = dir.join("looks");
        std::fs::create_dir_all(&store).unwrap();
        table(&store, "Neon.cube", 2);
        table(&store, "Mono.cube", 3);
        LOOK_STORE.with(|s| *s.borrow_mut() = Some(store.clone()));
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, crate::testing::folder(3));
        app.invoke_select(0);
        {
            let mut st = state.borrow_mut();
            assert_eq!(st.current, Some(0));
            st.develop.looks = crate::panel::look_remove::list_in(Some(store.clone()));
            st.develop.edit.look_lut = LookLut {
                lut: LutChoice::Named("Mono".into()),
                strength: 0.6,
            };
            show_looks(&st, &st.develop.edit.look_lut, &app);
            st.develop.look = resolve(
                Some(store),
                &st.develop.edit.look_lut,
                st.develop.edit.display_curve,
            );
        }
        // The section's body grows to its rows over a moment.
        for _ in 0..4 {
            crate::testing::count_labeled(&app, "x");
            wait(300);
        }
        Setup { app, state, dir }
    }

    fn teardown(s: Setup) {
        LOOK_STORE.with(|s| *s.borrow_mut() = None);
        READ.with(|r| r.borrow_mut().clear());
        crate::testing::remove_scratch(s.state, &s.dir);
    }

    /// The middle of the row of the look `name`.
    fn row(app: &App, name: &str) -> slint::LogicalPosition {
        let (at, size) = crate::testing::labeled(app, &format!("Look {name}"));
        slint::LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0)
    }

    fn point_at(app: &App, position: slint::LogicalPosition) {
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
    }

    /// Off the list, over nothing that a move does anything to.
    fn away(app: &App) {
        point_at(app, slint::LogicalPosition::new(2.0, 400.0));
    }

    /// The look the viewport would draw now, by its table's size and its
    /// strength.
    fn drawn_now(s: &Setup) -> Option<(usize, f32)> {
        let st = s.state.borrow();
        let edit = read_edit(&s.app, &st.develop.edit, st.develop.target);
        drawn(&st, &s.app, &edit).map(|l| (l.lut.size, l.strength))
    }

    const MONO: Option<(usize, f32)> = Some((3, 0.6));
    const NEON: Option<(usize, f32)> = Some((2, 0.6));

    /// Resting on Neon's row shows Neon at the Strength set, once the
    /// rest is over and the table has landed; leaving the list brings
    /// Mono back after a rest. Nothing of it reaches the edit or the
    /// history.
    #[test]
    fn a_rest_on_a_row_shows_its_look_and_leaving_brings_the_kept_one_back() {
        let s = setup("rest");
        let states = || s.state.borrow().sidecars[0].states();
        let before = (states(), s.state.borrow().develop.edit.clone());
        assert_eq!(drawn_now(&s), MONO);
        point_at(&s.app, row(&s.app, "Neon"));
        assert_eq!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .under
                .as_deref(),
            Some("Neon")
        );
        // Not before the rest.
        wait(REST.as_millis() as u64 / 2);
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        assert_eq!(drawn_now(&s), MONO);
        // Asked for after it; the kept look stands until the table lands.
        rest();
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_some()
        );
        assert_eq!(drawn_now(&s), MONO);
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), NEON);
        // Off the list: Neon until the rest is over, then Mono.
        away(&s.app);
        assert_eq!(s.state.borrow().develop.look_hover.borrow().under, None);
        assert_eq!(drawn_now(&s), NEON);
        rest();
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        assert_eq!(drawn_now(&s), MONO);
        // Long past any save's wait: no step, the edit as it was.
        wait(crate::panel::edit::SAVE_MS + 100);
        assert_eq!(states(), before.0, "no history step");
        assert_eq!(s.state.borrow().develop.edit, before.1);
        assert_eq!(s.state.borrow().develop.edit.look_lut.lut.name(), "Mono");
        teardown(s);
    }

    /// The kept look's own row asks for nothing; None's shows no look at
    /// all, at once, with nothing to read.
    #[test]
    fn the_kept_row_shows_the_kept_look_and_none_shows_none() {
        let s = setup("none");
        point_at(&s.app, row(&s.app, "Mono"));
        rest();
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        point_at(&s.app, row(&s.app, "None"));
        rest();
        assert!(READ.with(|r| r.borrow().is_empty()), "nothing to read");
        assert_eq!(drawn_now(&s), None);
        away(&s.app);
        rest();
        assert_eq!(drawn_now(&s), MONO);
        teardown(s);
    }

    /// A table that lands after the pointer left its row is dropped; one
    /// that lands for a row since left for another is dropped too, and
    /// the next row's shows.
    #[test]
    fn a_table_landing_after_its_row_was_left_is_not_shown() {
        let s = setup("late");
        point_at(&s.app, row(&s.app, "Neon"));
        rest();
        away(&s.app);
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), MONO);
        rest();
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );

        // Neon asked for, then None's row before it lands.
        point_at(&s.app, row(&s.app, "Neon"));
        rest();
        let neon = READ.with(|r| r.borrow_mut().pop()).unwrap();
        point_at(&s.app, row(&s.app, "None"));
        landed(&mut s.state.borrow_mut(), &s.app, neon.0, neon.1);
        assert_eq!(drawn_now(&s), MONO);
        rest();
        assert_eq!(drawn_now(&s), None);
        teardown(s);
    }

    /// A sweep from row to row goes from one look to the next: the last
    /// row's stays on show until the next one's lands, never the kept
    /// look between.
    #[test]
    fn a_sweep_goes_from_look_to_look() {
        let s = setup("sweep");
        point_at(&s.app, row(&s.app, "Neon"));
        rest();
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), NEON);
        point_at(&s.app, row(&s.app, "None"));
        assert_eq!(drawn_now(&s), NEON);
        rest();
        assert_eq!(drawn_now(&s), None);
        teardown(s);
    }

    /// Another frame ends the preview: the draw hands the renderer the
    /// kept look, with no table of the last frame's left over, and a
    /// table still on its way for the last frame lands as nothing.
    #[test]
    fn a_frame_change_ends_the_preview() {
        let s = setup("frame");
        point_at(&s.app, row(&s.app, "Neon"));
        rest();
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), NEON);
        s.app.invoke_select(1);
        assert_eq!(s.state.borrow().current, Some(1));
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        assert_eq!(drawn_now(&s), MONO);

        point_at(&s.app, row(&s.app, "Neon"));
        rest();
        let late = READ.with(|r| r.borrow_mut().pop()).unwrap();
        s.app.invoke_select(2);
        landed(&mut s.state.borrow_mut(), &s.app, late.0, late.1);
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        assert_eq!(drawn_now(&s), MONO);
        teardown(s);
    }

    /// A Strength moved mid-hover ends the preview, as does a display
    /// curve switched; and the draw ends one that no longer fits the
    /// panel even when no change was reported.
    #[test]
    fn a_strength_or_curve_change_ends_the_preview() {
        let s = setup("strength");
        let previewed = |s: &Setup| {
            point_at(&s.app, row(&s.app, "Neon"));
            rest();
            arrive(&s.state, &s.app);
            assert_eq!(drawn_now(s).map(|l| l.0), Some(2));
        };
        previewed(&s);
        s.app.set_look_strength(0.3);
        s.app.invoke_view_changed();
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        assert_eq!(drawn_now(&s), MONO);

        // Asked again, now at 30%.
        away(&s.app);
        rest();
        previewed(&s);
        assert_eq!(drawn_now(&s), Some((2, 0.3)));
        s.app.invoke_look_curve_changed();
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );

        // Unreported: the draw sees the Strength moved and ends it.
        away(&s.app);
        rest();
        previewed(&s);
        s.app.set_look_strength(0.8);
        assert_eq!(drawn_now(&s), MONO);
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        teardown(s);
    }

    /// The pointer leaving the window leaves the row, and the kept look
    /// is back after a rest.
    #[test]
    fn the_pointer_leaving_the_window_ends_the_preview() {
        let s = setup("window");
        point_at(&s.app, row(&s.app, "Neon"));
        rest();
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), NEON);
        s.app.window().dispatch_event(WindowEvent::PointerExited);
        assert_eq!(s.state.borrow().develop.look_hover.borrow().under, None);
        rest();
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        assert_eq!(drawn_now(&s), MONO);
        teardown(s);
    }

    /// A click on the previewed row keeps the look through the list's
    /// own path: the edit names it at the Strength set, the preview is
    /// over, and the save writes one history step.
    #[test]
    fn a_click_keeps_the_look_as_one_step() {
        let s = setup("click");
        let states = || s.state.borrow().sidecars[0].states();
        let before = states();
        let neon = row(&s.app, "Neon");
        point_at(&s.app, neon);
        rest();
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), NEON);
        click(&s.app, neon.x, neon.y);
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        assert_eq!(s.state.borrow().develop.edit.look_lut.lut.name(), "Neon");
        assert_eq!(s.state.borrow().develop.edit.look_lut.strength, 0.6);
        wait(crate::panel::edit::SAVE_MS + 100);
        assert_eq!(states(), before + 1, "one history step");
        assert_eq!(
            s.state.borrow().sidecars[0].current.look_lut.lut.name(),
            "Neon"
        );
        // Resting on the row now kept asks for nothing.
        point_at(&s.app, slint::LogicalPosition::new(neon.x + 1.0, neon.y));
        rest();
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        assert!(READ.with(|r| r.borrow().is_empty()));
        teardown(s);
    }

    /// Off the row and back within a rest: a table that landed while the
    /// pointer was off is asked for again and shown, and one still on
    /// its way when the pointer came back is shown when it lands.
    #[test]
    fn back_on_the_row_within_a_rest_shows_its_look() {
        let s = setup("back");
        let neon = row(&s.app, "Neon");
        point_at(&s.app, neon);
        rest();
        away(&s.app);
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), MONO);
        point_at(&s.app, neon);
        rest();
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), NEON);

        // Again, back before it lands: the one read is enough.
        away(&s.app);
        rest();
        assert_eq!(drawn_now(&s), MONO);
        point_at(&s.app, neon);
        rest();
        away(&s.app);
        point_at(&s.app, neon);
        rest();
        assert_eq!(READ.with(|r| r.borrow().len()), 1, "asked once");
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), NEON);
        teardown(s);
    }

    /// The panel put away with the pointer on a row (F8, Tab) takes the
    /// row with it and reports no leave: the preview ends, the pointer
    /// is off the list, and the panel back shows the kept look.
    #[test]
    fn the_panel_put_away_ends_the_preview() {
        use slint::platform::Key;
        let s = setup("hidden");
        for key in [Key::F8, Key::Tab] {
            point_at(&s.app, row(&s.app, "Neon"));
            rest();
            arrive(&s.state, &s.app);
            assert_eq!(drawn_now(&s), NEON);
            crate::testing::press(&s.app, key);
            wait(0);
            assert!(s.app.get_right_hidden(), "{key:?} put the panel away");
            assert_eq!(s.state.borrow().develop.look_hover.borrow().under, None);
            assert!(
                s.state
                    .borrow()
                    .develop
                    .look_hover
                    .borrow()
                    .preview
                    .is_none()
            );
            assert_eq!(drawn_now(&s), MONO);
            crate::testing::press(&s.app, key);
            wait(0);
            assert!(!s.app.get_right_hidden());
            rest();
            assert_eq!(drawn_now(&s), MONO);
            // The section's body grows to its rows again.
            for _ in 0..4 {
                crate::testing::count_labeled(&s.app, "x");
                wait(300);
            }
        }
        teardown(s);
    }

    /// The list's rows put up again (an undo, a history pick, a sync
    /// landing all show the edit, and its looks with it) end the
    /// preview: the row under the pointer is a new one that never
    /// reports a leave.
    #[test]
    fn the_rows_put_up_again_end_the_preview() {
        let s = setup("rows");
        point_at(&s.app, row(&s.app, "Neon"));
        rest();
        arrive(&s.state, &s.app);
        assert_eq!(drawn_now(&s), NEON);
        {
            let st = s.state.borrow();
            crate::panel::edit::show_edit(&st, &st.develop.edit, &s.app, st.develop.target);
        }
        assert_eq!(s.state.borrow().develop.look_hover.borrow().under, None);
        assert!(
            s.state
                .borrow()
                .develop
                .look_hover
                .borrow()
                .preview
                .is_none()
        );
        assert_eq!(drawn_now(&s), MONO);
        teardown(s);
    }
}
