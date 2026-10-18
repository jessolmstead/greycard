use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::WindowEvent;
use slint::{ComponentHandle, ModelRc};

use crate::worker::Worker;
use crate::{App, State, Thumb, grid, install_callbacks};

// The helpers the render crate's tests share with these.
pub(crate) use greycard_render::testing::{GFX, bar_png, remove_dir_retry, white_shift};

thread_local! {
    /// The look folder a test's removal or rename works on, in place of
    /// the user's (`panel::look_remove::store`).
    pub(crate) static LOOK_STORE: RefCell<Option<std::path::PathBuf>> =
        const { RefCell::new(None) };
}

/// An App with a folder of eleven frames on it, shown and
/// focused, on a backend that opens no window.
pub(crate) fn window(count: usize) -> App {
    // Once a thread, whether the tests run one at a time or all
    // at once: the backend is a thread's own and setting it twice
    // panics.
    thread_local! {
        static BACKEND: () = i_slint_backend_testing::init_no_event_loop();
    }
    BACKEND.with(|()| ());
    let app = App::new().expect("the window builds");
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
    app.on_grid_max_scroll(grid::max_scroll);
    app.on_grid_reveal_to(grid::reveal);
    app.on_grid_tiles_top(grid::tiles_top);
    app.on_lightbox_cell(crate::lightbox::cell);
    app.on_bar_length(grid::bar_length);
    app.on_bar_offset(grid::bar_offset);
    app.on_bar_drag(grid::bar_drag);
    app.show().expect("the window shows");
    app.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    app
}

/// A press and a release at a point of the window, logical pixels
/// from its top left: what a click on whatever is there does.
pub(crate) fn click(app: &App, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let button = slint::platform::PointerEventButton::Left;
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
    let at = |p: (f32, f32)| slint::LogicalPosition::new(p.0, p.1);
    let button = slint::platform::PointerEventButton::Left;
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
pub(crate) fn state_for(
    app: &App,
    files: Vec<std::path::PathBuf>,
) -> (Rc<RefCell<State>>, Rc<Worker>) {
    let state = Rc::new(RefCell::new(State::empty(files, app)));
    app.set_patch_handles(ModelRc::from(state.borrow().develop.patch_handles.clone()));
    let worker = Rc::new(Worker::new(|_| {}));
    install_callbacks(app, state.clone(), worker.clone());
    (state, worker)
}

/// A file-less state and a no-op worker: enough to drive the retouch
/// callbacks without a picture open.
pub(crate) fn retouch_state(app: &App) -> (Rc<RefCell<State>>, Rc<Worker>) {
    state_for(app, Vec::new())
}

/// A folder under the system's temporary directory no test has used,
/// named for `what`, made, and in its canonical form: on macOS the
/// temporary directory is a symlink, and the index keys folders by
/// their canonical path, so a test that lists a view from the index
/// would find nothing under the spelling `temp_dir` gives. A test
/// removes it with [`remove_dir_retry`].
pub(crate) fn scratch_dir(what: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "greycard-{what}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dunce::canonicalize(&dir).unwrap()
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
    use i_slint_backend_testing::ElementHandle;
    let find = || {
        // A control a property has just brought in is built on the
        // next event: a pointer move at the corner, where nothing is.
        app.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(0.0, 0.0),
        });
        ElementHandle::find_by_accessible_label(app, label).next()
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
            let element = find().unwrap_or_else(|| panic!("{label:?} scrolled away"));
            return (element.absolute_position(), element.size());
        }
        // Less than the view's height, so nothing falls between steps.
        scroll_to(app.get_panel_scroll() - ((bottom - top) * 0.8).max(40.0));
    }
    panic!("nothing labeled {label:?}");
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

/// Where row `i` of the strip is, in a 1500 by 950 window: the strip
/// is the window's foot, 148 high, its cells 178 wide and 186 apart
/// after a pad of 12.
pub(crate) fn strip_cell(i: usize) -> (f32, f32) {
    (12.0 + i as f32 * 186.0 + 89.0, 950.0 - 148.0 + 60.0)
}

/// Type `text` a key at a time into whatever has the focus.
pub(crate) fn type_text(app: &App, text: &str) {
    for c in text.chars() {
        press(app, c.to_string());
    }
}

/// A folder renamed away, as a root unplugged: tried again for a
/// while when the system refuses, since on Windows a folder cannot be
/// renamed while any file under it is open, and the thumbnail pool may
/// still be reading one for its thumbnail or its preview when the test
/// gets here. The last refusal is the panic.
pub(crate) fn rename_away(from: &std::path::Path, to: &std::path::Path) {
    let start = std::time::Instant::now();
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return,
            Err(_) if start.elapsed() < std::time::Duration::from_secs(20) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => panic!("{} could not be renamed away: {e}", from.display()),
        }
    }
}

/// A test's scratch folder removed once the state has let go of what
/// it holds open under it: the index's reader is closed first, since
/// the window's callbacks keep their own handle on the state and a
/// `drop` in the test does not end it, and Windows refuses to remove
/// an open file. The removal is tried again for a while, as
/// [`rename_away`] is, for the pool's reads still in flight.
pub(crate) fn remove_scratch(state: Rc<RefCell<State>>, dir: &std::path::Path) {
    state.borrow_mut().views.index_reader = None;
    drop(state);
    remove_dir_retry(dir);
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

/// A linear DNG of our own writing from a body of `make` and `model`, a
/// textured scene of `width` by `height` with a ramp across it: a raw
/// the engine opens and names by its body, which no sample raw in the
/// repository may stand in for.
pub(crate) fn linear_dng(
    path: &std::path::Path,
    width: usize,
    height: usize,
    make: &str,
    model: &str,
) {
    use greycard_core::raw::{
        Calibration, CfaPattern, LevelPattern, Levels, Orientation, Samples, SensorLayout,
    };
    let m = greycard_core::color::WORKING_SPACE
        .from_xyz_matrix()
        .expect("the working space has a matrix");
    let frame = greycard_core::RawFrame {
        make: make.into(),
        model: model.into(),
        width,
        height,
        channels: 1,
        layout: SensorLayout::Cfa(CfaPattern::rggb()),
        samples: Samples::U16(Vec::new()),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 1),
            white: LevelPattern::uniform(65535.0, 1),
        },
        as_shot_coefficients: Some([2.0, 1.0, 1.6]),
        calibrations: vec![Calibration {
            illuminant: 21,
            color_matrix: m.rows.iter().flatten().map(|v| *v as f32).collect(),
            forward_matrix: None,
        }],
        crop: None,
        orientation: Orientation::Normal,
        shot: Default::default(),
    };
    let mut data = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32, y as f32);
            let v =
                0.5 + 0.2 * (fx * 0.13).sin() * (fy * 0.09).cos() + 0.1 * ((fx + fy) * 0.031).sin();
            let ramp = 0.02 + 0.6 * fx / width as f32;
            data.extend_from_slice(&[ramp * v * 0.5, ramp * v, ramp * v * 0.6]);
        }
    }
    let image = greycard_core::CameraImage::from_data(width, height, data).unwrap();
    let mut file = std::fs::File::create(path).unwrap();
    greycard_core::dng::write_linear_dng(
        &mut file,
        &frame,
        &image,
        [2.0, 1.0, 1.6],
        &Default::default(),
    )
    .unwrap();
}
