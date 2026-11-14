# §665. The camera match fits a monochrome style as a grey (2026-11-13)

A look fitted on a portrait shoot in a maker's Monochrome style was not
monochrome: lips came out pale red, some areas blue. The camera match
had no notion of monochrome. `docs/camera-match.md` said its fit was
"trivial"; nothing in the code acted on that.

**Cause.** The fit is the color fit whatever the set. Its priors fill
what the data leaves free with the develop's own color:

- The matrix's ridge pulls toward identity (`fit.rs:185-189`). A
  monochrome target alone would give a matrix whose rows are one set
  of weights, so a grey out. But a muted set's chroma directions carry
  less energy than the ridge (λ = 9.9 against a trace of 1490 on the
  synthetic set), so in those directions the matrix stays near
  identity and passes the develop's chroma through.
- The table's pull holds every node no pair reaches at the matrix and
  curves (`fit.rs:411`, with smoothness at 406). The hold on greys
  acts only near the axis, and §260's flatness fades out past chroma
  0.15, so a saturated node keeps the matrix's color.

The tester's reading was right in spirit: the unsampled cells go to the
develop's color, not to a grey.

**Numbers before.** Synthetic: a portrait gamut (skin and every hue
at a fifth, input chroma up to 0.082), the camera grey by channel
weights in linear light through an S-curve. On the data, output
chroma 0.0003 mean; at six saturated colors it kept 0.044 to 0.187.
A real in-camera monochrome: an OM-1 frame in Monotone (filter and
toning Neutral), shot within a minute of the same scene in Vivid,
Natural, Muted, and Portrait. Fitted on the Monotone frame alone (1257
blocks; the scene is colorful, develop chroma p99 0.16), the table kept
the develop's color off the data: over a 9³ grid of the cube, output
chroma median 0.066, p99 0.175, max 0.189. On the scene's own colors it
was nearly grey (p99 0.002 to 0.003, max 0.006 on each frame's
develop), since the frame samples them. A second check, from real JPEGs
of other bodies turned to BT.601 luma and encoded again at quality 92:
fitted on one OM-1 Natural frame, the table over three bodies' frames
kept chroma p99 0.025 to 0.088, max 0.036 to 0.102.

**Fix.** `greycard_match::mono`. A set is monochrome when the 99th
percentile of its camera blocks' Oklab chroma is under 0.0105 and that
of its develop blocks is over 0.025: the camera dropped color the
develop shows. Then `Model::fit` replaces each camera block with the grey at its Oklab
lightness (dropping a slight cast), fits matrix, curves, and table as
for color, and sets `Model::monochrome`. `apply` then returns the grey
at the stages' output lightness, R = G = B, at every node. So the
lightness, and with it the camera's channel weighting, is exactly the
color fit's. Only the color goes. After the fix: every node has
R = G = B. On the Monotone fit: 0.0000 on the grid and on all five
frames' develops; lightness against the Monotone JPEG unchanged to four
places (mean 0.0011 on its own blocks, 0.0015 to 0.0018 with the other
four frames' develops laid under it, p99 0.0068 to 0.0080). The
re-encoded stand-in: 0.0000, its lightness error unchanged too.

**The threshold.** The OM-1's Monotone JPEG scores 0.0000 at its 99th
percentile and at its max (mean a and b +0.0000): Olympus writes a
neutral grey, so subsampling adds nothing. Its four color siblings
score 0.159 to 0.186. Also measured on the blocks of five color
frames from five makers, their JPEGs made grey and re-encoded: neutral 0.0000; one level
of Cb and Cr off neutral 0.0062 to 0.0094; two levels 0.013 to 0.019;
a weak sepia (four levels at mid-grey) 0.0117 to 0.0120; a strong one
0.032 to 0.033; the frames as shot 0.045 to 0.100. 0.0105 sits between
the one-level cast and the weak sepia, and under a visible step. A
camera with a grey two levels off neutral is read as toned and keeps
the color fit.

**The guard on the develop side.** The camera side alone would call a
color style monochrome on a set with next to no color (fog, snow, copy
work of prints), and its table would then grey every later colorful
frame of that style. So the develop side must show color too. Its 99th
percentile measured 0.044 to 0.106 on five frames of five bodies (0.106
a portrait), 0.16 on the OM-1's Monotone scene, and 0.066 on the
synthetic portrait shoot (skin and colors muted to a fifth). The bound,
0.025, sits under the least of them with margin and over twice the
camera's. A set grey on both sides takes the color fit, as before.

**In the log.** A group fitted as monochrome gets an info line with its
body and style and both percentiles; a group within a factor of two of
either bound gets one at debug.

**What we rejected.** A model of its own for monochrome sets (one set
of channel weights with a ridge toward Rec.709's luminance, one curve,
and a lightness-only table). It held lightness worse off the data:
on the synthetic set the saturated blue landed 0.11 off the camera's,
and on the real frames lightness error rose from 0.024 to 0.040 on one
body. A smaller ridge did not help on real frames. The color fit's
lightness carries the develop's own luminance where the shoot has no
data, and that measured better.

**Not a fit bump.** A color set's table is the same to the bit:
refitting the oracle's R6 Mark II Faithful pairs through the `refit`
example gives a `.cube` byte for byte equal to master's. A monochrome
table fitted before keeps its leak until it is refitted. A refit of the
same scope replaces it, since its frame count is the same. A refit from
fewer frames than the table there (a folder after a library fit) does
not: since the fit version is not bumped, `may_write` leaves the old
table alone, leak and all, unless Replace existing looks is on. Bumping
`FIT_VERSION` would show the §260 refit line, with its wrong reason, on
every color table.

**Downstream.**
- §260's flatness and coupling. Unchanged. They act on lightness, which
  is the color fit's. The coupling a monochrome table logs is mostly the
  camera's channel weighting, as it should be.
- The grey check (§517). Drift is zero; the fall is the color fit's.
- Per-curve tables. Each curve's run detects on the same camera JPEGs,
  so both tables agree.
- Look preview and masks. A look table is global (`Edit::look_lut`); a
  mask carries no table. So a mask's look and tint are rendered grey
  by the table after them.
- Saturation, Vibrance, the mixer, the black and white conversion, the
  tint, the grading wheels, and the color curves. All run before the
  table (`finish.rs` 833-839, `viewport.wgsl` 681-685), so at Strength
  1 a monochrome look erases any toning the user adds; they move only
  the grey, through the channel weighting. Lowering Strength brings the
  frame's own color back with the toning. The guide now says so.
- `--match-compare` stages carry the flag.

Is a neutral table the right minimal fix, or should a monochrome look
route through the black and white section? The table is the minimal
fix and keeps the camera's tone curve, which the black and white
section has no place for. Routing would mean fitting the section's
weights and leaving the tone to a table. That is a design change, not
a fix.

**Toned monochrome (not built).** Sepia, cyanotype: a and b a function
of lightness along one hue. Detection: on a set that fails the
monochrome test, fit a and b of the camera blocks against their own
lightness in bins (as `GreyCast` does against the input's), and call it
toned when the residual's 99th percentile is under the same threshold.
Fit: as now on the greys, then output (L, a(L), b(L)) from those bins
instead of a grey. Today a sepia set takes the color fit and leaks as
monochrome did.

**Tests.** `a_monochrome_set_renders_every_color_grey`: the synthetic
case. Fitted as color, saturated chroma is 0.044 to 0.187; fitted now,
every node is grey, and lightness matches the color fit's to 0.002.
`a_slight_cast_is_monochrome_and_dropped`, `a_color_set_fits_as_before`,
`a_sepia_set_is_not_monochrome`, and
`a_set_grey_on_both_sides_fits_as_color` each check equality with the color fit. `mono` has the percentile's
edge (one stray block in a hundred), the threshold's edges, the guard's
(both sides grey, the develop bound's edge, either side missing), when
a reading is near, and `grey`'s inverse.

**Open.**
- For the owner: a monochrome look leaves the user no way to tone the
  picture at full Strength. Toning after the look would need a stage
  after the table, or the look routed through the black and white
  section.
- Only one maker's monochrome was measured, and it is exactly neutral.
  A maker whose grey is a level or two off neutral sits near the
  threshold.
- A two-level cast reads as toned. A camera that renders its
  monochrome that warm would keep the leak until toned monochrome is
  built.
- Lightness at saturated colors no frame showed is still the
  develop's, which the synthetic set puts up to 0.067 off a camera that
  weights channels its own way. Only a set that covers them fixes that.
