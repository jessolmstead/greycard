# 64. The viewport's keys, its zoom readout and its dropper (2026-09-18)

The shortcuts had quietly died. `keys`, the FocusScope that carries
space, Z, S, J, the arrows and ctrl+Z, is a sibling of the
VerticalLayout that holds the whole window, and `forward-focus: keys`
gave it the focus at startup. That worked until the first slider was
touched: `EditSlider` calls `fs.focus()` on press so the wheel and the
arrow keys can move it (§19), and from then on the focus lived inside
the panel. Slint delivers a key to the focused item and bubbles a
rejected one only to ancestor FocusScopes, and `keys` is nobody's
ancestor, so a rejected key fell off the end. Only clicking still
zoomed, because that is a pointer event.

Wrapping the layout in `keys` would have fixed it and re-indented
2300 lines under five branches in flight. Instead the pointer hands
the keys back: the viewport's TouchArea focuses `keys` when hover
arrives (`changed has-hover`, Slint 1.17) and on any press, and so do
the navigator's press and the filmstrip's click. A slider clicked on
the panel keeps focus while the pointer stays there, so §19 stands;
the picture takes the keys the moment the pointer is over it. The
sheets are full-window overlays with their own dismiss areas, so no
LineEdit in them can be robbed this way.

Two smaller things from the same region. The TouchArea covers the
letterbox too, and a click beside a fitted portrait frame zoomed to
1:1 on a point outside the picture; `toggle_zoom` now maps the click
into frame pixels with the transform the crop overlay uses and does
nothing outside `0..image_size`. And the zoom was invisible: it is a
muted reading at the right end of the NAVIGATOR header ("Fit", or
"100%", "300%", "67%"), set from the one place the effective zoom is
settled per frame, the rendering notifier, so it cannot go stale.
`Section` grew a `note` property for it, an absolutely positioned
Text in the header's stretch; a before-and-after snapshot of the
whole window differs by 54 pixels, all inside the 17x12 box where
"Fit" now is.

The picker has a dropper. Slint 1.17 has no custom cursor images, so
while `picking` is set the cursor is `MouseCursor.none` and Lucide's
pipette is drawn at the pointer the way the brush circle is: white,
with four black copies offset a pixel behind it so it reads on sky
and on shadow alike, its tip at (2, 22) of the icon's 24 box.
Measured against a marker drawn at the exact point, the tip lands
within a pixel. If the icon fails to load the cursor stays a
crosshair. Ctrl+Shift+E opens the export sheet, gated as the Export
button is and accepting the key either way so it cannot stack a
second sheet.
