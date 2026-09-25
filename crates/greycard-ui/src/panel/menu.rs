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
/// menu checks the one the selection wears, so a check taken off is
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
/// there are none): what the menu checks.
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

/// Explorer's argument for `path` chosen in its folder: "/select,"
/// and the path, quoted, as one argument. Explorer does not take the
/// quoting Rust would put around the whole of it, so the command is
/// given it raw. Pure, and tested on every platform.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn explorer_select(path: &Path) -> String {
    format!("/select,\"{}\"", path.display())
}

/// `path` made absolute against the working directory: a file opened
/// by a relative name has an empty folder, and the file manager would
/// be handed nothing.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The command that shows `path` in the desktop's file
/// manager: the Finder with the file chosen on a Mac, Explorer with it
/// chosen on Windows, and elsewhere the folder through `xdg-open`,
/// which has no way to choose a file in it.
#[cfg(target_os = "macos")]
pub(crate) fn reveal_command(path: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("open");
    cmd.arg("-R").arg(absolute(path));
    cmd
}

#[cfg(windows)]
pub(crate) fn reveal_command(path: &Path) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("explorer");
    cmd.raw_arg(explorer_select(&absolute(path)));
    cmd
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn reveal_command(path: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("xdg-open");
    let path = absolute(path);
    cmd.arg(path.parent().unwrap_or(Path::new("/")));
    cmd
}

/// Show `path` in the file manager, without waiting on it.
fn reveal(path: &Path) -> std::io::Result<()> {
    let path = path.to_path_buf();
    let mut child = reveal_command(&path).spawn()?;
    // Reaped on a thread of its own, so it is not left a zombie and
    // the window never waits on the file manager; a failure is logged.
    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => {
            tracing::warn!("reveal {}: the file manager said {status}", path.display());
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("reveal {}: {e}", path.display()),
    });
    Ok(())
}

/// Settle what a right-click on browser row `row` (-1: the frame on
/// screen, from the viewport) is about, and fill the menu's checks
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
    app.set_menu_file(
        st.files
            .get(file)
            .map(|p| file_name(p))
            .unwrap_or_default()
            .into(),
    );
    app.set_menu_can_meta(!frames.is_empty());
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
    // The menu's Copy: the frame right-clicked, which is not always
    // the frame on screen (another frame of the set).
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_menu_copy(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            match st.menu_file.or(st.current) {
                Some(f) => crate::panel::sync::copy_settings_of(&mut st, &app, f),
                None => crate::panel::sync::copy_settings(&mut st, &app),
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
                tracing::warn!(
                    "reveal {}: could not start the file manager: {e}",
                    path.display()
                );
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
    fn the_checks_are_what_the_whole_selection_shares() {
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

    #[test]
    fn explorers_argument_is_one_quoted_select() {
        assert_eq!(
            explorer_select(Path::new("/shoot/day one/IMG_0001.CR3")),
            "/select,\"/shoot/day one/IMG_0001.CR3\""
        );
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn reveal_of_a_relative_name_opens_the_working_folder() {
        let cmd = reveal_command(Path::new("5M0A3021.CR3"));
        let args: Vec<_> = cmd.get_args().collect();
        let here = std::env::current_dir().unwrap();
        assert_eq!(args, [here.as_os_str()]);
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
    fn a_macs_control_click_is_a_right_click_and_not_a_click() {
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(6));
        // What `install` sets on a Mac, set here on any platform.
        app.set_ctrl_click_menu(true);
        app.invoke_select(1);
        state.borrow_mut().picked = vec![1, 3];
        // A Mac's Control is Slint's `meta`.
        let meta = slint::platform::Key::Meta;
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: meta.into() });
        let (x, y) = strip_cell(3);
        crate::testing::click(&app, x, y);
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text: meta.into() });
        // The menu, over the set: a plain click would have opened
        // frame 3 and collapsed the set to it.
        assert_eq!(state.borrow().menu_file, Some(3));
        assert_eq!(state.borrow().current, Some(1));
        assert_eq!(chosen_frames(&state.borrow()), vec![1, 3]);
        // Off (Linux, Windows), the same is a plain click.
        crate::testing::press(&app, slint::platform::Key::Escape);
        app.set_ctrl_click_menu(false);
        app.window()
            .dispatch_event(WindowEvent::KeyPressed { text: meta.into() });
        crate::testing::click(&app, x, y);
        app.window()
            .dispatch_event(WindowEvent::KeyReleased { text: meta.into() });
        assert_eq!(state.borrow().current, Some(3));
        assert_eq!(chosen_frames(&state.borrow()), vec![3]);
    }

    #[test]
    fn the_menus_copy_takes_the_frame_right_clicked_and_ctrl_c_the_one_on_screen() {
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let files = folder(6);
        let (state, _worker) = state_for(&app, files.clone());
        {
            let mut st = state.borrow_mut();
            let mut edit = Edit::default();
            edit.light.exposure = 0.9;
            st.sidecars[3].record(edit);
        }
        app.invoke_select(1);
        state.borrow_mut().picked = vec![1, 3];
        let (x, y) = strip_cell(3);
        right_click(&app, x, y);
        assert_eq!(app.get_menu_file(), "IMG_0003.CR3");
        app.invoke_menu_copy();
        {
            let st = state.borrow();
            let clip = st.clipboard.as_ref().expect("a copy");
            assert_eq!(clip.from, files[3]);
            assert_eq!(clip.edit.light.exposure, 0.9);
        }
        // On the frame on screen, the panel's edit, recorded or not.
        app.set_exposure(-0.4);
        let (x, y) = strip_cell(1);
        right_click(&app, x, y);
        app.invoke_menu_copy();
        assert_eq!(
            state
                .borrow()
                .clipboard
                .as_ref()
                .map(|c| c.edit.light.exposure),
            Some(-0.4)
        );
        // Ctrl+C, after a right-click on another frame: the frame on
        // screen still.
        right_click(&app, strip_cell(3).0, strip_cell(3).1);
        app.invoke_copy_asked();
        assert_eq!(
            state.borrow().clipboard.as_ref().map(|c| c.from.clone()),
            Some(files[1].clone())
        );
    }

    #[test]
    fn the_press_that_closes_the_menu_is_not_a_click() {
        let app = window(6);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = state_for(&app, folder(6));
        app.invoke_select(1);
        // The menu opened over frame 1, and left open.
        let (x, y) = strip_cell(1);
        let position = slint::LogicalPosition::new(x, y);
        for event in [
            WindowEvent::PointerMoved { position },
            WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Right,
            },
            WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Right,
            },
        ] {
            app.window().dispatch_event(event);
        }
        assert!(app.get_menu_up(), "the menu took the keys' focus");
        // A left click on frame 4 closes it and opens nothing.
        let (x, y) = strip_cell(4);
        crate::testing::click(&app, x, y);
        assert_eq!(state.borrow().current, Some(1));
        assert!(!app.get_menu_up(), "and gave it back as it closed");
        // The next one is a click again.
        crate::testing::click(&app, x, y);
        assert_eq!(state.borrow().current, Some(4));
        // Closed by Escape instead: the next click is a click.
        let (x, y) = strip_cell(2);
        let position = slint::LogicalPosition::new(x, y);
        app.window().dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Right,
        });
        app.window().dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Right,
        });
        assert!(app.get_menu_up());
        assert_eq!(state.borrow().current, Some(2));
        crate::testing::press(&app, slint::platform::Key::Escape);
        assert!(!app.get_menu_up(), "Escape closed it");
        let (x, y) = strip_cell(5);
        crate::testing::click(&app, x, y);
        assert_eq!(state.borrow().current, Some(5));
    }

    #[test]
    fn the_press_that_closes_the_menu_over_the_picture_does_not_zoom() {
        let app = window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (_state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        let zooms = Rc::new(RefCell::new(0));
        let counted = zooms.clone();
        app.on_toggle_zoom(move |_, _| *counted.borrow_mut() += 1);
        // The middle of the picture, clear of the panels and the strip.
        let position = slint::LogicalPosition::new(700.0, 400.0);
        crate::testing::click(&app, 700.0, 400.0);
        assert_eq!(*zooms.borrow(), 1, "a plain click zooms");
        for event in [
            WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Right,
            },
            WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Right,
            },
        ] {
            app.window().dispatch_event(event);
        }
        assert!(app.get_menu_up());
        crate::testing::click(&app, 720.0, 420.0);
        assert!(!app.get_menu_up());
        assert_eq!(*zooms.borrow(), 1, "the closing click did not");
        crate::testing::click(&app, 720.0, 420.0);
        assert_eq!(*zooms.borrow(), 2);
    }

    #[test]
    fn the_menus_export_waits_for_the_develop() {
        let app = window(3);
        let (_state, _worker) = state_for(&app, folder(3));
        app.invoke_select(0);
        app.set_busy(true);
        slint::platform::update_timers_and_animations();
        app.invoke_menu_export();
        assert!(!app.get_export_open());
        assert!(app.get_export_waiting());
        app.set_busy(false);
        slint::platform::update_timers_and_animations();
        assert!(
            app.get_export_open(),
            "the develop landed and the sheet opened"
        );
        assert!(!app.get_export_waiting());
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
