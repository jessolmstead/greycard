//! The window's opening size fitted to the monitor it opens on.
//!
//! The window asks for its preferred size, and nothing in Slint or
//! winit keeps that inside the screen: on a laptop shorter than the
//! preferred height, the window's foot (and a sheet's, which measures
//! itself against the window) lands under the Dock or the task bar.
//! Neither reports the screen's work area, the part a menu bar, a Dock
//! or a task bar leave, only the monitor's whole size; so we take that
//! less a margin wide enough for a title bar, a menu bar and a Dock at
//! its default size, which a platform's true work area would replace.

use slint::ComponentHandle;

use crate::App;

pub(crate) mod scale;

use scale::Scale;

/// Logical pixels left free across: a Dock or task bar on the side.
pub(crate) const MARGIN_WIDTH: f32 = 80.0;
/// Logical pixels left free down: what the window's size leaves out,
/// its title bar (about 28 on a Mac), and a menu bar (37 under a
/// notch) with a Dock at its default size (about 70) at the foot, or a
/// task bar.
pub(crate) const MARGIN_HEIGHT: f32 = 140.0;

/// The size to open at, in logical pixels: the size asked for, no
/// larger than the monitor's less the margin, and no smaller than the
/// layout's least. A monitor too small for even the least gets the
/// least; the window then runs past the screen's foot, as any window
/// with a minimum does there, and is moved or resized by the user.
pub(crate) fn fit(
    requested: (f32, f32),
    monitor: (f32, f32),
    margin: (f32, f32),
    least: (f32, f32),
) -> (f32, f32) {
    let one = |asked: f32, room: f32, margin: f32, least: f32| {
        asked.min(room - margin).max(least).round()
    };
    (
        one(requested.0, monitor.0, margin.0, least.0),
        one(requested.1, monitor.1, margin.1, least.1),
    )
}

/// A monitor's size in logical pixels: its physical size over the
/// scale. None for a monitor with no size, which Wayland reports for
/// one with no current mode.
pub(crate) fn logical(physical: (u32, u32), scale: f64) -> Option<(f32, f32)> {
    if physical.0 == 0 || physical.1 == 0 || scale <= 0.0 {
        return None;
    }
    Some((
        (f64::from(physical.0) / scale) as f32,
        (f64::from(physical.1) / scale) as f32,
    ))
}

/// The window the fit is for, put in once it is built: the handler
/// that fits it goes to the backend before it is.
pub(crate) type Slot = std::rc::Rc<std::cell::RefCell<Option<slint::Weak<App>>>>;

/// Fits the window before it exists. winit's `resumed` comes as the
/// event loop starts, before Slint creates the windows shown while it
/// was not running; a size set then goes into the window's attributes,
/// so the window is created, and placed (centered, on a Mac), at the
/// fitted size rather than shown at the preferred one and resized
/// under the user's eyes. Nothing remembers a window's size between
/// runs, so there is none here to respect; and with no window yet
/// there is none maximized or full screen to leave alone.
///
/// It also holds the window's scale to the system's times the
/// setting (`scale`), the one handler the backend takes doing both.
pub(crate) struct Handler {
    slot: Slot,
    done: bool,
    follow: scale::Follow,
}

/// The handler to give the backend, under this run's `scale`, and the
/// slot to put the window in once it is built.
pub(crate) fn handler(scale: Scale) -> (Handler, Slot) {
    let slot = Slot::default();
    let handler = Handler {
        slot: slot.clone(),
        done: false,
        follow: scale::Follow::new(scale),
    };
    (handler, slot)
}

impl slint::winit_030::CustomApplicationHandler for Handler {
    fn resumed(
        &mut self,
        event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
    ) -> slint::winit_030::EventResult {
        // Once: a platform that suspends and resumes would otherwise
        // refit a window the user has sized since.
        if !std::mem::replace(&mut self.done, true) {
            let app = self.slot.borrow().as_ref().and_then(|w| w.upgrade());
            if let Some(app) = app {
                let size = fit_window(&app, event_loop, self.follow.scale());
                self.follow.planned(size);
            }
        }
        slint::winit_030::EventResult::Propagate
    }

    fn window_event(
        &mut self,
        _event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
        _window_id: slint::winit_030::winit::window::WindowId,
        winit_window: Option<&slint::winit_030::winit::window::Window>,
        slint_window: Option<&slint::Window>,
        event: &slint::winit_030::winit::event::WindowEvent,
    ) -> slint::winit_030::EventResult {
        self.follow.window_event(winit_window, slint_window, event);
        slint::winit_030::EventResult::Propagate
    }
}

/// What the fit decided, and what the log says of it.
#[derive(Debug, PartialEq)]
pub(crate) enum Plan {
    /// Wayland: left to the compositor at the size asked for.
    Wayland,
    /// No monitor with a size: the size asked for.
    NoMonitor((f32, f32)),
    /// The monitor fitted to, in logical pixels, its scale, and the
    /// size to open at.
    Fit {
        monitor: (f32, f32),
        scale: f64,
        opened: (f32, f32),
    },
}

impl Plan {
    /// The one line in the log.
    pub(crate) fn line(&self) -> String {
        match self {
            Plan::Wayland => "window: Wayland, left to the compositor".into(),
            Plan::NoMonitor(asked) => format!(
                "window: no monitor reported, opening at {:.0}x{:.0}",
                asked.0, asked.1
            ),
            Plan::Fit {
                monitor,
                scale,
                opened,
            } => format!(
                "window: monitor {:.0}x{:.0} at {scale:.2}x, opening at {:.0}x{:.0}",
                monitor.0, monitor.1, opened.0, opened.1
            ),
        }
    }
}

/// The fit from what winit reports, as a pure function. On Wayland
/// nothing is fitted: winit reports a monitor's scale there rounded up
/// to a whole number (125 % and 150 % both read 2), so a 1920 x 1080 at
/// 125 % would read 960 x 540 and the window open at its least, and
/// neither the fractional scale nor the work area is known before the
/// window exists; the compositor places and sizes the window. Elsewhere
/// the smallest of the monitors, by logical area, that holds the
/// layout's least with the margins: the window opens on the focused
/// screen, which need not be the primary one (a laptop with a main
/// display beside it), and the smallest fits it wherever it opens. A
/// monitor too small for the least (a little USB display) is passed
/// over, so it does not shrink the window on the others; with none
/// that holds it, the largest. Slint's logical pixels on each monitor
/// are its own scale under the run's `scale`: times the interface
/// scale setting, or the factor `SLINT_SCALE_FACTOR` sets by hand.
pub(crate) fn plan(
    wayland: bool,
    asked: (f32, f32),
    monitors: &[((u32, u32), f64)],
    scale: Scale,
    least: (f32, f32),
) -> Plan {
    if wayland {
        return Plan::Wayland;
    }
    let sized: Vec<_> = monitors
        .iter()
        .filter_map(|&(physical, own)| {
            let scale = scale.on(own);
            logical(physical, scale).map(|size| (size, scale))
        })
        .collect();
    let area = |m: &((f32, f32), f64)| m.0.0 * m.0.1;
    let holds = |m: &&((f32, f32), f64)| {
        m.0.0 >= least.0 + MARGIN_WIDTH && m.0.1 >= least.1 + MARGIN_HEIGHT
    };
    let chosen = sized
        .iter()
        .filter(holds)
        .min_by(|a, b| area(a).total_cmp(&area(b)))
        .or_else(|| sized.iter().max_by(|a, b| area(a).total_cmp(&area(b))))
        .copied();
    match chosen {
        None => Plan::NoMonitor(asked),
        Some((monitor, scale)) => Plan::Fit {
            monitor,
            scale,
            opened: fit(asked, monitor, (MARGIN_WIDTH, MARGIN_HEIGHT), least),
        },
    }
}

/// Whether winit runs on Wayland.
fn is_wayland(event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop) -> bool {
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    {
        use slint::winit_030::winit::platform::wayland::ActiveEventLoopExtWayland;
        event_loop.is_wayland()
    }
    #[cfg(not(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )))]
    {
        let _ = event_loop;
        false
    }
}

/// The fit, with what winit reports, and its line in the log. The size
/// asked for is the layout's preferred one: before the window exists
/// Slint's window holds the layout's least, and applies the preferred
/// size only as it creates the window, when no size was set. The
/// size it opens at, in logical pixels: fitted, or as asked
/// (`scale::unfitted`).
fn fit_window(
    app: &App,
    event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
    scale: Scale,
) -> (f32, f32) {
    let asked = (app.get_asked_width(), app.get_asked_height());
    let monitors: Vec<_> = event_loop
        .available_monitors()
        .map(|m| {
            let p = m.size();
            ((p.width, p.height), m.scale_factor())
        })
        .collect();
    let least = (app.get_least_width(), app.get_least_height());
    let plan = plan(is_wayland(event_loop), asked, &monitors, scale, least);
    tracing::info!("{}", plan.line());
    let opened = match plan {
        Plan::Fit { opened, .. } => opened,
        // Nothing fitted: the size asked for, which under the interface
        // scale is set smaller by its percentage, so the window covers
        // what it would at 100 % rather than that times the scale.
        Plan::Wayland | Plan::NoMonitor(_) => {
            let size = scale::unfitted(scale, asked, least);
            if size == asked {
                return asked;
            }
            tracing::info!(
                "window: at {:.0}x{:.0} under the interface scale",
                size.0,
                size.1
            );
            // On Wayland the window opens at 1 times the setting
            // (`scale::decide`), and the size goes in at that factor as
            // pixels: Slint 1.18 sends a logical one on to winit as
            // logical once the window exists, and winit makes pixels of
            // it at its own scale, 1 until the compositor's comes,
            // which leaves the window the percentage too small.
            if plan == Plan::Wayland {
                let factor = scale.on(1.0) as f32;
                app.window().set_size(slint::PhysicalSize::new(
                    (size.0 * factor).round() as u32,
                    (size.1 * factor).round() as u32,
                ));
                return size;
            }
            size
        }
    };
    app.window()
        .set_size(slint::LogicalSize::new(opened.0, opened.1));
    opened
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARGIN: (f32, f32) = (MARGIN_WIDTH, MARGIN_HEIGHT);
    /// The layout's least today, which the last test reads.
    const LEAST: (f32, f32) = (560.0, 509.0);
    /// The preferred size, which the last test reads.
    const ASKED: (f32, f32) = (1500.0, 800.0);

    /// A 14-inch MacBook at its default scaling: the preferred height
    /// fits under the menu bar and above the Dock, and the width is
    /// held clear of a Dock on the side.
    #[test]
    fn a_fourteen_inch_macbook_keeps_its_height_and_narrows() {
        assert_eq!(fit(ASKED, (1512.0, 982.0), MARGIN, LEAST), (1432.0, 800.0));
        // The old preferred height, 950, cut to what the screen has.
        assert_eq!(
            fit((1500.0, 950.0), (1512.0, 982.0), MARGIN, LEAST),
            (1432.0, 842.0)
        );
    }

    /// A 13-inch Air: at 1470 x 956 the height fits; at 1440 x 900 it
    /// is cut.
    #[test]
    fn a_thirteen_inch_air() {
        assert_eq!(fit(ASKED, (1470.0, 956.0), MARGIN, LEAST), (1390.0, 800.0));
        assert_eq!(fit(ASKED, (1440.0, 900.0), MARGIN, LEAST), (1360.0, 760.0));
    }

    /// A 1080p laptop at 100 % opens as asked; at 150 %, 1280 x 720
    /// logical, it is cut both ways.
    #[test]
    fn a_1080p_laptop() {
        assert_eq!(fit(ASKED, (1920.0, 1080.0), MARGIN, LEAST), ASKED);
        assert_eq!(fit(ASKED, (1280.0, 720.0), MARGIN, LEAST), (1200.0, 580.0));
    }

    /// A 4K desktop, at 100 % or 200 %: never grown past the ask.
    #[test]
    fn a_4k_desktop_opens_as_asked() {
        for monitor in [(3840.0, 2160.0), (1920.0, 1080.0)] {
            assert_eq!(fit(ASKED, monitor, MARGIN, LEAST), ASKED);
        }
    }

    /// A monitor shorter than the layout's least plus the margin: the
    /// least wins, and the window runs past the screen's foot rather
    /// than squashing its layout; nothing scrolls the window itself.
    #[test]
    fn a_tiny_monitor_gets_the_least() {
        assert_eq!(fit(ASKED, (1024.0, 600.0), MARGIN, LEAST), (944.0, 509.0));
        assert_eq!(fit(ASKED, (640.0, 480.0), MARGIN, LEAST), LEAST);
    }

    /// A monitor's logical size: a Mac's 3024 x 1964 at 2 is the
    /// 14-inch's 1512 x 982; Wayland's whole-number 2 for a 1.5
    /// display reads it short; a monitor with no mode is none.
    #[test]
    fn a_monitors_logical_size() {
        assert_eq!(logical((3024, 1964), 2.0), Some((1512.0, 982.0)));
        assert_eq!(logical((2880, 1800), 2.0), Some((1440.0, 900.0)));
        assert_eq!(logical((2880, 1800), 1.5), Some((1920.0, 1200.0)));
        assert_eq!(logical((0, 0), 1.0), None);
        assert_eq!(logical((1920, 0), 1.0), None);
    }

    /// The plan from what winit reports: a 14-inch Mac alone, and with
    /// a larger main display beside it (the smallest wins, whichever is
    /// primary); a monitor with no mode skipped; none at all; Slint's
    /// own scale over the monitor's, set by hand or by the interface
    /// scale; and Wayland left alone.
    #[test]
    fn the_plan_from_the_monitors() {
        let mac = ((3024, 1964), 2.0);
        let main_display = ((3840, 2160), 1.5);
        let line = |p: &Plan| p.line();
        let alone = plan(false, ASKED, &[mac], Scale::System, LEAST);
        assert_eq!(
            line(&alone),
            "window: monitor 1512x982 at 2.00x, opening at 1432x800"
        );
        assert_eq!(
            plan(false, ASKED, &[main_display, mac], Scale::System, LEAST),
            alone
        );
        assert_eq!(
            plan(false, ASKED, &[((0, 0), 1.0), mac], Scale::System, LEAST),
            alone
        );
        // A little USB display beside a 4K one: too small for the
        // least with the margins, it is passed over, and the window
        // opens as asked on the 4K.
        let usb = ((1024, 600), 1.0);
        let four_k = ((3840, 2160), 1.0);
        assert_eq!(
            line(&plan(false, ASKED, &[usb, four_k], Scale::System, LEAST)),
            "window: monitor 3840x2160 at 1.00x, opening at 1500x800"
        );
        // A 1366 x 768 secondary holds the least, and counts.
        assert_eq!(
            line(&plan(
                false,
                ASKED,
                &[four_k, ((1366, 768), 1.0)],
                Scale::System,
                LEAST
            )),
            "window: monitor 1366x768 at 1.00x, opening at 1286x628"
        );
        // A tiny monitor alone is the largest there is: the least.
        assert_eq!(
            line(&plan(
                false,
                ASKED,
                &[((800, 480), 1.0)],
                Scale::System,
                LEAST
            )),
            "window: monitor 800x480 at 1.00x, opening at 720x509"
        );
        let none = plan(false, ASKED, &[((0, 0), 1.0)], Scale::System, LEAST);
        assert_eq!(
            line(&none),
            "window: no monitor reported, opening at 1500x800"
        );
        // SLINT_SCALE_FACTOR=1 on the Mac's panel: 3024 x 1964 of
        // Slint's pixels, the size asked for fits.
        let slint = plan(false, ASKED, &[mac], Scale::ByHand(1.0), LEAST);
        assert_eq!(
            line(&slint),
            "window: monitor 3024x1964 at 1.00x, opening at 1500x800"
        );
        // The interface scale at 150 %: each monitor's own scale times
        // it, the Mac's 2 as 3 and a 1080p panel's 1 as 1.5.
        assert_eq!(
            line(&plan(false, ASKED, &[mac], Scale::Times(1.5), LEAST)),
            "window: monitor 1008x655 at 3.00x, opening at 928x515"
        );
        assert_eq!(
            line(&plan(
                false,
                ASKED,
                &[((1920, 1080), 1.0)],
                Scale::Times(1.5),
                LEAST
            )),
            "window: monitor 1280x720 at 1.50x, opening at 1200x580"
        );
        let wayland = plan(true, ASKED, &[mac], Scale::System, LEAST);
        assert_eq!(line(&wayland), "window: Wayland, left to the compositor");
    }

    /// The fit's inputs in the running editor: the preferred size the
    /// window asks for, and its least, read from the layout. The
    /// least moves a little with the font, so it is held to a range.
    #[test]
    fn the_window_asks_for_its_preferred_size_and_reports_its_least() {
        let app = crate::testing::window(1);
        assert_eq!((app.get_asked_width(), app.get_asked_height()), ASKED);
        let size = app.window().size().to_logical(app.window().scale_factor());
        assert_eq!((size.width, size.height), ASKED);
        let (w, h) = (app.get_least_width(), app.get_least_height());
        assert!(
            (LEAST.0 - 60.0..=LEAST.0 + 60.0).contains(&w),
            "least width {w}"
        );
        assert!(
            (LEAST.1 - 60.0..=LEAST.1 + 60.0).contains(&h),
            "least height {h}"
        );
    }
}
