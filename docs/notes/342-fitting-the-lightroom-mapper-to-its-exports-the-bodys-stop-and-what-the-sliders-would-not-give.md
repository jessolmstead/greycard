# §342. Fitting the Lightroom mapper to its exports: the body's stop, and what the sliders would not give (2026-10-17)

§323 asked for the mapper's scales by number rather than by eye, and
§333 built the measurement and said what it found: the import renders
the GFX 100S II about a stop brighter than Lightroom under every
profile and the R5 Mark II within a twentieth, the chroma is small
everywhere, and no slider can be told from another inside one shoot.
This is the second step: a per-body exposure offset the mapper now
applies, one scale moved, a fit that runs headless over the
measurement's blocks and says which scales the data holds and which it
does not, and the three fit-1 look tables refitted. The headline
numbers are the measurement's own, before and after, over the frames
of the catalog of about 280 that registered both times; the fit's
numbers are in between and reproduce in minutes from the blocks it
kept.

## The offset per body

**Measured, not read.** Nothing we open gives Adobe's `BaselineExposure`
for a body: a RAF or a CR3 carries no such tag, rawler's camera data
has none, and the DCP reader (`greycard-core::dcp`) reads
`BaselineExposureOffset` from a profile we do not have for either
body. The number exists only in an Adobe DNG or DCP of each body, which
the archive does not hold (its DNGs are a drone's and three panoramas).
So the offset is a constant measured from the exports, as §323 allowed:
per body, the median over its history pairs of the stops the Exposure
slider moved to match the export (the measurement's `matched_offset`,
since the constant goes onto that slider; the fit reports the median
of the output's own offset, `offset`, beside it), at the shipped
scales and under the refitted looks.
`lightroom::BODY_OFFSETS` holds it, keyed on the body as the library
names one and compared with case, spaces and punctuation dropped;
`read_table_for` adds it to the Exposure slider, carrying the Light
section even when the settings leave the slider alone; a body not in
the table takes `UNKNOWN_BODY_OFFSET`, zero, as the R5 Mark II does on
its history pairs; §333's R6 Mark II lead of about -0.8 was on name
pairs alone, exports of another catalog's settings, and is not
adopted. The
import's `imported_edit` and `developed` look the body up from the
raw's probe, and the measurement prints the offset it applied per
frame and per body, with `--no-body-offset` to measure without it and
`--scale NAME=VALUE` to measure an earlier or a proposed mapper.

| body | frames | median matched offset, shadows 0.6, fit-1 looks | the same under the refitted looks | ΔE optimum, fit-1 | shipped |
|---|---|---|---|---|---|
| Canon EOS R5 Mark II | 29 | +0.02 | +0.04 | -0.13 | 0.00 |
| Fujifilm GFX 100S II | 39 | -0.90 | -0.72 | -0.89 | -0.90 |

(`--fit-lightroom --offsets-only` over the blocks of the
`--no-body-offset` run: `target/scratch/fit/cat3-offsets3` and
`cat3-offsets3-s06`; off the output's own median the same runs read
-1.00 and -0.84, since the slider's stop buys less than a stop of the
output's median where the tone shifts read the exposure too.) The
constant is the median of the matched exposure over the body's history
pairs under the look tables the user has installed, the fit-1 ones,
whose ΔE optimum is -0.89: a number between two tables' medians would
not be a measurement, so the constant names its tables, and installing
other tables means measuring again and changing it in the same commit.
The structure is `BaselineExposure`'s, one number a body, and the day
someone converts one frame of each body through Adobe's DNG converter
the difference of the two tags is the check on whether a table like
this generalizes to a body the archive lacks; a table does not, and
says so.

**What the refitted tables do to it.** Under the refitted Provia and
Reala Ace tables (below) the same median is -0.72; by profile, fit-1
against refitted, Provia -1.11 against -0.79, Reala Ace -0.72 against
-0.54, Acros (no table) -1.25 under both. Under the fit-1 tables the
GFX frames with a look and the look-free black and white ones sit
together; the refitted tables darken Provia by about 0.3 stops and
Reala Ace by about 0.15 against the no-look render. A look is meant to
leave brightness to the Exposure slider (§180 solves each frame's
offset before the table is fitted), so that is a question to put to
the camera match before the refit is installed, not a reason to split
the constant. What each candidate leaves in the GFX's median, in real
renders of the catalog, under fit-1 and under the refit: -0.90 leaves
0.00 and +0.18; -0.84 leaves -0.06 and +0.12; -0.72 leaves -0.18 and
0.00. The reviewer's run at -0.84 with the user's fit-1 tables: all 65
frames ΔE 0.0497, offset -0.05 ± 0.41, matched 0.0367; GFX 0.0578,
median -0.06; R5 Mark II 0.0398: the improvement is the mapper's, not
the looks'. The offset itself is never held out: one shoot, one number
a body.

**Not all of it is the body's.** The fit reads each block's offset by
the export's lightness, and four bands say two things are stacked.
Over every frame of a body, the median of the frames' median offsets
(frames with 50 blocks in the band), at the shipped scales with
shadows at 1.0 (`target/scratch/fit/all4`):

| body | under L 0.3 | 0.3 to 0.5 | 0.5 to 0.7 | from 0.7 |
|---|---|---|---|---|
| R5 Mark II | -0.84 | -0.20 | +0.04 | +0.05 |
| GFX 100S II | -1.30 | -1.01 | -0.73 | -0.43 |

In the bright blocks the R5 Mark II sits on Lightroom and the GFX
about half a stop under it: that half stop is the body's. In the deep
shadows both bodies sit a stop under, and the GFX's dark and mid
bands sit under where the R5 Mark II's do not: a body's constant
cannot carry a slope. The R5 Mark II's frames are bright-dominated
(their medians sit in the bright band, 1200 to 2300 blocks against
200 to 500 dark), the GFX's dark-dominated (evening, front-lit, the
flat blocks in the clothing and the shade), so the whole-frame medians
read 0 and -1, and §333's "a stop per body" was half a body and half
the frames. The shipped constant is still the whole-frame median,
because that is what lands the archive's own frames where Lightroom
put them; the decomposition is the number to carry to the feel track.

**What it says about §141's 0.8 stops.** The baseline per body §150
listed as open item 2 is real and is about half a stop between these
two bodies in the bright blocks, with the GFX the one over. The dark
bands are not the baseline's business: they are the shape of
Highlights, Whites and Shadows under the midpoint (§141, §146, §149),
which the default develop does not touch. Nothing here changes the
default develop; that stays the user's decision.

## The three Reala Ace frames

§333 left it open what the three Reala Ace frames within a tenth of a
stop at ΔE 0.018 share that the twelve at -0.63 to -1.44 do not. Not
the exposure: the three were shot 24 seconds after one of the twelve
at the same aperture, ISO 80, DR100, the same lens, in the same light
(exiv2 on the five files). Not the masks: theirs are a sky at -8
highlights and -21 whites and a subject at +7 exposure. Not the
mixer: they alone carry the green band at luminance -63, saturation
-51, hue -35 over foliage, and with our mixer's luminance scaled to
nothing their offset moves by a tenth of a stop (`three-mixer` against
`three-asis`: -0.04 to -0.15, -0.01 to -0.07, -0.11 to -0.18). What the
three share is where their blocks are: every kept block is in the
deep shadows (937, 1229 and 1226 blocks under L 0.3, under 15 above
0.5; backlit scenes whose sky clipped out), and there the two
renderings agree, +0.2 with the refitted look, where every other GFX
frame's deep-shadow band reads -0.9 to -2.2 (`three4`, `all4`),
including the frame shot 24 seconds before them with no masks and the
shoot's other edit. Their edit is the shoot's one with Blacks at +29
and Whites at -37 (the rest -52 to -83); the frame beside them with
Blacks at +38 reads -1.31 in the same band, so the blacks alone are
not it either. The thing the three show is that the deep-shadow gap
between us and Lightroom is not a constant of the body: on three
frames it is zero. What in Lightroom's rendering of those frames
closes it is not found, and is open. They are three of fifteen, and
the body's constant is the median; it stands.

## The fit

**Where it runs.** `greycard-ui --measure-lightroom CATALOG --blocks
DIR` keeps, for every block the comparison kept, what the finish read
over it (the scene's mean in the working space, the guide plane's
mean, the vignette's stops) and what both sides came out as;
`greycard-ui --fit-lightroom DIR...` reads those and runs the finish's
own pixel function (`finish_pixel_with`, through
`export::scene_framed`) over the blocks for every setting of the
mapper it tries, thousands of times, without developing a frame
again. A kept block is flat on both sides, so the finish of its mean
is within a hair of the mean of its finish: against the real render's
block means the emulation sits 0.0012 ΔE off on average and 0.0029 at
most over the 68 frames when it runs at the scales, the offset and the
look tables the measurement ran under (`cat3-offsets3`; under the
refitted tables against renders made with the fit-1 ones it reads
0.0133 and 0.0399, and says so) (the record keeps the scales
and a hash of the table; the report says when the store's table is
another one), which the report prints every run. The fit
is a coordinate descent, golden section a parameter, four rounds with
the steps halved, over one offset a body and ten scales
(`lightroom::Scales`), minimizing the mean over frames of the frame's
mean ΔE against the export, brightness and all; the matched error is
reported beside it. History pairs only; name pairs are read and left
out. Held out by catalog where more than one has history pairs, by
body, and by pasted edit (the frames sharing one slider set, exposure
aside); `--no-hold-out` skips those, `--offsets-only` moves the
offsets alone, `--fit-scale NAME=VALUE` starts a scale elsewhere.
Two frames of synthetic blocks hold it: an export made at an offset
is found, and a scale a frame was made under is found.

**One shoot cannot hold out a shoot.** Of the four catalogs two have
no exports in the archive and one pairs by name alone, so every
history pair is one shoot's, with two bodies and about thirty-five
pasted edits of one to eleven frames each. Held out by edit the fit
holds (most edits land within 0.005 of the fit on everything); held
out by body it does not: scales fitted on the GFX alone put the R5
Mark II at 0.0463 against 0.0421 as shipped, and the two bodies
fitted apart want different numbers for most sliders. That is the
test §323 asked for, failed, and the reason the fit's scales are
proposed and not shipped but one.

**What the fit proposes** (`target/scratch/fit/cat3-1` over all 68
with the fit-1 looks; `gfx-refit` and `r5ii-refit` a body each with
the refitted looks):

| scale | shipped | fitted on all 68 | GFX alone | R5 Mark II alone |
|---|---|---|---|---|
| highlights | 2.0 | 2.35 | 3.13 | 1.65 |
| shadows | 1.0 | 0.60 | 0.62 | 0.23 |
| whites | 0.75 | 0.39 | 2.60 (flat) | 0.25 |
| blacks | 0.3 | 0.40 | 0.21 | 0.57 |
| curve | 1.0 | 0.93 | 0.79 | 0.66 |
| saturation | 1.0 | 0.45 (flat) | 1.0 (flat) | 0.52 (flat) |
| mixer luminance | 1.0 | 0.00 | 0.70 (flat) | 0.09 |
| mixer saturation | 1.0 | 0.00 | 0.40 | 0.01 |
| contrast | 0.5 | one frame moves it | - | - |
| vibrance | 1.0 | no frame moves it | - | - |

Each report prints the cost along each scale with the rest held at
the fit, which is what "flat" reads off: whites and saturation move
the cost by under 0.001 over their whole range on either body, so the
data does not hold them. Highlights and blacks are held but disagree
between the bodies by more than their gain over the shipped value is
worth (the R5 Mark II at the joint 2.35 is worse than at 2.0). Shadows
is the one scale both bodies hold and agree on in direction: the R5
Mark II's cost at 1.0 is 0.0374 against 0.0337 at its 0.23, the GFX's
0.0531 against 0.0525 at its 0.62, and the R5 Mark II's dark band
moves from -0.58 to -0.27 when it goes to 0.6. Swept alone, the other scales as shipped
and the offsets refitted at each step, the mean cost reads 0.0502,
0.0497, 0.0496, 0.0498 and 0.0502 at 1.0, 0.8, 0.6, 0.4 and 0.2: a
shallow bowl with its floor at 0.6, the whole of the gain the R5 Mark
II's (0.0395 to 0.0381) and the GFX flat at 0.0581. Shipped:
`shadows` 1.0 to 0.6. Our Shadows at +0.7 lifted the darks more than
Lightroom's +70, which §141 saw on the low-key interior and called an
over-lift of the mid-tones.

**The mixer's green band.** On the R5 Mark II, whose 23 Faithful
frames carry the HSL mixer's green at luminance -52 and saturation
-55 over foliage, the fit puts both scales near zero and the cost
along them is not flat: 0.0371 at the shipped luminance scale against
0.0337 at none, a tenth of the body's error. The same band on the GFX
is held by three frames and reads flat. A scale is not the fix for
that: at a tenth the import would drop an edit the user made. What it
says is that our green band lands on hues Lightroom's green band does
not, or lands harder; the foliage in these frames is yellow-green,
which Lightroom splits between Yellow and Green and we may not split
the same way. That is a band-center question for the mixer, measured
here for the first time, and left as an item.

**Before and after, the measurement's own numbers.** `greycard-ui
--measure-lightroom` on the catalog of about 280: before is
`--no-body-offset --scale shadows=1.0` with the fit-1 looks in the
store (`target/scratch/measure/cat3-before2`), after is the GFX at
-0.84 with the refitted looks (`cat3-after2`; the shipped -0.90 under
the fit-1 tables lands the GFX's median at 0.00 where this run's reads
+0.13, and the reviewer's run at -0.84 under fit-1 is above); 68 frames
registered before, 65 after, and the table is over the 65 both
measured (`paired.py`). ΔE is the mean, L ours minus Lightroom's,
offset Lightroom over us with its spread, matched the ΔE at the
matched brightness:

| body | profile | frames | before: ΔE, L, offset, matched | after: ΔE, L, offset, matched |
|---|---|---|---|---|
| R5 Mark II | Camera Faithful | 23 | 0.0378, +0.010, -0.05 ± 0.14, 0.0320 | 0.0368, -0.002, -0.00 ± 0.13, 0.0326 |
| R5 Mark II | Adobe Standard | 5 | 0.0559, +0.043, -0.34 ± 0.23, 0.0328 | 0.0514, +0.037, -0.28 ± 0.22, 0.0309 |
| R5 Mark II | Camera Monochrome | 1 | 0.0623, -0.039, +0.08, 0.0582 | 0.0629, -0.044, +0.09, 0.0578 |
| GFX 100S II | Camera REALA ACE | 13 | 0.0960, +0.089, -0.81 ± 0.49, 0.0283 | 0.0563, -0.001, +0.36 ± 0.64, 0.0301 |
| GFX 100S II | Camera PROVIA/Standard | 12 | 0.1142, +0.098, -1.21 ± 0.35, 0.0579 | 0.0684, -0.025, +0.07 ± 0.27, 0.0595 |
| GFX 100S II | Camera ACROS | 7 | 0.1196, +0.115, -1.32 ± 0.39, 0.0383 | 0.0461, +0.015, -0.30 ± 0.21, 0.0375 |
| GFX 100S II | Camera ACROS+G Filter | 3 | 0.0698, +0.063, -0.80 ± 0.37, 0.0373 | 0.0590, -0.006, +0.37 ± 0.50, 0.0300 |
| GFX 100S II | Camera Monochrome | 1 | 0.1037, +0.103, -0.57, 0.0347 | 0.0384, -0.018, +0.15, 0.0346 |
| R5 Mark II | every profile | 29 | 0.0417, +0.014, -0.09 ± 0.20, 0.0330 | 0.0402, +0.003, -0.05 ± 0.19, 0.0332 |
| GFX 100S II | every profile | 36 | 0.1046, +0.095, -1.04 ± 0.47, 0.0411 | 0.0581, -0.007, +0.13 ± 0.51, 0.0415 |
| every body | every profile | 65 | 0.0766, +0.059, -0.62 ± 0.60, 0.0375 | 0.0501, -0.002, +0.05 ± 0.41, 0.0378 |

The GFX's error halves and its lightness error goes from +0.095 to
-0.007; the R5 Mark II moves by a thousandth. The matched error, which
no offset can move, stays at 0.038: what is left is the sliders'
shape and the looks, as §333 said. The Reala Ace frames' spread (±0.64
after) is the three frames above and the rest, with the constant
between them. Over the 58 frames that registered as mapped in both
runs, without the seven the retry below rescued, it is 0.0735 to
0.0457; the three frames lost after the offset are out of both
columns. The emulation put the same change at 0.0712 to 0.0494 under
the refitted looks (its before is at shadows 0.6).

**The registration, when the render is a stop off.** The after run
first lost ten more frames than the before (fifteen against five, the
correlation 0.81 to 0.90 against the match's 0.9): the registration
is of the picture's structure, which the tone curve bends a little
differently at every brightness, and a render shifted by the offset
sat under the cutoff on frames that registered before. So a miss is
tried again with the render shifted to the export's median brightness
(a whole-picture reading that needs no registration), and the blocks
and the offset are taken on the mapped render either way, the
registration being geometry alone. That rescued seven of the ten; the
frame line says "laid at +x stops" for those. Three Provia and Reala
Ace frames still miss after the offset that registered before it, and
stay out of the table.

**An edited frame and its unedited neighbor.** `developed()` returns
early for an image Lightroom counts as never developed, so an unedited
GFX frame comes across at the engine's default and its edited neighbor
at the default less 0.9 stops: the two now sit 0.9 stops apart where
in Lightroom they sat together. Whether the offset belongs to every
frame of a body the import touches (the engine's default is then
Lightroom's for that body) or to the edited ones alone (an untouched
frame is the engine's) is a design question for the user, open here.

## The looks

**Refitted.** `--match-compare` gained `--match-write DIR`, which
writes each fitted group's tables where asked as the sheet writes them
into the store, both curves, so a refit runs headless against a copy
of the library index and the store is not touched. The three fit-1
tables refitted over the library's own sample (fitted ΔE, held out
over every frame): R5 Mark II Faithful 0.0133 and 0.0139 (22 frames),
GFX 100S II Provia 0.0198 and 0.0216 (24), Reala Ace 0.0111 and 0.0129
(40); under AgX 0.0152 and 0.0163, 0.0205 and 0.0224, 0.0134 and
0.0160. Against Lightroom they change little: Reala Ace's matched
error 0.0280 to 0.0293, the R5 Mark II's 0.0322 to 0.0339 with its
chroma size 0.0091 to 0.0075, and Provia's 0.0659 to 0.0657. So the
Provia row is not a fit-1 table's, as §333 guessed; it is three frames
of one edit with heavy local work (matched 0.09 to 0.12 against 0.02
to 0.06 for the other ten), and Lightroom's "Camera PROVIA" is Adobe's
rendering of the style, not the camera's. The tables are proposed,
not installed: `target/scratch/looks-proposed/` with a README naming
what each replaces.

**None for the monochrome styles, and none possible from the camera.**
The library index has no frame shot in Acros, Acros+G Filter or
Monochrome on either body: every GFX frame's own JPEG is Provia or
Reala Ace, every R5 Mark II's Faithful or Standard. The 12 black and
white exports are Lightroom's "Camera ACROS" and "Camera Monochrome"
profiles over color raws, with `ConvertToGrayscale` set, and the
camera match has no camera JPEG to fit against. They are measured
through the black and white section with no look and land, after the
offset, at 0.038 to 0.059 against 0.037 to 0.068 for the color rows,
so a look for them is not what the numbers are short of. A table
fitted from Lightroom's own exports is possible from the blocks the
measurement keeps and is not built: seven frames of one edit are
under the match's twenty, and the thing fitted would be Lightroom's
profile, not the camera's style.

## What it changes

`lightroom.rs`: `Scales` with the shipped constants as
`Scales::DEFAULT` (`get`, `set`, `set_from` by name), `shadows` 0.6,
`read_table_for` and `read_table_with`, `BODY_OFFSETS` and
`body_offset`, `Imported.offset`. `lightroom_catalog.rs`:
`imported_edit` returns `FrameEdit` (the edit, what was unmapped, the
body and the offset), `imported_edit_with` takes the scales, and the
import's own path applies the body's offset. `measure_lightroom.rs`:
`--blocks`, `--no-body-offset`, `--scale`, the retried registration,
the offset in every frame line and a line a body. `fit_lightroom.rs`:
`--fit-lightroom` with `--fit-rounds`, `--offsets-only`,
`--no-hold-out`, `--fit-scale`. `export::scene_framed` and
`FrameDevelop::scene` give the finish's input for the emulation.
`match_compare.rs`: `--match-write`. A block record keeps the catalog's
settings without the names the photographer gave masks and
corrections, so what sits beside a report holds no one's name. Tests:
the import writing the offset once for the body's frame and its copy
and not for another body or a picture, the RawTherapee import taking
none, the record's stripped names and its scales, the offset onto the
exposure by body and the fallback, the scales on each slider, the
offset darkening the emulated blocks and the match landing on it, a
scale found from frames made under it, the fit's tables.

## Open

- The scales the fit proposes and we did not ship: highlights, blacks,
  the curve, and the mixer's bands. Two bodies of one shoot disagree
  on them; a second shoot with history pairs is what settles them,
  and the fit reruns in minutes from its blocks once that catalog's
  exports are in the archive.
- The deep shadows: both bodies sit a stop under Lightroom under L
  0.3, and the shadows scale took part of the next band. The rest is
  the reach of Highlights and Whites under the midpoint, the feel
  track's §146 and §149, and whatever closes it on the three Reala Ace
  frames.
- The mixer's green band against Lightroom's: a band-center
  measurement, from these frames, before any scale.
- `BaselineExposure` from an Adobe DNG of each body, to test whether
  the measured offsets are the tags' difference.
- The Provia row's three heavy frames: a per-frame look at what the
  masks do there.
- Three frames that register before the offset and not after it, with
  or without the retry.
