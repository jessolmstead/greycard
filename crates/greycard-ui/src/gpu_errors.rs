//! What the window's device does with a wgpu error nothing caught.
//!
//! wgpu sends an error raised outside every error scope to the
//! device's uncaptured handler, and the one it has by default panics.
//! The window's device is Slint's, and Slint 1.18 sets no handler of
//! its own, so we set this one. The engine's ops share the device from
//! the worker thread and catch their own errors in scopes, which are a
//! thread's own (greycard-gpu's `scoped`); what reaches the handler is
//! the main thread's: Slint's drawing and its surface, and the
//! viewport's renderer.
//!
//! One of those is not a fault. wgpu-core 30's `Surface::configure`
//! waits for everything submitted, then requires the queue to be
//! empty, which it can only fail to be when another thread submitted
//! during the wait (gfx-rs/wgpu#4214); it then gives up with
//! `GpuWaitTimeout`, "Failed to wait for GPU to come idle before
//! reconfiguring the Surface". Slint configures the surface on the main
//! thread on every resize, and whenever a frame's surface comes back
//! outdated, suboptimal or lost; a compositor that tiles resizes the
//! window on its own, from the moment it maps. The worker submits on
//! the same queue, and a develop that remakes the base (a white
//! balance, a demosaic, and the first develop of every file) is its
//! longest run of submits. A configure in the middle of one used to
//! end the process.
//!
//! The failed configure gives up before it touches the swapchain, so
//! the surface keeps the configuration it had, and any configure after
//! it whose wait the worker leaves alone succeeds. So we log it and
//! have Slint configure again, as soon as the event loop comes round.
//! Until then, after a resize, the window and its surface disagree on
//! the size: each frame Slint draws in the meantime fails validation,
//! its depth attachment the window's size and its color attachment the
//! surface's. Those are the failed configure's and not a fault of their
//! own, so they are let through too, and only while the configure again
//! is pending. Such a frame still reaches the screen, empty: femtovg
//! presents the surface texture after the failed submit, and wgpu-core
//! clears a texture nothing drew to before presenting it (`present.rs`),
//! so the window flashes black, or see-through under a transparent
//! alpha mode, until the configure again lands.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use greycard_gpu::wgpu;
use slint::ComponentHandle;
use wgpu::wgc::command::RenderPassErrorInner;
use wgpu::wgc::error::ContextError;
use wgpu::wgc::present::ConfigureSurfaceError;

use crate::App;

/// How long after a configure again that also failed we ask for the
/// next: a beat for the worker's run of submits to move on, rather than
/// a configure, and its wait on the GPU, on every turn of the event
/// loop while the run lasts. The first configure again goes at once.
const RETRY_AFTER: Duration = Duration::from_millis(50);

/// Set our handler on the window's device, from the window's thread.
/// `app` is the window whose surface a failed configure leaves at its
/// old configuration.
pub(crate) fn install(device: &wgpu::Device, app: slint::Weak<App>) {
    let window_thread = std::thread::current().id();
    let retry =
        Retry::new(move |pending, retrying, after| schedule(app.clone(), pending, retrying, after));
    device.on_uncaptured_error(Arc::new(move |error| {
        handle(error, &retry, std::thread::current().id() == window_thread)
    }));
}

/// What an uncaptured error is to us.
#[derive(Debug, PartialEq, Eq)]
enum Class {
    /// A `Surface::configure` that lost the race with the worker's
    /// submits: the surface is as it was, and configures again.
    ConfigureRaced,
    /// A frame of the window's drawn on a surface such a configure
    /// left at the old size, before the configure again.
    StaleFrame,
    /// Anything else, which ends the process as wgpu's own handler
    /// would.
    Fatal,
}

/// Which class `error` is. Read from the error's types, not its text:
/// wgpu wraps what wgpu-core returned in a [`ContextError`] naming the
/// call; a configure's is a [`ConfigureSurfaceError`], and a frame's
/// size mismatch a [`RenderPassErrorInner`] down its chain of causes.
/// The call's name is not read: wgpu-core raises `GpuWaitTimeout` from
/// a configure's wait and nowhere else, and a name a wgpu release
/// changed would bring the panic back with nothing to say so.
/// `stale` is whether a failed configure is waiting to be done again,
/// and the error was raised on the window's thread.
fn classify(error: &wgpu::Error, stale: bool) -> Class {
    let wgpu::Error::Validation { source, .. } = error else {
        return Class::Fatal;
    };
    let Some(context) = source.downcast_ref::<ContextError>() else {
        return Class::Fatal;
    };
    let raced = matches!(
        context.source.downcast_ref::<ConfigureSurfaceError>(),
        Some(ConfigureSurfaceError::GpuWaitTimeout)
    );
    if raced {
        return Class::ConfigureRaced;
    }
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(context);
    while let Some(e) = cause {
        if matches!(
            e.downcast_ref::<RenderPassErrorInner>(),
            Some(RenderPassErrorInner::AttachmentsDimensionMismatch { .. })
        ) {
            return if stale {
                Class::StaleFrame
            } else {
                Class::Fatal
            };
        }
        cause = e.source();
    }
    Class::Fatal
}

/// The handler's body; `on_window_thread` is whether the error was
/// raised on the thread the window draws on.
///
/// - A configure that lost the race is logged at warn and configured
///   again, through `retry`: nothing is wrong with the device or the
///   surface, and a later configure succeeds.
/// - A frame whose attachments differ in size while that configure
///   again is pending is logged at warn and let through: nothing is
///   drawn on it, and it is presented cleared, a flash of black until
///   the configure again mends the next. The same error with no
///   configure pending, or off the window's thread, is a bug and
///   panics with the rest.
/// - Every other validation error panics, as wgpu's own handler does.
///   One is a bug in what was asked of the device, ours or Slint's, and
///   whatever the call made is invalid from then on: going on would
///   repeat the error every frame and could show a wrong picture with
///   nothing said. Every other configure failure is here too: each is
///   a setting the surface refuses or comes after the old swapchain is
///   let go, and a second try would mend neither.
/// - Out of memory panics too. It comes from a resource the main thread
///   was making, Slint's or the viewport's, which is invalid from
///   then on, so every use of it after would be a validation error
///   naming the wrong cause; the panic names the right one. The
///   worker's own run out of memory in its scopes and fall back to the
///   CPU.
/// - An internal error (a shader the driver will not compile, a limit
///   of the system) panics: we know of no way to go on from one.
///
/// The panic carries wgpu's full text, and our panic hook writes it to
/// the log with the backtrace. A lost device never comes here: wgpu
/// reports it to the device-lost callback instead.
fn handle(error: wgpu::Error, retry: &Retry, on_window_thread: bool) {
    let stale = on_window_thread && retry.pending.load(Ordering::SeqCst);
    match classify(&error, stale) {
        Class::ConfigureRaced => {
            tracing::warn!(
                "the window's surface: {}; configuring it again",
                chain(&error)
            );
            retry.request();
        }
        Class::StaleFrame => {
            tracing::warn!(
                "a frame on the surface left at its old size: {}",
                chain(&error)
            );
        }
        Class::Fatal => panic!("wgpu error: {}", full_text(&error)),
    }
}

/// `error` and its causes on one line, for the warn.
fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut out = String::new();
    let mut next = error.source();
    while let Some(e) = next {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&e.to_string());
        next = e.source();
    }
    if out.is_empty() {
        error.to_string()
    } else {
        out
    }
}

/// wgpu's own text for `error`, which for an out of memory is the two
/// words alone: then the call and its cause after them.
fn full_text(error: &wgpu::Error) -> String {
    match error {
        wgpu::Error::OutOfMemory { .. } => format!("{error}: {}", chain(error)),
        _ => error.to_string(),
    }
}

/// The schedule a [`Retry`] is made with: given the two flags, and
/// how long to wait before the configure again.
type Schedule = dyn Fn(Arc<AtomicBool>, Arc<AtomicBool>, Duration) + Send + Sync;

/// One configure again at a time, however many failures ask for it.
/// `pending` is set when one is scheduled and cleared just before it
/// runs, so a configure that fails again schedules the next; and
/// `retrying` is set while one runs, so that the next, which follows a
/// configure again the worker raced too, waits [`RETRY_AFTER`] where
/// the first goes at once.
struct Retry {
    pending: Arc<AtomicBool>,
    retrying: Arc<AtomicBool>,
    schedule: Box<Schedule>,
}

impl Retry {
    fn new(
        schedule: impl Fn(Arc<AtomicBool>, Arc<AtomicBool>, Duration) + Send + Sync + 'static,
    ) -> Self {
        Self {
            pending: Arc::new(AtomicBool::new(false)),
            retrying: Arc::new(AtomicBool::new(false)),
            schedule: Box::new(schedule),
        }
    }

    fn request(&self) {
        if !self.pending.swap(true, Ordering::SeqCst) {
            let after = if self.retrying.load(Ordering::SeqCst) {
                RETRY_AFTER
            } else {
                Duration::ZERO
            };
            (self.schedule)(self.pending.clone(), self.retrying.clone(), after);
        }
    }
}

/// From the event loop, `after` from now: the handler runs inside
/// Slint's configure, on the main thread, where calling back into the
/// window is not ours to do. With no wait, on the event loop's next
/// turn: under weston on RADV that came after two frames, some 25 to
/// 70 ms, where the timer it replaced came after six, half a second
/// to a second and a half on a loaded loop.
fn schedule(
    app: slint::Weak<App>,
    pending: Arc<AtomicBool>,
    retrying: Arc<AtomicBool>,
    after: Duration,
) {
    let posted = slint::invoke_from_event_loop(move || {
        let run = move || {
            pending.store(false, Ordering::SeqCst);
            if let Some(app) = app.upgrade() {
                retrying.store(true, Ordering::SeqCst);
                configure_again(app.window());
                retrying.store(false, Ordering::SeqCst);
            }
        };
        if after.is_zero() {
            run();
        } else {
            slint::Timer::single_shot(after, run);
        }
    });
    if let Err(e) = posted {
        // The event loop has gone, and the window with it.
        tracing::debug!("configuring the surface again: {e}");
    }
}

/// What a configure again asks of the window.
trait Surface {
    fn size(&self) -> slint::PhysicalSize;
    fn scale_factor(&self) -> f32;
    /// The window told it is `size`: Slint's renderer configures the
    /// surface at that size.
    fn resized(&self, size: slint::LogicalSize);
    fn request_redraw(&self);
}

impl Surface for slint::Window {
    fn size(&self) -> slint::PhysicalSize {
        slint::Window::size(self)
    }

    fn scale_factor(&self) -> f32 {
        slint::Window::scale_factor(self)
    }

    fn resized(&self, size: slint::LogicalSize) {
        self.dispatch_event(slint::platform::WindowEvent::Resized { size });
    }

    fn request_redraw(&self) {
        slint::Window::request_redraw(self)
    }
}

/// Have Slint configure the surface again at the window's size, then
/// draw a frame on it.
///
/// A frame alone is not enough. After a resize whose configure failed,
/// Slint holds the new size and the surface the old. Where the driver
/// reports that swapchain outdated, Slint configures on the next
/// frame; on Wayland, where a surface has no size of its own, nothing
/// need report it, and every frame fails validation until something
/// configures again (seen under weston on RADV). So we send the window the
/// resize Slint's winit backend sends on a real one (its size made
/// logical, which the event makes physical again the same way), which
/// configures on the main thread there and then. It is not
/// `set_size`: that asks the window manager for a size, which could
/// take a maximized or tiled window out of its place. The event moves
/// nothing (the size is the one the window has); like any resize it
/// scrolls the focused item into view.
fn configure_again(surface: &impl Surface) {
    let size = surface.size();
    // Minimized on Windows: Slint keeps a zero size from its renderer,
    // and so do we, until the window comes back with its own resize.
    if size.width > 0 && size.height > 0 {
        surface.resized(size.to_logical(surface.scale_factor()));
    }
    surface.request_redraw();
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use wgpu::wgc::device::DeviceError;

    use super::*;

    /// An error as wgpu builds one for the sink: wgpu-core's error
    /// in a `ContextError` naming the call, and wgpu's description of
    /// the two, laid out as its `format_error` lays them out, which is
    /// what its own handler prints.
    fn validation(
        fn_ident: &'static str,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> wgpu::Error {
        let description = format!("Validation Error\n\nCaused by:\n  In {fn_ident}\n    {cause}\n");
        let context = ContextError {
            fn_ident,
            source: Box::new(cause),
            label: String::new(),
        };
        wgpu::Error::Validation {
            source: Box::new(context),
            description,
        }
    }

    #[test]
    fn a_configure_that_lost_the_race_is_recoverable() {
        let error = validation("Surface::configure", ConfigureSurfaceError::GpuWaitTimeout);
        assert_eq!(classify(&error, false), Class::ConfigureRaced);
        assert_eq!(classify(&error, true), Class::ConfigureRaced);
        assert_eq!(
            chain(&error),
            "In Surface::configure: Failed to wait for GPU to come idle before \
             reconfiguring the Surface"
        );
    }

    #[test]
    fn every_other_configure_failure_is_fatal() {
        for cause in [
            ConfigureSurfaceError::PreviousOutputExists,
            ConfigureSurfaceError::InvalidSurface,
            ConfigureSurfaceError::ZeroArea,
            ConfigureSurfaceError::Device(DeviceError::Lost),
        ] {
            let error = validation("Surface::configure", cause);
            assert_eq!(classify(&error, true), Class::Fatal, "{error}");
        }
    }

    #[test]
    fn out_of_memory_and_internal_errors_are_fatal() {
        let source = || -> wgpu::ErrorSource {
            Box::new(ContextError {
                fn_ident: "Surface::configure",
                source: Box::new(ConfigureSurfaceError::GpuWaitTimeout),
                label: String::new(),
            })
        };
        let memory = wgpu::Error::OutOfMemory { source: source() };
        assert_eq!(classify(&memory, true), Class::Fatal);
        assert!(
            full_text(&memory).starts_with("Out of Memory: In Surface::configure: "),
            "{}",
            full_text(&memory)
        );
        let internal = wgpu::Error::Internal {
            source: source(),
            description: "Internal error".into(),
        };
        assert_eq!(classify(&internal, true), Class::Fatal);
    }

    /// A frame's size mismatch as wgpu-core nests it, under the
    /// call's `ContextError`: the encoder's error, which names the
    /// pass's scope, then the mismatch. This stands in for wgpu-core's
    /// `CommandEncoderError::RenderPass` and the `RenderPassError` in
    /// it, which tests cannot build (its mismatch is private to
    /// wgpu-core); it prints and nests as they do.
    #[derive(Debug)]
    struct InAPass(RenderPassErrorInner);

    impl std::fmt::Display for InAPass {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("In a pass parameter")
        }
    }

    impl std::error::Error for InAPass {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    fn mismatch(fn_ident: &'static str) -> wgpu::Error {
        let extent = |width, height| wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        validation(
            fn_ident,
            InAPass(RenderPassErrorInner::AttachmentsDimensionMismatch {
                expected_location: wgpu::wgc::command::AttachmentErrorLocation::Depth,
                expected_extent: extent(1200, 900),
                actual_location: wgpu::wgc::command::AttachmentErrorLocation::Color {
                    index: 0,
                    resolve: false,
                },
                actual_extent: extent(1180, 880),
            }),
        )
    }

    #[test]
    fn a_frame_at_the_old_size_is_let_through_only_while_a_configure_is_pending() {
        // wgpu 30 raises an encoder's finish under "a CommandEncoder"
        // and the frame's submit, which fails on the same encoder's
        // error, under "Queue::submit": what the window's log showed.
        for call in ["a CommandEncoder", "Queue::submit"] {
            assert_eq!(classify(&mismatch(call), true), Class::StaleFrame);
            assert_eq!(classify(&mismatch(call), false), Class::Fatal);
        }
    }

    #[test]
    fn a_frame_at_the_old_size_off_the_window_thread_panics() {
        let retry = Retry::new(|_, _, _| {});
        handle(
            validation("Surface::configure", ConfigureSurfaceError::GpuWaitTimeout),
            &retry,
            true,
        );
        // On the window's thread, with the configure pending: dropped.
        handle(mismatch("Queue::submit"), &retry, true);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle(mismatch("Queue::submit"), &retry, false)
        }));
        assert!(caught.is_err());
    }

    #[test]
    fn a_race_asks_for_one_configure_until_it_runs() {
        // What the schedule was asked: the wait, and the flags it is
        // handed, as `schedule` would hold them until it runs.
        type Asked = Vec<(Duration, Arc<AtomicBool>, Arc<AtomicBool>)>;
        let asked = Arc::new(std::sync::Mutex::new(Asked::new()));
        let retry = {
            let asked = asked.clone();
            Retry::new(move |pending, retrying, after| {
                asked.lock().unwrap().push((after, pending, retrying));
            })
        };
        let raced = || validation("Surface::configure", ConfigureSurfaceError::GpuWaitTimeout);
        let waits = || -> Vec<Duration> { asked.lock().unwrap().iter().map(|a| a.0).collect() };
        // As `schedule` runs one: the flag cleared, the configure
        // again, during which the handler may be called.
        let run = |during: &dyn Fn()| {
            let (_, pending, retrying) = asked.lock().unwrap().last().cloned().unwrap();
            pending.store(false, Ordering::SeqCst);
            retrying.store(true, Ordering::SeqCst);
            during();
            retrying.store(false, Ordering::SeqCst);
        };
        handle(raced(), &retry, true);
        handle(raced(), &retry, true);
        assert_eq!(
            waits(),
            [Duration::ZERO],
            "one at a time, the first at once"
        );
        // The configure again races the worker too: the next waits.
        run(&|| handle(raced(), &retry, true));
        assert_eq!(waits(), [Duration::ZERO, RETRY_AFTER]);
        // That one succeeds; a later race goes at once again.
        run(&|| {});
        handle(raced(), &retry, true);
        assert_eq!(waits(), [Duration::ZERO, RETRY_AFTER, Duration::ZERO]);
    }

    #[test]
    fn anything_else_still_panics_with_the_text() {
        let retry = Retry::new(|_, _, _| panic!("nothing to schedule"));
        let error = validation(
            "Device::create_texture",
            ConfigureSurfaceError::InvalidSurface,
        );
        let caught =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(error, &retry, true)))
                .expect_err("a panic");
        let message = caught.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(
            message.contains("In Device::create_texture") && message.contains("Invalid surface"),
            "{message}"
        );
    }

    /// A window that writes down what it is asked.
    struct Fake {
        size: slint::PhysicalSize,
        asked: RefCell<Vec<String>>,
    }

    impl Surface for Fake {
        fn size(&self) -> slint::PhysicalSize {
            self.size
        }
        fn scale_factor(&self) -> f32 {
            1.5
        }
        fn resized(&self, size: slint::LogicalSize) {
            self.asked
                .borrow_mut()
                .push(format!("resized {}x{}", size.width, size.height));
        }
        fn request_redraw(&self) {
            self.asked.borrow_mut().push("redraw".into());
        }
    }

    #[test]
    fn a_configure_again_resizes_to_the_same_size_then_draws() {
        let fake = Fake {
            size: slint::PhysicalSize::new(1500, 900),
            asked: RefCell::new(Vec::new()),
        };
        configure_again(&fake);
        assert_eq!(*fake.asked.borrow(), ["resized 1000x600", "redraw"]);
    }

    #[test]
    fn a_minimized_window_is_only_asked_for_a_frame() {
        let fake = Fake {
            size: slint::PhysicalSize::new(0, 0),
            asked: RefCell::new(Vec::new()),
        };
        configure_again(&fake);
        assert_eq!(*fake.asked.borrow(), ["redraw"]);
    }

    /// On a real window, as far as one goes here: the testing
    /// backend's window keeps its size whatever it is told and its
    /// renderer's resize does nothing, so this checks only that the
    /// resize and the frame are taken without a panic, and that the
    /// window is not moved off its size.
    #[test]
    fn a_configure_again_on_a_real_window_does_not_panic() {
        let app = crate::testing::window(1);
        let before = app.window().size();
        configure_again(app.window());
        assert_eq!(app.window().size(), before);
    }
}
