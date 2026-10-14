# §306. The window opens no taller than its screen (2026-10-11)

Issue #21, the fit §298 left open: we opened the window at its preferred
1500 x 950, nothing held that inside the screen, and a 14-inch MacBook
has about 845 px of content below the menu bar and the title bar and
above a Dock at its default size. The export sheet measures itself
against the window, so with a watermark's block its foot fell under
the Dock where it could not be seen and its choices never scrolled.

We did the two cheap steps and left the third.

## A lower preferred height

The window now asks for 1500 x 800. With the title bar's 28 that is
828, which fits a 14-inch MacBook Pro at its default scaling (1512 x
982, a 37 px menu bar under the notch, about 70 for the Dock) and a
13-inch Air from the M2 on (1470 x 956). The older 13-inch screens at
1440 x 900 have about 778 of content; the clamp below takes those.

We measured the layout in the test harness at 950, 850, 800 and 760
high. At 800 the viewport is 940 x 652 (a 3:2 landscape frame still
fills its width), the strip keeps its 148, the panel's footer sits
above it and the panel scrolls as it did, the grid scrolls its rows,
and the export sheet with a text watermark keeps its header and
buttons inside the window with Opacity below the fold until the
choices are scrolled, which is the behavior §298's test guards. The
layout's least, read from the window's own `min-width` and
`min-height` through two out properties, is 560 x 509.

Four tests had been written against the old default height without
saying so (the Masks tab's list and shape rows, and the Cull tab's
rejects buttons found by scrolling the panel); each now sets the
1500 x 950 window it was written at. None of them showed a fault at
800: the panel's lower rows are there when scrolled to, and the
testing helper's scroll step overshot a shorter panel.

## A clamp to the monitor

Slint gives us winit through the `unstable-winit-030` feature, which we
turned on; the winit backend was already the one in use (the wgpu
selection runs on it).

The fit runs before the window exists. A first cut fitted it once the
winit window was there, which is after Slint has shown it: the window
appeared at 1500 x 800 and jumped, on every monitor the fit changes
anything on (always on the 14-inch Mac it is for), and on macOS, where
winit centers a new window and a resize keeps its bottom-left corner,
a window centered at 1500 on a 1440 wide Air kept its left 30 px off
the screen. So we hand the backend a `CustomApplicationHandler`
(`BackendSelector::with_winit_custom_application_handler`) whose
`resumed` runs as the event loop starts, before Slint creates the
windows shown while it was not running. There we read the monitors
and call the window's `set_size`; with no winit window yet Slint keeps
the size in the window's attributes, so the window is created, and
centered, at the fitted size. There is no jump, and with no window
yet there is nothing maximized or full screen to leave alone.

The size asked for is the layout's preferred one, read through two
more out properties (`asked-width` and `asked-height`), not the
window's size: before the winit window exists Slint's window holds the
layout's least (showing it raises the stored 0 x 0 to the minimum),
and the preferred size is applied only as the window is created, and
only when no size has been set. A first build read the window's size
and so fitted 560 x 509, which changed nothing.

The monitor is the smallest of winit's available monitors, by logical
area, never the primary one: a new window opens on the focused screen,
which need not be the primary (a laptop with a main display beside
it), and the smallest fits it wherever it opens. Only a monitor that
holds the layout's least with the margins counts, so a 1024 x 600 USB
display beside a 4K one does not shrink the window to 944 x 509 there
(it opens at 1500 x 800); with none that holds it, the largest, which
for a single small screen gets the least. We divide a monitor's
physical size by Slint's `SLINT_SCALE_FACTOR` when that is set, since
`set_size` takes Slint's units, else by the monitor's own scale
factor. A monitor with no width or height counts as none. Then we open
at the size asked for, no larger than the monitor less 80 across and
140 down, no smaller than the layout's least. The 140 is a title bar
(28), a menu bar (37 under a notch) and a Dock at its default size
(about 70) with a little to spare; a task bar at the foot of a Windows
or Linux desktop is less. 80 across keeps a Dock on the side clear.

On Wayland we fit nothing and leave the 1500 x 800 to the compositor.
winit reports a monitor's scale there rounded up to a whole number
(125 % and 150 % both read 2), so a 1920 x 1080 panel at 125 % would
read 960 x 540 and the window open at its least; the window's
fractional scale is known only once the window exists, which is after
the point where a size can be set without a jump; Wayland names no
primary monitor and tells a client nothing of the work area; and the
compositor places and sizes the window anyway. The lower preferred
height is what keeps it inside a laptop's screen there.

The fit is a pure function, tested for a 14-inch MacBook (1500 x 800
opens at 1432 x 800; the old 950 would have been cut to 842), a
13-inch Air at 1470 x 956 and at 1440 x 900 (1360 x 760), a 1080p
laptop at 100 % (as asked) and at 150 % (1200 x 580), a 4K desktop at
100 % and 200 % (as asked, never grown), and a monitor too small for
the layout (1024 x 600 opens 944 x 509, 640 x 480 at the least). The
choice of monitor and the log's line are a pure function too, tested
with the Mac alone and beside a larger main display (the Mac wins), a
monitor with no mode skipped, none at all, `SLINT_SCALE_FACTOR` over
the monitor's scale, and Wayland. Below the least the least wins: the
window runs past the screen's foot as any window with a minimum does,
nothing scrolls the window itself, and the user moves or resizes it. A
sheet inside still fits the window, which is what it measures.

Nothing remembers the window's size between runs, so the clamp has
nothing of the user's to fight; if a remembered size is ever added it
should pass through the same fit. A rotated portrait monitor reads as
landscape (winit gives the mode's pixels unrotated), which we leave.
With no monitor at all the window keeps the 1500 x 800 it asked for;
with every available monitor asked, that should be rare.

The log says what happened, one line at info:

    window: monitor 1512x982 at 2.00x, opening at 1432x800

or `window: no monitor reported, opening at 1500x800`, or
`window: Wayland, left to the compositor`. That is the line to read on
the Mac. We ran the editor under a headless weston at 1440 x 900, with
no file and a private configuration, once on Wayland and once on X11
through weston's Xwayland, and the lines were:

    window: Wayland, left to the compositor
    window: monitor 1440x900 at 1.00x, opening at 1360x760

Captures with `--snapshot` are now 1500 x 800 rather than 1500 x 950,
or smaller on a short monitor, so a `--panel-scroll` offset chosen for
a §101-style capture of the panel at the old height may land on
different rows.


## Left open: the true work area

The margin is a guess at the menu bar and the Dock. The platform knows
exactly: on macOS `NSScreen`'s `visibleFrame` (the frame less the menu
bar and the Dock, wherever the Dock is and however large; reached
through `objc2-app-kit`'s `NSScreen`, which the Finder's open already
puts `objc2` in the tree for, matched to winit's monitor by its
`CGDirectDisplayID`), on Windows `SystemParametersInfoW` with
`SPI_GETWORKAREA` for the primary monitor or `GetMonitorInfoW`'s
`rcWork` for the one the window is on (`windows-sys`, already a
dependency, wants its `Win32_Graphics_Gdi` and
`Win32_UI_WindowsAndMessaging` features), and on X11 the window
manager's `_NET_WORKAREA`. Wayland has nothing portable: the
compositor places and sizes the window, and a layer shell's
exclusive zones are not reported to a client. Each would replace the
margin in the fit on its platform; the pure function stays as it is.
We would build the macOS one first, and only if the Mac check below
shows the margin wrong (a large or side Dock, a hidden Dock, a scaled
display).

## Checked on a Mac, 2026-10-13

A 14-inch MacBook with the Dock at the foot: the log read `window:
monitor 1512x982 at 2.00x, opening at 1432x800`, the window opened
with no jump and its foot clear of the Dock, and the export sheet's
buttons were in view with a text watermark on. The true work area
(above) goes to the roadmap's pool for the large or side Dock the
guess does not cover.

## To check on a Mac (as written before the check)

Open the editor on a 14-inch MacBook with the Dock at the foot and
read the log's `window:` line: the monitor should read 1512x982 at 2.00x
and the window open at 1432x800, its foot clear of the Dock. Then
open Export with a text watermark: the buttons should be inside the
window, the choices scrolling. With a larger Dock, or "More Space"
scaling (1800 x 1169), the line says what the fit chose.
