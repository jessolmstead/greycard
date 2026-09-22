use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::WindowEvent;
use slint::{ComponentHandle, ModelRc, VecModel};

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
    app.set_thumbs(ModelRc::new(VecModel::from(thumbs)));
    app.on_grid_columns(grid::columns);
    app.on_grid_slack(grid::slack);
    app.on_grid_max_scroll(grid::max_scroll);
    app.on_grid_reveal_to(grid::reveal);
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

/// A folder of `count` frames nothing has ever written, named as a
/// camera names them, in a directory that does not exist: the
/// browser reads the sidecars it is given and never the disk.
pub(crate) fn folder(count: usize) -> Vec<std::path::PathBuf> {
    (0..count)
        .map(|i| std::path::PathBuf::from("/nowhere").join(format!("IMG_{i:04}.CR3")))
        .collect()
}
