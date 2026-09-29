# 75. The perspective guide (2026-09-18)

The next step §49 named: two strokes along lines that should be
vertical, and the tilt and the turn together. `Geometry::guided_by`
takes the two strokes as their ends in source pixels, a [`Guide`]
axis and the source's size, and gives back the turn and that axis's
keystone, both the value the slider takes rather than a correction to
it.

**The solve, against `to_source`.** The two lines meet at a vanishing
point. Every plane point at infinity along (0, 1) reaches one source
point: `to_source` of `t(0, 1)` is `t(0, 1) / (1 + k.y t)`, which
tends to `(0, 1 / k.y)` before the matrix, so the matrix untwisted,
the vertical vanishing point is `(0, 1 / k.y)` — and `k.x` is not in
it. So the turn is the one that puts the vanishing point on the
plane's own vertical axis, and the tilt the one whose `perspective`
row has the rest of it for its reciprocal: `tan v = −(ph/2) / u.y`,
which is the row's convention of §49 read backwards, the half side and
the sign and all. The horizontal axis is the same with the parts
swapped, `tan h = (pw/2) / u.x`, so the two guides fall out of one
function and the keystone already on the other axis is left alone,
since neither vanishing point depends on the other's row. Each stroke
is the line through its ends with a unit normal, so the cross product
of the two is the sine of the angle between them and the arithmetic
keeps its scale whatever the pixels; it is all in f64, since a tilt of
a degree puts the vanishing point fifty plane heights away. The
matrix's fixed part — the mirror, then the quarter turns backwards —
comes off the vanishing point before the turn is solved for, which is
all `guided_by` reads from the geometry it is called on; the turn is
taken the way round that turns least, a line being the same line
either way up.

Two ends. Lines already parallel have no vanishing point: the third
part of the cross product is then zero, the division gives a tilt of
exactly nothing, and only the turn is set — which is what the level
tool would have done with one of them. Lines that cross within a pixel
of the center would be a tilt of a quarter turn and are refused, as
are a stroke shorter than a pixel and two strokes on the one line.

**In the window.** Two buttons, Verticals and Horizontals, under the
PERSPECTIVE heading in the GEOMETRY section, beside the sliders they
set; either turns crop mode on as Level does, so the picture is shown
whole while the correction spreads it. The overlay is the leveler's,
grown a second stroke: the first stays drawn in the accent color
while the second is traced, a plate at the top says which is wanted,
the cursor is the crosshair the level tool uses, Escape drops the tool
(and now drops the level tool too, which had no way out), and the
second stroke's release sets the two sliders and lets go. The stroke
ends go back to source pixels through `view_to_source`, the masks' and
the dropper's way in, so the answer does not depend on what the
picture has already been turned or keystoned by, and the tool run on
its own result says the same thing again.

**Where the first stroke lives.** It began in the overlay, in view
pixels, read only when the second landed. That is wrong twice over.
The guider is `if root.guide-mode != ""`, so switching Verticals to
Horizontals does not recreate it and the half-finished vertical stroke
survives to be paired, under the wrong axis, with a horizontal one.
And between the two strokes the view can move: the guider binds no
`scroll-event`, so the wheel reaches the viewport's; it focuses `keys`,
so space and Z zoom; nothing cleared the tool on a file change, and the
source's size goes with the file. A stroke kept in view pixels is then
read against a view, or a picture, that is not the one it was drawn on.
Both are now settled in Rust: `Guiding` holds the first stroke in
source pixels, converted at its own release, with the axis it was drawn
for, and refuses to pair across axes; the overlay keeps only the stroke
in hand and draws the kept one from `guide-kept`, two points Rust maps
back through `source_to_view` every frame, so a zoom or a pan moves the
line with the picture rather than under it. Which of the two a stroke
is is Rust's to say, not the overlay's, so there is no window in which
a fast second stroke could be taken for a first. A pair that says
nothing — the same line twice, or two lines crossing at the center —
keeps the tool in hand and says so on the status line, rather than
dropping it silently with nothing changed.

**Three sections on the Crop tab.** One GEOMETRY section held the crop,
the turn and the keystone, with a PERSPECTIVE text heading inside it;
once the guide's two buttons sat under that heading it read as a
section inside a section, with the aspect and the orientation stranded
below it. It is CROP, ROTATE and PERSPECTIVE now, three `Section`s that
fold on their own (`crop`, `rotate` and `perspective` in the settings'
`collapsed` list; a file that folded `geometry` folds all three, and
one that names neither leaves them open). Reset moves to the end of the
last section as "Reset all": it puts back the whole of the tab, and a
plain Reset sitting in any one of the three reads as that one's. The
icons are Lucide's crop and rotate-cw, and `keystone.svg`, a trapezoid
drawn here in Lucide's conventions because Lucide has none that says a
leaning wall; the icons' LICENSE says so, since the rest of that
directory is ISC. And the same day, on every tab: Fit and Export sat
at the end of the panel's scroll, so a long tab hid them; they are a
bar beneath the scroll now, and the sections scroll behind it.

Not done: four strokes, two of each, for both keystones and one turn
between them. Each axis's pair settles the turn on its own, so a
Horizontals run after a Verticals run moves the turn to what the
horizontals ask; a four-line solve would have to weigh the two, and
nothing yet says how.

**Checked.** Over a grid of turns (−10 to 10) and tilts (−30 to 30 on
either side of nothing), on the plain geometry and under a quarter
turn, a mirror, and three quarters and a mirror together, with the
other axis's keystone set to 12 degrees throughout: two plane
verticals spanning three fifths of the plane, projected through
`to_source` at 6000 by 4000, come back as the turn and the tilt that
made them to 1.3e-5 degrees, and the strokes' ends then land on one
plane x to a four-thousandth of a pixel. The same for two horizontals
through the horizontal guide. A hand costs more than the arithmetic:
strokes over a seventh of the plane with both ends nudged up to two
pixels are out by 0.28 degrees, over three fifths of it by 0.072, and
the error is linear in the nudge and inverse in the span — worth
saying in the hint if it ever matters, but a stroke drawn along a
visible edge is better than two pixels.

In the window, on `5M0A8354.CR3` (the lantern-lit doorway, R6 II, 24
MP): two lines that are vertical in the plane of a known turn of 4 and
tilt of 18 degrees, projected to the source, mapped to view pixels and
handed to the overlay's callback, set the sliders to 4.0000 and
18.0000 and the tool let go of itself; the two lines then stand on one
plane x to a four-thousandth of a pixel. The doorway's own two reveals,
measured off the frame at five heights, give a turn of 0.432 degrees
and a tilt of 0.144 — the facade was shot very nearly plumb — and give
the same two numbers to five figures whether the view they are drawn
on is straight or already leaning by that 4 and 18, which is the point
of reading the strokes back through the geometry in force.

The two bugs above were caught in review, not by the tests, and both
were checked in the window afterwards. With the tool armed for
Verticals, one stroke drawn, a zoom to 3:1 and a re-center, and the
second stroke drawn there, the sliders still read 4.00001 and 18.000004
against the ground truth of 4 and 18; the stale view-pixel reading of
the first stroke's upper end would have been (2068.5, 977.0) instead of
(2235.2, 939.8), 171 source pixels out. With a stroke drawn for
Verticals and then Horizontals pressed, the second stroke does not
complete a pair: no slider moves, the tool stays in hand and the new
stroke becomes the first for its own axis. Two strokes on one line
leave the sliders alone and put "the guide needs two different lines"
on the status plate. `Guiding` has a unit test of its own for all
three, since it is a plain struct with no window in it.

What did not pay: nothing was tried and dropped, but two things were
considered and left. Fitting more than two points per stroke would
buy accuracy a drag cannot deliver, since the hand, not the
arithmetic, sets the error. And the turn is absolute rather than added
to the turn in force, unlike `leveled_by`: a correction would make
the tool disagree with itself when run twice.
