# 266. Nikon High Efficiency raws behind a build feature (2026-10-08)

§176 left two pieces of work: run dnglab/dnglab#835 on our Z6 III frames
and report on the PR, then carry the decoder behind a build feature that
is off by default. Both are done; the report is written and not yet on
the PR.

**The PR.** It is still open. The head is unchanged since 2026-08-30
(0f044c2c), and nobody has reviewed it. On 2026-09-27 the maintainer
wrote that they want a README in the decompressor covering the format
and how Nikon's differs from plain JPEG XS, that they need time to
review, and that they would check the output bit for bit against
LibRaw's decoder once LibRaw publishes it.

**The set.** It has 90 NEF files, and 87 of them are distinct (three are
copies). All 87 come from the one Z6 III: 70 are HE\* and 17 are
lossless. None is plain HE, so the split §176 gave (64 and 23) was
wrong. Every HE\* codestream has the same header and the same fixed
length, 15,311,360 bytes. Each also carries the same 13 vendor bytes
after the standard picture-header fields.

**What it decodes.** All 70 frames decode with no error and no panic.
Black (1008), white (15892) and size match the lossless frames. We found
no level gaps: the fitted curve's slope, 2(v + V0)/D, is 0.56 DN per
code at the white level, so it never steps over a value.

**What is wrong: a floor at 1050.** The PR's decompanding clamps its
square at zero, so nothing comes out below 1050, which is 42 DN above
black. 53 of the 70 frames have their minimum exactly there. At ISO 5000
and above, a median of 13% of samples sit on that one value. We dumped
the coded values before decompanding, and the stream does carry samples
below the clamp: 12 to 17% of all samples at ISO 8000 and above. They
continue smoothly across zero. The darkest tiles there are symmetric in
the coded domain (skew about -0.1, spread both sides of zero), as read
noise around black would be under a curve that is linear near black.
JPEG XS's extended non-linearity has such a middle. In SVT-JPEG-XS it is
an inverted quadratic below T1, linear between T1 and T2, and a
quadratic above T2, joined with matching slopes. We haven't shown that
Nikon's curve is that one. Decoded through the PR, those tiles average
1058 to 1102 DN with skew +3. Lossless unexposed tiles average 1007 with
skew 0.

**The 13 vendor bytes are not an NLT segment.** We hoped they would hold
the curve's parameters. The standard's NLT segment for the extended
curve is a type byte (2), two 32-bit thresholds and an exponent byte,
and SVT-JPEG-XS parses it that way. The bytes `50 88 70 83 f0 15 23 d1
49 cd 3f 7f 07` don't read like that: no aligned 32-bit field is a
threshold in range, and no field at any bit offset gives V0, D or C. One
layout stands out: a 12-bit lead (0x508), six 14-bit fields (8642, 1008,
1352, 15636, 10036, 16255) and a byte (7). Its second field is exactly
the black level. We chose that field grid after looking at the bytes,
though, and we couldn't fit the rest to the data. So it is weak
evidence, and the exact curve stays unknown.

**The interim fix: a tangent toe, for the floor.** The fork's branch
carries a commit on top of #835 and our gate. In it, `decompand` no
longer clamps at the square's zero. Below t_k = 2836 it follows the
straight line tangent to the square at t_k: same value and slope where
the two meet, continuing down to 0 DN. Above that point (1092 DN)
nothing changes. t_k is chosen so the line passes through black (1008)
at t = 0, t_k² = D (C − 1008). That maps t = 0 to black (1008) instead
of to 1050, and adds no constant beyond the black level. It is a fit, an
interim that removes the floor, not the curve. A test checks that the
curve never falls, steps at most 1 DN a code, reaches every value from 0
to the toe, and matches slopes at the join.

On all 70 HE\* frames:
- **The floor is gone.** Nothing sits at 1050 anymore. Minima run from
  260 to 1293 DN, and no value below 1200 holds more than 1.03% of a
  frame (lossless reaches 1.49%).
- **The darkest tiles come back to black.** In the night frames at ISO
  1000 to 40000 they average 998 to 1015 DN. Skew is −0.3 to +0.14 at
  ISO 4000 and above (+0.81 at 40000).
- **The channels still don't agree at black.** Across 15 night frames, G
  − R at black has a median of +9.8 DN (−1.3 to +16.3), against +3.0 in
  lossless.
- **Highlights and everything above 1092 DN are bit for bit as before.**

**What the toe gets wrong.** The review measured noise near black in G.
It is about twice lossless at matched ISO: 28 to 31 DN against 14.6 at
ISO 1600 to 2500, and 64 to 65 against 36 at ISO 8000. The two converge
by 160 to 320 DN above black, and the HE\* noise falls as the signal
rises from black to about 100 DN, which sensor noise never does. The
lossless figures come from the darkest bins there were (40 to 80 DN
above black at ISO 8000, 20 to 40 at ISO 1600 to 2500), so they include
some shot noise. If anything, the true ratio at black is larger. So the
toe's slope at black is too steep by about two; the noise implies about
0.015 DN a code. A line through black at that slope stays under the PR's
square, so the square itself is probably too high over its lowest 150 to
300 DN.

The same shows in color. At ISO 100 in daylight, at 30 to 100 DN above
black, white-balanced R/G is 1.29 in HE\* (1.17 to 2.14, 14 frames)
against 0.91 in lossless (0.89 to 1.03, 5 frames), and B/G is 1.15
against 0.78. In every HE\* frame the ratio rises steadily as the level
falls. That is a lift common to all channels before white balance. R/G
puts it at about 40 DN at 30 to 100 DN, an order-of-magnitude estimate,
and B/G implies more.

The toe took that R/G from 1.46 to 1.25, so the floor was part of the
shadow tint but not most of it. Pixels below L\* 6 in our renders went
from a median a\* of +3.8, +6.0 and +4.0 (ISO 100 to 800, 1000 to 2500,
4000 and above) to +3.2, +5.0 and +4.5, against about −0.3 for lossless.
We first read the tint as the floor's doing, and that was wrong. The
rest most likely comes from the bottom of the decoder's curve.

At high ISO the L\* < 6 measure is also unreliable: noise around a true
black clips unevenly per channel in the render. The ISO 8000 crop went
from maroon to blue-violet chroma noise when the floor went away. We
don't change the curve again until the Adobe data says what it should
be.

**The tint against Nikon's own decode.** Every NEF carries a 6048x4032
JPEG rendered in camera from the same exposure, possibly before HE
compression. It can't give absolute levels, because its tone curve and
Picture Control are unknown. It can say which blocks the camera saw as
neutral, though, and in those blocks white-balanced camera RGB should be
equal in all three channels.

We cut each frame into 32x32-pixel blocks and registered them to the
JPEG by correlation of smoothed log luminance (shifts of up to 16
pixels; Nikon corrects distortion in the JPEG). Then we kept the flat
blocks the JPEG shows neutral within 12%. A lift L common to all
channels before white balance then reads from the red channel as L = G
(ρ − ρ₀)/(w − ρ₀), and the same from the blue. Here ρ is R·w/G, and ρ₀
is the frame's own value in its 1000 to 4000 DN neutral blocks, which
takes out Picture Control and any global tint. Red and blue give two
independent estimates, and they should agree if the lift is common.

| level above black | lossless (baseline) | HE\*, interim toe | HE\*, PR clamp |
|---|---|---|---|
| 30–100 DN | −3.5 / −2.5 (2 frames) | +21.6 / +28.6 (22 frames) | +36.2 / +42.2 |
| 100–300 DN | +12.1 / −16.3 (9 frames) | +33.5 / +22.8 (35 frames) | +35.1 / +29.0 |
| 300–1000 DN | +5.3 / −21.6 (9 frames) | +23.2 / +28.1 | +23.5 / +28.3 |

(Lift from R / lift from B, in DN.) At ISO 100 to 800 alone, HE\* gives
+22.8 / +35.3 at 30–100 DN and +34.9 / +28.2 at 100–300. The raw ratios
show the same trend directly. In camera-neutral blocks, HE\*'s
white-balanced R/G climbs from 1.03 at 3000+ DN to 1.20 at 100–300 and
1.23 at 30–100 (B/G from 1.03 to 1.08 and 1.12). Lossless stays at 0.96
to 1.00 (B/G 0.98 to 1.03).

So against the camera's own rendering of the same exposure, our HE\*
decode drifts magenta as the level falls, and lossless does not. Red and
blue agree on a lift of roughly 20 to 35 DN between 30 and 1000 DN above
black, so it is common to all channels; a green deficit would make the
blue estimate about 2.5 times the red one. The interim toe removed 10 to
15 DN of it at 30 to 100 DN and nothing above 100 DN.

The lossless baseline is thin (2 to 9 frames a bin) and scatters by ±15
DN, mostly in blue. Above ISO 800 there are only 5 HE\* frames with a
usable neutral reference, and they say little.

**What the camera implies for the curve's bottom.** We binned the same
blocks by G's coded value t. The implied true G (our toe minus the mean
lift) at the median t of each bin comes out as follows; the third column
is the PR's square lowered by 30 DN, for comparison:

| median t | implied true G (DN above black) | t²/D + 12 |
|---|---|---|
| 2530 | 46 | 45 |
| 3678 | 80 | 83 |
| 5380 | 168 | 163 |
| 7562 | 318 | 311 |
| 10463 | 598 | 584 |

The IQR across frames is about ±10 to ±15 DN. Over t ≈ 2500 to 10500 (45
to 600 DN above black), the camera is consistent with the PR's square
lowered by about 30 DN: C ≈ 1020 rather than 1050, with the same D. The
lift is measured against each frame's 1000 to 4000 DN blocks, so a lift
that runs on into those blocks is partly absorbed. The true offset at
600 DN may be closer to 35 to 40. Below t ≈ 2000 (under about 45 DN),
the JPEG is too dark (8-bit luma 4 to 7) to pin the shape, so the toe's
slope at black stays the noise argument's to settle. This is an estimate
with ±10 to 15 DN error, not a curve to ship. It does say the error is
mostly an offset in the square's bottom, not the slope of a toe.

**Not the convexity of the curve.** An unbiased codec error n in t
passes through a convex curve as a positive bias of about σₙ²/D where
the curve is the square, and none in a linear toe. At ISO 100, HE\*
noise at 128 to 512 DN matches lossless (8.05 against 8.14 DN in G), so
the codec's own error there is at most about 50 coded units: a bias of
0.01 DN. Even taking the whole dark-tile spread at ISO 100 (470 to 660
coded units) as codec error, a square from t = 0 would give at most 2.3
DN. At ISO 8000, a 2000-unit spread under a pure square would give about
21 DN. That is part of what the clamp did there, and the toe's linear
piece doesn't do it. So convexity can't account for a 20 to 35 DN lift
at ISO 100. Note that a correct decoder would show such a bias too,
wherever its curve is convex.

**Not the component transform.** The PR's inverse Star-Tetrix matches
SVT-JPEG-XS's `inverse_star_tetrix` (`Mct.c`):
- the same four steps in the same order (average, delta, Y, CbCr)
- the same access with its reflections and Cf = 3 row clamp
- the same swap of planes at the end
- its `>> 2` in the Y step is exactly SVT's `(2 (b_l + b_r) + 2 (r_t +
  r_b)) >> 3` for e1 = e2 = 1

The standard also orders the inverse transform before the non-linearity,
as the PR does.

The component transform's parameters can't be read from Nikon's stream,
which has no CTS or CRG marker, so we decoded every frame with each in
turn changed:
- e1 = e2 = 0 instead of 1
- no chroma gains

Neither changes the neutral-block lift by more than about 3 DN in any
bin, nor G − R at black by more than 0.3 DN.

At black, G1 and G2 center on the same t within about 30 units. In the
darkest blocks of ISO 100 to 5000 frames, R sits 370 to 480 coded units
below G, or 11 to 14 DN under the toe. Lossless frames' darkest blocks
also have R below G (G − R +1.7 to +6.4 DN, 4 frames). The darkest
blocks are picked by their G, which biases G low at high ISO, and dark
scene content isn't neutral. So the per-site figure (+7.5 DN median G −
R in HE\* against +2.5 lossless, over our 16 night frames) isn't clean
evidence of a per-site offset. The camera-neutral test is the cleaner
one, and it shows R and B both high against G, as a common lift does.

**The fitted curve: C = 1018, with the toe through black.** We kept the
PR's V0 and D, and the toe as the tangent through black at t = 0. We
fitted C to the camera-neutral blocks between 45 and 600 DN above black,
asking that the lift common to all channels be zero, with the site
values taken as the curve of each block's mean t.

- **The fit.** C = 1018. A bootstrap over frames gives a 68% range of
  1016 to 1022 and a 95% range of 1014 to 1028.
- **The toe follows from it.** t_k² = D (C − 1008), so t_k = 1384, with
  a slope at black of 0.0145 DN a code. The review's noise estimate,
  from HE\* against lossless noise near black at matched ISO,
  independently asked for about 0.015. Two measurements of different
  things agree.
- **Overfitting check.** Fitting on half the frames gives the same C
  (1017.8). The held-out half's lift is +4.6, −4.1 and −3.6 DN at
  30–100, 100–300 and 300–1000 DN, against +22.3, +28.8 and +9.9 for the
  same frames at 1050.
- **Where it lives.** It is the fork's next commit (ec749df1). It
  changes two constants, and the code still marks them fitted, with a
  line on how C was measured.
- **Above the fitted range.** Every value above the toe moves down 32
  DN, 0.2% at white, but the fit only constrains about 45 to 1000 DN
  above black; above that the shift is unmeasured. We held the blown
  cores fixed as the C = 1050 decode found them. The share of those core
  samples under the white level then goes from about 0.7–1.1% to
  1.8–2.4% at ISO 100 to 5000 (0.74 → 2.18% in one ISO 100 frame), with
  no visible effect in an export: blown areas differ by at most 2/255.
  Adobe's decode decides the top.

On all 70 HE\* frames:

| | PR clamp | toe at C = 1050 | fitted (C = 1018) | lossless |
|---|---|---|---|---|
| frames with minimum at 1050 | 53 | 0 | 0 | — |
| largest share on one value below 1200 | 18% | 1.03% | 2.1% | 1.49% |
| darkest tiles, night frames ISO 1000–40000 (DN) | 1051–1103 | 998–1015 | 1005–1014 (1032 at 40000) | 1007 |
| camera-neutral lift R / B, 30–100 DN | +36 / +42 | +22 / +29 | +5 / +6 | −4 / −3 |
| same, 100–300 DN | +35 / +29 | +34 / +23 | +5 / −4 | +12 / −16 |
| same, 300–1000 DN | +24 / +28 | +23 / +28 | 0 / +9 | +5 / −22 |
| camera-neutral R/G at 30–100 DN | 1.44 | 1.23 | 1.07 | 0.97 |
| G noise ISO 1600–2500, 0–20 / 20–40 / 40–80 / 80–160 DN | — | 31.5 / 24.5 / 17.6 / 19.0 | 14.1 / 12.4 / 16.1 / 21.3 | – / – / 18.2 / 25.2 |
| same, ISO 8000 | — | 66.9 / 66.9 / 64.7 / 58.9 | 41.9 / 46.2 / 49.6 / 57.3 | 80–160: 49.7 |
| ISO 100 daylight R/G, B/G at 30–100 DN (all tiles) | 1.41, 1.16 | 1.24, 1.15 | 1.09, 1.04 | 0.85, 0.59 (3 frames) |
| render a\* at L\* < 6 by ISO band | +3.8 / +6.0 / +4.0 | +3.2 / +5.0 / +4.5 | +0.8 / +2.2 / +2.5 | −0.3 / +2.8 / +0.1 |
| blown cores (fixed on the C = 1050 decode) under the white level | 0.07–1.06% | same | 0.09–2.41% (0.74 → 2.18% at ISO 100) | 0.02–1.47% |

How to read the table:
- **Lift.** The camera-neutral lift now sits inside the lossless
  baseline's scatter.
- **Noise.** It now nearly rises with signal from black, as shot noise
  does (a slight dip at 20–40 DN at ISO 1600–2500), and its level at
  black sits under lossless at 80–160 DN.
- **Render tint.** The ISO 100–800 band went from +3.8 to +0.8 a\*. The
  higher bands keep some, against lossless frames that are few there.
- **Pile-up.** The 2.1% single-value share (one ISO 1000 night frame, at
  1026) is simply that frame's histogram mode. The values around it hold
  2.02–2.11%, and the number of codes per DN is even across the toe's
  join. It is not a floor: minima run from 643 to 1261 DN.
- **Midtones.** HE\* camera-neutral blocks read about 3% magenta in
  absolute terms at 1000–4000 DN: R/G 1.03 and B/G 1.03, against 0.975
  and 1.00 in lossless. That is either Picture Control Auto (every HE\*
  frame) against Neutral (most lossless frames), or a gain on R and B
  against G in the decode. The per-channel midtone fit against Adobe's
  decode will settle it.
- **ISO 100 daylight row.** It covers all tiles, so scene color is in
  it. The camera-neutral rows are the clean measure.
- **Skew at high ISO.** Dark-tile skew is now +0.2 to +1.1 at ISO 4000
  and above (+1.95 at 40000). Coded noise there spreads well past the
  toe into the square, which is convex.

**Still open.**
- **G − R at black.** It has a median of about +7.5 DN across night
  frames, against +2.5 in lossless, and the curve doesn't move it. The
  darkest blocks are picked by G, and dark scene content isn't neutral,
  so it may not mean much.
- **The exact curve.** The fitted curve is a measured estimate, not the
  encoder's exact curve. Adobe's decode, through the comparison script,
  is the exact check, and it is pending.

**The check that settles it.** Adobe's DNG Converter reads HE\*, and its
mosaic is the reference we can't get otherwise. The check takes ten HE\*
frames from ISO 100 to 40000 and one lossless control. A script crops
Adobe's uncompressed mosaic to its ActiveArea, applies its
LinearizationTable, and subtracts its black pattern and deltas. It then
compares black-subtracted DN with ours, without rescaling; Adobe's white
is printed, not used, and the control's fit says whether Adobe scales at
all. It prints a table per channel and level, and writes Adobe's curve
against our coded value t. We tested it on synthetic DNGs covering:
- an odd ActiveArea offset
- a 2x2 black pattern
- per-row and per-column deltas
- a linearization table
- a white level unlike ours
- both byte orders, strips and tiles

We also ran it on DNGs our own decoder wrote, which matched exactly. The
converter runs only on a Mac or Windows machine, so the check waits on
one.

**What is fine.** Blown highlights stay above white: about 1% of samples
inside blown areas land within 200 DN below it, which is no worse than
lossless. Their decoded value is 16100 to 16350 at low ISO where
lossless reads 16383, which may be the fitted top end. Rows and columns
show no step at precinct or slice boundaries, and high-passed 100% crops
show no blocking. Noise at matched ISO is lossy in the expected way. G
sits at lossless (median sigma ratio 1.02). R and B are lower (median
0.78) and the noise is more correlated (lag-1 residual correlation -0.05
to -0.11, against -0.17). That is the rate control dropping chroma
detail. Color can't be judged here: there is no same-scene pair, and the
camera JPEGs used Picture Control Auto for every HE\* frame and Neutral
for most lossless ones. Decoding takes 324 ms a frame, single-threaded,
against 90 ms for lossless.

So the decoder is good enough to offer behind an off-by-default feature,
to people who build it for themselves. The curve's bottom was the fault:
a floor at 1050, and above it a 20 to 35 DN lift in every channel that
tinted the shadows magenta. The fitted curve (C = 1018, toe through
black) removes both on our frames. That is measured against the camera's
own JPEG and confirmed by the noise; the exact curve waits on the Adobe
comparison. It isn't good enough to stand in for lossless.

**The feature.** A cargo `[patch]` entry can't depend on a feature, so
the decoder has to be gated inside rawler. The fork's branch
`nikon-he-jpegxs` is our pinned rev (4b616412, one Canon fix on dnglab
main) with #835 merged on top. One commit then puts the
`decompressors::jpegxs` module and the NEF hook behind a rawler feature,
`jpegxs`, off by default. With the feature off, HE and HE\* are refused
where they always were, right after the compression mode is read and
before the strip is touched, and with rawler's old error word for word
(a copy cut off where the strip starts fails the same way on both revs).
greycard-core has a feature `nikon-he = ["rawler/jpegxs"]`, and
greycard-ui and greycard-cli pass it through. The default build and CI
never name it, so `cargo tree -e features -i rawler` shows no `jpegxs`.
Every error the decoder can return names JPEG XS, so its text is a
fingerprint: the release binaries carry none of it, against 27 matching
lines in each binary of a `nikon-he` build. The release workflow checks
both. Before the build it fails if the feature tree shows `jpegxs`,
taking the tree first so that a failing `cargo tree` fails the step.
After the build it fails if either binary contains "JPEG XS". The second
check would also catch an upstream release that took the decoder in
without a gate. Cargo checks `rawler/jpegxs` against rawler's manifest
even when the feature is off, so the feature and the rev change land
together.

In a build without the feature, an HE file used to say "decode failed:
Failed to decode image, possibly corrupt image: NEF compression
Some(HighEfficencyStar) is not supported". The file is not corrupt. It
now says "unsupported: Nikon High Efficiency raw (HE or HE\*), not
supported in this build". We recognize rawler's message by its text,
since that is the only thing that sets this refusal apart. The match
stops at `HighEffic`, so rawler fixing its spelling of HighEfficency
keeps it. building.md names the feature and why it is off. The user
guide's list of what doesn't open points to it.

What is left:
- Run the Adobe comparison; keep the fitted curve if it agrees, replace
  it with the measured one if not.
- Put the report on the PR, with the fitted curve as the proposed fix.
- Read the 2,700 lines against the standard (§176).
- Try HE\* from a Z8 or Z9 when we have files.
