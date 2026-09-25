//! The frame menu: a right-click over a frame in the strip, the grid
//! or the viewport (`ui/panel/frame-menu.slint`).
//!
//! Which frames it is about is `selection::right_click`'s: a frame in
//! the selection keeps the selection, one outside it is opened first
//! and becomes the selection, as a file manager does. Every item is
//! the code a key already runs: the culling keys' `set_meta` over the
//! selection, Ctrl+C and Ctrl+V's copy and paste (`panel::sync`),
//! Ctrl+Shift+E's export sheet. Only Reveal is new, and it is one
//! command per platform, chosen when the editor is compiled.

use crate::panel::browser::{chosen_frames, file_name, meta_on_selection};
use crate::*;
use greycard_edit::meta::{Change, Flag, Label};

/// What a menu item asks for, as a culling key would: a rating of
/// `value` stars, the flag whose code is `value`, the label whose
/// code is `value` (a toggle over the selection, as the key is; the
/// menu ticks the one the selection wears, so a tick taken off is
/// what it looks like).
pub(crate) fn menu_change(kind: &str, value: i32) -> Option<Change> {
    match kind {
        "rating" => u8::try_from(value)
            .ok()
            .filter(|&n| n <= meta::STARS)
            .map(Change::Rating),
        "flag" => Flag::ALL
            .into_iter()
            .find(|f| f.code() == value)
            .map(Change::Flag),
        "label" => Label::ALL
            .into_iter()
            .find(|l| l.code() == value)
            .map(Change::Label),
        _ => None,
    }
}

/// The value every one of `values` has, or -1 when they differ (or
/// there are none): what the menu ticks.
fn shared(values: impl IntoIterator<Item = i32>) -> i32 {
    let mut values = values.into_iter();
    let Some(first) = values.next() else {
        return -1;
    };
    if values.all(|v| v == first) {
        first
    } else {
        -1
    }
}

/// The command that shows `path` in the desktop's file manager: the
/// Finder with the file chosen on a Mac, Explorer with it chosen on
/// Windows, and elsewhere the folder through `xdg-open`, which has no
/// way to choose a file in it.
#[cfg(target_os = "macos")]
pub(crate) fn reveal_command(path: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("open");
    cmd.arg("-R").arg(path);
    cmd
}

#[cfg(windows)]
pub(crate) fn reveal_command(path: &Path) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    // Explorer reads "/select," and the path as one argument and does
    // not take the quoting Rust would put around the whole of it.
    let mut cmd = std::process::Command::new("explorer");
    cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    cmd
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn reveal_command(path: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path.parent().unwrap_or(Path::new(".")));
    cmd
}

/// Show `path` in the file manager, without waiting on it.
fn reveal(path: &Path) -> std::io::Result<()> {
    let mut child = reveal_command(path).spawn()?;
    // Reaped on a thread of its own, so it is not left a zombie and
    // the window never waits on the file manager.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Settle what a right-click on browser row `row` (-1: the frame on
/// screen, from the viewport) is about, and fill the menu's ticks
/// and counts for it.
pub(crate) fn menu_asked(state: &Rc<RefCell<State>>, app: &App, row: i32) {
    let file = {
        let st = state.borrow();
        match usize::try_from(row) {
            Ok(r) => st.shown.get(r).copied(),
            Err(_) => st.current,
        }
    };
    let Some(file) = file else {
        return;
    };
    let click = {
        let st = state.borrow();
        selection::right_click(&st.picked, st.current, file)
    };
    if let selection::Click::Open(_) = click {
        // Opened, and the set collapsed to it, as a plain click does.
        app.invoke_select(row);
    }
    let mut st = state.borrow_mut();
    st.menu_file = Some(file);
    let frames = chosen_frames(&st);
    let metas: Vec<&Meta> = frames
        .iter()
        .filter_map(|&f| st.sidecars.get(f))
        .map(|s| &s.meta)
        .collect();
    app.set_menu_count(frames.len().max(1) as i32);
    app.set_menu_rating(shared(metas.iter().map(|m| i32::from(m.rating))));
    app.set_menu_flag(shared(metas.iter().map(|m| m.flag.code())));
    app.set_menu_label(shared(metas.iter().map(|m| m.label.code())));
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
    // A right-click is a Control+click on a Mac, and a Mac's Control
    // is what Slint calls `meta`.
    app.set_ctrl_click_menu(cfg!(target_os = "macos"));
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_frame_menu_asked(move |row| {
            if let Some(app) = app_weak.upgrade() {
                menu_asked(&state, &app, row);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_menu_meta(move |kind, value| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if let Some(change) = menu_change(&kind, value) {
                meta_on_selection(&state, &app, change);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_menu_reveal(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let st = state.borrow();
            let Some(path) = st.menu_file.or(st.current).and_then(|f| st.files.get(f)) else {
                return;
            };
            if let Err(e) = reveal(path) {
                tracing::warn!("reveal {}: {e}", path.display());
                app.set_status(format!("could not show {}: {e}", file_name(path)).into());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{folder, state_for, window};
    use slint::platform::{PointerEventButton, WindowEvent};

    #[test]
    fn a_menu_item_is_the_change_its_key_makes() {
        for key in ["0", "3", "5", "p", "x", "u", "6", "7", "8", "9"] {
            let want = Change::from_key(key).expect("a key");
            let got = match want {
                Change::Rating(n) => menu_change("rating", i32::from(n)),
                Change::Flag(f) => menu_change("flag", f.code()),
                Change::Label(l) => menu_change("label", l.code()),
            };
            assert_eq!(got, Some(want), "{key}");
        }
        // The menu reaches what no key does: purple, and no label.
        assert_eq!(menu_change("label", 5), Some(Change::Label(Label::Purple)));
        assert_eq!(menu_change("label", 0), Some(Change::Label(Label::None)));
        assert_eq!(menu_change("rating", 6), None);
        assert_eq!(menu_change("rating", -1), None);
        assert_eq!(menu_change("flag", 3), None);
        assert_eq!(menu_change("turn", 1), None);
    }

    #[test]
    fn the_ticks_are_what_the_whole_selection_shares() {
        assert_eq!(shared([3, 3, 3]), 3);
        assert_eq!(shared([3, 2]), -1);
        assert_eq!(shared([0]), 0);
        assert_eq!(shared([]), -1);
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn reveal_opens_the_folder() {
        let cmd = reveal_command(Path::new("/shoot/day one/IMG_0001.CR3"));
        assert_eq!(cmd.get_program(), "xdg-open");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, [std::ffi::OsStr::new("/shoot/day one")]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reveal_chooses_the_file_in_the_finder() {
        let cmd = reveal_command(Path::new("/shoot/day one/IMG_0001.CR3"));
        assert_eq!(cmd.get_program(), "open");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            [
                std::ffi::OsStr::new("-R"),
                std::ffi::OsStr::new("/shoot/day one/IMG_0001.CR3")
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn reveal_chooses_the_file_in_explorer() {
        let cmd = reveal_command(Path::new(r"C:\shoot\IMG_0001.CR3"));
        assert_eq!(cmd.get_program(), "explorer");
    }

    /// A right-click at a point of the window.
    fn right_click(app: &App, x: f32, y: f32) {
        let position = slint::LogicalPosition::new(x, y);
        let button = PointerEventButton::Right;
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed { position, button },
            WindowEvent::PointerReleased { position, button },
        ] {
            app.window().dispatch_event(event);
        }
        // Whatever the menu put up, gone: the next event is the
        // test's own.
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: slint::platform::Key::Escape.into(),
        });
    }

    /// Row `i` of the strip, in a 1500 by 950 window.
    fn strip_cell(i: usize) -> (f32, f32) {
        (12.0 + i as f32 * 186.0 + 89.0, 950.0 - 148.0 + 60.0)
    }

    #[test]
    fn a_right_click_outside_the_set_opens_the_frame_and_inside_keeps_the_set() {
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(6));
        app.invoke_select(1);
        {
            let mut st = state.borrow_mut();
            st.picked = vec![1, 3];
            st.sidecars[1].meta.rating = 4;
            st.sidecars[3].meta.rating = 4;
            st.sidecars[3].meta.label = Label::Red;
        }
        // Inside the set, on a frame that is not the current one: the
        // set stands, the frame on screen with it, and the menu is
        // about both.
        let (x, y) = strip_cell(3);
        right_click(&app, x, y);
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 3]);
        assert_eq!(state.borrow().menu_file, Some(3));
        assert_eq!(app.get_menu_count(), 2);
        assert_eq!(app.get_menu_rating(), 4);
        assert_eq!(app.get_menu_flag(), 0);
        assert_eq!(app.get_menu_label(), -1);
        // A rating from the menu reaches the whole set, as the key.
        app.invoke_menu_meta("rating".into(), 2);
        let ratings: Vec<u8> = state
            .borrow()
            .sidecars
            .iter()
            .map(|s| s.meta.rating)
            .collect();
        assert_eq!(ratings, [0, 2, 0, 2, 0, 0]);
        // Outside it: that frame opened, alone.
        let (x, y) = strip_cell(4);
        right_click(&app, x, y);
        assert_eq!(state.borrow().current, Some(4));
        assert_eq!(chosen_frames(&state.borrow()), vec![4]);
        assert_eq!(app.get_menu_count(), 1);
        assert_eq!(app.get_menu_rating(), 0);
        app.invoke_menu_meta("flag".into(), 1);
        app.invoke_menu_meta("label".into(), 5);
        let st = state.borrow();
        assert_eq!(st.sidecars[4].meta.flag, Flag::Pick);
        assert_eq!(st.sidecars[4].meta.label, Label::Purple);
        assert_eq!(st.sidecars[3].meta.flag, Flag::None);
    }

    #[test]
    fn the_viewport_menu_is_the_frame_on_screen_and_its_set() {
        let app = window(4);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(4));
        app.invoke_select(2);
        state.borrow_mut().picked = vec![0, 2];
        app.invoke_frame_menu_asked(-1);
        assert_eq!(state.borrow().current, Some(2));
        assert_eq!(chosen_frames(&state.borrow()), vec![0, 2]);
        assert_eq!(state.borrow().menu_file, Some(2));
        assert_eq!(app.get_menu_count(), 2);
    }
}
