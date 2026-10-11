# §286. A tint is a blend, not a turn (2026-10-10)

The user tinted a mask on a warm autumn portrait at a hue of 236 (blue)
and 13 percent, to cool a figure lit by low sun, and the picture came
back deep-fried: the beige coat green, the blonde hair an acid yellow,
the skin magenta. The same tint on the whole picture does the same.
The tint did exactly what we designed in §83, and the design was
wrong.

**What §83 got wrong.** §83's tint *turned* the pixel's hue toward the
tint's, the short way round, by the share of the chroma the tint
answered for, `amount * target / ((1 - amount) * chroma + amount *
target)`. That share is what made a neutral take the tint's hue
outright, and it is also what broke a warm scene under a cool tint.
A near-neutral pixel's share is large even at a small amount, and its
hues are almost opposite the tint's, so the turn is a large fraction
of a gap near 180 degrees and the pixel is sent through every hue in
between. A near-neutral beige at hue 70 (chroma 0.03), 166 degrees from
the tint, turned a third of that gap at an amount of 0.13 and landed
at 125, which is green; a paler beige of 0.02 lands at 141. A
saturated orange at 60 turned 23 degrees, to 83, yellow (above the
floor the share is the amount itself). A red at 30 has the tint 154
degrees the *other* way, so it turned down through zero to 10,
magenta; a skin tone at 45 to 11. Each color was
turned toward the tint and went past every hue on the way there; the
sum of those is a picture whose colors have each been rotated by a
different amount in one of two directions. A real cooling filter does
none of that: a blue over an orange makes a duller orange, then grey,
then a blue.

**The rule.** The tint is now a blend in Oklab's a and b. The target
is the tint's unit direction times `max(chroma, floor(L))`, the same
floor bell §83 drew (`CHROMA * max(0, 1 - ((L - Lm)/Lm)²)`, 0.1 at
mid grey and nothing at black and at twice mid grey's lightness), and
the result is

    ab' = ab + amount * (target - ab)

A straight line in a and b from the pixel to the target. The hue
falls out of where the point lands; nothing turns.

**What it keeps, and what it changes.**

- An amount of nothing is a return, bit for bit, as before.
- A neutral at a full amount lands on the tint's hue exactly, at the
  floor's chroma; at half the amount, half the floor's chroma, still
  on the hue. The swatch beside Reset, which shows what a mid grey
  becomes, is unchanged.
- A neutral outside the bell, black or white or past either end, has
  a target of nothing and stays neutral.
- A color of the tint's own hue keeps its hue and rises toward the
  floor; one already above the floor is its own target and is not
  moved.
- A color at a full amount is the target outright: the tint's hue at
  the pixel's own chroma, so §83's bright sky, colored past the bell,
  still lands on the tint's hue with its chroma at a full amount.
- What changes is everything in between. A color far from the tint's
  hue loses chroma on the way, as the chord between it and the
  target passes nearer grey than either end; a color opposite passes
  close by grey and never through the hues between. At half the
  amount the sky's pixel is now the midpoint of the two, not its own
  chroma turned halfway.

The user's colors at 236 and 0.13, at mid grey's lightness: the beige
(chroma 0.03 at 70) to 0.014 at 83; the orange (0.12 at 60) to 0.089
at 61; the red (0.1 at 30) to 0.076 at 26; a skin tone (0.06 at 45)
to 0.040 at 41. Each is cooler by losing its warmth, and none moves
more than 13 degrees. An orange under a blue tint at every amount
from 0 to 1: wherever its hue is more than ten degrees from both
orange and blue, its chroma is a sixth of the orange's or less (a
quarter for a pale orange, below the floor, whose target is longer
than itself), and it reads green only below about an eighteenth,
0.007 on an orange of 0.12, where it is grey to the eye.

A red under a blue at a large amount does pass through a faint purple
on the way, the side of grey the chord falls on when the two hues are
less than opposite. That is what mixing red and blue light is, and at
chroma 0.03 at most for a red of 0.1, at hues of 270 to 330.

**Both paths.** `Tint::applied` in `tint.rs` and `tint_ab` in the
shader are the same three lines; the shader takes the direction off
the blended vector as `tint / length(tint)` where the CPU takes it off
the hue `from_vector` reads back, the same vector either way. `pick`
and `from_vector` needed nothing: the dropper reads the tint through
the same vector round trip and the same `mix_with`. The parity test's
note on what makes pixels steep loses its tint item: at 180 degrees
the turn picked a side by rounding, and the blend is continuous
there, so the item is gone rather than kept as history.

**The recipe.** `DEVELOP_RECIPE` goes to 2 (§278): every edit with a
tint makes another picture now, and the kept pictures made under 1
are never asked for again.

**Checked.** The user's frame exported headless at full size, under
the old rule and the new, measured in the exported sRGB as Oklab over
three patches (the coat's sleeve, the cheek, the backlit hair at the
shoulder). Untinted: coat chroma 0.066 at hue 56, cheek 0.086 at 31,
hair 0.105 at 85. The user's mask tint (236, 0.13), turned: coat 0.075
at 130, green; cheek 0.093 at 355, magenta; hair 0.107 at 98, acid.
Blended: coat 0.036 at 80, a cooler, greyer beige; cheek 0.045 at 23,
a cooler skin, no pink cast; hair 0.077 at 87, paler blonde. The same
tint global instead of on the mask reads the same on these patches,
and away from the figure the turn put a green cast on the road and
the trees and pushed the orange leaves to crimson, where the blend
dulls the leaves toward a browner red and the road toward a neutral
grey. With no tint the two builds' exports are identical to the
pixel. A tint near the scene's own hue still lifts: a mask tint at 50
and 0.3 takes the coat from 0.066 to 0.100 at 42 (0.102 at 41 under
the turn), the two rules 0.36 percent RMSE apart over the frame,
since near the tint's hue a blend and a turn are nearly the same
move. The shader against the CPU (§221's random check) passes on
lavapipe at the default seed and seeds 1 to 10, 400 edits each, and
at seeds 11 to 13 with 1000 edits each and every part but the tint,
the locals, the mixer and the mono left at its default; a shader tint
2 percent short fails 134 of 300 of those edits, up to 189 levels.

**Tests.** `tint.rs`: the identity bit for bit; a mid grey at a full
amount on the hue with the floor's chroma, and half at half; a
neutral at either end left alone and the bell symmetric; a red under
a tint at 210 blended along the straight line at four amounts, the
target outright at a full amount, and nearer grey than half its
chroma at half; a color of the tint's own hue keeping its hue and
lifted halfway to the floor at half, and one above the floor
untouched; an orange under a blue tint at 1001 amounts, at three
chromas, never more than ten degrees off both ends unless its chroma
is under a quarter of its own, and green only under a tenth; the
user's four colors at 0.13 each duller and within 15 degrees of its
own hue; past the bell a color landing on the hue at its chroma at a
full amount and on the midpoint at half; and the sidecar and vector
tests as they were. `finish.rs`: the red at half through `mix_with`
on the midpoint in a and b, its chroma under half its own. Replaced:
§83's "a saturated red turning the short way and keeping its chroma,
halfway at half", "a pale color giving way sooner than a saturated
one", and the sky's "halfway at half, its chroma kept".
