# §436. Full screen on F11

*2026-10-26*

Issue #47 asked for full screen. F11 now toggles it on Linux and
Windows, in every view (grid, lightbox, loupe, Develop, Cull, and the
crop), outside a sheet as the panes' F6 to F8 are, and over the grid
too. A held F11 toggles once. A text field keeps it, as it keeps F8,
and types neither. The Fit button's zoom menu ends with Full screen
(F11), so it can be found.

## Not on a Mac

There the window's green button does it, and F11 is the system's show
desktop key. The key table gains `NOT_MAC`, beside `MAC_ONLY`: on a Mac
F11's chord is unbound, and its line is off the keys sheet. The menu
item stays, without the key, since the toggle works there too.

## Back as it was

Slint passes the toggle to winit, which restores the window on leaving:
on Windows the placement saved on entering (size, position, and
maximized), on X11 and Wayland the window manager's memory of it. Slint
follows winit's maximized state, and Windows reports a full-screen
window as not maximized. So we keep whether it was maximized on
entering and maximize again on leaving if the restore has not; a second
maximize is a no-op. When the platform entered full screen itself (the
green button), nothing is kept and nothing is put back. A Resized that
winit reports out of full screen spends what was kept, so leaving by a
window manager's key and entering by it again is not taken for ours.

Slint holds the layout's minimum off the window while it is full screen
and applies it on leaving. A minimum taller than the screen would push
the restored window past the screen's foot, the Windows concern the
side pane's `ScrollPane` comment names. Nothing here tests that: the
testing backend's toggle never touches the constraints.

## What was checked

The testing backend tracks full screen and maximized as window
properties, so the tests toggle by key and by the menu's callback and
read both back. They also simulate Windows' report: a maximized window,
unmaximized while full screen, comes back maximized. They do not reach
winit or a window manager. On Linux under KDE's Wayland session, F11
sent by `--keys` (which now names f11) took a 2250x1200 window to the
monitor's 3840x2160 and back to 2250x1200, and a maximized one to full
screen and back maximized at its 3840x2060. Under XWayland the size and
the maximized state came back too, but the window's reported size did
not change while full screen, so that run shows little. The Windows
placement restore and a Mac were not run.
