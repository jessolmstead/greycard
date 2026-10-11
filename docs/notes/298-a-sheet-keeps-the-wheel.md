# §298. A sheet keeps the wheel, and the export sheet's fit (2026-10-11)

Issue #21: on a short screen, with a watermark on, the export sheet ran
past what the tester could see, and a touchpad swipe over it zoomed the
picture behind.

## The wheel

Every sheet is a full-window Rectangle with a dimmed background and a
TouchArea that closes it on a click, and its card holds an empty
TouchArea of its own. Those took presses, but a TouchArea with no
`scroll-event` handler rejects a wheel event, and Slint then offers it
to whatever lies beneath: the viewport, which zooms about the pointer.
So a wheel over the dimmed surround, the card's header or buttons, or a
list already at its end reached the picture (or a pane, or the grid).

We put one TouchArea under every sheet and over everything else in the
window, there whenever `sheet-open()` is, that accepts every scroll.
One place covers all fifteen sheets and any sheet added to
`sheet-open()` later; a sheet's own scrolling lists and sliders are
above it and still take their wheel first. We did not give each sheet's
surround a handler, which would have to be remembered in each new sheet.
The one card that is not a sheet, the archive's progress card over the
window's foot, got a handler of its own, since the scrim is not there
while it shows with no sheet open.

The test sends PointerScrolled over each of the export, preset, import,
settings, archive, sync and match sheets, at the surround and at the
card, and over the export sheet's header, buttons and choices, and
checks the zoom and the picture's center after each single event: a
swipe out and its swipe back would cancel (1.25 x 0.8), which hid the
leak in a first cut of the test.

## The fit

Inside the window the export sheet already fits: since the watermark
work its card is the smaller of what it needs and the window's height
less a margin, the header and the buttons stay put, and the choices
between scroll with their bar. A test at about the shortest window
(520 px, the window's minimum height is about 509) with a text
watermark and the queue's row on shows every button inside the window,
Opacity below the fold until the choices are scrolled, and the header
and buttons not moving when they are. It passes on the code as it was,
so it guards the behavior rather than reproducing the report.

What the tester saw is most likely the window itself taller than the
visible screen: we open at the preferred 1500 x 950, and a 14-inch
laptop has less than that below the menu bar and above the Dock. The
sheet measures itself against the window, so by its measure everything
fits, the choices never scroll, and with a watermark's block the card's
foot falls off the visible screen. Without the watermark the card is
short enough to stay clear, which matches "trying to add a watermark."
This is not confirmed on macOS. The fix for it is to open the window no
taller than the screen's visible area. Slint does not report it, and
winit (reachable through Slint's winit accessor) gives a monitor's full
size, not its work area, so the cheap fix is a lower preferred height
that fits any 13 or 14 inch laptop, and the true visible frame needs
the platform (NSScreen's visibleFrame on macOS). It is left as its own
item, to be tried on a Mac.
