# 229. Hot pixels on by default: the other colors decide (2026-10-03)

The hot pixel pass (darktable's rule, ported in §13s) has been off since
it took a catchlight: a site more than six sigmas and three times above
its eight same-color neighbors two photosites away is hot to it, and so
is anything the lens draws at most two photosites across. A hot site
left in renders badly. In the backlit couple (the 100 MP Fujifilm frame
at ISO 80) one blue site reads 0.0338 of white against same-color
neighbors at 0.0009, 37 times; the demosaic spreads it to a 5 by 5
pure-blue cluster with a black ring and the sharpen haloes it in yellow.

**The rule.**

What separates the defect from the picture is the other colors. A hot
site is one photosite: its neighbors of the other colors read what the
scene put there. A glint, a star or a catchlight is spread over several
sites by the optics, so the sites round it of every color are lit.

We keep darktable's test as the candidate test (six sigmas of the
measured noise and three times the brightest same-color neighbor, or
below the darkest for a dead site) and add one. For each of the three
other positions of the 2x2 pattern (across, down, diagonal) we take
its sites adjacent to the candidate (two, two and four) and the median
of its sites two and three photosites out (ten, ten and twelve) as its
local background. The candidate is repaired only when no position is
lit: a position is lit when its brightest adjacent site is both more
than 1.5 times its background and more than three sigmas of the noise
above it (for a dead candidate, the darkest adjacent site below the
background by both). `Candidate::others_factor` gives the largest
factor among the positions past the sigma test, and the repair spares
the site when it is above 1.5; the measuring example prints the same
number, so the two cannot drift.

Judging each position against its own background, not against the
candidate's level, is what makes the test color-blind: a red site on a
strongly green, faintly blue surface is compared with green at the
green level and blue at the blue level. The sigma half keeps the noise
in a dark patch, where any ratio is easy, from lighting a neighbor; it
is lower than the candidate's six on purpose, since an error there
spares a defect and an error the other way recolors a highlight.

**A floor under the noise.**

Both tests use the frame's noise model, and the model is blind at
black. The levels are clamped at zero, so in the shadows the estimate
sees no spread and puts the read noise at zero; on most of our frames
it does. With a sigma of zero, a site a few units above neighbors
clamped at zero is a candidate at any value, and the three-sigma half
of the others test is no test. On the one-second frame of a dark room,
847 of 1661 repairs were that: values of 1 to 11 units of the R6 II's 13,496 unit
range over neighbors at zero, read noise and not defects. So both tests
now take the noise as at least 2e-4 of the range from black to white,
about three units of a 14-bit raw, which is a clean sensor's read noise
at base ISO. That frame now has 13 repairs, the large repeat defects
among them, and one more than before: a recurring defect of about 930
units that the first version spared because a neighbor read lit at
2.25 times only through a sigma near zero. The review matched the
sites the floor dropped by sensor position across the eleven R6 II
frames and the three 100 MP frames in the test folder: 1,670 and 248
sites gone, none of them at a position repaired in any other frame,
and every position repaired in two or more frames still repaired in
all of them. In units the floor is 2.6 to 3.3 on the bodies here (all
near 14 bits of range); as a fraction of range it should hold across
bit depths, since read noise in electrons is alike and the units per
electron scale with the range. The risk left is high ISO, where read
noise as a fraction of range rises above it: if the fit pins the read
noise to zero on such a frame, the noise candidates come back. The one
high-ISO dark frame here, the moon, measured its noise and is unaffected,
so that case is untested. The couple's blue site (37 times, 0.0338 of white) and the
moon frame's warm sites (6 to 21 sigma of a model that does see the
noise there) are well past it. A synthetic dark field, read noise of
1.8e-4 clamped at zero and a model with no read noise, loses 123 sites
without the floor and none with it, and a 40-unit defect in it still
goes.

**Smaller things that moved.**

- The window is seven photosites wide, so the band at the frame's
  edges that is never judged grew from two sites to three.
- The CLI's `stack` and `register` (its `luminance_of`) had the old
  rule switched on explicitly; they now take the default, the new rule.
- The CLI's `--hot-pixels` became `--no-hot-pixels`, with `--hot-others`
  beside `--hot-sigmas` and `--hot-ratio`. `--hot-pixels` is still
  accepted, hidden, for one release, and warns that it does nothing
  and names `--no-hot-pixels`.
- The thumbnail recipe is raised, since the fallback develop's picture
  changes.

**Measurements.**

Candidates per frame (hot / dead), all from `examples/hot_candidates`
on the final rule (noise floor included): the same-color test alone at
ratio 2 (where §13s found the catchlight) and 3 (its default), and what
the others test keeps of each. Six sigmas throughout. The default is
the last count column, and it matches what the develop logs.

The rendered columns count 24-pixel cells of the 8-bit preview that
change by more than six levels against the repair off, with
`--no-ca` and no sharpen on every render, so that the difference is the
repair's own (see below for what the CA fit does).

| frame | old, ratio 2 | old, ratio 3 | new, ratio 2 | new, ratio 3 (default) | rendered cells, old ratio 2 | rendered cells, default |
|---|---|---|---|---|---|---|
| backlit couple, 100 MP, ISO 80 | 117 / 20 | 30 / 16 | 55 / 19 | 26 / 15 | 118 | 33 |
| portrait on a fur rug with a neon sign, R6 II, ISO 100 | 323 / 5 | 77 / 0 | 9 / 0 | 0 / 0 | 209 | 0 |
| portrait in a leopard coat under string lights at dusk, R6 II | 163 / 5 | 36 / 4 | 3 / 1 | 1 / 1 | 101 | 1 |
| church interior with candles, R6 II, ISO 200 | 268 / 11 | 36 / 3 | 38 / 1 | 12 / 1 | 276 | 14 |
| lanterns on a street at night, R6 II, ISO 320 | 154 / 6 | 66 / 0 | 36 / 0 | 22 / 0 | 120 | 25 |
| amplifier in a dark room, R6 II, 1 s | 21 / 1 | 13 / 1 | 20 / 1 | 12 / 1 | 26 | 19 |
| plants, R6 II, 1/8 s | 3 / 0 | 1 / 0 | 3 / 0 | 1 / 0 | 4 | 2 |
| snowy mountains, R6 II | 1477 / 123 | 67 / 0 | 2 / 0 | 0 / 0 | 1363 | 0 |
| woman on a sofa by a brick wall, R6 II, ISO 500 | 1418 / 386 | 22 / 1 | 12 / 0 | 0 / 0 | 1412 | 0 |
| sea and cliff, R6 II, ISO 500 | 1871 / 0 | 346 / 0 | 11 / 0 | 1 / 0 | 1080 | 1 |
| full moon in a black sky, R5 II, ISO 1000 | 5329 / 0 | 5223 / 0 | 2990 / 0 | 2990 / 0 | 4739 | 2754 |
| mountain overlook, R6 II | 148 / 16 | 8 / 0 | 2 / 0 | 0 / 0 | | |
| couple on a bench, 100 MP, ISO 320 | 191 / 95 | 38 / 93 | 167 / 84 | 35 / 82 | | |
| a second 100 MP frame, ISO 80 | 11 / 5 | 3 / 3 | 11 / 5 | 3 / 3 | | |
| an R6 frame, ISO 100 | 112 / 11 | 0 / 0 | 1 / 0 | 0 / 0 | | |
| skyline at sunset, R6 II | 5731 / 31 | 1220 / 0 | 21 / 0 | 4 / 0 | | |
| city at dusk, R6 II, ISO 1250 | 11151 / 765 | 3923 / 53 | 93 / 2 | 70 / 0 | | |
| skyline, R6 II, ISO 500 | 8972 / 486 | 2770 / 28 | 145 / 4 | 10 / 0 | | |
| street with lanterns, R6 II | 12 / 1 | 7 / 0 | 6 / 0 | 6 / 0 | | |
| wheat-field portrait (R7), couple at a table (R6 II) | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | | |

There is no starfield in the test set. The night frames are the
lanterns, the candles, the string lights, the dark room at one second
and the full moon at ISO 1000; we used their point lights and the
moon's disc and limb in place of stars.

What the crops show, the repair off against the default:

- Backlit couple: the blue cluster is gone, the background in its
  place. Every other change is a single colored dot in the shadows
  (red, green, blue, a yellow cross where a hot site sat between two
  others); nothing in the backlit hair changes with the CA fit held off.
- Fur rug portrait: the catchlights in both eyes are the sites 13s
  found (green sites at 3.4 to 5.8 times their same-color neighbors,
  with the other colors round them 3 to 9 times their background). At
  ratio 2 the old rule turns them purple; the default leaves the frame
  identical to the repair off, pixel for pixel. At ratio 2 the new rule
  would still take eight specks in the rug and the red of a glint on a
  phone (others factor 1.2 to 1.5), which is why the ratio stays 3.
- Leopard coat portrait: the old rule turns string-light bulbs cyan
  and purple; the default changes one site, a green speck in the dark.
- Candles and lanterns: the old rule recolors candle flames, gilding
  highlights and a lantern bulb; the default leaves every light alone
  and removes colored dots in the dark.
- Moon: the repair takes 2990 warm sites in the black sky, 22 to 68
  units and 6 to 21 sigma between the 5th and 95th percentiles, which render as pure-blue blotches of up to
  thirty pixels at ISO 1000. With the CA correction off on both renders
  66,349 pixels differ, 7,090 by more than six levels, all in the sky;
  the moon's disc (221,419 pixels brighter than 60) is byte-identical.

The best evidence that the kept sites are defects is that they come
back. The R6 II frames are one body; of the sites the default repairs
in the candles, lanterns, plants, dark room and sea frames, the largest
sit at the same sensor positions frame after frame (one position
flagged in four of the eleven R6 II frames in the test folder and in
more of the frames beyond it, several in three to six). Those repeat sites
read exactly at background in the others test in most frames (factor
1.0); a few read 1.6 to 4.7 in one frame, where a busy background lit
a neighbor. That is the rule's miss: a defect on texture is spared, as
a glint is. The one recurring site still spared in the rendered frames
is on the lanterns: 368 units above black, 15.7 times its same-color
neighbor, and its horizontal neighbor 4.67 times a background of
0.00021, an excess that is about three percent of the defect's own,
which looks like the defect's bleed into the next photosite rather
than the scene. A refinement would ask a neighbor's excess to be some
fraction of the candidate's before it counts as lit; it is on the
roadmap, and the per-camera defect map remains the answer for a site
the rule misses.

**A frame-wide side effect: the CA fit.**

With the CA correction on, the repair changes more than the repaired
sites, and it is not the highlight reconstruction: the opposed
reconstruction's chrominance and votes are identical on and off on
every frame (the couple: R +0.4801, G -0.0017, B +0.0274 from 135,298,
284,460 and 99,104 votes both ways). What moves is the lateral CA fit,
which reads the mosaic after the repair: on the couple it fits 1.36 /
0.74 pixels from 6017 tiles with the repair and 1.24 / 0.72 from 6018
without; on the moon 0.26 / 0.42 against 0.25 / 0.43. A refit moves
every pixel by a level or two, and edges by more: on the couple with
`--highlights clip` and CA on, 639,511 pixels change; with `--no-ca`
and the reconstruction on, 4,581 change, none above a luma of 100. On
the moon with CA on, 1.87 million pixels differ, and 31 on the limb
move, the worst by 33 levels at one pixel. This is probably an
improvement, since a hot site is an outlier in the tile it sits in, but
it means a repair is never only local. The sharpen's automatic radius
(`measure_radius`) also reads the mosaic after the repair, so with
`--sharpen` it can move the same way.

**The choice of thresholds.**

The others factor across every ratio-3 candidate of the frames above
(14,160): 3,292 at exactly 1 (no position past three sigmas), none
between 1 and 1.25, 14 to 1.5, 34 to 1.75, 54 to 2, then a long tail to
the hundreds and 1,389 at infinity (a background at zero), the glints.
Every repair the default makes in the eleven rendered frames looked
like a lone dot; nothing it spared looked like one. We take 1.5 and
three sigmas; 2.0 would add a handful of sites, most in the dusk
skylines, and start into the tail. The candidate test stays at six
sigmas and ratio 3.

**Tests.**

Synthetic mosaics pin the rule: a one-color hot site goes while a white
2x2 glint and a 3x3 catchlight stay (the old rule takes all six); a
dead site goes while a dark 2x2 speck stays; cell-scale texture with a
wide contrast is untouched (the old rule takes some); the others are
judged on their own background; every one of the four pattern phases
and four candidate parities repairs a lone hot site and only it, spares
it when any one adjacent site is lit, still repairs it when a site two
or three out is lit, mirrors all of that for a dead site, and leaves
the three-site edge band alone; and clamped read noise with a model
blind to it loses no site.

**What the editor cannot do yet.**

The edit schema has no per-photo switch for the repair. A misfire, say
a colored point light one photosite across on busy texture, has no
escape in the editor today; only the CLI can turn it off. That belongs
on the roadmap, not in this change.
