# 121. The defringe finds its hue (2026-09-20)

§74 ported RawTherapee's `PF_correct_RT` and said in as many words what
it had left behind: the reference's hue curve, which lets a user say
which hues the pass may act on. Without it the pass acts on every hue,
so a red berry on a grey wall is a fringe, and the 27 percent of the
lighthouse frame it called "a lot to call fringing" was partly that.
This is the curve, as two windows.

**Two windows, not a curve.** Axial aberration puts one color in front
of the focus and its complement behind, so the hues that want naming
are two, and each of them wants a place, a spread and a strength: a
center in degrees, a width in degrees, an amount from 0 to 1. Three
numbers a slider can hold and a dropper can set, against a curve's
eight control points and an editor to draw them with. The purple
window is the violet-to-magenta side, the green one the other. Nothing
stops a user pointing both at the same hue, or opening one to the whole
circle, which is the escape hatch: a window of 360 degrees at amount 1
is the pass exactly as it was, and `--defringe-all-hues` on the CLI is
that in one flag.

**The hue is the deviation's, not the pixel's.** What the window reads
is `atan2(b - mean_b, a - mean_a)` in Oklab — the direction of the same
chroma deviation whose length the threshold already tests. That is the
right quantity and not the obvious one: a fringe on the shadow side of
a white twig is a pixel that is barely colored at all, and its own hue
is noise, while the direction it departs from its neighborhood in is
the fringe. It also means the two sides of one edge read as
complementary hues, which is why both windows earn their keep: the
violet band and the yellow-green band across the road from it are one
fault.

**The falloff.** A window is full strength over a plateau and
smoothsteps to nothing at its edge. The falloff is a quarter of the
width each side — so a 120 degree window is 60 degrees of plateau and
30 of shoulder either way — but never more than half of what is left
of the circle outside the window. That second clause is what makes the
width slider's last notch behave: with a fixed half-and-half split, a
width of 355 gave a hue two thirds of the way round the circle a
fiftieth of the pass and a width of 360 gave it everything, a cliff at
the end of the slider. With the clause, the plateau grows and the
shoulder shrinks from 240 degrees on, the two meeting at 60 each, and
the window reaches the whole circle without a step: the pass summed
over the hues is 0.75 of the width up to 240 and 1.5 of it less 180
from there, piecewise linear and continuous, which is what the test
checks. Below the full amount the neighborhood's chroma is mixed with
the pixel's rather than replacing it, which makes an amount a dial
rather than a switch; at exactly 1 it replaces, bit for bit as the
ungated pass did, and there is a test that holds the pass to the
numbers taken off it before the windows went in. Where the two windows
overlap the stronger one wins, not the sum: two windows may not ask for
more of the pass than there is.

**Where the defaults came from.** Measured, not guessed. An ignored
test (`measure_the_fringe_hues`, `GREYCARD_FRINGE_FRAME` a raw and
`GREYCARD_FRINGE_BOX` an optional box) prints the histogram of the
deviation's hue over the pixels the default threshold selects, weighted
by the length of the deviation. On the orchids (`5M0A5391.CR3`, §13's
axial corner) the distribution is cleanly bimodal; §74's 40x40 box at
5715,55 gives 280 and 290 on the violet side and 100 on the green one,
and the worst tenth of that box sharpens to 280 and 100. On the
lighthouse (`4Z4A3525.CR3`, §61's frame, §74's table) the whole frame
gives 110 and 120 on the green side, and the 200x200 box on the water
sparkles that §74 measured gives 320 to 340 on the magenta side.
Oklab's landmarks, for reading those: red 29, yellow 116, green 142,
cyan 204, blue 264, magenta 329.

So: purple centered at 310, green at 130, both 120 degrees wide, both
at amount 1. The purple plateau is 280 to 340, which is the orchids'
violet and the lighthouse's magenta whole; the green plateau is 100 to
160, which is the orchids' 100 and the lighthouse's 110 to 120 whole.
The oranges at 80 come out at 0.26 and everything below 70 at nothing,
and the cyans from 190 to 245 at nothing. The one measured bin the
windows only part-hold is the lighthouse sparkles' far side at 180,
which gets 0.26.

That the two centers land exactly 180 apart was not imposed; it is
what the measurement gave, and it is what an axial aberration ought to
give, one color in front of the focus and its complement behind. Worth
saying because the first pass at this put them at 305 and 140, 165
apart, which fit the peaks a little worse on both sides and had nothing
to recommend it.

**What the default does to the picture.** On the lighthouse the pass
moves 23.1 percent of the frame where it moved 27.3; on the orchids
26.0 where it moved 27.2. The frames are fringed nearly everywhere, so
the windows spare little of them — that is the honest reading, and on a
frame with one purple edge and a lot of red it would spare almost
everything. Against the ungated pass the full-size preview differs by
0.10 of 255 on average on the lighthouse and 0.04 on the orchids;
against the defringe off, by 0.21 where the ungated pass differed by
0.32, and by 0.23 where it differed by 0.27, so the windows do about
two thirds of the work on the lighthouse and six sevenths on the
orchids. At 1:1 on the water sparkles the magenta rims go under both,
and what the windows leave behind is a faint teal on the far side of
some of the specular points, whose deviation hue is past 180 and only
part-held; mean saturation of that crop, 14.2 percent off, 12.9 with
the windows, 11.7 ungated. Whether that teal is fringe or the water's
own color on a specular point is a judgment; the windows mostly leave
it, and the width slider reaches it. The saturated yellow flowers on
the orchids measure 80.74 percent either way, as they did before: the
pass was already leaving them alone, and the windows do not change that.

**The schema.** Six flat fields on `Lens` beside the three that were
there, `defringe_purple_center` and its width and amount and the same
for green, all under `serde(default)`, so no version bump — §48's rule
still holds, nothing changed meaning. The decision worth recording: a
sidecar written with the defringe on, before the windows, reads with
today's windows rather than with an all-hues window that would
reproduce what it looked like when it was saved. §74 called the missing
hue curve the fault and the all-hues pass its cost, so the windows are
the fix, not a new option to opt into, and an old file reopened is a
little less defringed than it was and nowhere more. The alternative —
defaulting old files to all hues — would have left every file written
this week carrying the bug forever, with nothing in the panel to say so.

**The panel and the dropper.** Under the Radius and Threshold sliders,
a Purple heading with center, width and amount, the same for Green, and
one Pick hue button at the foot in the place the other sections put
theirs. The center sliders are tinted with the Oklab hue circle the
grading's Hue slider already carries, so the number has a color under
it. One dropper, not two: a click goes to whichever window's center is
nearer the hue read, which is what a user means by clicking a fringe.
It runs through the existing pick path — `pick-started("Defringe")`,
`pick-pressed` — and reads the picture through the same GPU readback
the white-balance and mixer droppers use, twice: the usual 5x5 mean for
the point and a box as wide as the pass's own averaging window for the
neighborhood. Both are flat boxes where the pass takes a Gaussian and
the outer one is a couple of pixels wider than the pass's window;
neither changes the direction of the deviation, which is all that is
read off it. No white balance and no look between, unlike the curve and
mixer droppers, because the defringe runs on the base and the base is
what the sample is.

**A tool that has to switch its own pass off.** The first version of
the dropper read the developed picture as it stood, which is the
picture after the defringe — so on a fringe the windows already handle
there is nothing left to read, the residual sits under the floor, and
the dropper says "no fringe there". Nor was there any way round it: the
Pick button is dead with the defringe off, since the settings it sets
belong to a pass that is not running. So while this dropper is out, the
worker develops with the defringe off. The edit is untouched; only what
is handed to the worker is changed, by one function on the way to the
job, and the pass comes back when the dropper is put down or Esc is
pressed. That is right twice over: it is what the user needs to see to
aim, and it is exactly the pass's own input. The hint says so, and says
to zoom to 1:1 first — at fit, one screen pixel is several image pixels
and a three-pixel fringe is not there to click. A dropper in hand now
keeps its hint through a develop, too, which it had to, since this one
arms itself by asking for one.

**What it costs.** One more float plane while the pass runs: the
deviation is kept signed now, as `da` and `db`, where before only its
squared length was. The old peak was four planes, about 720 MB on
45 MP — §74 said five and 900 MB, which was a miscount and is corrected
here — and the new peak is five, about 900 MB. The gate is written over
the `da` plane once the hue has been read off it, which is what keeps
it at five rather than six, and the `bool` plane the old code used for
the fringing flags is gone, replaced by the gate's float; the count of
what cleared the threshold comes out of the gate pass a row at a time
rather than a second walk of the plane. Time on the 45 MP lighthouse is
0.24 s, against §74's 0.30 s, and the ungated pass measures the same
0.24 s today, so that is the machine and not the change.

**Left out.** No CLI flags for the windows themselves, only
`--defringe-all-hues`: six more flags for a tool whose whole point is
that you point it at a fringe and look. A per-window preview of which
pixels a window holds — the obvious next thing, and the same want as
the mask overlay — is not here. The dropper still reads the base
through the retouch, the local contrast, the dehaze and the sharpen,
which sit between it and the defringe in the worker; switching the
defringe off was the one that mattered, the rest move a deviation's
direction hardly at all, and turning the whole tail off for a dropper
would cost a develop nobody asked for. And the curve itself is still
not here: if a lens turns out to fringe in a third color, the windows
cannot say so, and that is when the curve becomes worth its editor.
