# 119. Registration: Lucas-Kanade on a pyramid (2026-09-20)

§71 put registration second in the stacking order and sized it at three
to five days. It is in, as `greycard-core::register`: a pyramidal
inverse-compositional Lucas-Kanade fit of a translation, a similarity
or an affine, a bilinear warp that applies one, and the tests. No
OpenCV, no feature detection, no correspondences; written from Baker
and Matthews, "Lucas-Kanade 20 Years On" (IJCV 56(3), 2004), §3.2,
nothing ported.

**What it fits.** `fit(reference, moving, w, h, &Options)` takes two
luminance planes of one size — stack frames from one camera — and
returns the transform from reference coordinates to moving
coordinates, so that the moving frame sampled at `t.apply(x, y)` is the
reference's `(x, y)`. That is the direction a merge wants: warp the
moving frame into the reference's frame and average. `warp_plane`,
`warp_image` and `warp_camera_image` do that warp; `Transform::inverse`
goes the other way. Three models, two to six parameters: a translation
for a tripod that was nudged, a similarity for handheld frames and
focus breathing, an affine for a distant scene's small viewpoint
change. `fit_from` takes a starting guess, for a long stack where the
previous frame's answer is most of the next one's.

**Stops, and why the fit does not see them.** A bracket's frames differ
by two or three stops, which a plain sum of squares reads as motion.
Three things together make the fit blind to it. The fit runs on the log
of the luminance, where a stop is an additive constant, and each
pyramid level has its own mean taken off, which removes that constant
exactly — exactly, because a constant survives a blur and a decimation
unchanged, so it is still a constant at every level. The log's dark
floor is set from each frame's own median rather than at an absolute
value, so the clamp lands on the same scene luminance in both frames
and the difference stays constant down there too. And whatever is left
after that — the mean is estimated over the whole frame, but the frames
only mostly overlap — is eliminated from the normal equations rather
than solved for: keeping the sums of the steepest-descent images and of
the error alongside their products and subtracting the outer products
is the Schur complement of a brightness parameter the fit never has to
carry. Measured: a frame put one stop down, one stop up, and three
stops down gives the same transform as the even-exposure pair, to zero
pixels of corner distance. On a real 45-megapixel frame developed a
stop apart, the fit is the identity to 0.0001 px.

The deviation is divided out too, so that the residual and the
convergence threshold mean the same thing at any contrast, but it is
the *reference's* deviation for both frames. Dividing each by its own
was the first version, and it was wrong: where the frames' content is
not quite the same — the edge a shift brings in, one frame's vignetting
— their deviations differ, which puts a gain between them, and a gain
is a mismatch the fit pays for with motion. On a 512-pixel frame a
64-pixel shift came back 0.48 px out. With one shared divisor and the
offset eliminated it is 0.004 px, and flat in the size of the shift.

**One bad sample.** A review found the sharp edge of taking the floor
from the mean: a mean belongs to its outliers. One photosite reading a
billion times the white level lifted the floor far enough to swallow
the picture, and the fit moved 0.78 px. One non-finite sample was
worse — the mean went to infinity, the floor with it, every sample
clamped, the deviation came out NaN, the comparisons a NaN loses sent
it past both guards, and the answer came back as a clean identity with
a residual of zero over all of the frame: a failure wearing the face of
a perfect fit. The floor now comes off the median, by a histogram a
quarter of a stop wide (a quarter stop being a width that whole stops
land on exactly, so two exposures still get floors exactly a stop
apart). A sample that is not finite has no log and takes the median,
not the floor: on the floor it would be a black dot fourteen stops out,
which is a feature, and one the blur spreads down every level. Zero and
below are real dark and still go to the floor.

That is the only outlier defense in the module, and the docs now say
so. The normal equations weight every pixel alike. A hot photosite
still pulls on the fit through its own gradient — it just cannot take
the rest of the frame with it: the transform holds to 0.05 px. The
residual is another matter and is written down as such, because a root
mean square belongs to its outliers as much as a mean does: that one
pixel takes the residual from 0.002 to 0.18 on a half-megapixel frame,
enough to fail the threshold on a fit that is in fact exact. A single
pixel has to be `residual * sqrt(pixels)` deviations out to matter, so
at 24 megapixels the same pixel moves it a fiftieth as much.

**Saying whether to believe it.** `Fit::residual` is the deviation of
the difference over the overlap in the units the fit works in, zero for
an exact match and about 1.4 for two frames with nothing in common.
`Fit::aligned` applies the threshold — a twentieth, with a decade of
room either side of it — so a caller has one thing to check. What it is
not is `Fit::converged`, which says the iteration stopped moving.
Gauss-Newton stops just as contentedly at the bottom of the wrong
valley: asked for a shift of twenty-eight percent of the frame's width,
the fit walks into another valley, settles, reports two thirds of the
frame overlapping and converged, and is 158 pixels wrong. There is a
test standing on exactly that case. Nor does `aligned` promise
sub-pixel accuracy — a single level asked for a shift at the edge of
what it can capture managed 1.4 px at a residual of 0.043, just inside
the threshold. Only a residual near the floor says the frames are on
top of each other to a fraction of a pixel.

A frame with nothing in it is refused rather than fitted: if a level's
log luminance deviates by less than a thousandth of a stop, or the
normal equations are singular by Cholesky's pivots, or the frames
overlap by less than a quarter, `fit` returns `Error::NoFit`.

**How far it reaches.** Cold, about a seventh of the frame's width.
Measured at three frame sizes, the pyramid holds at fifteen percent of
the width and is gone by eighteen; one level alone holds under a tenth,
which is what the pyramid is there to buy. It is a fraction of the
width rather than a number of pixels because the coarsest level is a
fixed size: what a level captures is set by the size of the detail in
it, and every doubling on the way down multiplies that by two. Which is
why the depth cap was raised until `min_side` is what ends the pyramid:
at eight levels the cap bit first on any frame over about 8000 pixels,
costing reach on exactly the largest sensors. A seventh of a
6000-pixel frame is 850 pixels, and a handheld bracket does not move
that far.

**Speed**, on this machine (16 threads), release, over the synthetic
texture, first fit discarded so the timing is not of page faults:

| frame | levels | coarsest | to level 0 | to level 1 |
| --- | --- | --- | --- | --- |
| 6 MP, 3000x2000 | 6 | 94x63 | 40 ms | 19 ms |
| 24 MP, 6000x4000 | 7 | 94x63 | 153 ms | 71 ms |

Eleven or twelve iterations over all levels, the model barely changing
the time. The full-resolution level is half the work, and stopping one
level short costs a few hundredths of a pixel, which is why
`Options::finest_level` exists. A warp of a 24-megapixel plane is
15 ms. A real 45-megapixel frame against a shifted copy of itself:
8 levels down to 64x43, 420 ms. So ten frames of a 24-megapixel stack
is under two seconds of registration — a one-off, as §71 said, and
there is no case for a GPU version yet.

The textbook inverse-compositional algorithm inverts the Hessian once
per level. Here it is accumulated every iteration, because the pixels
that go into it are the ones that land inside the moving frame and
which those are changes as the warp does; a Hessian taken over one set
of pixels and applied to a gradient taken over another is a wrong step,
and the more of the frame hangs over the edge the wronger it is. The
steepest-descent images are not kept either, and that one was measured
rather than argued: built once per level and streamed — six planes a
pixel for an affine, 576 MB at 24 megapixels — a 6000x4000 affine fit
took 211 ms against 160 ms for recomputing them from a template that is
in cache anyway, and with the allocation in play it wandered past a
second.

**Accuracy**, against known warps of a texture with structure at five
scales: a translation to 0.0005 px, a rotation of 3° and a scale of
1.03 to 0.0008 px (the scale itself to two parts in a million, the
angle to a hundredth of a thousandth of a degree), a resampled frame
with a missing border to 0.001 px. A 45-megapixel frame rotated 0.8° by
ImageMagick comes back as 0.8000° about the right fixed point, and
shifted by (12.4, -8.7) comes back as (12.35, -8.72). The one soft spot
is a strong affine: a 1.3% shear with anisotropic scales comes back
0.2 px out, and it is the resampling's error rather than the fit's —
the test now asserts both halves of that, since they are what separates
a resampling bias from a wrong Jacobian: starting from the truth lands
in the same place, and so does a single level with no pyramid under it.
Bilinear interpolation of a frame with detail near the sampling limit
biases the sum of squares by a fraction of a pixel, and with six free
parameters the bias has somewhere to go. A gentler affine, nearer what
a stack really shows, is 0.05 px. A better interpolator in the fit
would take it out; nothing needs it yet.

**Left out.** No panorama homography — §71 keeps that separate, and it
is feature matching and RANSAC, not this. No per-pixel flow and no
deghosting; both belong to the merges. Nothing about which frame is the
reference, which is the merge's decision. The fit is CPU only and
whole-frame only: no region weighting, no mask for a moving subject and
no robust loss, so a large moving object in an otherwise still scene
will pull the fit a little. A stack of frames of different sizes is
refused rather than handled.

`greycard register <reference> <moving>` fits two files — raws through
a short develop, or any picture — and prints the transform, where the
reference's middle lands in the moving frame, the scale and rotation,
the residual and a verdict from `Fit::aligned`. That is how the
real-frame numbers above were taken. The short develop keeps white
balance and the matrix, which is what makes a working image, and skips
what the fit cannot use: the good demosaic, the CA correction, and
highlight reconstruction — that last on purpose, since reconstruction
invents detail above the clip and two frames of a bracket clip in
different places, so it would invent a difference where the scene has
none. Hot photosite repair is on, which nothing else in that command
defaults to: a stuck photosite is a bright speck in the same place in
every frame, which is to say a feature that does not move, and the fit
would rather not be shown one.
