use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::WindowEvent;
use slint::{ComponentHandle, ModelRc};

use crate::worker::Worker;
use crate::{App, Shared, State, Thumb, grid, install_callbacks};

mod by_label;

// The helpers the render crate's tests share with these.
pub(crate) use greycard_engine::testing::{linear_dng, monochrome_dng, scratch_dir};
pub(crate) use greycard_render::testing::{bar_png, remove_dir_retry};

thread_local! {
    /// The look folder a test's removal or rename works on, in place of
    /// the user's (`panel::look_remove::store`).
    pub(crate) static LOOK_STORE: RefCell<Option<std::path::PathBuf>> =
        const { RefCell::new(None) };
}

/// The windows a test's thread has shown, hidden as the thread ends.
/// A shown window is held by the backend, and with it every callback
/// and the state and worker they hold, so none would drop: each
/// test's worker and culling threads would then sleep on for the rest
/// of the run, and macOS caps a process's threads well short of the
/// suite's thousand and more workers.
struct Shown(Vec<slint::Weak<App>>);

impl Drop for Shown {
    fn drop(&mut self) {
        for weak in self.0.drain(..) {
            // Run as the thread's locals are torn down, some of them
            // perhaps gone already: a panic here, in the hide or in the
            // drops of the App and all it holds, would abort the whole
            // run rather than fail one test. So the App is upgraded and
            // dropped inside the catch.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                if let Some(app) = weak.upgrade() {
                    let _ = app.hide();
                }
            }));
        }
    }
}

thread_local! {
    static SHOWN: RefCell<Shown> = const { RefCell::new(Shown(Vec::new())) };
}

/// The off-card checks a test's, before any test of this process runs,
/// whichever runs and whether it opens a window: no test reads the
/// user's model store, ONNX Runtime, or an adapter for them
/// (`greycard_ai::runtime::off_card_for_tests`). A constructor the
/// system loader runs at the test binary's start, as the `ctor` crate
/// makes one; only the test binary has it, so a binary keeps the real
/// path.
#[used]
#[cfg_attr(
    any(target_os = "linux", target_os = "android", target_os = "freebsd"),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
static OFF_CARD_FOR_TESTS: extern "C" fn() = {
    extern "C" fn off_card_for_tests() {
        greycard_ai::runtime::off_card_for_tests();
    }
    off_card_for_tests
};

/// Whether the process start ran [`OFF_CARD_FOR_TESTS`].
#[test]
fn every_test_of_this_binary_starts_with_the_off_card_checks_a_tests() {
    assert!(greycard_ai::runtime::off_card_is_for_tests());
}

/// The testing backend, on a thread that has none yet: once a
/// thread, whether the tests run one at a time or all at once, since
/// the backend is a thread's own and setting it twice panics.
pub(crate) fn backend() {
    BACKEND.with(|set| {
        if !set.replace(true) {
            i_slint_backend_testing::init_no_event_loop();
        }
    });
}

thread_local! {
    static BACKEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`backend`] with Slint's software renderer in it, so the window can
/// be drawn (`Window::take_snapshot`), for a test that captures what a
/// drag shows: first on its thread, ahead of [`window`].
pub(crate) fn drawing_backend() {
    BACKEND.with(|set| {
        assert!(
            !set.replace(true),
            "a backend is already set on this thread"
        );
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .expect("no platform yet on this thread");
    });
}

/// An App with a folder of eleven frames on it, shown and
/// focused, on a backend that opens no window.
pub(crate) fn window(count: usize) -> App {
    backend();
    // A camera profile directory of the tests' own, never the user's.
    greycard_edit_io::profiles::use_store(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        format!("../../target/scratch/profiles-{}", std::process::id()),
    ));
    let app = App::new().expect("the window builds");
    // English, whatever the machine's locale, which Slint would pick
    // for its bundled catalogs on its own. `GREYCARD_TEST_LANG` names a
    // catalog to run the suite in instead: what fails then is a test
    // that finds a control by its words rather than its id, or one
    // whose subject is the words.
    let language = std::env::var("GREYCARD_TEST_LANG").unwrap_or_default();
    slint::select_bundled_translation(&language).expect("the window's catalogs are bundled");
    crate::i18n::select(&language);
    crate::panel::prefs::show_languages(&app, "");
    crate::panel::startup::show_unopened(&app);
    let thumbs: Vec<Thumb> = (0..count)
        .map(|i| Thumb {
            name: format!("f{i:02}.CR3").into(),
            image: slint::Image::default(),
            ..Default::default()
        })
        .collect();
    crate::cells::set_rows(&app, thumbs);
    // The key table, without which no key does anything.
    crate::keys::install(&app);
    app.on_grid_columns(grid::columns);
    app.on_grid_slack(grid::slack);
    crate::panel::browser::grid_layout(&app);
    crate::panel::browser::strip_layout(&app);
    app.on_grid_tiles_top(grid::tiles_top);
    app.on_grid_common_pitch(grid::common_pitch);
    app.on_lightbox_cell(crate::lightbox::cell);
    crate::panel::browser::tile_size(&app);
    // The sections in their first order, as a fresh install has them.
    crate::panel::order::show(&app, &Default::default());
    app.on_tab_panes(crate::panes::tab);
    app.on_bar_length(grid::bar_length);
    app.on_bar_offset(grid::bar_offset);
    app.on_bar_drag(grid::bar_drag);
    app.show().expect("the window shows");
    SHOWN.with(|s| s.borrow_mut().0.push(app.as_weak()));
    app.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    app
}

/// The window's words in the bundled language of `tag`, on both sides
/// (Slint's catalogs and `tr!`), for this test's thread alone, until
/// the guard drops and hands back the suite's language. A thread the
/// test spawns does not inherit it: it reads the suite's language.
pub(crate) fn in_language(tag: &str) -> Language {
    slint::select_bundled_translation(tag).expect("the language is bundled");
    crate::i18n::select_on_this_thread(Some(tag));
    Language
}

/// [`in_language`]'s guard.
pub(crate) struct Language;

impl Drop for Language {
    fn drop(&mut self) {
        let suite = std::env::var("GREYCARD_TEST_LANG").unwrap_or_default();
        // A test that failed has its window gone; the catalogs may be
        // too, and a panic here would hide the failure.
        let _ = slint::select_bundled_translation(&suite);
        crate::i18n::select_on_this_thread(None);
    }
}

/// A press and a release at a point of the window, logical pixels
/// from its top left: what a click on whatever is there does.
pub(crate) fn click(app: &App, x: f32, y: f32) {
    click_with(app, slint::platform::PointerEventButton::Left, x, y);
}

/// [`click`] with another button.
pub(crate) fn click_with(app: &App, button: slint::platform::PointerEventButton, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    for event in [
        WindowEvent::PointerMoved { position },
        WindowEvent::PointerPressed { position, button },
        WindowEvent::PointerReleased { position, button },
    ] {
        app.window().dispatch_event(event);
    }
}

/// A press at `path[0]`, a move through each point after it, and a
/// release at the last: what dragging across the window, logical
/// pixels from its top left, does. Each point is its own
/// `PointerMoved`, so a caller after a click-versus-drag threshold
/// gets to decide, on the way through, which side of it a step lands.
pub(crate) fn drag(app: &App, path: &[(f32, f32)]) {
    drag_with(app, slint::platform::PointerEventButton::Left, path);
}

/// [`drag`] with another button.
pub(crate) fn drag_with(
    app: &App,
    button: slint::platform::PointerEventButton,
    path: &[(f32, f32)],
) {
    let at = |p: (f32, f32)| slint::LogicalPosition::new(p.0, p.1);
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: at(path[0]),
    });
    app.window().dispatch_event(WindowEvent::PointerPressed {
        position: at(path[0]),
        button,
    });
    for &p in &path[1..] {
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position: at(p) });
    }
    app.window().dispatch_event(WindowEvent::PointerReleased {
        position: at(*path.last().unwrap()),
        button,
    });
}

pub(crate) fn press(app: &App, key: impl Into<slint::SharedString>) {
    let text = key.into();
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}

/// A state over `files` and a no-op worker, wired to `app` exactly
/// as `run` wires the real one: enough to drive any of the window's
/// callbacks without a picture open or a file on disk.
pub(crate) fn state_for(app: &App, files: Vec<std::path::PathBuf>) -> (Rc<Shared>, Rc<Worker>) {
    let worker = Rc::new(Worker::new(|_| {}));
    (state_with(app, files, worker.clone()), worker)
}

/// [`state_for`] with the caller's worker: one whose outcomes the test
/// takes in itself.
pub(crate) fn state_with(
    app: &App,
    files: Vec<std::path::PathBuf>,
    worker: Rc<Worker>,
) -> Rc<Shared> {
    let state = Rc::new(Shared::new(State::empty(files, app)));
    // What culling asks of its decode threads, kept for the tests to
    // read; a shipping build keeps none of it.
    state.borrow().culling.prefetch.record();
    app.set_patch_handles(ModelRc::from(state.borrow().develop.patch_handles.clone()));
    install_callbacks(app, state.clone(), worker);
    state
}

/// The current generation's develop landed, a 60x40 picture with
/// `learned` and `fills` its reports, and the status line it leaves.
pub(crate) fn land_develop(
    app: &App,
    state: &Shared,
    learned: crate::worker::LearnedReport,
    fills: crate::worker::FillReport,
) -> String {
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
            seconds: 13.7,
            sharpen: None,
            sources: Vec::new(),
            learned,
            fills,
        },
    );
    app.get_status().to_string()
}

/// Frame 0 on screen with its open sent, then its file followed to each
/// path of `to` in turn and opened again under each, as the follows do
/// (`browser::reopen_moved`). Every path it had, the first first.
pub(crate) fn open_and_follow(
    st: &mut State,
    app: &App,
    worker: &Worker,
    to: &[&str],
) -> Vec<std::path::PathBuf> {
    st.current = Some(0);
    let first = st.path_of(0).clone();
    st.develop.open_sent = Some((st.develop.generation, (first.clone(), None)));
    let mut paths = vec![first];
    for to in to {
        let to = std::path::PathBuf::from(to);
        st.move_file(0, to.clone());
        assert!(crate::panel::browser::reopen_moved(st, app, worker));
        paths.push(to);
    }
    paths
}

/// A file-less state and a no-op worker: enough to drive the retouch
/// callbacks without a picture open.
pub(crate) fn retouch_state(app: &App) -> (Rc<Shared>, Rc<Worker>) {
    state_for(app, Vec::new())
}

/// A folder of `count` frames nothing has ever written, named as a
/// camera names them, in a directory that does not exist: the
/// browser reads the sidecars it is given and never the disk.
pub(crate) fn folder(count: usize) -> Vec<std::path::PathBuf> {
    (0..count)
        .map(|i| std::path::PathBuf::from("/nowhere").join(format!("IMG_{i:04}.CR3")))
        .collect()
}

/// How many elements anywhere in the window carry `label` as their
/// accessible label: whether a control the layout means to show once
/// is there once, or twice by a mistake in it.
pub(crate) fn count_labeled(app: &App, label: &str) -> usize {
    use i_slint_backend_testing::ElementHandle;
    // As `labeled`'s own first move: a control a property has just
    // brought in is built on the next event. Query only once the
    // layout has settled into the shape under test — a query before
    // a control's `if` turns true leaves later ones, even of a
    // control already there, finding nothing: call this again after
    // such a change rather than before it.
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    ElementHandle::find_by_accessible_label(app, label).count()
}

/// The bounds of the element a screen reader knows as `label`,
/// scrolled into the panel's view first: its top left and its size,
/// in logical pixels from the window's top left.
pub(crate) fn labeled(app: &App, label: &str) -> (slint::LogicalPosition, slint::LogicalSize) {
    labeled_where(app, label, |_| true)
}

/// The part `part` (as `Section::row`) of the first section of the
/// type `section` (as `LightSection`) that has it built.
pub(crate) fn section_part(
    app: &App,
    section: &str,
    part: &str,
) -> Option<i_slint_backend_testing::ElementHandle> {
    i_slint_backend_testing::ElementHandle::find_by_element_id(app, &format!("{section}::root"))
        .find_map(|e| e.query_descendants().match_id(part).find_first())
}

/// The header of the section of the type `section` (as
/// `LightSection`), the row a click folds it by, scrolled into the
/// panel's view as [`by_id`] scrolls.
pub(crate) fn header(app: &App, section: &str) -> (slint::LogicalPosition, slint::LogicalSize) {
    section_bounds(app, section, "Section::row")
}

/// The reset in the header of the section of the type `section` (as
/// `LightSection`), scrolled into the panel's view as [`by_id`]
/// scrolls.
pub(crate) fn header_reset(
    app: &App,
    section: &str,
) -> (slint::LogicalPosition, slint::LogicalSize) {
    section_bounds(app, section, "Section::reset-button")
}

/// The bounds of [`section_part`], scrolled into the panel's view as
/// [`by_id`] scrolls.
pub(crate) fn section_bounds(
    app: &App,
    section: &str,
    part: &str,
) -> (slint::LogicalPosition, slint::LogicalSize) {
    in_panel_view(app, &format!("{part} of {section}"), |app| {
        section_part(app, section, part)
    })
}

/// [`labeled`] of the first element labeled `label` that `pick` takes.
pub(crate) fn labeled_where(
    app: &App,
    label: &str,
    pick: impl Fn(&i_slint_backend_testing::ElementHandle) -> bool,
) -> (slint::LogicalPosition, slint::LogicalSize) {
    let what = format!("labeled {label:?}");
    in_panel_view(app, &what, |app| {
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(app, label)
            .find(|e| pick(e))
    })
}

/// The bounds of the panel's element `id` (as `Section::name`),
/// scrolled into the panel's view as [`labeled`] scrolls.
pub(crate) fn by_id(app: &App, id: &str) -> (slint::LogicalPosition, slint::LogicalSize) {
    in_panel_view(app, &format!("with the id {id:?}"), |app| {
        i_slint_backend_testing::ElementHandle::find_by_element_id(app, id).next()
    })
}

/// The bounds of the panel's element `pick` finds, scrolled into the
/// panel's view; `what` names it for a panic.
fn in_panel_view(
    app: &App,
    what: &str,
    pick: impl Fn(&App) -> Option<i_slint_backend_testing::ElementHandle>,
) -> (slint::LogicalPosition, slint::LogicalSize) {
    use i_slint_backend_testing::ElementHandle;
    let find = || {
        // A control a property has just brought in is built on the
        // next event: a pointer move at the corner, where nothing is.
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        pick(app)
    };
    // The panel's view, measured rather than guessed: it starts under
    // the tab bar and, on the Masks tab, the Adjustments list pinned
    // above it, and ends over the footer. What sits above a control
    // depends on the machine (the note naming the profile directory,
    // the looks in the store), and a step longer than the view lets a
    // control jump from under its foot to half behind the tab bar,
    // where a click at its middle misses it. Looked up once the layout
    // has its shape, as `find` does.
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    let view = || {
        let view = ElementHandle::find_by_element_id(app, "App::panel-view").next()?;
        let sections = ElementHandle::find_by_element_id(app, "App::panel-sections").next()?;
        let (at, size) = (view.absolute_position(), view.size());
        // As far as the view scrolls: the sections' foot at its own,
        // where a wheel stops.
        let end = (sections.size().height - size.height).max(0.0);
        Some((at.y, at.y + size.height, end))
    };
    let (top, bottom, _) = view().unwrap_or_else(|| {
        let height = app
            .window()
            .size()
            .to_logical(app.window().scale_factor())
            .height;
        (80.0, height, 0.0)
    });
    let scroll_to = |to: f32| {
        let end = view().map_or(f32::INFINITY, |(_, _, end)| end);
        app.set_panel_scroll(to.clamp(-end, 0.0));
    };
    // The panel's scroll only builds what is in view: down it most of
    // a view at a time from the top until the control is there, then
    // to where it is well inside the view.
    scroll_to(0.0);
    for _ in 0..40 {
        if let Some(element) = find() {
            let (at, size) = (element.absolute_position(), element.size());
            // Above the view (the tab bar, the pinned list): there
            // already.
            if at.y > 80.0 && at.y + size.height <= top {
                return (at, size);
            }
            if at.y >= top && at.y + size.height <= bottom {
                return (at, size);
            }
            scroll_to(app.get_panel_scroll() - (at.y - (top + bottom) / 2.0));
            let element = find().unwrap_or_else(|| panic!("the element {what} scrolled away"));
            return (element.absolute_position(), element.size());
        }
        // Less than the view's height, so nothing falls between steps.
        scroll_to(app.get_panel_scroll() - ((bottom - top) * 0.8).max(40.0));
    }
    panic!("nothing {what}");
}

/// The panel's scroll after the layout has settled, and how far its
/// sections can scroll: their height less the view's.
pub(crate) fn panel_scroll_and_end(app: &App) -> (f32, f32) {
    use i_slint_backend_testing::ElementHandle;
    settle(app);
    let find = |id: &str| ElementHandle::find_by_element_id(app, id).next();
    let view = find("App::panel-view").expect("the panel's view");
    // Scrolled wholly out of the view, the sections are not found:
    // they reach nowhere the scroll is.
    let Some(sections) = find("App::panel-sections") else {
        return (app.get_panel_scroll(), f32::NAN);
    };
    let end = (sections.size().height - view.size().height).max(0.0);
    (app.get_panel_scroll(), end)
}

/// The panel scrolled no further than its sections reach: never past
/// their foot, with room left empty under it and their top out of
/// reach above, and never above their top.
pub(crate) fn assert_panel_in_range(app: &App, what: &str) {
    let (scroll, end) = panel_scroll_and_end(app);
    assert!(
        scroll <= 0.0 && scroll >= -end - 0.5,
        "{what}: the panel scrolled to {scroll}, its sections reach {}",
        -end
    );
}

/// The panel scrolled to its sections' foot.
pub(crate) fn panel_to_end(app: &App) {
    let (_, end) = panel_scroll_and_end(app);
    assert!(!end.is_nan(), "the sections are out of the view");
    app.set_panel_scroll(-end);
    settle(app);
}

/// A rectangle of the window: its top left and its size, in logical
/// pixels from the window's top left.
pub(crate) type Bounds = (slint::LogicalPosition, slint::LogicalSize);

/// The bounds of `element`.
pub(crate) fn bounds_of(element: &i_slint_backend_testing::ElementHandle) -> Bounds {
    (element.absolute_position(), element.size())
}

/// The name drawn under each frame's cell named `name`, in the strip
/// and on the grid: the cell says it to a screen reader, its words
/// do not, so they are found inside the cell.
pub(crate) fn cell_names(app: &App, name: &str) -> Vec<i_slint_backend_testing::ElementHandle> {
    use i_slint_backend_testing::ElementHandle;
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    ElementHandle::find_by_accessible_label(app, name)
        .filter_map(|cell| {
            let words = match cell.id()?.as_str() {
                "Filmstrip::cell" => "Filmstrip::name-text",
                "GridSheet::cell" => "GridSheet::cell-name",
                _ => return None,
            };
            cell.query_descendants().match_id(words).find_first()
        })
        .collect()
}

/// A `Toggle`'s parts: the whole row, its label, its switch, and the
/// target a click flips it in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToggleParts {
    pub(crate) row: Bounds,
    pub(crate) label: Bounds,
    pub(crate) switch: Bounds,
    pub(crate) target: Bounds,
}

/// The parts of the `Toggle` under `root`.
pub(crate) fn toggle_parts(root: &i_slint_backend_testing::ElementHandle) -> ToggleParts {
    let part = |id: &str| {
        root.query_descendants()
            .match_id(id)
            .find_first()
            .map(|e| bounds_of(&e))
            .unwrap_or_else(|| panic!("a Toggle has its {id}"))
    };
    ToggleParts {
        row: bounds_of(root),
        label: part("Toggle::lbl"),
        switch: part("Toggle::knob-track"),
        target: part("Toggle::ta"),
    }
}

/// The parts of the `Toggle` labeled `label`, scrolled into the
/// panel's view first as [`labeled`] scrolls.
pub(crate) fn toggle(app: &App, label: &str) -> ToggleParts {
    use i_slint_backend_testing::ElementHandle;
    let (at, size) = labeled(app, label);
    let (x, y) = (at.x + 1.0, at.y + size.height / 2.0);
    let root = ElementHandle::find_by_element_id(app, "Toggle::root")
        .find(|e| {
            let (p, s) = bounds_of(e);
            x >= p.x && x <= p.x + s.width && y >= p.y && y <= p.y + s.height
        })
        .unwrap_or_else(|| panic!("no Toggle under {label:?}"));
    toggle_parts(&root)
}

/// The parts of the `Toggle` `id` (as
/// `WhiteBalanceSection::as-shot-toggle`), scrolled into the panel's
/// view first as [`by_id`] scrolls.
pub(crate) fn toggle_by_id(app: &App, id: &str) -> ToggleParts {
    by_id(app, id);
    let root = i_slint_backend_testing::ElementHandle::find_by_element_id(app, id)
        .next()
        .unwrap_or_else(|| panic!("no Toggle {id}"));
    toggle_parts(&root)
}

/// A click at the middle of the switch of the `Toggle` `id`.
pub(crate) fn click_switch_by_id(app: &App, id: &str) {
    let (at, size) = toggle_by_id(app, id).switch;
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

/// The buttons a screen reader knows as `label`, top first: their top
/// left and size, in logical pixels from the window's top left. A
/// text's own label is its words, so a row whose name is drawn in it
/// is found once here rather than twice.
pub(crate) fn buttons(app: &App, label: &str) -> Vec<(slint::LogicalPosition, slint::LogicalSize)> {
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    let mut found: Vec<_> = ElementHandle::find_by_accessible_label(app, label)
        .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
        .map(|e| (e.absolute_position(), e.size()))
        .collect();
    found.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
    found
}

/// The middle of row `i`'s picture in the strip, in logical pixels
/// from the window's top left: its cell where the strip's layout puts
/// it, less the strip's scroll, in the window's foot.
pub(crate) fn strip_cell(app: &App, i: usize) -> (f32, f32) {
    let (left, width) = crate::cells::strip_layout(app)
        .place(i as i32)
        .expect("a row the strip has");
    let height = app
        .window()
        .size()
        .to_logical(app.window().scale_factor())
        .height;
    (
        left + width / 2.0 - app.get_strip_scroll(),
        height - app.get_strip_h()
            + crate::strip::PAD
            + crate::strip::RING
            + app.get_strip_photo() / 2.0,
    )
}

/// Type `text` a key at a time into whatever has the focus.
pub(crate) fn type_text(app: &App, text: &str) {
    for c in text.chars() {
        press(app, c.to_string());
    }
}

/// A test's scratch folder removed once the state has let go of what
/// it holds open under it: the index's reader is closed first, since
/// the window's callbacks keep their own handle on the state and a
/// `drop` in the test does not end it, and Windows refuses to remove
/// an open file. The removal is tried again for a while, as
/// [`rename_away`] is, for the pool's reads still in flight.
pub(crate) fn remove_scratch(state: Rc<Shared>, dir: &std::path::Path) {
    state.borrow_mut().views.index_reader = None;
    drop(state);
    remove_dir_retry(dir);
}

/// What is under `dir` a moment after it was removed, the folder itself
/// first if it is there: what a thread still at work wrote after the
/// removal, which on Windows is a removal that fails.
pub(crate) fn left_in(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::thread::sleep(std::time::Duration::from_millis(200));
    let mut left = Vec::new();
    let mut walk = vec![dir.to_path_buf()];
    while let Some(d) = walk.pop() {
        if !d.exists() {
            continue;
        }
        left.push(d.clone());
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk.push(p);
            } else {
                left.push(p);
            }
        }
    }
    left
}

/// The pointer at the middle of the element a screen reader knows as
/// `label`, resting there past the hold; its place in the window.
pub(crate) fn rest_on(app: &App, label: &str) -> (f32, f32) {
    let (at, size) = labeled(app, label);
    let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(x, y),
    });
    slint::platform::update_timers_and_animations();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(800));
    slint::platform::update_timers_and_animations();
    (x, y)
}

/// [`rest_on`] the element `id`.
pub(crate) fn rest_on_id(app: &App, id: &str) -> (f32, f32) {
    let (at, size) = by_id(app, id);
    let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(x, y),
    });
    slint::platform::update_timers_and_animations();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(800));
    slint::platform::update_timers_and_animations();
    (x, y)
}

/// Timers and animations a few frames on, after a control a property
/// has just brought in is built on the next event: a sheet's layout
/// settled.
pub(crate) fn settle(app: &App) {
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    for _ in 0..5 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
    }
}

/// The top and height of the text that starts with `words`; `None`
/// while it is clipped out of sight whole, as past a pane's foot.
fn line_starting(app: &App, words: &str) -> Option<(f32, f32)> {
    use i_slint_backend_testing::ElementHandle;
    ElementHandle::find_by_element_type_name(app, "Text")
        .find(|e| e.accessible_label().is_some_and(|l| l.starts_with(words)))
        .map(|e| (e.absolute_position().y, e.size().height))
}

/// A sheet's scroller, the one `width` wide (at least the first
/// number and under the second): its top left and size.
fn pane(app: &App, width: (f32, f32)) -> (slint::LogicalPosition, slint::LogicalSize) {
    use i_slint_backend_testing::ElementHandle;
    ElementHandle::find_by_element_type_name(app, "Flickable")
        .map(|e| (e.absolute_position(), e.size()))
        .find(|(p, s)| s.width >= width.0 && s.width < width.1 && p.y > 0.0)
        .expect("the sheet's pane")
}

/// Whether the line that starts with `last` lies whole inside the pane
/// `pane_width` wide, as laid out now, and where it is.
fn inside(app: &App, last: &str, pane_width: (f32, f32)) -> (bool, String) {
    let (p, s) = pane(app, pane_width);
    let foot = p.y + s.height;
    match line_starting(app, last) {
        Some((y, h)) => (
            y >= p.y - 0.5 && y + h <= foot + 0.5,
            format!("{last:?} at {y}+{h}, the pane {}..{foot}", p.y),
        ),
        None => (
            false,
            format!("{last:?} out of sight, the pane {}..{foot}", p.y),
        ),
    }
}

/// The line that starts with `last` is past the foot of the pane
/// `pane_width` wide, and the wheel over the pane brings it in: the
/// pane's content is as tall as what it draws.
pub(crate) fn wheel_brings_in(app: &App, last: &str, pane_width: (f32, f32)) {
    settle(app);
    let (whole, said) = inside(app, last, pane_width);
    assert!(!whole, "the wheel has nothing to prove: {said}");
    let (p, s) = pane(app, pane_width);
    for _ in 0..40 {
        app.window().dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(p.x + 40.0, p.y + (s.height / 2.0).min(40.0)),
            delta_x: 0.0,
            delta_y: -120.0,
        });
        settle(app);
        if inside(app, last, pane_width).0 {
            return;
        }
    }
    let (_, said) = inside(app, last, pane_width);
    panic!("the wheel left {said}");
}

/// A sheet whose notes wrap keeps its last line, the text that starts
/// with `last`, in its pane `pane_width` wide: inside it in a window
/// `tall` high, room for all of the sheet, and in one 400 high the
/// wheel over the pane brings it in. A layout's preferred height, read
/// as a property, can count a wrapped note as one line, and a sheet or
/// a pane sized from it cuts its foot.
pub(crate) fn keeps_its_last_line(app: &App, last: &str, pane_width: (f32, f32), tall: f32) {
    app.window().set_size(slint::LogicalSize::new(1500.0, tall));
    settle(app);
    let (whole, said) = inside(app, last, pane_width);
    assert!(whole, "in a window {tall} high, {said}");
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 400.0));
    wheel_brings_in(app, last, pane_width);
}

/// The middle of the panel slider `id`'s track (as
/// `LightSection::exposure`), where a centered slider rests: a click
/// there focuses it and moves nothing. The row's own middle is not
/// it, the label's column and the value's field being different
/// widths.
pub(crate) fn track_middle(app: &App, id: &str) -> (f32, f32) {
    let (at, size) = by_id(app, id);
    let (field, _) = slider_field(app, id);
    let from = at.x + app.global::<crate::LabelColumn>().get_width();
    let to = field.x - 8.0;
    ((from + to) / 2.0, at.y + size.height / 2.0)
}

/// The value field of the panel slider `id`, scrolled into the
/// panel's view as [`by_id`] scrolls: where a click types a value.
pub(crate) fn slider_field(app: &App, id: &str) -> (slint::LogicalPosition, slint::LogicalSize) {
    by_id(app, id);
    i_slint_backend_testing::ElementHandle::find_by_element_id(app, id)
        .find_map(|e| {
            e.query_descendants()
                .match_id("EditSlider::field")
                .find_first()
        })
        .map(|e| bounds_of(&e))
        .unwrap_or_else(|| panic!("{id} has its value field"))
}

/// A point on the label of the panel slider `id`, scrolled into the
/// panel's view: a press there focuses it and moves nothing.
pub(crate) fn slider_label(app: &App, id: &str) -> (f32, f32) {
    by_id(app, id);
    let (at, size) = i_slint_backend_testing::ElementHandle::find_by_element_id(app, id)
        .find_map(|e| {
            e.query_descendants()
                .match_id("EditSlider::label-words")
                .find_first()
        })
        .map(|e| bounds_of(&e))
        .unwrap_or_else(|| panic!("{id} has its label"));
    (at.x + 8.0, at.y + size.height / 2.0)
}

/// A click at the middle of the element `id`, scrolled into the
/// panel's view as [`by_id`] scrolls.
pub(crate) fn click_id(app: &App, id: &str) {
    let (at, size) = by_id(app, id);
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

/// The panel's tabs, as the row `App::panel-tabs` lists its options.
pub(crate) const PANEL_TABS: [&str; 4] = ["Develop", "Crop", "Masks", "Retouch"];

/// The option `option` of the `Segmented` row `id`, whose options are
/// `options` in their order: its top left and size.
pub(crate) fn segment(
    app: &App,
    id: &str,
    options: &[&str],
    option: &str,
) -> (slint::LogicalPosition, slint::LogicalSize) {
    let at = options
        .iter()
        .position(|o| *o == option)
        .unwrap_or_else(|| panic!("no option {option:?} in {options:?}"));
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    i_slint_backend_testing::ElementHandle::find_by_element_id(app, id)
        .find_map(|row| {
            row.query_descendants()
                .match_id("Segmented::seg")
                .find_all()
                .into_iter()
                .nth(at)
        })
        .map(|e| bounds_of(&e))
        .unwrap_or_else(|| panic!("{id} has its option {option:?}"))
}

/// A click at the middle of the option `option` of the `Segmented`
/// row `id`, as [`segment`] finds it: a row in view, as a sheet's is.
pub(crate) fn click_segment(app: &App, id: &str, options: &[&str], option: &str) {
    let (at, size) = segment(app, id, options, option);
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

/// How many options the `Segmented` row `id` shows; none when the
/// row is not there.
pub(crate) fn segments_shown(app: &App, id: &str) -> usize {
    count_id(app, id);
    i_slint_backend_testing::ElementHandle::find_by_element_id(app, id)
        .next()
        .map_or(0, |row| {
            row.query_descendants()
                .match_id("Segmented::seg")
                .find_all()
                .len()
        })
}

/// A click on the panel's tab `tab` (one of [`PANEL_TABS`]).
pub(crate) fn click_panel_tab(app: &App, tab: &str) {
    let (at, size) = segment(app, "App::panel-tabs", &PANEL_TABS, tab);
    click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
}

/// Whether the control `id` is pressed, as a screen reader is told;
/// `None` when there is no such control.
pub(crate) fn pressed_id(app: &App, id: &str) -> Option<bool> {
    count_id(app, id);
    i_slint_backend_testing::ElementHandle::find_by_element_id(app, id)
        .next()
        .and_then(|e| e.accessible_checked())
}

/// How many elements anywhere in the window have the id `id`, as
/// [`count_labeled`] counts by a label: whether a control the layout
/// means to show is there.
pub(crate) fn count_id(app: &App, id: &str) -> usize {
    app.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(0.0, 0.0),
    });
    i_slint_backend_testing::ElementHandle::find_by_element_id(app, id).count()
}
