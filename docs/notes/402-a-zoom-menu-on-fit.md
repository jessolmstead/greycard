# §402. A zoom menu on Fit

*2026-10-23*

A tester asked for a menu of zooms beside Fit (pixls, 2026-10-22):
the keys cycle through the levels, and a new user does not know the
keys. The Fit button at the panel's foot gets a chevron strip at its
right edge, which opens a menu of Fit, 50%, 100%, 200%, and 400%; a
right-click anywhere on the button opens the same menu at the
pointer. A plain click still fits.

The menu is the help and import menus' pattern, a zero-sized
`ContextMenuArea` inside the button, and Slint's popup placement
clamps it into the window, so at the foot it opens upward over the
button rather than off the edge. Fit calls the view's reset; each
level calls the zoom callback the keys end in, about the view's
center in the same logical space, so 100% from the menu is Space's
1:1. The levels do not go through the keys' own toggle, which falls
back to Fit at the level already shown; a menu should not.

The shared panel button learned `has-menu` and a `menu(x, y)`
callback, off by default, so no other button changed; the tip area
learned to report a press, since a right press never reaches a
touch area's click. The keys take the focus before the menu shows,
as the other menus do, so a dismissal leaves the window's keys
working instead of leaving Space on the button, which the reviewer
caught and a test now covers.

## Not done

- No keyboard way to open the menu; the help and import menus have
  none either.
