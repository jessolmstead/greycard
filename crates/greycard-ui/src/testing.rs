use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::WindowEvent;
use slint::{ComponentHandle, ModelRc};

use crate::worker::Worker;
use crate::{App, State, Thumb, grid, install_callbacks};

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
    app.on_grid_columns(grid::columns);
    app.on_grid_slack(grid::slack);
    app.on_grid_max_scroll(grid::max_scroll);
    app.on_grid_reveal_to(grid::reveal);
    app.on_grid_tiles_top(grid::tiles_top);
    app.on_grid_bar_length(grid::bar_length);
    app.on_grid_bar_offset(grid::bar_offset);
    app.on_grid_bar_drag(grid::bar_drag);
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
    app.set_patch_handles(ModelRc::from(state.borrow().patch_handles.clone()));
    let worker = Rc::new(Worker::new(|_| {}));
    install_callbacks(app, state.clone(), worker.clone());
    (state, worker)
}

/// A file-less state and a no-op worker: enough to drive the retouch
/// callbacks without a picture open.
pub(crate) fn retouch_state(app: &App) -> (Rc<RefCell<State>>, Rc<Worker>) {
    state_for(app, Vec::new())
}

/// A path under the system's temporary directory no test has used,
/// named for `what`, not yet made; a test makes it and removes it with
/// [`remove_dir_retry`].
pub(crate) fn scratch_dir(what: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "greycard-{what}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
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
    let height = app
        .window()
        .size()
        .to_logical(app.window().scale_factor())
        .height;
    // The panel's view ends above its footer (Fit and Export) and the
    // strip, not at the window's bottom: a control under the footer is
    // found and drawn over, and a click there lands on the footer. How
    // far down the footer sits depends on the system's font, so it is
    // looked up rather than assumed: the lowest "Fit", since the zoom's
    // words say "Fit" too, higher up.
    let bottom = ElementHandle::find_by_accessible_label(app, "Fit")
        .map(|e| e.absolute_position().y)
        .reduce(f32::max)
        .unwrap_or(height)
        - 40.0;
    // The panel's scroll only builds what is in view: down it a
    // screen at a time from the top until the control is there, then
    // to where it is well inside the view.
    app.set_panel_scroll(0.0);
    for _ in 0..40 {
        if let Some(element) = find() {
            let at = element.absolute_position();
            if at.y > 80.0 && at.y + element.size().height < bottom {
                return (at, element.size());
            }
            app.set_panel_scroll(app.get_panel_scroll() - (at.y - (80.0 + bottom) / 2.0));
            let element = find().unwrap_or_else(|| panic!("{label:?} scrolled away"));
            return (element.absolute_position(), element.size());
        }
        app.set_panel_scroll(app.get_panel_scroll() - height / 3.0);
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
    state.borrow_mut().index_reader = None;
    drop(state);
    remove_dir_retry(dir);
}

/// A test's folder removed, tried again for a while when the system
/// refuses: on Windows a file a thread has just let go of can still be
/// held for a moment after the thread was joined. The last refusal is
/// the panic.
pub(crate) fn remove_dir_retry(dir: &std::path::Path) {
    let start = std::time::Instant::now();
    loop {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) if start.elapsed() < std::time::Duration::from_secs(20) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => panic!("{} could not be removed: {e}", dir.display()),
        }
    }
}

/// A camera profile with a warm and a daylight calibration, the
/// matrices of an ordinary sensor (a little wider than Rec.2020 in its
/// reds, a little narrower in its blues), for the masks' white
/// balances in tests that have no raw: the profile and a develop at
/// 5500 K, what [`greycard_edit::WhiteShift`] is made from.
pub(crate) fn white_shift() -> greycard_edit::WhiteShift {
    use greycard_core::raw::{
        Calibration, CfaPattern, LevelPattern, Levels, Samples, SensorLayout,
    };
    let calibration = |illuminant: u16, m: [f32; 9]| Calibration {
        illuminant,
        color_matrix: m.to_vec(),
        forward_matrix: None,
    };
    let frame = greycard_core::RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        width: 2,
        height: 2,
        channels: 1,
        layout: SensorLayout::Cfa(CfaPattern::rggb()),
        samples: Samples::U16(vec![0; 4]),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 1),
            white: LevelPattern::uniform(1.0, 1),
        },
        as_shot_coefficients: Some([2.0, 1.0, 1.6]),
        calibrations: vec![
            calibration(
                17,
                [
                    1.1564, -0.4626, -0.1081, -0.4229, 1.2260, 0.2187, -0.0711, 0.1500, 0.6268,
                ],
            ),
            calibration(
                21,
                [
                    0.9766, -0.2953, -0.1254, -0.4276, 1.2116, 0.2433, -0.0437, 0.1336, 0.5131,
                ],
            ),
        ],
        crop: None,
        orientation: Default::default(),
        shot: Default::default(),
    };
    let profile = greycard_core::color::profile_from_frame(&frame).expect("two calibrations");
    let base = greycard_core::color::white_balance_at(
        &profile,
        greycard_core::TempTint {
            cct: 5500.0,
            duv: 0.0,
        },
    )
    .expect("daylight resolves");
    greycard_edit::WhiteShift::new(profile, &base)
}
