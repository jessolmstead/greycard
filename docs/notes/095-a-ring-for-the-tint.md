# 95. A ring for the tint (2026-09-19)

The roadmap's "the Tint tool needs a more precise way of choosing the
hue". §83's panel put the whole circle on one slider: 360 degrees
over about 190 pixels of track, near two degrees a pixel, and a step
of one degree that no drag could hit anyway. The track's painted hue
circle told you which way to go and then gave you no hand fine enough
to get there. So the section gets the control the hue wanted all
along, a ring, and keeps the sliders.

**It is the grading wheels' picture, not another one.** A wheel and a
tint ring are the same two numbers in the same two places — an angle
that is a hue, a distance from the center that is a strength — so
they are now one drawing and one geometry rather than two that agree
by hand. `draw_wheel` became `draw_hue_circle(hue, strength, n)`, the
three wheels and the ring all call it, and the press arithmetic that
lived inline in `on_grade_press` came out as a pair of pure
functions: `wheel_pick(x, y)` gives a hue and a strength from a place
in the picture, `wheel_place(hue, strength)` gives the place back.
The dead center — four percent of the radius, where the strength is
nothing and the hue is left as it was so a control put back does not
forget it (§26) — is in `wheel_pick` once and answers for both. The
UI side of the panel is where geometry usually goes untested; these
two are ordinary arithmetic on f32 and a test covers the compass (red
right, 90 up, y up as `grading.rs` and `tint.rs` both read it), the
dead center, the strength holding at one past the rim so a drag off
the edge does not jump, and the round trip at five hues and four
strengths. `CIRCLE_LIGHTNESS` and `CIRCLE_CHROMA`, 0.72 and 0.13, are
now named, and a second test pins them to the `hue-circle` gradient's
own stops: the ring's rim at 0°, 90°, 210° and 330° has to come out
`#e680a1`, `#c4a032`, `#00bad1`, `#d285cb`, the four the slider track
carries, or the two controls disagree by eye about what a hue looks
like. The Slint side is `CurveEditor` again, which already takes a
picture and hands back a press in 0 to 1 with y up — the fourth
control on it after the curve, the wheels and the tone equalizer.

**The drag, and the one history entry.** Press and move set both
numbers and redraw; the viewport follows live because the shader
reads the look off the panel at each frame, and only release calls
`view-changed`, so a drag across the ring is one step in history and
not forty, exactly as a wheel's drag is. A double click on the ring
calls `tint-reset`, the button's own callback rather than a second
one beside it: putting the tint back to nothing is one thing and it
should not have two implementations. The sliders stay and stay in
step both ways — a drag on the ring writes `tint-hue` and
`tint-amount`, which the sliders are two-way bound to, and a slider
writes the same two properties and redraws the ring through
`show_tint`, which now draws the picture as well as the swatch.

**The sliders after the ring.** The ring is the coarse hand and the
Hue slider the fine one, so its step is a tenth of a degree: the
arrow keys and the wheel move by that, and the reading carries one
decimal. Slint's `round(x * 10) / 10` drops a trailing zero and would
read "210°" at one place and "210.4°" at the next, a value box that
changes width as you drag, so the text is formatted in Rust —
`tint-hue-text`, set in `show_tint` beside the swatch and the
picture, which was already called everywhere the tint changes.

**Typed entry, not done.** Asked for if it were small and general in
`EditSlider`, and it is neither. A slider's `text` is a display
string its caller composes — "210.4°", "55%", "+0.35 EV", "3127 K" —
so a box that read it back would need a parser per slider or a second
callback to hand the number over, and the double click that a typed
box wants for "select all" is already the slider's reset. It is a
control worth having and it is its own piece of work, not a rider on
this one.

**The size.** 176 pixels across, centered, against the panel's 288 of
usable width. A full-width ring is what the enlarged grading wheel
does, but that one is opt-in and this one is always there: at full
width the section's own sliders fall off the bottom of a 950-pixel
window. At 176 the rim is about 0.64 degrees a pixel, three times
finer than the slider was, and the slider's tenth of a degree is
under it for the rest. The picture is drawn at 256 and scaled down —
nearer the size it is seen at than the enlarged wheel's 320, because
unlike a wheel it is redrawn on every move event of either slider and
those pixels are paid for on the UI thread. It is absent, not a blank
square, until a file opens and the window has drawn it, which is how
the wheels' row is guarded.

**Checked.** `5M0A3976.CR3` with a sidecar tint of 210.4° at 55
percent, the panel snapshotted: the marker sits left and below the
center, where 210 degrees with y up puts it, in the ring's cyan-blue;
the Hue reads "210.4°" and the Amount "55%"; the swatch beside Reset
is the dull teal a mid grey becomes there, the same color the ring
carries under the marker. `GREYCARD_UI_WHEEL=FILE` now dumps the last
hue circle drawn, the ring included.

**Not.** No second appearance for the ring — it is the wheels'
picture down to the marker's white dot and dark rim, which is either
consistency or a missed chance to say "this one is a tint" and can be
revisited if it reads as a fourth grading wheel. No ring on the
Masks tab that differs from the Develop tab's: the section is the
same section on both, as §83 left it.
