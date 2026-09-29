# 180. The camera match, tried and shaped (2026-09-26)

§78 proposed fitting the maker's picture style as a look from the
JPEG every raw carries. The trial ran today on four sets from the
author's archive, and its numbers and its script are in
`docs/camera-match.md` and `tools/camera-match/fit.py`. This section
is what the trial decided about the product.

**It works, and the residual is not the look's.** Held out one frame
at a time, a matrix, per-channel curves and a 33³ LUT bring the
develop within a ΔE of 0.012 to 0.019 in Oklab of the camera's JPEG
on R6 II Faithful, R5 II Standard, R5 II Faithful and GFX Reala Ace,
with chroma error under 0.002 on every set. What remains is
lightness, and it runs with radius from the frame's center: the
camera's own vignetting correction is about half of lensfun's on the
RF 50mm f/1.2, the opposite sign on the RF 70-200mm, and the whole of
the lens's falloff on a Sigma that has no profile at all. That is the
lens block's business and it is on the roadmap there. Crossed, the
looks read the style: Faithful and Standard on one body are four
times apart, and Faithful on the two Canons is nearly one rendering,
a look from one body serving the other at about twice its native
error.

**Where it lives.** An action in the Look section: "Fit this camera's
look". It runs over the current folder, or the library where there
is one, groups the frames by body and picture style from the maker's
tags, drops the frames with an adaptive setting on (Canon's Auto
Lighting Optimizer and Auto style, Fujifilm's DR200 and DR400,
Nikon's Active D-Lighting, Sony's DRO) and tells the user how many it
left out and why, and writes one table per group into the looks
store, named by body and style. The looks are then looks like any
other, with the strength slider, and a preset can carry one. Nothing
is shipped: the tables are the user's, from the user's frames, which
is what keeps the makers' names off anything we distribute.

**What the engine needs: nothing new in the pipeline.** The pieces
are all there. The camera's JPEG is decoded for the culling loupe
(§123) and the orientation with it; the export path develops a frame
at a chosen long edge; the look slot (§127) applies the result. The
new code is the fit in a crate of its own, `greycard-match`, that
depends on core and nothing of the UI: registration, block means with
the texture and clipping cuts, the ridge matrix, the curves, and the
LUT. Every step has its CPU reference in the Python already, which
becomes the test oracle: the same 32 R6 II frames' block pairs,
committed as a small fixture of numbers rather than pictures, since
no raw or slice of one goes in the tree, fitted by both and required
to agree.

**The LUT is the one piece not to port as is.** The script fits the
residual with thin-plate radial basis functions from scipy and pulls
it toward zero away from the data. In Rust the same job is a
regularized least squares on the lattice itself: each block pair
votes for the eight nodes of its cell with trilinear weights, a
second-difference term along each axis asks the table to be smooth,
and a small term pulls every node toward the matrix-and-curve
prediction, which is what "identity where there is no data" means
here. It is a sparse symmetric positive system of 33³ × 3 unknowns
and conjugate gradients solves it in under a second with no
dependency. It is also the better model: the thin-plate fit made the
GFX look overfit its thin frames where the lattice's smoothness term
would have held it.

**The radial term is measured and kept out.** During the fit, after
the look, the lightness residual is regressed on radius per lens, and
the fit reports it: "on the RF 50mm f/1.2 the camera corrects about
half of what the profile does". The number goes nowhere near the
table. It is the measurement the vignetting roadmap line wants, made
for every lens in the library at no cost.

**Borrowing.** A group under about twenty usable frames, or one whose
matrix the ridge had to hold (its data leaving a channel free), does
not get a look of its own. It gets the same maker's look for that
style from the body that has the frames, named for what it is, since
the cross table says that lands at about twice the native error and
is far better than none. The ridge stays in either way: twenty-two
frames of one warm room ran the blue diagonal of a plain least
squares to 0.4.

**The per-frame brightness is not the look's either.** The camera's
JPEG sits at its own exposure per scene, 0.2 to 0.9 stops under
greycard's default on the R6 II's Faithful, 0.3 to 1.4 under it on
the GFX's Reala Ace. The fit solves that offset per frame and applies
it through the Exposure slider before fitting the shared table, so
the look carries the style and the brightness stays where the user
can see it. Those offsets are also the measurement the v0.2.0 feel
line "the default develop matches the camera JPEG's brightness" was
waiting for, and they point the other way from §141's +0.8 set
against Lightroom. That is a decision, not a finding, and it is
open.

**Scope: the library by default, a folder allowed.** A folder is
usually one session, one place, one light, one lens, which is the
R5 II Faithful case above: 22 frames of a warm room left blue
unconstrained, fitted themselves well and transferred badly. The
library holds every body and style across years of places, which is
the coverage the table needs. So the action defaults to the library
where one is indexed, offers the open folder as the fallback, and
says when a folder-only fit is one session. It does not develop the
archive: per body and style it samples about 40 frames spread across
folders and dates, preferring the adaptive settings off, and
develops only those, 2.5 seconds a frame at 2048 wide on the
desktop, so a group is a minute or two under a progress line. Under
20 usable frames, the borrowing rule. The radial term is regressed
per lens across the whole run, which is the vignetting measurement
for every lens the user owns.

**The tags live in the index.** Nothing in the tree read a maker's
picture style or its adaptive settings; rawler parses Canon's maker
note and keeps it to itself. So a reader in core's `decode::style`
gives one file's maker, style, each adaptive setting and whether it
was on, and the camera's own peripheral correction flag where the
file carries one: Canon from the CR3's CMT3 box through rawler's
public box and TIFF parsers, Fujifilm from the maker note in the
JPEG the RAF embeds, checked against exiv2. The library index gains
those fields beside the body and lens, filled at index time and by a
migration on the next scan, so "every R6 II Faithful frame with the
adaptive settings off" is one query and not nine thousand file
opens. A folder without an index reads the files directly, which is
cheap at folder scale. The same fields are a facet for the filter
bar, style beside body and lens, and the peripheral flag is the
per-frame truth the radial measurement wants on whether the camera
corrected at all.

**A look applies anywhere; the picker says the match.** A look is a
table on display-referred sRGB after the accurate profile, so a
Reala Ace look on a Canon frame is well defined and is §78's "give
me the film sims" use. The cross table puts a GFX look on Canon
frames at 0.006 to 0.010 of chroma error against the real rendering:
visible, plausible, and the strength slider is there. Each fitted
table carries the body and style it came from in its title, and the
picker says nothing for the same body and style and "fitted on the
R6 II" for another body or maker. Unlike the camera profile, which
is a measurement and is left off for another body (§122), a look is
a preference and applies. A preset carries a look by name and applies
it on another body with the note, never silently switched to that
body's own, since a silent substitution is the one thing the user
could not see.

**The crate, landed the same evening.** `greycard-match`, held to
the Python's numbers on the R6 II Faithful fixture: fitted 0.0134
against 0.0136, held out over 32 frames 0.0168 against 0.0166, the
matrix within 0.002 an entry. The lattice least squares turned out
to be the better model and not only the port-friendly one: the
smoothness prior is explicit, the empty corners are held by a term
rather than a decay from the nearest sample, and it lands on the
thin-plate fit's held-out error with no dependency and no sampling
step. A sweep over smoothness 0.1 to 3 and pull 0.005 to 0.1 is flat
from 0.3 up, so 0.5 and 0.02 sit in the flat part. Conjugate
gradients on 33³ unknowns a channel, 300 iterations, under a second
in release.

Two things the trial's script had wrong came out in the review and
were fixed in both. The registration's refinement pass searched
around zero rather than around the first pass's shift, so any shift
over three pixels was lost while the frame still passed the
correlation cutoff; today's frames all sat under two pixels, which is
why the trial never saw it. And the script's inside mask, a warp of
ones with the edge repeated, was one everywhere and cut nothing. Two
more were the crate's own: the curves' bin minimum was an absolute
30 samples, which on a subsampled set dropped the top bins and left
white rendering as a faintly cyan grey, so the minimum scales with
the set and the model pins white to white outright; and the held-out
assertion's tolerance could not tell a working table from none, so
it is tighter and also requires the table to beat the curves alone.

**The style reader, landed the same evening.** `decode::style` in
core, checked against exiv2 on 315 files. Two tables the plan was
written from were wrong and exiv2 was right: Canon's LightingOpt
array is maker note tag 0x4018 (0x4020 is AmbienceInfo, and reading
it would have marked every frame as having the optimizer on), and
Fujifilm's FilmMode codes are not the ones in circulation (0x120
Astia, 0x200 Velvia, 0x500 and 0x501 the Pro Negs, 0x600 Classic
Chrome, 0x700 Eterna, 0x800 Classic Neg, 0x900 Bleach Bypass, 0xa00
Nostalgic Neg, 0xb00 Reala Ace, the monochromes in the Color tag).
The tag tables come from exiv2's canonmn_int.cpp and fujimn_int.cpp,
GPL-2.0-or-later, named in the file header. The camera's own
vignetting correction on the R bodies is in VignettingCorr2, not the
older LightingOpt flag, which reads off on every CR3 including the
R6 II frames whose JPEG measurably carries the correction; that is
the per-frame truth the radial measurement wanted, and it says the
camera did correct, at about half the profile's strength. A setting
the reader expects and cannot find or name counts as on and is
marked not known, so no frame is called fixed on a guess; Fujifilm's
D-Range Priority is read and counts as adaptive. The group key
carries the maker, since both makers have a Monochrome, and the
per-body fit adds the model. Not seen yet, so a group may mix them:
Canon's per-style contrast, saturation and color tone, and
Fujifilm's Color beside a simulation, the Color Chrome effects and
Clarity. Nikon, Sony and Panasonic come back with the maker and no
style until files with the settings varied make exiv2 a clean oracle
there. One thing found on the way: rawler's TIFF reader follows the
next-IFD pointer with no cycle check when given no chain limit, so a
crafted file whose IFD0 points at itself grows memory without end, a
hang the panic guard cannot catch. The reader sidesteps it by
reading IFD0 alone, but rawler's own CR3 path parses the CMT boxes
the same way; that goes upstream.

**Where the look sits against the sliders.** The table is the last
stage before the output transform (§127), so exposure and tone, the
mixer, the color and the tint in Oklab, the curves and the black and
white all happen before it sees a pixel. A fitted look is therefore a
rendering rather than an adjustment: at the defaults it gives the
camera's picture, and after a slider it gives the camera's rendering
of the adjusted picture, which is what the camera would have done had
the scene been shot that way. The consequences, so nobody is
surprised by them: a mixer hue shift lands on the look's input and
the look then applies its own saturation and hue behavior to the
shifted color, mild for Faithful, a real bend for a strong film
simulation; a global tint and a look with a hue twist add rather than
replace each other, predictably, since the table is smooth and
monotone; a mono picture stays mono, because a look fitted on color
frames maps neutrals to neutrals within the trial's 0.002 of chroma,
and takes the look's tone; a local adjustment's own look and tint act
where its mask says and the global table then renders that patch too,
the same rule as the global sliders. The other order, the look first
and the sliders on the styled picture, is how a grade works in a
video tool and would make the sliders more literal and the look less
like the camera; it is not available anyway, since a table made for
a display can only be read where the picture is display-referred.

**Order.** The crate with the fit and its oracle test, and the style
reader in core, in parallel; then the action with its index fields,
its grouping, its sampling and its report; the borrowing rule; then
the radial report into the lens track. The chart-based profile of
§78 stays behind it and shares the block sampling.
