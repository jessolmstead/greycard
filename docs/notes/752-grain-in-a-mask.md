# §752. Grain in a mask (2026-11-26)

Grain was the picture's alone, and a tester's table of what a mask
can and cannot do put it among the few tools left out (pixls 61420).
A mask's look now has a GRAIN section with one slider, Amount, −100
to +100.

**The rule adds.** At each pixel the grain's amount is
max(A + k, 0): A the picture's Amount, 0 while its switch is off, and
k the masks' Amounts there, each by its weight and opacity, summed
and held to −1..1. So −100 takes all grain away under the mask
whatever the picture has, +30 over a picture at 70% is 100%, and a
mask at +50 over a picture with none grains at 50% inside it alone.

We first built it as a mask's Sharpen is, a share of the picture's.
But the picture's Amount rests at 0, and a share of nothing is
nothing; a second rule for that case made the mask's grain jump from
2% to 100% as the picture's Amount went from 1% to 0. One rule in
one unit has no such step: a sweep of A or of k through 0 stays
within one 16-bit level of the picture's own grain at A + k.

**One field.** The mask draws no grain of its own. The frame has one
field, at the Develop tab's Size and kind, drawn at A and scaled by
(A + k) / A, or drawn whole and scaled by k where A is 0. A feathered
edge is the same crystals growing fainter, with no seam. Size and
kind per mask are not built.

**Edits as they were.** With k at 0 the scale is exactly 1 and the
multiply is skipped, on the CPU and in the shader, so no picture
changes and the process version stands: 80 exports of edits with
masks and no grain Amount are byte for byte master's. One function,
`finish::masks_act_on_grain`, says whether any mask acts, and where
none does the finish does no work for it. The field is left out of
the file at its default, and lives only in a state at schema 10,
which 0.12 already steps aside for.

**Where it runs.** After the output sharpen, the display curve, and
the encode, as the picture's grain does. The GPU's is the CPU's to
the 8-bit read's own half level, and what the masks change agrees to
within 1% in slope, on lavapipe and on the card. A grey export takes
it to luma with the rest (§751). The Develop tab's Grain controls
now dim while its switch is off, as Detail's do.
