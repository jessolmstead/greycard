//! A watch for a window that stops drawing while its input goes on: a
//! press that no frame follows.
//!
//! On Wayland every frame Slint presents asks the compositor for a
//! frame callback (`pre_present_notify`), and winit 0.30 hands out no
//! `RedrawRequested` until that callback comes. A compositor that never
//! sends it (a window covered on a monitor not in use, under mutter)
//! leaves the window deaf to the eye: clicks are handled, timers fire,
//! and nothing is drawn, until the callback comes. Nothing here can
//! end it; this only says so in the log, with what the window knew.
//!
//! The winit hook (`window_events`, through `side_buttons`) tells us
//! of each press, each `RedrawRequested` (a minimized window's held
//! ones too), and each change of focus or occlusion; the
//! rendering notifier of each frame drawn. The 1 s choices timer asks
//! [`tick`], which after a second asks for one redraw (a press on a
//! spot that changes nothing draws none), and after three says which
//! of two stalls it is: winit delivered no redraw, or it delivered
//! some and none was drawn (the surface skipped them: occluded, timed
//! out, or outdated, which Slint's renderer keeps to itself). The next
//! frame says how long the stall lasted. Each hook is a few `Cell`s;
//! nothing allocates or logs while the window is well.

use std::cell::Cell;
use std::time::{Duration, Instant};

use slint::ComponentHandle;

use crate::App;

/// How old a press with no frame since is before we ask for a redraw.
const NUDGE: Duration = Duration::from_secs(1);
/// How old before we say so, once the redraw was asked for.
const SAY: Duration = Duration::from_secs(3);
/// How long unfocused before the focus coming back is said, so a stall
/// can be lined up with the user's return.
const AWAY: Duration = Duration::from_secs(60);

/// A press no frame has followed yet, and the counts as it came.
#[derive(Debug, Clone, Copy)]
struct Pending {
    at: Instant,
    redraws: u64,
    frames: u64,
    nudged: bool,
}

#[derive(Default)]
struct Watch {
    /// `RedrawRequested` events delivered, and the last one's time.
    redraws: Cell<u64>,
    last_redraw: Cell<Option<Instant>>,
    /// Frames drawn (the rendering notifier's `BeforeRendering`), and
    /// the last one's time.
    frames: Cell<u64>,
    last_frame: Cell<Option<Instant>>,
    press: Cell<Option<Pending>>,
    /// The press of a stall said, until a frame ends it.
    stalled: Cell<Option<Instant>>,
    /// The last focus and occlusion winit reported, and since when.
    focused: Cell<Option<(bool, Instant)>>,
    occluded: Cell<Option<(bool, Instant)>>,
}

thread_local! {
    static WATCH: Watch = Watch::default();
}

/// What the timer does about the press waiting for a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing yet.
    Rest,
    /// A frame came: the press is done with.
    Clear,
    /// Ask for one redraw.
    Nudge,
    /// Say the stall.
    Say(Stall),
}

/// Which stall: winit delivered no `RedrawRequested` since the press,
/// or it delivered some and none was drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stall {
    NoneDelivered,
    NoneDrawn,
}

/// The rule, for a press `age` old with `delivered` redraws and
/// `drawn` frames since, and whether a redraw was asked for it.
pub(crate) fn step(age: Duration, delivered: u64, drawn: u64, nudged: bool) -> Step {
    if drawn > 0 {
        Step::Clear
    } else if !nudged {
        if age >= NUDGE {
            Step::Nudge
        } else {
            Step::Rest
        }
    } else if age >= SAY {
        Step::Say(if delivered == 0 {
            Stall::NoneDelivered
        } else {
            Stall::NoneDrawn
        })
    } else {
        Step::Rest
    }
}

/// A press of any button over the window. The first since the last
/// frame is the one timed; none while a stall said is still on.
pub(crate) fn pressed(now: Instant) {
    WATCH.with(|w| {
        if w.press.get().is_none() && w.stalled.get().is_none() {
            w.press.set(Some(Pending {
                at: now,
                redraws: w.redraws.get(),
                frames: w.frames.get(),
                nudged: false,
            }));
        }
    });
}

/// winit handed the window a `RedrawRequested`.
pub(crate) fn redraw_delivered(now: Instant) {
    WATCH.with(|w| {
        w.redraws.set(w.redraws.get() + 1);
        w.last_redraw.set(Some(now));
    });
}

/// A frame is being drawn; a stall said is over.
pub(crate) fn frame_drawn(now: Instant) {
    WATCH.with(|w| {
        w.frames.set(w.frames.get() + 1);
        w.last_frame.set(Some(now));
        if let Some(at) = w.stalled.take() {
            tracing::info!(
                "window: drawing again after {:.1} s",
                now.saturating_duration_since(at).as_secs_f64()
            );
        }
    });
}

/// winit's focus, said when it comes back after a minute away.
pub(crate) fn focused(on: bool, now: Instant) {
    WATCH.with(|w| {
        let was = w.focused.get();
        if was.is_some_and(|(f, _)| f == on) {
            return;
        }
        if let Some((false, since)) = was {
            let away = now.saturating_duration_since(since);
            if on && away > AWAY {
                tracing::info!(
                    "window: focused again after {:.0} s away",
                    away.as_secs_f64()
                );
            }
        }
        w.focused.set(Some((on, now)));
    });
}

/// winit's occlusion, said at each change. winit 0.30 reports none on
/// Wayland (it reads no xdg_toplevel `suspended` state); X11, Windows,
/// and macOS do.
pub(crate) fn occluded(on: bool, now: Instant) {
    WATCH.with(|w| {
        let was = w.occluded.get();
        if was.is_some_and(|(o, _)| o == on) {
            return;
        }
        match was {
            Some((_, since)) => tracing::info!(
                "window: occluded {on} (was {} for {:.0} s)",
                !on,
                now.saturating_duration_since(since).as_secs_f64()
            ),
            None => tracing::info!("window: occluded {on}"),
        }
        w.occluded.set(Some((on, now)));
    });
}

/// The timer's look at the press waiting for a frame: see the module.
pub(crate) fn tick(app: &App) {
    tick_at(app, Instant::now(), &|| app.window().request_redraw());
}

/// [`tick`] at `now`, asking for a redraw through `redraw`; what it did.
pub(crate) fn tick_at(app: &App, now: Instant, redraw: &dyn Fn()) -> Step {
    WATCH.with(|w| {
        let Some(mut press) = w.press.get() else {
            return Step::Rest;
        };
        let age = now.saturating_duration_since(press.at);
        let delivered = w.redraws.get() - press.redraws;
        let drawn = w.frames.get() - press.frames;
        let step = step(age, delivered, drawn, press.nudged);
        match step {
            Step::Rest => {}
            Step::Clear => w.press.set(None),
            Step::Nudge => {
                press.nudged = true;
                w.press.set(Some(press));
                redraw();
            }
            Step::Say(stall) => {
                w.press.set(None);
                w.stalled.set(Some(press.at));
                say(w, app, now, age, delivered, stall);
            }
        }
        step
    })
}

/// The stall's line, with what the window knew.
fn say(w: &Watch, app: &App, now: Instant, age: Duration, delivered: u64, stall: Stall) {
    let ago = |t: Option<Instant>| match t {
        Some(t) => format!(
            "{:.1} s ago",
            now.saturating_duration_since(t).as_secs_f64()
        ),
        None => "never".to_owned(),
    };
    let state = |s: Option<(bool, Instant)>| match s {
        Some((b, t)) => format!(
            "{b} for {:.0} s",
            now.saturating_duration_since(t).as_secs_f64()
        ),
        None => "unknown".to_owned(),
    };
    let what = match stall {
        Stall::NoneDelivered => "winit delivered no redraw".to_owned(),
        Stall::NoneDrawn => format!("winit delivered {delivered} redraws and none was drawn"),
    };
    tracing::info!(
        "window: a press {:.1} s ago and no frame since: {what}; last redraw delivered {}, \
         last frame {}; focused {}, occluded {}, popups {}, menu-up {}, sheet open {}",
        age.as_secs_f64(),
        ago(w.last_redraw.get()),
        ago(w.last_frame.get()),
        state(w.focused.get()),
        state(w.occluded.get()),
        crate::panel::side_buttons::popups(app),
        app.get_menu_up(),
        app.invoke_sheet_open(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;
    const MS: fn(u64) -> Duration = Duration::from_millis;

    /// A frame ends the wait at any age; one redraw is asked for after
    /// a second; the stall is said at three, after the ask, by what
    /// winit delivered.
    #[test]
    fn the_rule() {
        assert_eq!(step(MS(500), 0, 0, false), Step::Rest);
        assert_eq!(step(MS(500), 0, 1, false), Step::Clear);
        assert_eq!(step(S(10), 4, 1, true), Step::Clear);
        assert_eq!(step(MS(1000), 0, 0, false), Step::Nudge);
        // A late timer asks first, whatever the age.
        assert_eq!(step(S(30), 0, 0, false), Step::Nudge);
        assert_eq!(step(MS(2500), 0, 0, true), Step::Rest);
        assert_eq!(step(S(3), 0, 0, true), Step::Say(Stall::NoneDelivered));
        assert_eq!(step(S(3), 2, 0, true), Step::Say(Stall::NoneDrawn));
    }

    /// Through the window's own hook: a press, no frame, one redraw
    /// asked for and no second, the stall said once, and a frame after
    /// it ends it so the next press is timed again. A press a frame
    /// follows asks for nothing.
    #[test]
    fn one_redraw_asked_for_a_press_no_frame_follows() {
        use slint::winit_030::winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};
        let app = crate::testing::window(1);
        let mut hook = crate::panel::side_buttons::SideButtons::default();
        let asked = Cell::new(0);
        let redraw = || asked.set(asked.get() + 1);
        let press = |hook: &mut crate::panel::side_buttons::SideButtons| {
            hook.event(
                &app,
                &WindowEvent::MouseInput {
                    device_id: DeviceId::dummy(),
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                },
            );
        };
        // A press a frame follows.
        let t0 = Instant::now();
        press(&mut hook);
        hook.event(&app, &WindowEvent::RedrawRequested);
        frame_drawn(t0);
        assert_eq!(tick_at(&app, t0 + S(5), &redraw), Step::Clear);
        assert_eq!(tick_at(&app, t0 + S(9), &redraw), Step::Rest);
        assert_eq!(asked.get(), 0);
        // A press none follows.
        let t0 = Instant::now();
        press(&mut hook);
        assert_eq!(tick_at(&app, t0 + MS(300), &redraw), Step::Rest);
        assert_eq!(tick_at(&app, t0 + MS(1300), &redraw), Step::Nudge);
        assert_eq!(asked.get(), 1);
        // More presses, and the redraw asked for never comes.
        press(&mut hook);
        assert_eq!(tick_at(&app, t0 + MS(2300), &redraw), Step::Rest);
        assert_eq!(
            tick_at(&app, t0 + MS(3300), &redraw),
            Step::Say(Stall::NoneDelivered)
        );
        assert_eq!(asked.get(), 1, "one redraw asked for");
        // Said once: the presses during the stall are not timed.
        press(&mut hook);
        assert_eq!(tick_at(&app, t0 + S(20), &redraw), Step::Rest);
        assert_eq!(asked.get(), 1);
        // The frame ends it; the next press is timed, and this time
        // winit delivers a redraw that is not drawn.
        frame_drawn(t0 + S(30));
        let t1 = Instant::now();
        press(&mut hook);
        assert_eq!(tick_at(&app, t1 + MS(1100), &redraw), Step::Nudge);
        hook.event(&app, &WindowEvent::RedrawRequested);
        assert_eq!(
            tick_at(&app, t1 + MS(3100), &redraw),
            Step::Say(Stall::NoneDrawn)
        );
        assert_eq!(asked.get(), 2);
    }
}
