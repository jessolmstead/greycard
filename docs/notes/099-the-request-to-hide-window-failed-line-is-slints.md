# 99. The "request to hide window failed" line is Slint's (2026-09-19)

We see this on stderr on quitting, on multiple runs:

    Slint winit backend: request to hide window failed because
    references to the window still exist. This could be an
    application issue, make sure that there are no
    slint::WindowHandle instances left

It is not an application issue. The line comes from the winit
backend's `suspend`, which takes the window out of the adapter and
tries `Arc::into_inner` on it; anything else holding the `Arc` makes
that fail and prints this. Every handle to the app in `main.rs` is a
weak one, the rendering notifier upgrades one per frame and drops it,
the portal file chooser passes no parent handle, and the wgpu
surface, which does hold a clone of the window, is dropped by the
femtovg-wgpu renderer's `clear_graphics_context` before the check.
An `--export` run here, which quits through `quit_event_loop`, ends
without the line.

What holds the window is Slint itself: since 1.18.0 the event loop
keeps a reference to the window while it dispatches the close event,
so the check fires on every window closed by its close button, and
the window is also left registered and keeps receiving events. The
fix is slint-ui/slint#13507, merged 2026-09-19, which unregisters the
window in any case and does the check once the event is done with
it. No release carries it yet; 1.18.0, which §91 moved to, is the
newest on crates.io. So: harmless, cosmetic, and it goes away with
the next Slint point release, which the roadmap's Upstream list now
waits on. Not worth a git dependency on Slint's master for a line on
stderr.
