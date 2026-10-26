# §432. The slider and natural scrolling on a Mac

*2026-10-26*

On a Mac a focused slider still moved down for a two-finger swipe up,
with Shift held or not. macOS scrolls naturally by default, content
following the fingers, so a swipe up arrives as a scroll down: a
negative vertical delta, which Slint's winit backend passes through
unchanged. Linux and Windows wheels never turn it.

§427 assumed a Mac turns Shift with a vertical swipe into a sideways
delta. A mouse wheel under Shift does; the trackpad does not, its delta
stays vertical, so that flip never fired on the trackpad. Its signs only
matched what the user saw because they cancelled the vertical inversion,
the real cause. In winit's convention a positive delta moves content
right or down, and macOS keeps the sign when Shift turns a wheel
sideways, so the flip stays in shape with both signs reversed: Shift
with a sideways delta reads it as the vertical it was, and a sideways
delta without Shift reads a move right as up, under either setting where
it is read, and for any wheel. The mouse wheel under Shift is still to
be confirmed on the Mac.

We now read, once at the start, the global preference
`com.apple.swipescrolldirection` through CFPreferences: true or absent
is natural, absent being the default. It goes to the sliders' global,
and under it a slider turns its delta back on both axes, so it moves the
way the fingers went. The log says so at INFO. Lists, the panel, and the
picture keep the system's way: there content following the fingers is
right, and only a slider is a value rather than content. Other platforms
read false.

One test scrolls a focused slider both ways with the setting off and on,
with Shift sideways and plain sideways under it too. The reader is Mac
only and has no test; it checks for aarch64-apple-darwin.

Not done: Windows precision touchpads and GNOME touchpads default to
natural too, and the flag reads false there; their setting is not read
yet.
