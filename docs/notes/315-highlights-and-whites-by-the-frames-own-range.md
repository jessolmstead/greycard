# §315. Highlights and whites by the frame's own range (2026-10-13)

§250 placed highlights by the frame's own range on a branch off 0.4.0
and left the decision open. The user has chosen it: the adaptive
Highlights ships. This section brings it onto master and takes the
next step §250 named, whites by the same range. We also tried a
Brightness slider and kept the fixed-stops highlights beside it for a
side-by-side; neither ships, and what we measured is recorded at the
end.

**Bringing §250 onto master.** The commit came forward with three
conflicts, all where master had grown alongside it rather than through
it. `finish_pixel_with` takes the trial's guide with the frame's range
and also master's Detail planes. The `Renderer` keeps master's detail
texture and its uniforms and gains the trial's `frame_range`, in its
fields and in `new`. The shader merged without help. The trial's own
tests (the span sweeps, the shadows-then-highlights fold test, the
plane's range) passed against master unchanged, as did the GPU parity
run, so no test expectation moved.

**Whites by the frame's range.** Today's whites (§85, §146, §149) is a
white point pivoted at mid grey. A luminance `whites` stops under
display white is brought to display white by a power on the stops over
grey, eased in over the first stop, and nothing at or under grey
moves. §150 found it moved only the top thirty percent of a frame,
where Lightroom's moves the 25th to 85th percentiles. It adapts to
nothing.

We kept the white point and moved its pivot. The pivot is now the foot
of the highlights span (`highlights_span`), read from the guide plane
and brought to the picture's exposure and contrast. The span is hung
from the frame's top and held to 1.6 to 3.5 stops, so its foot is the
frame's median where the span is the frame's own. On a frame narrower
than 1.6 stops the foot sits under the median, and on one wider than 3.5
it sits over it. The pivot is then held between 2.5 stops under grey and
grey (`WHITES_PIVOT`). The aim is unchanged: a luminance `whites` stops
under display white still lands on display white. So the slider means
what it did and the Lightroom importer's scale holds. The exponent
becomes `(3.27 - f) / (3.27 - f - whites)` for a pivot `f` stops over
grey.

The two holds have one reason each. Over grey would bring the pole
nearer: at grey the slider's +2 is 1.27 stops short of it and the
floor's -3.5 gives an exponent of 0.48, over the 0.41 where the eased
curve folds (§149). With the pivot never over grey, both are at their
worst exactly where they were, so `WHITES_RANGE` holds unchanged.
Under 2.5 stops would put a low-key frame's whites on its shadows, as
§250's span limit keeps highlights off them.

Whites stays a white point on the pixel's own luminance, after
highlights in `shape`, CPU and WGSL alike: global, so no halo. It
reads the guide plane only for the frame's range. The export now
counts a set whites as reading the plane (`reads_guide`); without
that it placed the pivot by a typical frame's range while the
viewport used the frame's own, which the GPU parity run caught. The
monotonic sweep (§250's, with the span's foot from six stops under
grey to six over at four widths) now runs on all eight corners of
highlights, shadows and whites at their limits, so it covers the four
corners of highlights against whites. No schema change. A stored
whites opens a little differently on a frame whose median sits under
grey, which is most of them, as a stored highlights did under §250.

A typical frame's pivot is 0.9 stops under grey. A frame whose median
is over grey keeps the old pivot at grey: the sunset, the one frame
with Lightroom whites exports (§149), measures identically before and
after. Whites +2 leaves 26.0 percent at white on both builds, against
Lightroom's 27.9 at +100. Whites -2 leaves none on both, and moves the
70th to 100th percentiles by -0.34 to -0.39.

**Measured.** Six portraits, six landscapes with a bright sky over a
darker foreground, and two high-key frames from the test set, all at
the default edit, 1600 pixels, 16-bit sRGB. The table gives the
median change in display stops by percentile band of each build's own
untouched render. The bands are 0-10, 10-25, 25-40, 40-55, 55-70,
70-85, 85-95 and 95-100. Highlights -75 is -1.5 and whites -75 is
-0.5625, both on the importer's scale. "Fixed" is master's ramps. A
blank cell is a box we did not measure. Boxes were placed by hand:
skin on two portraits, and on the landscapes the sky and the darkest
foreground.

Highlights -75 alone:

| frame | adaptive | fixed | boxes, adaptive / fixed |
|---|---|---|---|
| pale portrait, greenhouse | 0 0 -.02 -.12 -.31 -.44 -.42 -.25 | 0 0 -.01 -.08 -.23 -.36 -.40 -.35 | |
| wheat field | 0 0 -.02 -.13 -.24 -.33 -.42 -.33 | 0 0 -.06 -.13 -.19 -.24 -.46 -.45 | |
| sweater against stone | 0 0 -.06 -.15 -.23 -.38 -.27 -.15 | 0 0 0 -.02 -.09 -.30 -.33 -.20 | |
| couple by the water | 0 0 -.02 -.18 -.37 -.64 -.54 -.35 | 0 0 0 0 0 -.24 -.40 -.31 | |
| studio portrait | 0 0 -.04 -.22 -.46 -.51 -.56 -.43 | 0 0 -.01 -.07 -.17 -.20 -.26 -.32 | face -.49 / -.28 |
| close portrait, backlit | 0 0 0 -.12 -.44 -.59 -.56 -.38 | 0 0 0 0 -.10 -.22 -.34 -.34 | face -.44 / -.11 |
| cliffs, overcast | 0 0 0 -.02 -.37 -.24 -.16 -.11 | 0 0 0 0 -.43 -.31 -.20 -.14 | sky -.20 / -.26, fg 0 / 0 |
| sea from a wooded slope | 0 0 0 -.02 -.61 -.42 -.36 -.34 | 0 0 0 0 -.26 -.45 -.46 -.46 | sky -.34 / -.46, fg 0 / 0 |
| lighthouse, under-exposed | 0 0 0 -.16 -.53 -.85 -.69 -.51 | 0 0 0 0 0 -.02 -.17 -.24 | sky -.79 / -.11, fg 0 / 0 |
| alpine, snow | 0 0 0 -.15 -.37 -.42 -.45 -.29 | 0 0 0 -.10 -.21 -.24 -.30 -.28 | sky -.47 / -.27, fg 0 / 0 |
| ridges at dusk | 0 0 0 -.13 -.44 -.66 -.56 -.34 | 0 0 0 0 0 -.23 -.40 -.44 | sky -.62 / -.26, fg 0 / 0 |
| sea stack, low sun | 0 0 0 0 -.21 -.24 -.14 -.10 | 0 0 0 0 -.40 -.36 -.11 -.10 | sky -.25 / -.38, fg 0 / 0 |
| sunset over a city | 0 0 0 0 -.04 -.20 -.11 -.09 | 0 0 0 0 -.22 -.28 -.10 -.10 | |
| jets on a pale sky (high-key) | -.64 -.77 -.79 -.78 -.76 -.71 -.60 -.49 | -.05 -.12 -.16 -.18 -.20 -.24 -.27 -.30 | |
| cathedral, overcast (high-key) | 0 0 -.01 -.11 -.49 -.55 -.40 -.30 | 0 0 0 0 -.17 -.42 -.46 -.40 | |

Whites -75 alone:

| frame | adaptive | fixed | boxes, adaptive / fixed |
|---|---|---|---|
| pale portrait | 0 0 0 0 -.05 -.14 -.16 -.15 | 0 0 0 0 -.01 -.11 -.14 -.14 | |
| wheat field | 0 0 0 0 -.01 -.04 -.14 -.15 | 0 0 0 0 0 -.02 -.13 -.14 | |
| sweater | 0 0 0 0 -.03 -.15 -.15 -.12 | 0 0 0 0 0 -.06 -.14 -.12 | |
| couple | 0 0 0 0 -.06 -.20 -.22 -.20 | 0 0 0 0 0 -.01 -.12 -.14 | |
| studio portrait | 0 0 0 0 -.03 -.07 -.14 -.17 | 0 0 0 0 0 0 -.06 -.13 | face -.14 / -.07 |
| close portrait | 0 0 0 0 -.05 -.14 -.18 -.18 | 0 0 0 0 0 -.01 -.10 -.14 | face -.09 / 0 |
| cliffs | 0 0 0 0 -.16 -.14 -.13 -.11 | 0 0 0 0 -.13 -.13 -.12 -.11 | sky -.14 / -.13, fg 0 / 0 |
| sea from a slope | 0 0 0 0 -.16 -.19 -.19 -.19 | 0 0 0 0 -.02 -.13 -.13 -.13 | sky -.19 / -.13, fg 0 / 0 |
| lighthouse | 0 0 0 0 -.02 -.23 -.26 -.26 | 0 0 0 0 0 0 0 -.02 | sky -.25 / 0, fg 0 / 0 |
| alpine | 0 0 0 0 -.03 -.09 -.14 -.15 | 0 0 0 0 0 -.03 -.12 -.14 | sky -.06 / -.01, fg 0 / 0 |
| ridges | 0 0 0 0 -.14 -.21 -.22 -.21 | 0 0 0 0 0 -.01 -.11 -.14 | sky -.21 / -.01, fg 0 / 0 |
| sea stack | 0 0 0 0 -.12 -.14 -.10 -.09 | 0 0 0 0 -.12 -.14 -.10 -.09 | sky -.13 / -.13, fg 0 / 0 |
| sunset | 0 0 0 0 0 -.12 -.09 -.09 | the same | |
| jets | -.06 -.13 -.14 -.15 -.16 -.17 -.17 -.18 | 0 0 0 0 0 0 -.01 -.03 | |
| cathedral | 0 0 0 0 -.14 -.19 -.19 -.18 | 0 0 0 0 0 -.12 -.13 -.14 | |

The pair, Highlights -75 and Whites -75 together (how the user works:
flatten the top, then shape it with the curve):

| frame | adaptive | fixed | boxes, adaptive / fixed |
|---|---|---|---|
| pale portrait | 0 0 -.02 -.12 -.33 -.52 -.55 -.41 | 0 0 -.01 -.08 -.23 -.39 -.50 -.49 | |
| wheat field | 0 0 -.02 -.13 -.24 -.34 -.53 -.46 | 0 0 -.06 -.13 -.19 -.25 -.55 -.56 | |
| sweater | 0 0 -.06 -.15 -.25 -.50 -.44 -.30 | 0 0 0 -.02 -.09 -.33 -.46 -.34 | |
| couple | 0 0 -.02 -.18 -.40 -.81 -.74 -.57 | 0 0 0 0 0 -.24 -.43 -.44 | |
| studio portrait | 0 0 -.04 -.22 -.47 -.52 -.64 -.58 | 0 0 -.01 -.07 -.17 -.20 -.29 -.42 | face -.59 / -.33 |
| close portrait | 0 0 0 -.12 -.45 -.65 -.71 -.55 | 0 0 0 0 -.10 -.23 -.37 -.45 | face -.48 / -.11 |
| cliffs | 0 0 0 -.02 -.52 -.40 -.31 -.25 | 0 0 0 0 -.52 -.45 -.34 -.27 | sky -.36 / -.40, fg 0 / 0 |
| sea from a slope | 0 0 0 -.02 -.72 -.59 -.54 -.52 | 0 0 0 0 -.27 -.50 -.52 -.53 | sky -.53 / -.53, fg 0 / 0 |
| lighthouse | 0 0 0 -.16 -.54 -1.03 -.92 -.76 | 0 0 0 0 0 -.02 -.17 -.25 | sky -.96 / -.11, fg 0 / 0 |
| alpine | 0 0 0 -.15 -.37 -.44 -.56 -.45 | 0 0 0 -.10 -.21 -.25 -.37 -.41 | sky -.48 / -.28, fg 0 / 0 |
| ridges | 0 0 0 -.13 -.53 -.83 -.77 -.56 | 0 0 0 0 0 -.23 -.43 -.54 | sky -.81 / -.26, fg 0 / 0 |
| sea stack | 0 0 0 0 -.30 -.39 -.26 -.22 | 0 0 0 0 -.46 -.49 -.23 -.21 | sky -.38 / -.51, fg 0 / 0 |
| sunset | 0 0 0 0 -.05 -.33 -.23 -.20 | 0 0 0 0 -.22 -.42 -.21 -.21 | |
| jets | -.64 -.79 -.82 -.83 -.82 -.80 -.73 -.64 | -.05 -.12 -.16 -.18 -.20 -.24 -.27 -.30 | |
| cathedral | 0 0 -.01 -.11 -.58 -.71 -.59 -.49 | 0 0 0 0 -.17 -.46 -.54 -.52 | |

**Reading it.** Whites now starts at the 55th percentile and is
strongest from the 70th, where it used to start at the 70th to 85th.
That is a step toward Lightroom's 25th to 85th, not all the way. On
a face it takes 0.07 to 0.09 more than before. The pair's adaptive
version is the stronger of the two on every portrait and on the
darker landscapes, and the two are about even on the bright ones.

On the landscapes, the adaptive highlights does not reach a dark
foreground. Every foreground box reads 0.00 on both builds, alone and
in the pair. It does reach lower in the tones: the 40th to 55th
percentile moves -0.13 to -0.16 on three of the six (the
under-exposed lighthouse, the alpine frame, the ridges), where the
fixed ramp leaves it, or takes -0.10 on the alpine frame. In those
frames that band is distant ridges, the sea and snow in shade, not
the foreground. With the foreground unmoved, nothing called for a
change to the span's 1.6 to 3.5-stop hold, and we left it alone.

The sky's top is mixed, as §250's sunset row hinted. At the 95th to
100th percentile the adaptive ramp takes less than the fixed on
three landscapes (the cliffs -0.11 against -0.14, the sea -0.34
against -0.46, the ridges -0.34 against -0.44), and as much on two.
It takes more on the under-exposed lighthouse (-0.51 against -0.24),
whose sky sits low on its own span. Across the body of the sky the
adaptive ramp takes more on the contrasty frames (the lighthouse
-0.79 against -0.11, the ridges -0.62 against -0.26) and less on the
bright ones (the sea stack -0.25 against -0.38).

Two things to watch. On a frame that is all sky (the jets), the
frame's range is under a stop, so the span is held at its least and
the whole frame sits high on the ramp. Highlights -75 then works as an
exposure of -0.6 to -0.8, and whites moves every band by -0.06 to
-0.18. That is the watch case §250 was built for, and Lightroom
behaves the same way there (-0.46 to -0.74 on the watch). On a sky
alone, though, it may read as the slider doing too much.

The other: on the under-exposed lighthouse the pair takes a stop off
the 70th to 85th percentile. The fixed ramps barely touched that frame
because its sky sat under their fixed stops.

**Against Lightroom.** The reference exports of §146 and §150 are on
the Windows machine, and only the sunset's raw is here (from the
archive). Highlights alone is §250's code, unchanged here, so §250's
summed distance stands: 1.54 stops over twenty bands, from 2.34. For
whites, the only Lightroom number is the sunset's clipped share, and
it is unchanged (26.0 percent against 27.9). No Lightroom number
exists for the pair. A summed distance for whites and for the pair
waits on a whites export on two more frames, as §150's third question
already asks.

The exports, one folder a variant, are under `target/scratch/shots/r2/`
in the worktree, with side-by-sides of the jets, the lighthouse, the
ridges and the two face frames. The first round's exports and its
compare sheets are under `target/scratch/shots/`.

**What else moves with it.** `DEVELOP_RECIPE` goes to 3: any stored
edit with highlights or whites set now renders differently. Without
the raise, the pictures kept for the strip, the grid and the lightbox
would show the old rendering until the edit was touched.

A stored whites opens noticeably different on a low-key frame, where
the pivot is held at 2.5 stops under grey. There, whites +2 lifts mid
grey by about 1.33 stops and -2 pulls it down about 0.64. Before, grey
was the pivot and did not move.

The dropper's `pick` reads one pixel and has no guide plane, so it
places the pivot from a typical frame's range, `FRAME_TYPICAL`, whose
foot is 0.9 stops under grey. Its reading differs a little from the
render on a frame whose foot sits elsewhere, as the highlights
reading has since §250. Giving it the frame's own range is for later.

The thumbnails are not new: they make their own guide plane from the
reduced develop (`reduced.rs`), and with it their own range.

**Not.** No retune of the span or the pivot's hold: the foreground did
not move. The Lightroom importer's scale is unchanged.

## Tried, not shipped

**Brightness.** The complaint was that none of the Light sliders
reaches skin, which sits a stop either side of grey. Capture One
answers it with Brightness, an exposure weighted toward the mid-tones.
We built one in log2 luminance over grey: `L' = L + b * exp(-L^2 /
(2 sigma^2))`, with sigma 1.5 stops and ±100 on the slider for ±1 stop
at grey. It read by region, came first in `shape`, was held to ±2
stops after the masks were summed (its steepest slope is 0.40 a stop,
so 0.81 at the hold, never folding), and a sweep tested that proof.

Beside it we kept the fixed-stops highlights as a second slider, so
the two highlights could be told apart on one build. The user found
the adaptive Highlights and Brightness alike on portraits, and chose
the one slider that answers a Lightroom user's first move over two
that overlap. A mid-tone slider would also take over the job of the
curve's Lights and Darks, leaving two controls for it. Neither
ships.

What we measured, on the six portraits at the default edit, as above:

| frame | Brightness +50 | Brightness -50 | Highlights -75 (range) | Highlights (fixed) -75 |
|---|---|---|---|---|
| pale portrait, greenhouse | +.39 +.53 +.59 +.56 +.46 +.32 +.21 +.08 | -.39 -.55 -.63 -.64 -.54 -.39 -.26 -.10 | 0 0 -.02 -.12 -.31 -.44 -.42 -.25 | 0 0 -.01 -.08 -.23 -.36 -.40 -.35 |
| wheat field | +.37 +.57 +.58 +.54 +.50 +.45 +.19 +.16 | -.37 -.62 -.65 -.62 -.58 -.53 -.23 -.19 | 0 0 -.02 -.13 -.24 -.33 -.42 -.33 | 0 0 -.06 -.13 -.19 -.24 -.46 -.45 |
| sweater against stone | +.09 +.30 +.54 +.57 +.53 +.38 +.06 +.02 | -.09 -.30 -.57 -.63 -.61 -.46 -.07 -.03 | 0 0 -.06 -.15 -.23 -.38 -.27 -.15 | 0 0 0 -.02 -.09 -.30 -.33 -.20 |
| couple by the water | +.02 +.12 +.25 +.43 +.53 +.45 +.29 +.18 | -.02 -.12 -.25 -.44 -.57 -.53 -.36 -.23 | 0 0 -.02 -.18 -.37 -.64 -.54 -.35 | 0 0 0 0 0 -.24 -.40 -.31 |
| studio portrait | +.30 +.54 +.57 +.56 +.53 +.49 +.39 +.26 | -.30 -.57 -.63 -.64 -.62 -.58 -.47 -.32 | 0 0 -.04 -.22 -.46 -.51 -.56 -.43 | 0 0 -.01 -.07 -.17 -.20 -.26 -.32 |
| close portrait, backlit | +.02 +.17 +.45 +.55 +.54 +.46 +.33 +.21 | -.02 -.17 -.46 -.59 -.62 -.54 -.41 -.26 | 0 0 0 -.12 -.44 -.59 -.56 -.38 | 0 0 0 0 -.10 -.22 -.34 -.34 |

Inside a face:

| frame | where the face sits | Brightness +50 | -50 | Highlights -75 (range) | (fixed) -75 |
|---|---|---|---|---|---|
| studio portrait | 91st percentile | +0.38 | -0.46 | -0.49 | -0.28 |
| close portrait, backlit | 67th percentile | +0.47 | -0.55 | -0.44 | -0.11 |

Brightness reached the face (0.4 to 0.55 either way at half the
slider) but did not leave the ends alone on high-key frames. Their
darkest tenth moved 0.3 to 0.4 and the top twentieth 0.1 to 0.3,
because scene white is only 2.5 stops over grey and the bump keeps a
quarter of its strength there. On the face of the backlit portrait,
the adaptive highlights takes -0.44, about what Brightness -50 does
(-0.55), which is the overlap the user saw.
