# 71. Stacking and stitching: the plan (2026-09-18)

The three roadmap items under "Stacking and stitching" were one line
each with nothing behind them. This is the assessment, so the order and
the sizes are on record before any of it is started.

More of the plumbing exists than the items suggest. The linear DNG
writer (`dng.rs`) is the output for every stack: a merge becomes a new
DNG beside its sources and opens as an ordinary file, the pattern the
denoiser cache has used since §37. Its `headroom` and `BaselineExposure`
already carry values above the white level, so an HDR merge fits the
format as it stands; a 16-bit file holds sixteen stops above black,
enough for a three-frame bracket at ±2 EV on a fourteen-stop sensor. A
deeper bracket wants DNG 1.4's float samples, which the writer does not
do yet and need not until someone asks. EXIF parsing reads shutter,
aperture and ISO, all an HDR merge needs to put the frames on one
scale. The develop path gives demosaiced camera-space RGB before white
balance, the domain to merge in. Geometry has the perspective warp on
CPU and GPU, so applying a fitted transform is done.

What is missing is registration and a pyramid blend. There is no
feature detection, phase correlation, optical flow or image pyramid
anywhere in the crates, and every stacking feature except tripod HDR
needs frames aligned. There is no Laplacian pyramid either; focus
stacking needs one for its blend, and panorama's multi-band blend is
the same code.

HDR merge, on a tripod, is a weighted average: each frame scaled by its
exposure, clipped samples dropped per channel, the rest weighted by
signal-to-noise so the longer exposure wins wherever it is not clipped.
The merged file must then skip highlight reconstruction and the
white-level clip in develop, since it has no clipping point; that is a
flag on the shot, not a new pipeline. Ghosts from moving subjects are
the open-ended part. A reference-frame consistency test (a sample that
disagrees with the reference by more than the noise predicts falls back
to the reference) handles most of them and is the version to ship;
better deghosting is research. Handheld brackets need translation and a
small rotation, which arrive with the registration module.

Focus stacking's merge is a sharpness map per frame, local Laplacian
energy, and a Laplacian pyramid blend weighted by it so the seams do not
show. That is a few hundred lines. The cost is alignment: focus
breathing changes the magnification from frame to frame, so even a
tripod stack needs scale, translation and a little rotation fitted per
frame. The quality problem is halos where a sharp near edge sits over a
blurred far one; the pyramid blend gets most of the way and the rest is
tuning against real stacks.

Registration will be a pyramidal inverse-compositional Lucas-Kanade fit
of a similarity or affine transform, in core, with a CPU reference and
tests like every other op, rather than OpenCV bindings. Stack frames
differ by small transforms, so a gradient-based fit on a Gaussian
pyramid converges without feature matching. It serves focus stacks,
handheld HDR and later the refinement pass of panorama stitching.
Panorama's coarse homography, where the overlap is partial and the
transform is large, is feature matching and RANSAC, a separate piece.

Everything runs on the CPU to start. A stack is a one-off operation,
and ten 24-megapixel frames through a pyramid is seconds, not a
viewport concern; GPU versions can follow if anyone waits on them.

Rough sizes, in working days of engine code before UI: tripod HDR with
the no-clip develop path, one to two; the registration module with
tests, three to five; the focus merge and pyramid blend, two to three;
deghosting for handheld HDR, two to four; the browser's multi-select,
the merge action, progress, and the new file appearing, two to three.

The order: tripod HDR first, since it ships alone; then registration;
then focus stacking; then handheld HDR; panorama last, since it reuses
the blend and the warp. The roadmap carries the pieces and what each
waits on.
