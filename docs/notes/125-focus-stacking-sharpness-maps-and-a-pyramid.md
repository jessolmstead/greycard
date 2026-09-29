# 125. Focus stacking: sharpness maps and a pyramid blend (2026-09-20)

Focus stacking: the sharpness map and the pyramid (2026-09-20)

§71 put focus stacking third in the stacking order and sized it at two
to three days of engine code. It is in, as `greycard-core::stack`,
sitting on the registration §119 landed this morning: a sharpness map
per frame from local Laplacian energy, a Laplacian pyramid blend
weighted by those maps, a coverage count, and `greycard stack`. Written
from Burt and Adelson, "The Laplacian Pyramid as a Compact Image Code"
(IEEE Trans. Communications 31(4), 1983) for the pyramid and Mertens,
Kautz and Van Reeth, "Exposure Fusion" (Pacific Graphics 2007) for the
weighting; nothing ported.

**Neighbor to neighbor.** The first thing built was the obvious
thing: fit every frame straight onto the reference and merge. It does
not work, and the reason is the thing that makes a focus stack a focus
stack. Registration measures how much two frames disagree and moves one
until they agree; two frames of a focus stack disagree because one of
them is out of focus, and no transform takes that out. The ends of a
stack have next to no structure in common — one is sharp exactly where
the other is a smear — so the fit has little to hold on to and wanders.
On the synthetic five-frame stack the tests build, fitted straight onto
the middle frame, two of the four come back with the magnification
wrong in *sign*, at residuals of 0.36 to 0.58. Fitted to their
neighbors instead, and composed down the chain, the same four links
are 0.18 to 0.21 and every magnification comes back in the right
direction and within 0.004 of the truth. Not to a part in a thousand:
the frames are built 0.4% apart and the fit recovers three fifths to
three quarters of that, the defocus pulling on the rest of it, since
the fit has no robust weighting to shield it. So the fits chain
outward from the reference,
each frame against the one before it, `fit_from` seeded with the
previous link because one focus step looks much like the next. The cost
is that a link's error accumulates down the chain; for the tens of
frames a focus stack has that is a small price, and it is the trade
every focus stacker makes. A frame whose link fails is dropped and the
chain carries on from the last one that held, two focus steps away
instead of one.

**The threshold is not `Fit::aligned`.** §119's `ALIGNED_RESIDUAL` is a
twentieth, calibrated for a frame against a warped copy of itself. Two
frames of a focus stack never see that number. Measured: a frame
against a copy of itself blurred by one pixel is 0.07, by two 0.23, by
four 0.48, by eight 0.82; the synthetic stack's links are 0.18 to 0.21;
two pictures of different scenes are 1.2 to 1.7, which is the 1.4 §119
named. The gate is `Options::max_residual`, six tenths, which sits
between an implausibly coarse focus step and a frame of something
else. It is measured on synthetic frames and on two real scenes and
not on a real focus stack, which the repo does not have; the help says
so.
On real files it separates cleanly: two frames of one scene from the
sample set link at 0.22, a frame of another scene at 0.90. Beside it is
`max_motion`, a twentieth of the diagonal, because a stack is one
camera that did not go anywhere and a fit that says otherwise found
something else. Both refusals are reported per frame rather than
guessed around — a dropped frame is named, with its residual and the
threshold, and the merge is of the rest.

**The sharpness map** is the local energy of a four-neighbor Laplacian,
box-smoothed over nine pixels, taken on the *log* of the luminance and
not on the luminance. In the log the Laplacian is a relative contrast,
so a sharp edge in the shadows counts as much as one in the light and a
frame's own exposure cannot tilt the comparison — the same argument
§119 made for the fit, and it reuses the same floor, taken from the
frame's median, for the same reason: one dead photosite must not become
the sharpest thing in the picture.

**Normalizing at the end, not the start.** Mertens's order is to
normalize the frames' weight maps against each other at full
resolution and then build each frame's Gaussian weight pyramid, so
that every level's weights already sum to one. That wants every
frame's map in memory at once — a gigabyte at ten 24-megapixel frames
— or a second pass over them. What is done instead is to carry each
frame's map down its own pyramid unnormalized, accumulate the weights
beside the weighted bands, and divide once at the end.

The first version of this comment claimed the two were the same thing
because the Gaussian pyramid is linear. They are not, and the review
caught it: the effective weight here is `G(w_i) / sum_j G(w_j)` where
Mertens's is `G(w_i / sum_j w_j)`, and a blur of a ratio is not the
ratio of blurs. Measured on the synthetic stack, the two part company
by up to 0.18 in the effective per-frame weight at levels 1 to 3.

It is kept, with the comment rewritten to say what it is: per-level
renormalization of unnormalized weights, which is the Burt and
Kolczynski side of the family rather than the Mertens side. What the
seam argument needs is true of both. At level 0 they are identical,
the pyramid being the identity there, so the detail is selected
exactly as Mertens selects it. At every level the result is a convex
combination of the frames' own bands — nonnegative weights over their
own sum — so no band can overshoot and the brightness is a weighted
mean of the frames' and not a scaling of it. Both carry the weight map
down a pyramid, which is the whole of why a seam is soft. And once
the band contrast term is on, the per-level weights are not the
pyramid of any full-resolution map at all, so normalizing first is not
even defined for them.

**A flat region is their mean, not black.** The weight floor is a
fraction of a frame's *own* mean sharpness, so a region that is flat
in every frame has nothing for the floor to be a fraction of and every
weight there is zero. Dividing the zero that accumulated by the
nothing that accumulated gave black. It wants a real value to show:
`log2(0.25)` is exactly -2, so its Laplacian is exactly zero, while
`log2(0.375)` is not exact and leaves a few ulps of energy behind that
hide the fault — the first version of the test used 0.375 and passed
for that reason and no other. `weight_floor: 0.0`, which the options
offer, blacks out any flat region at any value. The fix is a third
accumulator, the frames' bands with no weights on them, which stands
in wherever the summed weight is below the smallest normal float; it
costs the memory of one more band pyramid and it is what a merge with
nothing to choose between should give anyway.

**Halos**, which §71 named as the quality problem. Where a sharp near
edge sits over a blurred far one, the frames focused far carry the near
object's out-of-focus disc as a wide soft glow over the background, and
a merge that switches frames across the silhouette brings the glow in
beside the sharp edge. The fixture for it is a sharp bar over a
background defocused by eight pixels, with the far-focused frame
carrying the bar's disc over the background at the coverage a Gaussian
gives it; the number is the background's local mean in the thirty
pixels beyond the silhouette, over its level further out, as a fraction
of the step across the silhouette. The pyramid is the answer and most
of it: the same weights with no pyramid at all leave +13.3%, and the
pyramid leaves +3.4%. The depth is what does it — one level 13.3%, two
7.9%, three 3.4%, four 3.4% — and it stops improving once the pyramid
is deeper than the blur.

What did *not* help was the thing that was built for it. A band
contrast term — at each level multiply the frame's weight by its own
`|L_k|`, smoothed and raised to a power, Burt and Kolczynski's
selection rather than Mertens's average — moves the halo by two tenths
of a percent across exponents from 0 to 8 and smoothing radii from 0 to
4. In hindsight it should not have been expected to: the glow is low
frequency, it lives in the top of the pyramid, and the top of the
pyramid is not a band and has no contrast to select on. It was kept
anyway, because measuring it against the *detail* instead showed what
it is actually for. At exponent 0 a merely blurred frame still holds a
fifth of the weight where another frame is sharp, and the merge reaches
only 83% to 90% of the best frame's sharpness in that frame's own band;
at 2 it reaches 89% to 95%, at 4, 94% to 97%. What it costs is the
averaging, and that was measured too: with white noise a three-
hundredth of the range on every frame, the merged frame's own Laplacian
energy goes from 0.0156 at 0 to 0.0200 at 2 and 0.0243 at 8, against
0.0319 for a single frame — at 2 the stack is still worth two and a
half frames of averaging, at 8 barely one. Two is the default.

Two other things, for the record. An exponent on the sharpness map
itself, to make the full-resolution weights pick more decisively: at
1.5 it is a rounding error better and at 2 and above the merge falls
apart, one frame winning regions it should be sharing, with the error
against an all-sharp frame going 0.011, 0.030, 0.049. The map is
already a squared quantity, which is most of why another power is too
much. And eroding each frame's weight map by the width of the blur, so
a frame is never trusted right up to the edge of where it is sharp:
that would take the halo further down and eat real detail at every edge
narrower than the erosion, and the width it wants is the blur radius,
which is not known. The honest version of it is a depth map, which is
its own project. It was not written.

**Seams.** A weight map that switches over one pixel at full resolution
has switched over half the frame by the coarsest level, which is the
whole of why the pyramid blend exists. Two frames each sharp in one
half, one of them a twentieth brighter, cross that twentieth over
nineteen pixels and no faster. The right half does not reach the whole
twentieth and should not: a constant lives only in the top of the
pyramid, and at the top of the pyramid the two frames are equally sharp
and are averaged, which is the right answer for every part of a focus
stack that is not about detail.

**What the merge is worth**, on the synthetic five-frame stack — a
textured plane whose defocus runs across it, each frame sharp in its
own fifth, each seen through its own small similarity. Against a frame
that was never defocused at all, the best single frame is 0.031 away
and the merge 0.0098, three times nearer. By the module's own sharpness
measure the merge is 0.0073 against the sharpest frame's 0.0052 and an
all-sharp frame's 0.0084. And the claim that means *everywhere*: the
merge's softest fifth is 0.0064, where the frames' softest fifths are
0.0005 to 0.0013 — there is nowhere the merge is as soft as every frame
is somewhere.

**Speed**, release: 0.55 to 0.7 s a frame at 24 megapixels, run to
run, and about 1.5 s at 45. Five frames of 6000x4000 in 2.7 to 3.9 s,
ten in 5.4 to 7.0 — roughly linear in the count, with a fraction of a
second of fixed cost for the accumulators. The first version of these
numbers had the five-frame case a quarter slower a frame than the
ten-frame one and called the whole thing flat; it was the warm-up,
which ran at 512 pixels and so never made the allocator ask the kernel
for the pages a full-size stack wants. It warms at full size now.

**Memory**, measured as the process's own high-water mark: ten
45-megapixel frames peak at 9.3 GB, of which 5.4 GB is the frames
themselves and belongs to the caller. The merge's share is the three
accumulator pyramids and one frame's pyramid at a time. Two things
came out of it in review. Every frame's luminance plane was held
across the whole merge for no reason — the chain needs two at a time
and everything else one at a time — so they are taken and dropped as
they are used. And the Laplacian pyramid copied the warped frame to
make its base, half a gigabyte live beside the pyramid it was being
copied into; it takes it now. CPU only; a stack is a one-off and there
is no case for a GPU version yet.

**`greycard stack <files...> [-o out.dng]`** develops each frame the way
`register` does — bilinear, hot pixel repair on, no chromatic
aberration correction, highlights clipped rather than reconstructed —
but stops one step short, at `demosaic`, which leaves camera-space
samples with no white balance and no matrix on them. That is what a
linear DNG stores, so the merge is written as one, beside the sources,
with the reference frame's color tags and EXIF and a preview rendered
from the merge itself: §71 said the linear DNG is the output for every
stack, and it is. A merge taken all the way to the working space could
not be written as one — the DNG's `ColorMatrix` and `AsShotNeutral`
describe samples the matrix has not been applied to, and a reader would
apply it a second time — so the alternative is `-o out.tif`, the same
merge through the matrix into a 16-bit linear Rec.2020 TIFF. The
command prints each frame's sharpness, which frame it was linked to,
the link's residual and overlap, how far the composed transform moves
the middle of the picture, and the reason for anything dropped — and
for a dropped frame it prints the link that was refused, and says so,
rather than the identity it never got a transform onto.

**The white balance nearly did not survive the write.** `demosaic`
divides the gains back out after the demosaic, because a linear DNG
stores camera-native samples and says what neutralizes them in
`AsShotNeutral` rather than baking it in. So the merge has no white
balance on it, and everything that turns it back into a picture — the
DNG's embedded preview, the whole TIFF path — has to put the gains on
*before* the matrix, which maps balanced camera RGB into the working
space. The first version applied the matrix alone. It gave a green
picture: channel means of 0.20, 0.40, 0.19 where a develop of the same
frame gives 0.36, 0.36, 0.35, and nothing in the suite looked at it.
There is a test now that drives the whole write path on a synthetic
mosaic — `stack_camera`, the DNG written and decoded and developed
again, the TIFF read back, and the preview with its sRGB curve undone
— and compares each one's channel means against a plain develop of the
source. It fails by a third on red and blue if the gains come off. On
real frames the stacked TIFF and a direct develop of the reference now
agree to 0.6% on every channel, and the DNG round trip is 1e-5.

**Left out.** No UI and no multi-select; that is its own line. No
deghosting — a focus stack of a moving subject is not a focus stack.
Nothing reads the focus distance out of the EXIF, so the frames are
taken in the order given and `Reference::Middle` trusts that order. No
crop to the covered region: the coverage count is reported and what to
do with it is the consumer's. And the halo number is from a synthetic
edge; the tuning §71 said the rest of it wants is against real stacks,
which this repo has none of yet. The registration's own accuracy under
defocus is the soft spot worth naming: it recovers two thirds of a
built magnification and no more, because the fit weights every pixel
alike and a defocused half of the frame pulls as hard as a sharp one.
A robust loss, or fitting on a level or two of the pyramid where both
frames still agree, would be the next thing to try.
