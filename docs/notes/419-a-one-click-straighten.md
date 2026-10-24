# §419. A one-click straighten

*2026-10-24*

The Crop tab's ROTATE section has a Straighten button beside Level. It
finds the picture's dominant line and levels to it as if the user had
drawn that line with the level: the same `leveled_by`, crop re-fit, and
history step.

**Detector.** `greycard_core::horizon` returns the dominant line's
angle, folded to ±45°, and where it lies, or `None`. We tried the
suggested orientation histogram first (structure tensor, 0.1° bins, a
Hough refinement). It read synthetic edges within 0.02° but failed on
real frames: a sea horizon measured at 0.90° read 1.71°, and a wire in a
sky outvoted the sea. We use line segments instead: Scharr gradients
thinned to their crests (non-maximum suppression), grown LSD-style while
each pixel's gradient stays within 10° of its region's, each region's
angle its principal axis refit twice on the pixels within a pixel of it,
and regions wider than 3 px dropped. Thinning and the refit keep near-
parallel edges (a horizon and a cliff top 20 px above) from joining into
one tilted region, whose angle moved by up to 1° with the rendering.
Segments vote their length into a histogram modulo 90°; the answer is
the length-weighted mean within 0.5° of its peak.

**Rejection.** A line counts only when its segments add to a quarter of
the long edge, the longest alone reaches a fifth, and they hold a tenth
of all segment length. Ridges, forests, a wire, strong converging
verticals, and noise fail; a checkerboard reads 0 or nothing.

**Convention.** The detector reads the develop on the GPU, the source
before the edit's turn, as a grid 2048 long rounded to a whole texel
step (2000–3000 wide on 6000-pixel frames), shrunk by a whole factor to
about 1000. The line is taken to the screen through the geometry in
force at the segments' own middle, since a keystone turns a line by
where it lies; a leveled frame reads level and a second press changes
nothing. Changes under 0.01° are not applied.

**Cost.** The grid readback stays on the UI thread: 28–42 ms in a
release build (230–510 ms in debug). The gray and the detector run on a
thread of their own: 51–87 ms press to result in release.

**Measured.** Spread of the found angle on three coast frames across the
GPU gray, it sharpened, it at gamma 1.4, and the camera's preview:
0.12°, 0.09°, and 0.24° (the last the preview against the rest). In the
editor they read 0.72°, −0.20°, and −1.04°, and a second press logged
+0.00°. The first horizon bows with its wide lens; after the press its
left half traces at 0.10° on the window capture.

Two things the review left as nits, recorded here. `Line.at` averages
every segment at the peak angle, verticals folded onto it included, so
with a strong post beside a horizon the keystone mapping may read the
line between them. And one of the four coast frames moved from −0.96° to
+0.04° between the first detector and this one; edge traces of it range
from −0.49° to +0.48°, so which is right is for the eye, on a capture.
