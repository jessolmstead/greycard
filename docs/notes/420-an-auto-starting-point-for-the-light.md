# §420. An Auto starting point for the light

*2026-10-24*

**Auto** in the Look section sets Exposure, Highlights, Shadows, Whites,
and Blacks, and nothing else, as one history step under its name. It is
a pure function of a histogram and the baseline (`greycard_edit::auto`),
so a second press changes nothing. Like a preset, it is refused on a
panel still showing a stand-in.

**The read.** The grid of the developed scene the Auto white balance
already reads back (512 points across, linear Rec.2020 before exposure
and the display curve). Each point's luminance and its brightest channel
are binned in sixteenths of a stop; nothing is developed again.

**Exposure.** It brings the trimmed mean of log luminance (10th to 90th
percentile) toward 1.5 stops under mid grey before the baseline. The
brief's 0.18 rendered our sample set's ordinary frames a stop bright:
their middles sat 0.6 to 2.2 stops under grey at a default we already
judge right, so we put the target among them. To keep a dark frame's
key, a lift is whole up to a stop and half past it (a middle three stops
under takes +2 and ends a stop under), all held to ±2. A frame lit at
night, its 99th percentile within a stop of white and its median more
than two and a half stops under the target, rises a stop at most. A lift
also stops where the 90th percentile reaches 1.95 stops under display
white, where the curve gives 0.8 encoded. A cut never takes the 25th
percentile more than a stop under the target: a backlit frame's subject
is its dark part, and its middle is the sky. The cut refused goes to
Highlights, as a hand edit would take the sky down, and only what the
top asks past Highlights' limit comes off the exposure.

**The tone pair and the clipping pair.** Highlights pulls by half of how
far the 97th percentile sits over 0.75 stops under white, Shadows lifts
by a quarter of how far the 3rd sits under five stops under grey, each
one way only and to half its slider. Whites puts the 99.5th percentile a
tenth of a stop under white through the white point's own power; a
stretch comes in with the frame's range (from nothing at one stop to
whole at three), so a grey card stays grey. Whites is held so no more of
the frame reaches white than at the default, read on each pixel's
brightest channel: a saturated light reaches white by its brightest
channel long before its luminance does, and held by the luminance alone
a portrait's out-of-focus lights went from none at white to 0.9 percent.
Where the default clips nothing it is held from the 99.95th percentile
with a tenth of a stop to spare, not from the brightest point, which
moves with where the grid falls; that leaves 0.02 to 0.08 percent more
at white than the default on three of the frames we measured. A frame
the default clips is never stretched. Blacks never crushes past the
darkest light the default leaves.

**Measured** on the editor's grid (columns and rows from half a step),
1200-pixel JPEG exports, mean display-linear Y and share at white,
default to Auto:

| scene | Auto | mean Y | at white |
|---|---|---|---|
| snowy alpine valley | -0.10, Highlights -0.43, Whites +0.58 | 0.241 to 0.227 | 0 to 0.02% |
| portrait in a field | +0.28, Whites -0.29 | 0.190 to 0.220 | 0 to 0 |
| couple against an overcast sky | -0.04, Highlights -1 | 0.368 to 0.326 | 0.96 to 0% |
| lighthouse, under-exposed | +1.30, Shadows +0.10, Whites -1.43 | 0.079 to 0.143 | 0.01 to 0.01% |
| night street under lanterns | +1.00, Highlights -0.33, Shadows +0.09, Whites -1.35 | 0.054 to 0.076 | 3.72 to 3.62% |

The night street stays night: the night rule holds it at +1.00, where it
took +1.71 (mean Y 0.103) before the rule. The candlelit church is held
by the 90th-percentile cap at +0.83 (mean Y 0.090 to 0.099, white 6.0 to
0.5 percent). The shadows' hold moved four frames, all bright ones: the
couple's faces hold near the default while the sky comes down; sea
cliffs under an overcast sky went from -0.65 to 0 with Highlights -1
(mean Y 0.386 to 0.353, white 1.74 to 0.04 percent); a rocky coast under
a hazy sky from -0.89 to -0.34 with Highlights -1 (0.355 to 0.270).

**Known limits.** A low-key interior lit only by a watch's lume has no
lights near white, so it goes the full +2 (mean Y 0.005 to 0.067) and
becomes an ordinary interior: from luminance alone it is an underexposed
frame, and holding it back would hold back every underexposed frame.
Auto reads the uncropped frame. Blacks stayed at zero on every frame we
measured, under the rule that it clips no more than the default. And
where Whites cannot hold the top, the overflow still comes off the
exposure, past the shadows' hold: a frame with shadows five to eight
stops under and lights six over asked for a cut of 0.2 and took 1.5; no
real frame has shown it yet.
