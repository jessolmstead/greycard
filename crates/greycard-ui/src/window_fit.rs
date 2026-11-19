//! The window's opening size fitted to the monitor it opens on.
//!
//! The window asks for its preferred size, and nothing in Slint or
//! winit keeps that inside the screen: on a laptop shorter than the
//! preferred height, the window's foot (and a sheet's, which measures
//! itself against the window) lands under the Dock or the task bar.
//! Neither reports the screen's work area, the part a menu bar, a Dock,
//! or a task bar leave, only the monitor's whole size. So we ask the
//! platform for it (`work_area`) and take that less a title bar; where
//! it does not say (Wayland, or an X11 window manager that sets none),
//! the monitor's size less a margin wide enough for a title bar, a menu
//! bar, and a Dock at its default size.

use slint::ComponentHandle;

use crate::App;

pub(crate) mod full_screen;
pub(crate) mod kept;
pub(crate) mod scale;
pub(crate) mod work_area;

use scale::Scale;

/// Logical pixels left free across: a Dock or task bar on the side.
pub(crate) const MARGIN_WIDTH: f32 = 80.0;
/// Logical pixels left free down: what the window's size leaves out,
/// its title bar (about 28 on a Mac), and a menu bar (37 under a
/// notch) with a Dock at its default size (about 70) at the foot, or a
/// task bar.
pub(crate) const MARGIN_HEIGHT: f32 = 140.0;
/// Logical pixels left free across inside a work area the platform
/// reports: the window's frame, with a little to spare.
pub(crate) const WORK_MARGIN_WIDTH: f32 = 16.0;
/// Logical pixels left free down inside a work area: the window's
/// title bar, which its size leaves out (28 on a Mac, about 31 on
/// Windows, more under some Linux window managers).
pub(crate) const WORK_MARGIN_HEIGHT: f32 = 48.0;

/// The size to open at, in logical pixels: the size asked for, no
/// larger than the room (a work area, or a whole monitor) less the
/// margin, and no smaller than the layout's least. A monitor too small
/// for even the least gets the least; the window then runs past the
/// screen's foot, as any window with a minimum does there, and is
/// moved or resized by the user.
pub(crate) fn fit(
    requested: (f32, f32),
    room: (f32, f32),
    margin: (f32, f32),
    least: (f32, f32),
) -> (f32, f32) {
    let one = |asked: f32, room: f32, margin: f32, least: f32| {
        asked.min(room - margin).max(least).round()
    };
    (
        one(requested.0, room.0, margin.0, least.0),
        one(requested.1, room.1, margin.1, least.1),
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

/// One of winit's monitors as the fit reads it: its size in physical
/// pixels, its own scale, its work area in the same pixels where the
/// platform reports one, and whether the system would open a new
/// window on it (`opens_on`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Monitor {
    pub(crate) size: (u32, u32),
    pub(crate) scale: f64,
    pub(crate) work: Option<work_area::Rect>,
    pub(crate) opens_here: bool,
}

/// The window the fit is for, put in once it is built: the handler
/// that fits it goes to the backend before it is.
pub(crate) type Slot = std::rc::Rc<std::cell::RefCell<Option<slint::Weak<App>>>>;

/// Fits the window before it exists. winit's `resumed` comes as the
/// event loop starts, before Slint creates the windows shown while it
/// was not running; a size set then goes into the window's attributes,
/// so the window is created, and placed (centered, on a Mac), at the
/// fitted size rather than shown at the preferred one and resized
/// under the user's eyes. A window kept from the last run (`kept`)
/// opens at its size, through the same fit, at its place and maximized
/// as it was, and full screen once it is shown; with no window yet
/// there is none maximized or full screen to leave alone.
///
/// It also holds the window's scale to the system's times the
/// setting (`scale`), and sets a slider's way for each wheel event
/// from its device (`scrolling`), the one handler the backend takes
/// doing all three.
pub(crate) struct Handler {
    slot: Slot,
    done: bool,
    follow: scale::Follow,
    memory: kept::Memory,
    /// A maximize asked again once the window is shown (X11).
    maximize_owed: bool,
    /// Full screen asked once the window is shown, as the last run
    /// closed it, and whether it opened maximized under it.
    full_owed: Option<bool>,
}

/// The handler to give the backend, under this run's `scale` and with
/// the window kept from the last run (`memory`), and the slot to put
/// the window in once it is built.
pub(crate) fn handler(scale: Scale, memory: kept::Memory) -> (Handler, Slot) {
    let slot = Slot::default();
    let handler = Handler {
        slot: slot.clone(),
        done: false,
        follow: scale::Follow::new(scale),
        memory,
        maximize_owed: false,
        full_owed: None,
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
                let (size, maximized, full) = fit_window(
                    &app,
                    event_loop,
                    self.follow.scale(),
                    self.memory.kept.as_ref(),
                );
                self.follow.planned(size);
                self.follow
                    .least((app.get_least_width(), app.get_least_height()));
                // The shortcut's Run setting wins over what was kept.
                let shortcut = starts_maximized();
                if shortcut {
                    tracing::info!("window: opened maximized, as the shortcut asks");
                }
                if shortcut || maximized {
                    app.window().set_maximized(true);
                    self.maximize_owed = is_x11(event_loop);
                }
                // Full screen once shown, not in the attributes the
                // window is made with: the platform then holds the
                // window under it as the one to leave full screen to,
                // as F11 does, and X11 passes over no ask to an
                // unmapped window.
                if full {
                    self.full_owed = Some(shortcut || maximized);
                }
                if self.memory.on {
                    kept::start(full);
                }
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
        if let slint::winit_030::winit::event::WindowEvent::MouseWheel { delta, .. } = event {
            let pixels = matches!(
                delta,
                slint::winit_030::winit::event::MouseScrollDelta::PixelDelta(_)
            );
            let app = self.slot.borrow().as_ref().and_then(|w| w.upgrade());
            if let (Some(turned), Some(app)) = (crate::scrolling::for_event(pixels), app) {
                app.global::<crate::SliderKeys>().set_natural(turned);
            }
        }
        if let (Some(w), slint::winit_030::winit::event::WindowEvent::Resized(_)) =
            (winit_window, event)
        {
            full_screen::resized(w.fullscreen().is_some());
        }
        use slint::winit_030::winit::event::WindowEvent as E;
        let main = |slint_window: &slint::Window| {
            self.slot
                .borrow()
                .as_ref()
                .and_then(|w| w.upgrade())
                .is_some_and(|app| std::ptr::eq(app.window(), slint_window))
        };
        let (Some(w), Some(slint_window)) = (winit_window, slint_window) else {
            return slint::winit_030::EventResult::Propagate;
        };
        if !(self.memory.on || self.maximize_owed) || !main(slint_window) {
            return slint::winit_030::EventResult::Propagate;
        }
        let step = owed(
            self.maximize_owed,
            self.full_owed.is_some(),
            matches!(event, E::Resized(_) | E::Moved(_) | E::Focused(_)),
            w.is_visible(),
            slint_window.is_visible(),
        );
        if matches!(step, Owed::Maximize | Owed::MaximizeThenFull) {
            self.maximize_owed = false;
            if !w.is_maximized() {
                tracing::info!("window: maximized again, now that it is shown");
                w.set_maximized(true);
            }
        }
        if let (Owed::Full | Owed::MaximizeThenFull, Some(maximized)) = (step, self.full_owed) {
            self.full_owed = None;
            // After this event: Slint's own handling of it, which runs
            // next, sets its full screen to winit's, not yet so, and
            // would take back an ask made here before it reached winit.
            let app = self.slot.borrow().clone();
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                if let Some(app) = app.and_then(|w| w.upgrade()) {
                    full_screen::reopen(app.window(), maximized);
                }
            });
        }
        if step != Owed::Read {
            return slint::winit_030::EventResult::Propagate;
        }
        // The editor's window as it is sized and moved, for the close;
        // nothing once a close is asked, the window's withdrawal.
        match event {
            E::CloseRequested => kept::close_requested(),
            E::Resized(_) | E::Moved(_) => kept::observe(w, slint_window.is_visible()),
            _ => {}
        }
        slint::winit_030::EventResult::Propagate
    }

    fn exiting(
        &mut self,
        _event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
    ) -> slint::winit_030::EventResult {
        // Before the close's write, which reads what was kept.
        kept::exiting();
        crate::panel::startup::exiting();
        slint::winit_030::EventResult::Propagate
    }
}

/// What an event does with what the opening owes the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Owed {
    /// Not yet shown: wait, and read nothing.
    Wait,
    /// Ask the maximize again, and read nothing.
    Maximize,
    /// Ask the maximize again, then full screen, and read nothing.
    MaximizeThenFull,
    /// Ask full screen, and read nothing.
    Full,
    /// Nothing owed: read the event for the close.
    Read,
}

/// What an event does: on X11 a maximize asked before the window is
/// mapped goes to an unmapped window, which a window manager is free
/// to pass over (Mutter does), so it is asked again once the window is
/// shown (`maximize`) and nothing is read meanwhile, so the gap is not
/// taken for a normal window. Full screen as it was closed (`full`)
/// is asked at the first event that finds the window shown, after the
/// maximize under it: by Slint's word (`slint_visible`), since Wayland
/// says nothing of a window's being visible (`visible` None). The
/// tracker reads nothing of its own until the window is full screen
/// (`kept::Tracker::awaiting`). `shows`: the event is one a shown
/// window has (resized, moved, or focused).
pub(crate) fn owed(
    maximize: bool,
    full: bool,
    shows: bool,
    visible: Option<bool>,
    slint_visible: bool,
) -> Owed {
    let full_now = full && shows && slint_visible && visible != Some(false);
    if maximize {
        if !(shows && visible == Some(true)) {
            Owed::Wait
        } else if full_now {
            Owed::MaximizeThenFull
        } else {
            Owed::Maximize
        }
    } else if full_now {
        Owed::Full
    } else {
        Owed::Read
    }
}

/// Whether the process was started with the window asked to open
/// maximized: on Windows a shortcut's Run setting, handed on in the
/// startup info, which winit does not read. Elsewhere nothing asks.
#[cfg(windows)]
fn starts_maximized() -> bool {
    use windows_sys::Win32::System::Threading::STARTF_USESHOWWINDOW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMAXIMIZED;
    let info = startup_info();
    info.dwFlags & STARTF_USESHOWWINDOW != 0 && i32::from(info.wShowWindow) == SW_SHOWMAXIMIZED
}

/// The process's startup info, handed on by whatever started it.
#[cfg(windows)]
fn startup_info() -> windows_sys::Win32::System::Threading::STARTUPINFOW {
    use windows_sys::Win32::System::Threading::{GetStartupInfoW, STARTUPINFOW};
    // SAFETY: GetStartupInfoW fills the struct given and cannot fail;
    // a zeroed STARTUPINFOW is a valid one to fill.
    unsafe {
        let mut info: STARTUPINFOW = std::mem::zeroed();
        info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        GetStartupInfoW(&mut info);
        info
    }
}

/// Which of `monitors` the system opens a new window on: on Windows the
/// one the process was started from when the shell says so (the
/// startup info's monitor), else the primary.
fn opens_on(
    event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
    monitors: &[slint::winit_030::winit::monitor::MonitorHandle],
) -> Option<usize> {
    #[cfg(windows)]
    {
        use slint::winit_030::winit::platform::windows::MonitorHandleExtWindows;
        // Not in windows-sys: the shell sets it with the monitor's
        // handle in hStdOutput.
        const STARTF_USEMONITOR: u32 = 0x0000_0400;
        let info = startup_info();
        if info.dwFlags & STARTF_USEMONITOR != 0 {
            let started = info.hStdOutput as isize;
            if let Some(i) = monitors.iter().position(|m| m.hmonitor() == started) {
                return Some(i);
            }
        }
    }
    let primary = event_loop.primary_monitor()?;
    monitors.iter().position(|m| *m == primary)
}

#[cfg(not(windows))]
fn starts_maximized() -> bool {
    false
}

/// What the fit decided, and what the log says of it.
#[derive(Debug, PartialEq)]
pub(crate) enum Plan {
    /// Wayland: left to the compositor at the size asked for.
    Wayland,
    /// No monitor with a size: the size asked for.
    NoMonitor((f32, f32)),
    /// The monitor fitted to, in logical pixels, its scale, its work
    /// area where the platform reports one, the size to open at, and
    /// where in the work area the window is centered (`place`).
    Fit {
        monitor: (f32, f32),
        scale: f64,
        work: Option<(f32, f32)>,
        opened: (f32, f32),
        at: Option<(i32, i32)>,
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
                work,
                opened,
                ..
            } => {
                let work = work
                    .map(|w| format!(", work area {:.0}x{:.0}", w.0, w.1))
                    .unwrap_or_default();
                format!(
                    "window: monitor {:.0}x{:.0} at {scale:.2}x{work}, opening at {:.0}x{:.0}",
                    monitor.0, monitor.1, opened.0, opened.1
                )
            }
        }
    }
}

/// The fit from what winit reports, as a pure function. On Wayland
/// nothing is fitted: winit reports a monitor's scale there rounded up
/// to a whole number (125 % and 150 % both read 2), so a 1920 x 1080 at
/// 125 % would read 960 x 540 and the window open at its least, and
/// neither the fractional scale nor the work area is known before the
/// window exists; the compositor places and sizes the window. Elsewhere
/// each monitor's room is its work area less a title bar where the
/// platform reports one, else its whole size less the guessed margins,
/// and the fit takes the smallest room, by logical area, that holds the
/// layout's least: the window opens on the focused screen, which need
/// not be the primary one (a laptop with a main display beside it), and
/// the smallest fits it wherever it opens. A monitor too small for the
/// least (a little USB display) is passed over, so it does not shrink
/// the window on the others; with none that holds it, the largest.
/// Slint's logical pixels on each monitor are its own scale under the
/// run's `scale`: times the interface scale setting, or the factor
/// `SLINT_SCALE_FACTOR` sets by hand.
pub(crate) fn plan(
    wayland: bool,
    asked: (f32, f32),
    monitors: &[Monitor],
    scale: Scale,
    least: (f32, f32),
) -> Plan {
    if wayland {
        return Plan::Wayland;
    }
    struct Sized {
        monitor: (f32, f32),
        scale: f64,
        work: Option<(f32, f32)>,
        room: (f32, f32),
        margin: (f32, f32),
        rect: Option<work_area::Rect>,
        own: f64,
        opens_here: bool,
    }
    let sized: Vec<_> = monitors
        .iter()
        .filter_map(|m| {
            let scale = scale.on(m.scale);
            let monitor = logical(m.size, scale)?;
            // A work area larger than its monitor is read as the monitor.
            let work = m
                .work
                .and_then(|w| logical((w.2, w.3), scale))
                .map(|w| (w.0.min(monitor.0), w.1.min(monitor.1)));
            let (room, margin) = match work {
                Some(work) => (work, (WORK_MARGIN_WIDTH, WORK_MARGIN_HEIGHT)),
                None => (monitor, (MARGIN_WIDTH, MARGIN_HEIGHT)),
            };
            // The system's pixels in Slint's: never fewer than the
            // margin's own count, so an interface scale under 100 %
            // does not shrink the room left for a title bar.
            let system = (m.scale / scale).max(1.0) as f32;
            Some(Sized {
                monitor,
                scale,
                work,
                room,
                margin: (margin.0 * system, margin.1 * system),
                rect: m.work.filter(|_| work.is_some()),
                own: m.scale,
                opens_here: m.opens_here,
            })
        })
        .collect();
    // Each side held at 0, so two sides short of the margin do not
    // multiply to a large area.
    let area = |m: &Sized| (m.room.0 - m.margin.0).max(0.0) * (m.room.1 - m.margin.1).max(0.0);
    let holds = |m: &&Sized| m.room.0 - m.margin.0 >= least.0 && m.room.1 - m.margin.1 >= least.1;
    let chosen = sized
        .iter()
        .filter(holds)
        .min_by(|a, b| area(a).total_cmp(&area(b)))
        .or_else(|| sized.iter().max_by(|a, b| area(a).total_cmp(&area(b))));
    match chosen {
        None => Plan::NoMonitor(asked),
        Some(m) => {
            let opened = fit(asked, m.room, m.margin, least);
            let frame = |m: &Sized| {
                (
                    WORK_MARGIN_WIDTH * m.own as f32,
                    WORK_MARGIN_HEIGHT * m.own as f32,
                )
            };
            // Placed where the system would open it, if the size fitted
            // to the smallest room fits there with its frame; else on
            // the smallest room's monitor.
            let fits = |m: &&Sized| {
                let frame = frame(m);
                m.rect.is_some_and(|r| {
                    f64::from(opened.0) * m.scale + f64::from(frame.0) <= f64::from(r.2)
                        && f64::from(opened.1) * m.scale + f64::from(frame.1) <= f64::from(r.3)
                })
            };
            let home = sized
                .iter()
                .filter(|s| s.opens_here)
                .find(fits)
                .unwrap_or(m);
            Plan::Fit {
                monitor: m.monitor,
                scale: m.scale,
                work: m.work,
                opened,
                at: home
                    .rect
                    .map(|rect| place(rect, opened, home.scale, frame(home))),
            }
        }
    }
}

/// Where the window goes in a work area, in the desktop's physical
/// pixels: its outer size (the size opened at, in Slint's pixels at
/// `scale`, and its `frame` in physical ones) centered in the area, and
/// from its top-left corner when the area is the smaller. Windows
/// creates a window where it likes, cascaded from the work area's
/// corner, and winit's resize keeps that corner; the fit puts it here
/// instead. macOS centers a new window in the visible frame itself, and
/// an X11 window manager places it.
pub(crate) fn place(
    work: work_area::Rect,
    opened: (f32, f32),
    scale: f64,
    frame: (f32, f32),
) -> (i32, i32) {
    let one = |start: i32, room: u32, size: f32, frame: f32| {
        let outer = f64::from(size) * scale + f64::from(frame);
        start + ((f64::from(room) - outer) / 2.0).max(0.0).round() as i32
    };
    (
        one(work.0, work.2, opened.0, frame.0),
        one(work.1, work.3, opened.1, frame.1),
    )
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

/// Whether winit runs on X11.
fn is_x11(event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop) -> bool {
    cfg!(all(
        unix,
        not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        ))
    )) && !is_wayland(event_loop)
}

/// The fit, with what winit reports, and its line in the log. The size
/// asked for is the layout's preferred one: before the window exists
/// Slint's window holds the layout's least, and applies the preferred
/// size only as it creates the window, when no size was set. The
/// size it opens at, in logical pixels: fitted, or as asked
/// (`scale::unfitted`); and whether the window kept (`kept`) was
/// maximized, and full screen.
fn fit_window(
    app: &App,
    event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
    scale: Scale,
    kept: Option<&kept::Kept>,
) -> ((f32, f32), bool, bool) {
    let layout = (app.get_asked_width(), app.get_asked_height());
    let wayland = is_wayland(event_loop);
    let handles: Vec<_> = event_loop.available_monitors().collect();
    let home = opens_on(event_loop, &handles);
    let monitors: Vec<_> = handles
        .iter()
        .zip(work_area::read(wayland, &handles))
        .enumerate()
        .map(|(i, (m, work))| {
            let p = m.size();
            Monitor {
                size: (p.width, p.height),
                scale: m.scale_factor(),
                work,
                opens_here: home == Some(i),
            }
        })
        .collect();
    let screens: Vec<_> = handles.iter().map(kept::Screen::of).collect();
    let least = (app.get_least_width(), app.get_least_height());
    let restored = kept.and_then(|k| kept::restore(k, wayland, &screens, &monitors, scale, least));
    if let Some(r) = &restored {
        tracing::info!("{}", r.line);
    }
    let from_kept = restored.is_some();
    let (asked, plan, place, maximized, full) = match restored {
        Some(r) => (r.size, r.plan, r.at, r.maximized, r.full_screen),
        None => (
            layout,
            plan(wayland, layout, &monitors, scale, least),
            None,
            false,
            false,
        ),
    };
    tracing::info!("{}", plan.line());
    // macOS creates the window at points. A logical position is handed
    // to winit as points, as set, unless SLINT_SCALE_FACTOR is set (an
    // interface scale other than 100 %, or by hand), when Slint makes
    // pixels of it at that factor and winit points again at the main
    // screen's scale: there pixels at the primary display's scale,
    // which is the main screen's but with displays at two scales.
    // Elsewhere the desktop's pixels.
    let place = place.map(|p| -> slint::WindowPosition {
        if !cfg!(target_os = "macos") {
            slint::PhysicalPosition::new(p.physical.0, p.physical.1).into()
        } else if scale == Scale::System {
            slint::LogicalPosition::new(p.points.0 as f32, p.points.1 as f32).into()
        } else {
            let main = event_loop
                .primary_monitor()
                .map_or(1.0, |m| m.scale_factor());
            slint::PhysicalPosition::new(
                (p.points.0 * main).round() as i32,
                (p.points.1 * main).round() as i32,
            )
            .into()
        }
    });
    let opening = opening(&plan, asked, from_kept, place, scale, least, cfg!(windows));
    if let Some(note) = &opening.note {
        tracing::info!("{note}");
    }
    apply(app.window(), &opening);
    (opening.planned, maximized, full)
}

/// What the window is opened as, set before it exists.
#[derive(Debug, PartialEq)]
pub(crate) struct Opening {
    /// None for the layout's preferred size, which Slint applies as it
    /// creates the window.
    pub(crate) size: Option<slint::WindowSize>,
    pub(crate) position: Option<slint::WindowPosition>,
    /// The logical size it opens at, for the scale's correction.
    pub(crate) planned: (f32, f32),
    /// A line for the log.
    pub(crate) note: Option<String>,
}

/// The opening from the fit's `plan` of the size `asked` (the layout's
/// preferred, or a size `kept` from the last run in Slint's pixels),
/// a kept `place` as the window is created at, the run's `scale` and the
/// layout's `least`; `center` where the fit's own place is taken
/// (Windows).
pub(crate) fn opening(
    plan: &Plan,
    asked: (f32, f32),
    kept: bool,
    place: Option<slint::WindowPosition>,
    scale: Scale,
    least: (f32, f32),
    center: bool,
) -> Opening {
    match plan {
        Plan::Fit {
            opened, at: fit, ..
        } => {
            let centered = fit.filter(|_| center && place.is_none());
            Opening {
                size: Some(slint::LogicalSize::new(opened.0, opened.1).into()),
                position: place.or(centered.map(|p| slint::PhysicalPosition::new(p.0, p.1).into())),
                planned: *opened,
                note: centered
                    .map(|p| format!("window: placed at {},{} in the work area", p.0, p.1)),
            }
        }
        // Nothing fitted: the size asked for, which under the interface
        // scale is set smaller by its percentage, so the window covers
        // what it would at 100 % rather than that times the scale. A
        // kept size is in Slint's pixels already.
        Plan::Wayland | Plan::NoMonitor(_) => {
            let size = if kept {
                (asked.0.max(least.0).round(), asked.1.max(least.1).round())
            } else {
                scale::unfitted(scale, asked, least)
            };
            if !kept && size == asked {
                return Opening {
                    size: None,
                    position: None,
                    planned: asked,
                    note: None,
                };
            }
            let note = (!kept).then(|| {
                format!(
                    "window: at {:.0}x{:.0} under the interface scale",
                    size.0, size.1
                )
            });
            // On Wayland the window opens at 1 times the setting
            // (`scale::decide`), and the size goes in at that factor as
            // pixels. Slint converts a logical size at
            // `SLINT_SCALE_FACTOR` as it was at the start, which is this
            // factor only until the compositor's scale corrects it; the
            // pixels say the same whenever that correction lands.
            let set = if *plan == Plan::Wayland && matches!(scale, Scale::Times(_)) {
                let factor = scale.on(1.0) as f32;
                slint::PhysicalSize::new(
                    (size.0 * factor).round() as u32,
                    (size.1 * factor).round() as u32,
                )
                .into()
            } else {
                slint::LogicalSize::new(size.0, size.1).into()
            };
            Opening {
                size: Some(set),
                position: None,
                planned: size,
                note,
            }
        }
    }
}

/// The opening, set on the window before it exists: Slint keeps each
/// in the attributes it creates the window with.
pub(crate) fn apply(window: &slint::Window, opening: &Opening) {
    if let Some(p) = &opening.position {
        window.set_position(p.clone());
    }
    if let Some(size) = &opening.size {
        window.set_size(size.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARGIN: (f32, f32) = (MARGIN_WIDTH, MARGIN_HEIGHT);
    /// The layout's least today, which the last test reads.
    const LEAST: (f32, f32) = (560.0, 509.0);
    /// The preferred size, which the last test reads.
    const ASKED: (f32, f32) = (1500.0, 800.0);

    /// A monitor with no work area reported.
    fn m(size: (u32, u32), scale: f64) -> Monitor {
        Monitor {
            size,
            scale,
            work: None,
            opens_here: false,
        }
    }

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
        let mac = m((3024, 1964), 2.0);
        let main_display = m((3840, 2160), 1.5);
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
            plan(false, ASKED, &[m((0, 0), 1.0), mac], Scale::System, LEAST),
            alone
        );
        // A little USB display beside a 4K one: too small for the
        // least with the margins, it is passed over, and the window
        // opens as asked on the 4K.
        let usb = m((1024, 600), 1.0);
        let four_k = m((3840, 2160), 1.0);
        assert_eq!(
            line(&plan(false, ASKED, &[usb, four_k], Scale::System, LEAST)),
            "window: monitor 3840x2160 at 1.00x, opening at 1500x800"
        );
        // A 1366 x 768 secondary holds the least, and counts.
        assert_eq!(
            line(&plan(
                false,
                ASKED,
                &[four_k, m((1366, 768), 1.0)],
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
                &[m((800, 480), 1.0)],
                Scale::System,
                LEAST
            )),
            "window: monitor 800x480 at 1.00x, opening at 720x509"
        );
        let none = plan(false, ASKED, &[m((0, 0), 1.0)], Scale::System, LEAST);
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
                &[m((1920, 1080), 1.0)],
                Scale::Times(1.5),
                LEAST
            )),
            "window: monitor 1280x720 at 1.50x, opening at 1200x580"
        );
        let wayland = plan(true, ASKED, &[mac], Scale::System, LEAST);
        assert_eq!(line(&wayland), "window: Wayland, left to the compositor");
    }

    /// The plan where the platform reports a work area: the room is
    /// that less a title bar, not the monitor less the guess.
    #[test]
    fn the_plan_from_the_work_area() {
        let at = |size, scale, work: (u32, u32)| Monitor {
            size,
            scale,
            work: Some((0, 0, work.0, work.1)),
            opens_here: false,
        };
        let line = |monitors: &[Monitor], scale| plan(false, ASKED, monitors, scale, LEAST).line();
        // A 14-inch Mac with the Dock on the left (70 points) and the
        // menu bar (37): the width gives way, the height does not.
        assert_eq!(
            line(&[at((3024, 1964), 2.0, (2884, 1890))], Scale::System),
            "window: monitor 1512x982 at 2.00x, work area 1442x945, opening at 1426x800"
        );
        // The same with the Dock at its largest on the side.
        assert_eq!(
            line(&[at((3024, 1964), 2.0, (2768, 1890))], Scale::System),
            "window: monitor 1512x982 at 2.00x, work area 1384x945, opening at 1368x800"
        );
        // A 13-inch Air at 1440 x 900 with a large Dock at the foot:
        // shorter than the guess's 760, which ran under the Dock.
        assert_eq!(
            line(&[at((2880, 1800), 2.0, (2880, 1496))], Scale::System),
            "window: monitor 1440x900 at 2.00x, work area 1440x748, opening at 1424x700"
        );
        // A 1080p laptop's 48-pixel task bar: as asked at 100 %; at
        // 150 % a little more than the guess's 1200 x 580.
        let laptop = at((1920, 1080), 1.0, (1920, 1032));
        assert_eq!(
            line(&[laptop], Scale::System),
            "window: monitor 1920x1080 at 1.00x, work area 1920x1032, opening at 1500x800"
        );
        let laptop = at((1920, 1080), 1.5, (1920, 1032));
        assert_eq!(
            line(&[laptop], Scale::System),
            "window: monitor 1280x720 at 1.50x, work area 1280x688, opening at 1264x640"
        );
        // A work area larger than its monitor is the monitor.
        assert_eq!(
            line(&[at((1920, 1080), 1.0, (4000, 1032))], Scale::System),
            "window: monitor 1920x1080 at 1.00x, work area 1920x1032, opening at 1500x800"
        );
        // An empty one is none: the guess.
        assert_eq!(
            line(&[at((3024, 1964), 2.0, (0, 0))], Scale::System),
            "window: monitor 1512x982 at 2.00x, opening at 1432x800"
        );
        // The smallest room wins: the Mac's, with its Dock, over a 4K
        // display's, whichever reports a work area.
        let four_k = m((3840, 2160), 1.0);
        assert_eq!(
            line(
                &[four_k, at((3024, 1964), 2.0, (2768, 1890))],
                Scale::System
            ),
            "window: monitor 1512x982 at 2.00x, work area 1384x945, opening at 1368x800"
        );
    }

    /// Where the window goes in a work area: centered with its frame,
    /// from the area's own origin (a monitor left of the primary, a
    /// task bar at the top), and from the corner when it fills the area.
    #[test]
    fn the_window_is_placed_in_the_work_area() {
        // A 1366 x 768 laptop at 100 % with a 48-pixel task bar: the
        // fitted 1350 x 672 and its frame fill the area exactly.
        let laptop = Monitor {
            size: (1366, 768),
            scale: 1.0,
            work: Some((0, 0, 1366, 720)),
            opens_here: false,
        };
        let Plan::Fit { opened, at, .. } = plan(false, ASKED, &[laptop], Scale::System, LEAST)
        else {
            panic!("no fit");
        };
        assert_eq!((opened, at), ((1350.0, 672.0), Some((0, 0))));
        // The same with the task bar at the top.
        let top = (0, 48, 1366, 720);
        assert_eq!(place(top, opened, 1.0, (16.0, 48.0)), (0, 48));
        // A 1080p display left of the primary, its task bar at the
        // foot: the 1500 x 800 asked for, with its frame, centered.
        let left = (-1920, 0, 1920, 1040);
        assert_eq!(place(left, (1500.0, 800.0), 1.0, (16.0, 48.0)), (-1718, 96));
        // At 150 %: Slint's pixels times the scale, the frame in the
        // monitor's own.
        let laptop = Monitor {
            size: (1920, 1080),
            scale: 1.5,
            work: Some((0, 0, 1920, 1032)),
            opens_here: false,
        };
        let Plan::Fit { opened, at, .. } = plan(false, ASKED, &[laptop], Scale::System, LEAST)
        else {
            panic!("no fit");
        };
        assert_eq!((opened, at), ((1264.0, 640.0), Some((0, 0))));
        // The least, larger than the area, goes in at its corner.
        assert_eq!(
            place((100, 50, 400, 300), LEAST, 1.0, (16.0, 48.0)),
            (100, 50)
        );
        // No work area, no place: the guess leaves it to the system.
        let Plan::Fit { at, .. } = plan(false, ASKED, &[m((1366, 768), 1.0)], Scale::System, LEAST)
        else {
            panic!("no fit");
        };
        assert_eq!(at, None);
    }

    /// The monitor the window is placed on: the one the system opens
    /// it on when the size fitted to the smallest room fits there, else
    /// the smallest room's. A 1080p laptop at 150 % beside a 4K main
    /// display keeps the laptop's size and opens on the main one.
    #[test]
    fn the_window_is_placed_where_the_system_opens_it() {
        let laptop = |home| Monitor {
            size: (1920, 1080),
            scale: 1.5,
            work: Some((-1920, 0, 1920, 1032)),
            opens_here: home,
        };
        let main = |home| Monitor {
            size: (3840, 2160),
            scale: 1.5,
            work: Some((0, 0, 3840, 2112)),
            opens_here: home,
        };
        let placed = |monitors: &[Monitor]| match plan(false, ASKED, monitors, Scale::System, LEAST)
        {
            Plan::Fit { opened, at, .. } => (opened, at),
            other => panic!("{other:?}"),
        };
        // The primary larger: the laptop's size, centered on the main
        // display (1896 x 960 and a 24 x 72 frame in 3840 x 2112).
        assert_eq!(
            placed(&[laptop(false), main(true)]),
            ((1264.0, 640.0), Some((960, 540)))
        );
        // The primary smaller: it is the smallest room too.
        assert_eq!(
            placed(&[laptop(true), main(false)]),
            ((1264.0, 640.0), Some((-1920, 0)))
        );
        // No primary named: the smallest room's monitor.
        assert_eq!(
            placed(&[laptop(false), main(false)]),
            ((1264.0, 640.0), Some((-1920, 0)))
        );
        // A primary too small for the size fitted elsewhere (a little
        // display that does not hold the least): the fitted monitor.
        let little = Monitor {
            size: (800, 480),
            scale: 1.0,
            work: Some((0, 0, 800, 440)),
            opens_here: true,
        };
        let four_k = Monitor {
            size: (3840, 2160),
            scale: 1.0,
            work: Some((800, 0, 3840, 2112)),
            opens_here: false,
        };
        assert_eq!(
            placed(&[little, four_k]),
            ((1500.0, 800.0), Some((800 + 1162, 632)))
        );
    }

    /// The margins are the system's pixels: under an interface scale
    /// below 100 % they are as many of Slint's smaller ones as it takes,
    /// and above it they are not cut.
    #[test]
    fn the_margins_hold_under_a_small_interface_scale() {
        let mac = Monitor {
            size: (3024, 1964),
            scale: 2.0,
            work: Some((140, 74, 2884, 1890)),
            opens_here: false,
        };
        let big = (4000.0, 4000.0);
        let opened = |scale: Scale| match plan(false, big, &[mac], scale, LEAST) {
            Plan::Fit { opened, .. } => opened,
            other => panic!("{other:?}"),
        };
        // At 50 % Slint's pixel is the Mac's: 32 x 96 of them for the
        // 16 x 48 points of frame and title bar.
        assert_eq!(opened(Scale::Times(0.5)), (2852.0, 1794.0));
        // SLINT_SCALE_FACTOR=1 on the same panel: the same.
        assert_eq!(opened(Scale::ByHand(1.0)), (2852.0, 1794.0));
        // At 150 % the margins stay 16 x 48 of Slint's pixels.
        assert_eq!(opened(Scale::Times(1.5)), (945.0, 582.0));
        // The guess's margins likewise: 160 x 280 at 50 %.
        let bare = m((3024, 1964), 2.0);
        assert_eq!(
            match plan(false, big, &[bare], Scale::Times(0.5), LEAST) {
                Plan::Fit { opened, .. } => opened,
                other => panic!("{other:?}"),
            },
            (2864.0, 1684.0)
        );
    }

    /// With no monitor that holds the least, the largest room: one
    /// short of the margin both ways is no room at all, not a large
    /// one from two shortfalls multiplied.
    #[test]
    fn a_room_short_both_ways_is_not_the_largest() {
        let small = m((180, 200), 1.0);
        let none = m((1, 1), 1.0);
        assert_eq!(
            plan(false, ASKED, &[small, none], Scale::System, LEAST).line(),
            "window: monitor 180x200 at 1.00x, opening at 560x509"
        );
    }

    /// What the window is opened as, from the fit: the fitted size, the
    /// fit's own place on Windows alone, a kept place anywhere; and
    /// with nothing fitted the size as asked, held to the least, a kept
    /// one not shrunk again by the interface scale.
    #[test]
    fn the_opening_from_the_plan() {
        let fit = Plan::Fit {
            monitor: (1920.0, 1080.0),
            scale: 1.0,
            work: None,
            opened: (1400.0, 900.0),
            at: Some((10, 20)),
        };
        let o = opening(&fit, ASKED, false, None, Scale::System, LEAST, false);
        assert_eq!(o.size, Some(slint::LogicalSize::new(1400.0, 900.0).into()));
        assert_eq!((o.position, o.planned), (None, (1400.0, 900.0)));
        let o = opening(&fit, ASKED, false, None, Scale::System, LEAST, true);
        assert_eq!(
            o.position,
            Some(slint::PhysicalPosition::new(10, 20).into())
        );
        let o = opening(
            &fit,
            ASKED,
            true,
            Some(slint::PhysicalPosition::new(300, 40).into()),
            Scale::System,
            LEAST,
            true,
        );
        assert_eq!(
            o.position,
            Some(slint::PhysicalPosition::new(300, 40).into())
        );
        // Wayland, as the layout asks: Slint's own preferred size.
        let o = opening(
            &Plan::Wayland,
            ASKED,
            false,
            None,
            Scale::System,
            LEAST,
            false,
        );
        assert_eq!(o.size, None);
        // Under 150 %, the layout's size over the percentage, as pixels
        // at 1.5; a kept size already in Slint's pixels the same.
        let o = opening(
            &Plan::Wayland,
            ASKED,
            false,
            None,
            Scale::Times(1.5),
            LEAST,
            false,
        );
        assert_eq!(o.size, Some(slint::PhysicalSize::new(1500, 800).into()));
        assert_eq!(o.planned, (1000.0, 533.0));
        let o = opening(
            &Plan::Wayland,
            (1000.0, 600.0),
            true,
            None,
            Scale::Times(1.5),
            LEAST,
            false,
        );
        assert_eq!(o.size, Some(slint::PhysicalSize::new(1500, 900).into()));
        let o = opening(
            &Plan::Wayland,
            (1000.0, 300.0),
            true,
            None,
            Scale::System,
            LEAST,
            false,
        );
        assert_eq!(o.size, Some(slint::LogicalSize::new(1000.0, 509.0).into()));
    }

    /// A kept window applied to the editor's window: its size fitted to
    /// the monitor present, its place, and maximized. A run sized by
    /// `--window`, a capture, or keys sent keeps none and reads none.
    #[test]
    fn a_kept_window_opens_as_it_was_closed() {
        let app = crate::testing::window(1);
        let screen = kept::Screen {
            name: "eDP-1".into(),
            position: (0, 0),
            size: (1920, 1080),
            scale: 1.0,
        };
        let monitors = [m(screen.size, screen.scale)];
        let record = kept::Kept {
            width: 1700.0,
            height: 1000.0,
            at: Some([60.0, 30.0]),
            maximized: true,
            monitor: Some(kept::KeptMonitor {
                name: "eDP-1".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            full_screen: false,
        };
        let r = kept::restore(
            &record,
            false,
            std::slice::from_ref(&screen),
            &monitors,
            Scale::System,
            LEAST,
        )
        .unwrap();
        let place =
            r.at.map(|p| slint::PhysicalPosition::new(p.physical.0, p.physical.1).into());
        let o = opening(&r.plan, r.size, true, place, Scale::System, LEAST, false);
        apply(app.window(), &o);
        if r.maximized {
            app.window().set_maximized(true);
        }
        let size = app.window().size().to_logical(app.window().scale_factor());
        // 1920 x 1080 less the guess's 80 x 140; moved in to fit.
        assert_eq!((size.width, size.height), (1700.0, 940.0));
        assert_eq!(
            o.position,
            Some(slint::PhysicalPosition::new(60, 30).into())
        );
        assert!(app.window().is_maximized());

        let cli = |args: &[&str]| {
            let all = std::iter::once("greycard-ui").chain(args.iter().copied());
            crate::panel::startup::keeps_window(&crate::Cli::try_parse_from(all).unwrap())
        };
        use clap::Parser;
        assert!(cli(&[]));
        assert!(!cli(&["--window", "800x600"]));
        assert!(!cli(&["--snapshot", "x.png"]));
        assert!(!cli(&["--screenshot", "x.png"]));
        assert!(!cli(&["--keys", "down"]));
        let windows: kept::Windows = [("main".to_string(), record)].into();
        assert!(kept::Memory::of(false, &windows).kept.is_none());
        assert!(kept::Memory::of(true, &windows).kept.is_some());
    }

    /// What an event does with what the opening owes. A maximized start
    /// on X11 waits for the window to show, asks the maximize again at
    /// that event, reads neither, and then reads as any run: a plain
    /// maximized start records as before. Full screen waits for the
    /// window to show, by Slint's word on Wayland.
    #[test]
    fn what_an_event_does_with_what_is_owed() {
        let some = Some(true);
        // Nothing owed: every event read.
        assert_eq!(owed(false, false, true, some, true), Owed::Read);
        assert_eq!(owed(false, false, false, None, false), Owed::Read);
        // X11, maximized: hidden or not a showing event, wait.
        assert_eq!(owed(true, false, true, Some(false), true), Owed::Wait);
        assert_eq!(owed(true, false, false, some, true), Owed::Wait);
        assert_eq!(owed(true, false, true, some, true), Owed::Maximize);
        // Then the next event, nothing owed, is read: recorded as a
        // plain maximized run (`kept::tests`).
        assert_eq!(owed(false, false, true, some, true), Owed::Read);
        // Maximized and full screen: both at the showing event.
        assert_eq!(owed(true, true, true, some, true), Owed::MaximizeThenFull);
        // Shown by winit, not yet by Slint: the maximize, full later.
        assert_eq!(owed(true, true, true, some, false), Owed::Maximize);
        assert_eq!(owed(false, true, true, some, true), Owed::Full);
        // Full screen alone: Wayland's None is shown when Slint says so.
        assert_eq!(owed(false, true, true, None, true), Owed::Full);
        assert_eq!(owed(false, true, true, None, false), Owed::Read);
        assert_eq!(owed(false, true, true, Some(false), true), Owed::Read);
        assert_eq!(owed(false, true, false, None, true), Owed::Read);
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
