//! The window's own winit events, heard before Slint routes them: the
//! mouse's side buttons and what goes with them (`side_buttons`), and
//! no frame drawn while the window is minimized.
//!
//! A minimized window shows nothing, but Slint draws whenever it is
//! asked to: a develop landing, a scope's bins, a slider's timer. On
//! Windows winit reports no occlusion, and Slint's winit backend passes
//! over the zero size a minimize resizes the window to, so the surface
//! keeps its full size. A frame then finds the swapchain out of date,
//! and femtovg's wgpu renderer configures it again at that size; when
//! the frame still fails it is skipped and the next asked for. Each
//! configure is a new swapchain on a window that has no size, and one
//! AMD Vulkan driver held on to their memory until the device ran out
//! (issue 195). So a `RedrawRequested` while winit says the window is
//! minimized goes no further: Slint neither draws nor configures. The
//! frame it wanted is asked for again as the window comes back.
//!
//! Nothing else waits on a frame: a develop is taken in as it is
//! delivered (its state, its pictures, its saves), and only goes up
//! onto the GPU at the start of the next frame drawn, the one the
//! window comes back with. Where winit cannot say (Wayland), every
//! frame is drawn as before; so is a frame Slint draws from macOS's
//! display link, which never passes through here.

use slint::ComponentHandle;
use slint::winit_030::EventResult;
use slint::winit_030::winit::event::WindowEvent;

use crate::App;
use crate::panel::side_buttons::SideButtons;

/// Whether a redraw is drawn: not while the window is minimized, and
/// always where the platform does not say (`None`, Wayland).
pub(crate) fn draws(minimized: Option<bool>) -> bool {
    minimized != Some(true)
}

/// What the hook does with one event, as far as drawing goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Frame {
    /// Slint has the event as ever.
    Go,
    /// A redraw while minimized: Slint does not have it.
    Hold,
    /// The first event since redraws were held, with the window shown
    /// again: Slint has it, and a frame is asked for.
    Restored { held: u64 },
}

/// The redraws held while the window is minimized.
#[derive(Debug, Default)]
pub(crate) struct Held {
    count: u64,
}

impl Held {
    /// One event: `redraw` whether it is a `RedrawRequested`, and
    /// `minimized` winit's word on the window, asked only for a redraw
    /// or while redraws are held (on X11 it is a round trip to the
    /// server, which every mouse move should not make).
    pub(crate) fn event(
        &mut self,
        redraw: bool,
        minimized: impl FnOnce() -> Option<bool>,
    ) -> Frame {
        if !redraw && self.count == 0 {
            return Frame::Go;
        }
        if !draws(minimized()) {
            if redraw {
                self.count += 1;
                return Frame::Hold;
            }
            return Frame::Go;
        }
        match std::mem::take(&mut self.count) {
            0 => Frame::Go,
            held => Frame::Restored { held },
        }
    }
}

/// winit's word on whether `window` is minimized: None on Wayland, and
/// on a window that has no winit window (the tests' backend).
pub(crate) fn minimized(window: &slint::Window) -> Option<bool> {
    use slint::winit_030::WinitWindowAccessor;
    window.with_winit_window(|w| w.is_minimized()).flatten()
}

/// The frame the window comes back with. Slint's own request goes
/// nowhere while it holds one as pending, which a held redraw leaves
/// it doing until its frame timer next looks, so winit is asked too.
fn ask_for_frame(window: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    window.request_redraw();
    window.with_winit_window(|w| w.request_redraw());
}

/// The window's events heard from now on: the side buttons, and its
/// frames held while it is minimized. Nothing on a backend other than
/// winit's.
pub(crate) fn install(app: &App) {
    use slint::winit_030::WinitWindowAccessor;
    let weak = app.as_weak();
    let mut buttons = SideButtons::default();
    let mut held = Held::default();
    app.window().on_winit_window_event(move |window, event| {
        let redraw = matches!(event, WindowEvent::RedrawRequested);
        let frame = held.event(redraw, || minimized(window));
        let result = weak
            .upgrade()
            .map_or(EventResult::Propagate, |app| buttons.event(&app, event));
        match frame {
            Frame::Go => result,
            Frame::Hold => {
                if held.count == 1 {
                    tracing::info!("window: minimized; its frames are held until it is shown");
                }
                EventResult::PreventDefault
            }
            Frame::Restored { held } => {
                tracing::info!("window: shown again, {held} redraw(s) held while minimized");
                // A redraw is drawn as it goes on.
                if !redraw {
                    ask_for_frame(window);
                }
                result
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Events as winit hands them, each with the window minimized or
    /// not as winit says; what the hook does with each, and how often
    /// it asked.
    fn run(events: &[(bool, Option<bool>)]) -> (Vec<Frame>, usize) {
        let mut held = Held::default();
        let mut asked = 0;
        let frames = events
            .iter()
            .map(|&(redraw, minimized)| {
                held.event(redraw, || {
                    asked += 1;
                    minimized
                })
            })
            .collect();
        (frames, asked)
    }

    #[test]
    fn a_redraw_is_drawn_unless_the_window_is_minimized() {
        assert!(draws(Some(false)));
        assert!(!draws(Some(true)));
        assert!(draws(None), "Wayland: as before");
    }

    /// Minimized: every redraw held, the other events Slint's; the
    /// first event shown again asks for the frame, once.
    #[test]
    fn redraws_are_held_while_minimized_and_asked_for_on_restore() {
        let (redraw, other) = (true, false);
        let (frames, _) = run(&[
            (redraw, Some(false)),
            (other, Some(true)),
            (redraw, Some(true)),
            (redraw, Some(true)),
            (other, Some(true)),
            (redraw, Some(true)),
            (other, Some(false)),
            (redraw, Some(false)),
            (other, Some(false)),
        ]);
        use Frame::*;
        assert_eq!(
            frames,
            [Go, Go, Hold, Hold, Go, Hold, Restored { held: 3 }, Go, Go]
        );
    }

    /// The window back with a redraw of its own (an X11 expose): that
    /// redraw is the frame.
    #[test]
    fn a_redraw_on_restore_is_drawn() {
        let (frames, _) = run(&[(true, Some(true)), (true, Some(false))]);
        assert_eq!(frames, [Frame::Hold, Frame::Restored { held: 1 }]);
    }

    /// Where winit cannot say, nothing is held.
    #[test]
    fn nothing_is_held_on_wayland() {
        let (frames, _) = run(&[(true, None), (false, None), (true, None)]);
        assert!(frames.iter().all(|&f| f == Frame::Go), "{frames:?}");
    }

    /// The window is asked of only for a redraw, or while redraws are
    /// held: not for every mouse move.
    #[test]
    fn the_window_is_asked_only_when_it_matters() {
        let (_, asked) = run(&[
            (false, Some(false)),
            (false, Some(false)),
            (true, Some(false)),
        ]);
        assert_eq!(asked, 1);
        let (_, asked) = run(&[
            (true, Some(true)),
            (false, Some(true)),
            (false, Some(false)),
        ]);
        assert_eq!(asked, 3);
        let (_, asked) = run(&[
            (true, Some(true)),
            (false, Some(false)),
            (false, Some(false)),
        ]);
        assert_eq!(asked, 2, "none after the restore");
    }

    /// On the tests' backend there is no winit window: nothing to say,
    /// so every frame is drawn.
    #[test]
    fn a_window_with_no_winit_window_draws() {
        let app = crate::testing::window(1);
        assert_eq!(minimized(app.window()), None);
        assert!(draws(minimized(app.window())));
    }
}
