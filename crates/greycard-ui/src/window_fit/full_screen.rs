//! Full screen, from F11 or the zoom menu, and back to the window as
//! it was.
//!
//! Slint hands the toggle to winit, which on leaving puts the window
//! back where and how large it was: on Windows the placement saved on
//! entering, which holds the size and whether it was maximized; on X11
//! and Wayland the window manager's own memory of it. Slint holds the
//! layout's minimum off the window while it is full screen and sets it
//! again on leaving, so a minimum taller than the screen would push the
//! restored window past the screen's foot; the side pane scrolls rather
//! than lift it (`ScrollPane` in greycard-slint's `ui/app.slint`).
//!
//! What Slint does not keep is the maximized state: it follows what
//! winit reports, and Windows reports a full-screen window as not
//! maximized. So the state before is kept here and put back on
//! leaving, in case the platform's restore does not; maximizing a
//! window the restore maximized already is nothing. A window that
//! left full screen by the platform's own (a window manager's key)
//! forgets it, so a later entry by the platform is not taken for ours.

use std::cell::Cell;

use slint::ComponentHandle;

use crate::App;

thread_local! {
    /// Whether the window was maximized when we took it full screen;
    /// None when we did not, as when a Mac's green button did, or
    /// once it has left full screen since.
    static BEFORE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Wire the menu's item and F11 (the window's key handler) to the
/// toggle.
pub(crate) fn install(app: &App) {
    BEFORE.with(|b| b.set(None));
    let app_weak = app.as_weak();
    app.on_full_screen_toggled(move || {
        if let Some(app) = app_weak.upgrade() {
            toggle(app.window());
        }
    });
}

/// The window resized, full screen or not as winit has it: out of
/// full screen, whatever we kept from our entry is spent.
pub(crate) fn resized(full: bool) {
    if !full {
        BEFORE.with(|b| b.set(None));
    }
}

/// Full screen, or back out of it to the window as it was.
fn toggle(window: &slint::Window) {
    if window.is_fullscreen() {
        window.set_fullscreen(false);
        let maximized = BEFORE.with(Cell::take) == Some(true);
        if maximized && !window.is_maximized() {
            window.set_maximized(true);
        }
        tracing::info!(
            "window: full screen off{}",
            if maximized { ", maximized" } else { "" }
        );
    } else {
        let (size, maximized) = (window.size(), window.is_maximized());
        BEFORE.with(|b| b.set(Some(maximized)));
        window.set_fullscreen(true);
        tracing::info!(
            "window: full screen, from {}x{}{}",
            size.width,
            size.height,
            if maximized { " maximized" } else { "" }
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::App;
    use crate::testing::{click, labeled, press, type_text};
    use slint::ComponentHandle;
    use slint::platform::{Key, WindowEvent};

    fn develop_window() -> (
        App,
        std::rc::Rc<std::cell::RefCell<crate::State>>,
        std::rc::Rc<crate::worker::Worker>,
    ) {
        let app = crate::testing::window(3);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, worker) = crate::testing::state_for(&app, crate::testing::folder(3));
        app.invoke_select(0);
        (app, state, worker)
    }

    fn full(app: &App) -> bool {
        app.window().is_fullscreen()
    }

    /// F11 takes the window full screen and back in the grid, the
    /// lightbox, the loupe, Develop, Cull, and the crop.
    #[test]
    fn f11_goes_full_screen_and_back_in_every_view() {
        let (app, _state, _worker) = develop_window();
        type InView = dyn Fn(&App) -> bool;
        let views: [(&str, &InView); 6] = [
            ("g", &|a| a.get_grid_open()),
            ("l", &|a| a.get_lightbox()),
            ("e", &|a| !a.get_grid_open() && !a.get_lightbox()),
            ("c", &|a| a.get_cull_mode()),
            ("d", &|a| !a.get_cull_mode() && !a.get_grid_open()),
            ("r", &|a| a.get_crop_mode()),
        ];
        for (key, is_in) in views {
            press(&app, key);
            assert!(is_in(&app), "{key} took the view");
            press(&app, Key::F11);
            assert!(full(&app), "{key}: full screen");
            assert!(is_in(&app), "{key}: the view kept");
            press(&app, Key::F11);
            assert!(!full(&app), "{key}: back");
            if key == "l" {
                press(&app, "l");
            }
        }
    }

    /// A maximized window comes back maximized, though the platform
    /// reported it not while full screen, as Windows does; one that
    /// was not stays as it was.
    #[test]
    fn leaving_full_screen_puts_back_a_maximized_window() {
        let (app, _state, _worker) = develop_window();
        press(&app, Key::F11);
        press(&app, Key::F11);
        assert!(!app.window().is_maximized());

        app.window().set_maximized(true);
        press(&app, Key::F11);
        assert!(full(&app));
        app.window().set_maximized(false);
        press(&app, Key::F11);
        assert!(!full(&app));
        assert!(app.window().is_maximized(), "maximized again");

        // Full screen by the platform's own (a Mac's green button):
        // out again by F11 or the menu, nothing kept to put back.
        app.window().set_maximized(false);
        app.window().set_fullscreen(true);
        app.invoke_full_screen_toggled();
        assert!(!full(&app));
        assert!(!app.window().is_maximized());
    }

    /// Maximized while full screen, from a window that was not: left
    /// as Slint has it, maximized, for the platform's restore to
    /// settle; we unmaximize nothing.
    #[test]
    fn a_window_maximized_while_full_screen_is_left_so() {
        let (app, _state, _worker) = develop_window();
        press(&app, Key::F11);
        app.window().set_maximized(true);
        press(&app, Key::F11);
        assert!(!full(&app));
        assert!(app.window().is_maximized());
    }

    /// In by the menu while maximized, out by the platform's own,
    /// unmaximized, in by the platform again and out by F11: not
    /// maximized, since what we kept went with the platform's leaving.
    #[test]
    fn a_platforms_leaving_forgets_what_was_kept() {
        let (app, _state, _worker) = develop_window();
        app.window().set_maximized(true);
        app.invoke_full_screen_toggled();
        assert!(full(&app));
        // A window manager's key, as winit's Resized reports it.
        app.window().set_fullscreen(false);
        super::resized(false);
        app.window().set_maximized(false);
        app.window().set_fullscreen(true);
        super::resized(true);
        press(&app, Key::F11);
        assert!(!full(&app));
        assert!(!app.window().is_maximized(), "not maximized again");
    }

    /// Held, F11 is one toggle; under a sheet it does nothing, as the
    /// panes' keys do not.
    #[test]
    fn a_held_f11_toggles_once_and_a_sheet_keeps_it() {
        let (app, _state, _worker) = develop_window();
        app.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::F11.into(),
        });
        for _ in 0..3 {
            app.window().dispatch_event(WindowEvent::KeyPressRepeated {
                text: Key::F11.into(),
            });
        }
        app.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::F11.into(),
        });
        assert!(full(&app));
        press(&app, Key::F11);
        assert!(!full(&app));

        press(&app, "?");
        assert!(app.get_keys_open());
        let hidden = app.get_right_hidden();
        press(&app, Key::F8);
        press(&app, Key::F11);
        assert_eq!(app.get_right_hidden(), hidden, "F8 under the sheet");
        assert!(!full(&app), "F11 under the sheet");
    }

    /// Typing in a field, the field keeps F11 as it keeps F8, and
    /// types neither; Escape gives the keys back.
    #[test]
    fn f11_in_a_field_is_as_the_panes_keys_are() {
        let (app, _state, _worker) = develop_window();
        let (at, size) = labeled(&app, "Frame title");
        click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
        type_text(&app, "Du");
        assert!(!app.get_right_hidden());
        press(&app, Key::F8);
        assert!(!app.get_right_hidden(), "the field keeps F8");
        press(&app, Key::F11);
        assert!(!full(&app), "the field keeps F11");
        assert_eq!(app.get_title_text(), "Du", "nothing typed");
        press(&app, Key::Escape);
        press(&app, Key::F11);
        assert!(full(&app), "the window's again");
    }
}
