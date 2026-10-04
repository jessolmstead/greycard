# 238. The camera match under AgX, measured on the same frames (2026-10-04)

§230 chose AgX with Punchy's look as the coming default and said a
camera match refitted under it should land at a lower error than
§181's per-channel fits: "a lower error is the number that confirms
§230's call". The library was refitted under AgX from the editor on
the evening of §235, and its log gave a fitted ΔE of 0.0065 to 0.0220
per group. The only per-channel numbers on record were the September
trial's (different frames, the numpy script), and against them AgX
looked 0.002 to 0.0035 worse. Nothing there was like for like, so we
measured it properly, and asked why if AgX was worse. Three
hypotheses went in:

1. **Many to one.** AgX's compression renders scene colors the camera
   keeps apart nearly the same, and a table applied after the curve
   cannot pull them apart again. The expected signature: AgX's extra
   error is chroma and hue, in the high lightness and chroma bands.
2. **The brightness match.** The residual is mostly lightness (the
   trial's was), and the per-frame exposure match through the
   Exposure slider lands less exactly under AgX's steeper curve (19
   percent more contrast per stop, §231). The signature: the extra
   error is lightness, through the mid-tones, varying by frame.
3. **The comparison.** Frames, tool and sample size; on the same
   frames AgX is not worse.

Measured by an opus author with `--match-compare`, read twice by an
opus reviewer, who reproduced the report from the frame records and
re-developed six converted records to check them.

**The method.** `greycard-ui --match-compare DIR` runs the match's own
pieces with no window and no device: the library's groups, planned as
a fresh run would plan them (`plan_chosen` with an empty store), and
the sheet's sample of each; then every sampled frame developed once
(`FrameDevelop`, the fit's default edit) and finished under both
curves. The develop does not read the display curve; the finish does,
so one develop serves both. Under each curve the frame is laid against
its camera JPEG and matched as the match does it: `measure` was split
into `lay` (the finish, the registration, the warp, the first pairs),
`matched` (the offset solved once on the median and the pairs again at
it) and `pairs_at`, so the comparison and the match run the same code.
Then the fit (`Model::fit`) on each curve's pairs, and every way of
measuring it. Each frame's block means are kept under `DIR/frames`,
keyed by a BLAKE3 hash of the path alone, with the file's size and
time checked beside the key, and stamped with the build that made them
(the version and a hash of the binary), so a rerun of the same build
redoes only the analysis, three minutes for the library; another
build's numbers are taken only for a build named on the command line
(`--match-reuse-records BUILD`, meant for a change to the analysis
alone), and the report lists every build its records came from. A
frame whose develop or camera JPEG fails is not kept, so a read that
fails once is tried again. It writes no table, no sidecar and nothing
in the library, which it opens read-only (the run here was on a copy
of the index), and the report names no folder: the library is its file
name, the roots a count, a frame its file name. `--match-group`
narrows the run after the plan, so a borrower keeps the donor the
whole library would give it.

Per group and curve the report gives the error with no look; fitted at
each stage (matrix, its curves, the 33³ table); held out over the
sheet's four frames and over every frame; each split into lightness
(|ΔL|) and the a, b plane (and that into chroma and hue); and by the
camera's Oklab lightness band (under 0.35, 0.35 to 0.5, 0.5 to 0.65,
0.65 to 0.8, over 0.8) and chroma band (under 0.02, 0.02 to 0.05,
0.05 to 0.1, over 0.1). Because the texture, clipping and black cuts
are made on both pictures, the two curves keep different blocks, so
everything is given twice: on each curve's own blocks (the match as it
runs) and on the frames both registered and the blocks both kept.

The tests of the second hypothesis: the brightness the one exposure
pass leaves on each frame; the whole fit again with the exposure match
iterated until each frame's median sits within 0.005 stops of the
camera's; and two oracles on the held-out output, one taking each
frame's mean ΔL off its blocks, the other a line in the camera's
lightness per frame (an offset and a slope). The iteration is a secant
through the match's own two points (the residual at no offset, which
is the match's offset, and the residual at it), its slope held between
−4 and −0.25 and its step to a stop. A plain step of the residual,
which we tried first, overshoots on a curve steeper than a stop for a
stop and left 9.9 percent of the frames per channel and 18.4 percent
under AgX more than 0.02 stops off; the frames furthest off under AgX
started 0.5 to 1.6 stops from the camera with a local slope of 1.5 to
1.9. With the secant:

| curve | frames | of them redone with the secant | mean finishes, those only | stopped at the cap (8) | over 0.02 stops off | largest miss |
|---|---|---|---|---|---|---|
| per channel | 304 | 96 | 2.7 | 0 | 0 | 0.005 |
| AgX | 305 | 97 | 3.1 | 1 | 0 | 0.005 |

The other frames are the first build's, converted (below), whose plain
step had already converged; their finishes are that step's count and
are left out of the mean. For the product the cost is about two more
finishes than today, four in all: roughly half a second more a frame
on average, 1.8 seconds at the cap.

And the test of the first: a floor, blocks whose render falls in one
cell of the 33³ lattice, each against the cell's mean camera color,
which is error no table on the render could take away at the table's
resolution. Taken across every frame of a group it mixes two things,
what the camera separates that the render does not, and frames
disagreeing about the same render color; taken within one frame (the
cell keyed by frame and lattice cell) it is only the first, which is
exactly what many to one would raise.

The run: 334 frames over the library's 12 body and style groups, 9 of
which fit, 2 borrow (R7 and R8 Faithful, 3 frames each in the index)
and 1 is skipped (R6 III Neutral, 4 frames, no donor). Both roots were
mounted. 67 minutes of develops under `nice` on eight threads, about
12 seconds a frame (20 for the GFX), and 21 more for the 97 frames
whose iterated match the secant step had to redo; the analysis three
minutes.

Two builds' records make the report. The other 237 of the 334 come
from the first build's run, converted once by a script outside the
tree to the new record format (the BLAKE3 key, the build and step
stamps), keeping their plain-step iterated blocks, which is right only
because those had converged; every frame whose plain step had not was
developed again. The review checked the conversion against fresh
develops of six frames: the one-pass blocks bit-identical, the
converged offsets within 0.0054 stops of the secant's, and one cell of
the report moved in the fourth decimal. The conversion matched each
record to a file by name, size and time, and one frame that is in two
places in the library left a stale second record under the other
copy's key; the run never reads it. The AgX fits land on the user's
log to the fourth decimal in every group, frames and all (0.0160 on
36, 0.0065 on 29, 0.0142 on 22, 0.0161 on 35, 0.0119 on 40, 0.0159 on
38, 0.0123 on 40, 0.0220 on 25, 0.0155 on 40), so this is the same
sample the editor fitted.

**The numbers.** Each curve's own blocks, ΔE in Oklab, per channel /
AgX. Held out (4) is the sheet's figure, the mean of four frames'
means each held out of its own fit; held out (all) the same over every
frame; iterated is held out (all) with the exposure match converged.

| group | frames | no look | fitted | held out (4) | held out (all) | iterated |
|---|---|---|---|---|---|---|
| R5 Faithful | 36 / 36 | 0.0206 / 0.0264 | 0.0139 / 0.0160 | 0.0167 / 0.0240 | 0.0189 / 0.0218 | 0.0154 / 0.0178 |
| R5 Standard | 28 / 29 | 0.0155 / 0.0222 | 0.0056 / 0.0065 | 0.0081 / 0.0148 | 0.0105 / 0.0133 | 0.0081 / 0.0131 |
| R5 II Faithful | 22 / 22 | 0.0201 / 0.0278 | 0.0119 / 0.0142 | 0.0122 / 0.0140 | 0.0129 / 0.0154 | 0.0125 / 0.0145 |
| R5 II Standard | 35 / 35 | 0.0186 / 0.0265 | 0.0125 / 0.0161 | 0.0224 / 0.0273 | 0.0185 / 0.0248 | 0.0145 / 0.0210 |
| R6 Faithful | 40 / 40 | 0.0187 / 0.0231 | 0.0114 / 0.0119 | 0.0111 / 0.0120 | 0.0116 / 0.0121 | 0.0104 / 0.0121 |
| R6 II Faithful | 39 / 38 | 0.0225 / 0.0281 | 0.0132 / 0.0159 | 0.0172 / 0.0178 | 0.0168 / 0.0225 | 0.0126 / 0.0161 |
| R6 III Faithful | 40 / 40 | 0.0155 / 0.0212 | 0.0100 / 0.0123 | 0.0108 / 0.0133 | 0.0111 / 0.0139 | 0.0103 / 0.0119 |
| GFX 100S II Provia | 24 / 25 | 0.0410 / 0.0459 | 0.0202 / 0.0220 | 0.0225 / 0.0281 | 0.0239 / 0.0256 | 0.0208 / 0.0218 |
| GFX 100S II Reala Ace | 40 / 40 | 0.0249 / 0.0336 | 0.0133 / 0.0155 | 0.0193 / 0.0273 | 0.0197 / 0.0248 | 0.0117 / 0.0140 |

On the common frames and blocks the order is the same in every group:
fitted worse under AgX by 0.0005 (R6) to 0.0033 (R5 II Standard), held
out over every frame by 0.0005 to 0.0058. The per-channel fits here
sit where the trial's did on the bodies they share (R6 II Faithful
0.0132 against the trial's 0.0136, R5 II Standard 0.0125 against
0.0139, R5 II Faithful 0.0119 against 0.0107, Reala Ace 0.0133 against
0.0118), so the trial was not flattering per channel either.

**The third hypothesis is out.** On the same frames, the same blocks
and the same code, AgX is worse in all nine groups, and before any fit
it is worse by more: no look 0.0219 against 0.0284 over every group's
blocks. The table closes most of that and not all.

Pooled over the nine groups, block-weighted, held out over every
frame:

| | own blocks pc / AgX | gap | common blocks pc / AgX | gap |
|---|---|---|---|---|
| no look | 0.0219 / 0.0284 | +0.0065 | | |
| fitted | 0.0126 / 0.0148 | +0.0022 | | |
| held out, the match as it runs | 0.0161 / 0.0195 | +0.0034 | 0.0170 / 0.0194 | +0.0024 |
| held out, exposure match converged | 0.0131 / 0.0154 | +0.0023 | 0.0137 / 0.0154 | +0.0017 |
| held out, each frame's mean ΔL taken off | 0.0142 / 0.0163 | +0.0021 | | |
| held out, each frame's ΔL line taken off | 0.0131 / 0.0146 | +0.0015 | | |

**A third of the own-block gap is which blocks each curve keeps.** The
gap is +0.0034 on each curve's own blocks and +0.0024 on the blocks
both keep. The difference is mostly the deep shadows: of the blocks
under L 0.35 that per channel keeps, AgX loses 65,000 of 253,000,
which AgX renders darker (§231's 8 to 11 levels) and which most
likely fall under the black cut (a channel mean under 0.01). Leaving
them out is not neutral for AgX: its own-block numbers are on a
lighter set, and the toe it crushes is a real loss against the camera
that its error never sees. The common-block figures are the like for
like comparison; the own-block figures are what the sheet would
report.

**The extra error is lightness.** On the common blocks, held out over
every frame, AgX is 0.0194 against 0.0170: +0.0024, of which the mean
|ΔL| moves +0.0024 and the a, b distance +0.0002. In every group
lightness is 91 to 99 percent of the squared error under both curves,
and the chroma and hue error is 0.0014 to 0.0028 under both, moving by
at most 0.0003. By the camera's lightness the extra error is spread
through every band, not gathered at the top:

| camera L | share of blocks | held out pc / AgX | contributes to the +0.0024 | of it ΔL / Δab |
|---|---|---|---|---|
| < 0.35 | 31% | 0.0126 / 0.0145 | +0.00058 | +0.00058 / +0.00006 |
| 0.35-0.5 | 18% | 0.0209 / 0.0231 | +0.00039 | +0.00038 / +0.00005 |
| 0.5-0.65 | 16% | 0.0224 / 0.0247 | +0.00038 | +0.00037 / +0.00002 |
| 0.65-0.8 | 18% | 0.0183 / 0.0218 | +0.00060 | +0.00061 / +0.00003 |
| >= 0.8 | 17% | 0.0145 / 0.0171 | +0.00044 | +0.00044 / +0.00003 |

By chroma it follows the share of blocks: the neutral and near-neutral
blocks (C under 0.05, 82 percent of them) carry 0.0018 of the 0.0024.
In the most saturated band (C over 0.1) AgX's a, b error is 0.0080
against 0.0064 and the look leaves it less saturated than the camera
(mean ΔC −0.0032 against −0.0021), which is the first hypothesis's
color signature; but that band is 1.8 percent of the blocks and puts
0.00003 into the gap.

**The floor decides between the first two.** The a, b split alone
cannot rule out many to one, since a compression of the camera's
tones (two lightnesses the camera keeps apart rendered as one) would
also be lightness. The floors can:

| common blocks | across frames pc / AgX | within a frame pc / AgX |
|---|---|---|
| exposure matched once | 0.0152 / 0.0169 | 0.0098 / 0.0100 |
| exposure match converged | 0.0132 / 0.0144 | 0.0096 / 0.0098 |

Within one frame AgX separates the camera's colors and tones as well
as per channel does: 0.0100 against 0.0098, in every lightness band
(AgX 0.0075 to 0.0128, per channel 0.0072 to 0.0123, never more than
0.0007 apart in a band) and in the saturated band (0.0100 against
0.0096). There is no many to one to speak of. AgX's whole excess in
the floor is across frames, where blocks of the same render color from
different frames disagree about how light the camera made them (0.0169
against 0.0152, 94 percent of it lightness under both curves). That is
the second hypothesis's mechanism, measured directly.

**The second hypothesis is what the numbers carry.** The extra error
is lightness, through the mid-tones as much as anywhere, and it varies
by frame: the spread over frames of the held-out mean ΔL is larger
under AgX in all nine groups (R6 II Faithful 0.0124 against 0.0186,
Reala Ace 0.0183 against 0.0253, R5 II Standard 0.0141 against
0.0212), and within the lightness bands it is larger in 41 of the 45
group and band cells. The one-pass exposure match lands less exactly:
the brightness it leaves on a frame (the pairs' median after the pass,
in stops) spreads wider under AgX in all nine groups (R6 II Faithful
0.19 against 0.27, Reala Ace 0.19 against 0.26, R5 II Faithful 0.03
against 0.13), and over the 304 frames held out that residual
correlates with the frame's held-out mean ΔL at −0.56 per channel and
−0.54 under AgX: the pass's miss is the look's lightness error, frame
by frame. Converging the match takes the common-block gap from 0.0024
to 0.0017 and the across-frames floor gap from 0.0017 to 0.0012. What
is left is not brightness at the median but contrast around it: the
per-frame line through the lightness error slopes more steeply under
AgX in every group (R6 II Faithful −0.042 against −0.065, R5 II
Standard −0.044 against −0.072), and taking that line off each frame
brings the own-block gap to 0.0015. One exposure per frame cannot
align a frame at every tone when the develop's contrast is not the
camera's, and where it misses depends on where the frame's tones sit,
so the shared table cannot learn it.

This fits what the user saw: they looked at the AgX-matched looks and
said they "look pretty good". The color is the camera's under either
curve; what differs is how light each frame comes out, which a look
seen frame by frame shows least.

**The borrowers.** R7 and R8 Faithful have three frames each in the
index (the R7's from one folder, the R8's from two). Every Canon
Faithful table on their frames, per channel / AgX:

| table | on the R7 | on the R8 |
|---|---|---|
| no look | 0.0149 / 0.0205 | 0.0102 / 0.0143 |
| R5 | 0.0142 / 0.0201 | 0.0130 / 0.0163 |
| R5 II | 0.0188 / 0.0259 | 0.0166 / 0.0198 |
| R6 | 0.0134 / 0.0165 | 0.0126 / 0.0152 |
| R6 II (the plan's, most frames) | 0.0204 / 0.0224 | 0.0115 / 0.0128 |
| R6 III | 0.0188 / 0.0242 | 0.0164 / 0.0244 |

On color the plan's donor is the right one for both: the R6 II's
table leaves the lowest a, b error on the R7 (0.0023 / 0.0025) and on
the R8 (0.0022 / 0.0019), against 0.0027 to 0.0087 for the others. On
the R8, which shares the R6 II's sensor, it is also the best table
overall under both curves (though under per channel no table beats
none on those three frames). On the R7, the APS-C body, it is the
worst of the five under per channel and third under AgX, worse than no
look under both, and all of that is lightness (|ΔL| 0.0201 against an
a, b error of 0.0023). Three frames from one session cannot say
whether that tone is the body's or the session's; it is the same
per-frame lightness as above. So "most frames" picks the donor whose
color transfers best, and what goes wrong on a borrower is tone, which
neither the donor choice nor the sensor explains on this little.

**The sheet's four-frame figure.** The held-out error over four frames
is too noisy to size a gap. It is off the every-frame figure by up to
0.0039 (R5 II Standard 0.0224 against 0.0185 per channel), and on the
R6 II Faithful it puts the gap between the curves at 0.0006 where
every frame puts it at 0.0057. On each curve's own blocks it does rank
the curves the same way as every frame in all nine groups. Holding
every frame out cost about three seconds a group on eight threads.

**What it means.** §230's confirming number did not come: a camera
match under AgX is 10 to 20 percent worse than under per channel on
the same frames, fitted and held out. That is not a judgment of AgX as
a rendering, and the user's look at the AgX tables agrees. The match
measures the distance to the camera's own rendering; per channel's
tone is nearer the cameras' (no look 0.0219 against 0.0284), and a
table fitted after AgX must undo more of AgX's tone, of which the
per-frame part is what it cannot undo.

The fix the numbers argue for is the brightness match, and it helps
both curves more than the curve choice costs. Converging the exposure
match (a secant step, about two more finishes than today, four in all,
roughly half a second a frame and no extra develop) takes the held-out
error from 0.0161 to 0.0131 per channel and from 0.0195 to 0.0154
under AgX on own blocks, and from 0.0170 to 0.0137 and 0.0194 to
0.0154 on common blocks. So an AgX table with a converged match is
better than today's per-channel table, 0.0154 against 0.0161, and that
comparison is on own blocks, which are biased against AgX (on common
blocks it is 0.0154 against 0.0170). Beyond that, a per-frame contrast
term during the fit (the frame matched at two quantiles rather than
its median, the look then carrying the average) is worth measuring:
the offset and slope oracle puts its ceiling at 0.0131 and 0.0146.
Fitting part of the match on scene values before the display curve,
the first hypothesis's remedy, is not supported: the within-frame
floor is the same under both curves, and the color part of the gap is
0.0002. The sheet's held-out figure should be over every frame. And
the black cut costs AgX a quarter of its deep shadows; a cut on the
camera's side only, or one scaled to the curve, would let its table
see the toe it crushes. None of this is implemented here.

**Not measured.** The two borrowers have three frames each, so the
donor table is a sign, not a finding. The R6 III Neutral group (four
frames, no donor) was not developed. The residual after the offset and
slope oracle was not decomposed, and whether the 65,000 lost shadow
blocks went to the black cut or the texture cut was not counted. The
radial term was measured per lens under both curves and moves by about
0.01 or less between them except on a few lenses with one or two
frames; it is not the curve's business.