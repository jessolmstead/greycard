# 268. The capture sharpen on a pedestal (2026-10-08)

At high ISO the capture sharpen turned single near-black pixels into
full-brightness magenta, blue and white dots. On 5M0A7337 (EOS R6
Mark II, ISO 10000) a headless export with the default edit had 1,162
of them; the ISO 6400 and 4000 frames beside it had 2,784 and 552.
(A dot here: a pixel whose brightest channel is over 0.8 in the 16-bit
export while the 5x5 mean of its Rec.709 luma is under 0.08.) Turning
the sharpen off took them all away; the lens, CA, color and light
sections changed nothing. The dots were the same from run to run and
the same on the CPU and the GPU paths.

### Where they came from

The sharpen is RawTherapee's capture sharpening, which deconvolves
camera values that are never negative. Ours runs on the working
image, linear Rec.2020 after the camera matrix, where the noise at
black goes below zero: on 5M0A7337, 3,009 of the 24 million pixels
have a negative luminance. We logged the sharpen at the dots on the
CPU (the default settings; the automatic threshold finds no flat
patch on this frame and lands at zero, so the blend is one
everywhere).

Both steps we suspected make the outliers, and the first of them
makes most of them:

- **Richardson–Lucy on a luminance about zero.** Each iteration
  divides the original by the blurred estimate (floored at 1e-6) and
  multiplies the estimate by the blurred ratio. Where the blur is
  near zero or negative the ratio runs to hundreds and changes sign,
  and the estimate with it. At the dot pixels the old luminance had a
  median of −0.00033 (90% of them negative); the deconvolved
  luminance had a median of 0.031, a 90th percentile of 0.34 and a
  maximum of 844, against a neighborhood near 0.006.
- **The division by the old luminance.** Every channel was multiplied
  by `new / max(old, 1e-5)`. With the old luminance negative that is
  the new one times 1e5: at the dots the factor reached 1.5e5, and a
  pixel of (0.001, −0.0015, 0.0067) came out at (11, −18, 78).

Taken apart: the deconvolution as it was with a bounded scaling (the
one below) still left 402 dots, now neutral and up to 1,098; a stable
deconvolution with the old division left 62, colored. Both fixed, none.

### The fix

The deconvolution runs on `Y' = max(Y, 0) + k`, the luminance clipped
at zero on a pedestal `k`, and every channel is scaled about the
pedestal's negative: `c' = (c + k) · new' / old' − k`, with `old' = Y'`
and `new'` the deconvolved and blended `Y'`. Richardson–Lucy then has
positive data, which is what it assumes, and a factor never meets a
luminance under `k`. Well above `k` it is the old gain to within
`k / c`; below it, the change is nearer an equal step in every channel
than a gain, and the deconvolution is nearer linear than Richardson–
Lucy's multiplicative steps (which go slowly in the dark). A channel
under `−k` stays under it, scaled away from it by a factor that the
pedestal keeps near one. The blend mask and the clip mask read the
luminance and the channels as before: the pedestal is only under the
deconvolution and the scaling.

The GPU sharpen takes the same change in its three shaders. `k`
reaches them through the uniform from the reference's constant, as
the rule below does, so the two paths share the numbers and not
copies of them.

### k

We chose a fixed `k` of 0.001 of white (`BLACK_PEDESTAL`).

The concern was that `k` must clear the noise at black: a pixel
clipped to zero and then raised by its neighbors would get
`(new + k) / k`. Measured, it does not happen. The robust spread of
the luminance at black on 5M0A7337 is 0.00105. With `k` anywhere from
3e-5 (a thirtieth of that) to 0.02 the frame has no dot and no pixel
in its blacks rises by a hundredth of white; at the pixels whose
luminance was under zero the factor stays at or under one (at
`k = 0.001`, a 99th percentile of 0.96), because a pixel the clip
raised to the pedestal is a local minimum, which the deconvolution
deepens and does not lift. At the panel's strongest (radius 2, 50
iterations, threshold zero) `k = 0.001` leaves no dot and no pixel
rising by a hundredth (128 rising by between 0.005 and 0.01, grain),
where the old sharpen made 953 rise by over 0.02.

What `k` does cost is change at base ISO, which grows with it: on
5M0A3976 the 99.9th percentile change of pixels over a luminance of
0.1 was 1.4e-3 at `k = 0.001`, 2.4e-3 at 0.002 and 5.2e-3 at 0.005
(linear, measured with the stop rule read on the luminance exactly,
the second form below). So the smallest `k` that is clean everywhere, and since a
thirtieth of ISO 10000's noise is still clean, one fixed number
covers any ISO a camera sells. A `k` from the frame's measured noise
would have needed the noise model carried to the export's CPU path,
its output sharpen and the GPU worker, for nothing the measurement
asked for.

### The early stop

RawTherapee stops a block's iterations when any pixel's estimate falls
under half its blended start, before a halo goes dark. On the
pedestal there were three ways to read that; we measured each on
5M0A3976 at `k = 0.001`, counting pixels over a luminance of 0.005
that the sharpen takes under half their luminance (658 before):

- On the pedestal whole, `½ · b · Y'`: a pixel at black must fall half
  the pedestal under black to stop, and the deconvolution, nearer
  linear in the dark on its pedestal, left 1,111 such halos and moved
  the export about twice as much as the others.
- On the luminance exactly, `b · (½ · max(Y, 0) + k)`: 729 halos. But a
  pixel resting at black (a flat patch under zero, its estimate the
  pedestal to the last bits) sits on its own floor, and which side of
  it is rounding. The GPU sweep found it: five cases in 2,000 on
  NVIDIA, all blocks of exact black beside bright primaries, a block
  stopped an iteration apart on the two paths (relative differences
  up to 7e-2).
- Between: `b · (½ · max(Y, 0) + 0.9 · k)` (`stop_floor`,
  `FLOOR_PEDESTAL`). Where the blend is full a pixel stops its block
  when its luminance falls under half its start less a tenth of the
  pedestal; where the blend is nothing it stops nothing, as before.
  813 halos, and a tenth of the pedestal is far past any rounding.
  This is the one we took.

### What we measured

Headless exports (16-bit TIFF, default edit, every XDG directory in a
scratch folder), the old build against the new:

| frame | ISO | dots before | dots after |
|---|---|---|---|
| 5M0A7337 | 10000 | 1,162 | 0 |
| 5M0A8015 | 6400 | 2,784 | 0 |
| 5M0A7502 | 4000 | 552 | 0 |
| 5M0A6324 | 1600 | 0 | 0 |
| five base-ISO frames (Canon R5, R6 II, Sony) | 100–500 | 0 | 0 |

The command line's sharpened preview of 5M0A7337 went from 953 to 0.
4x crops of the old dot sites, on the lashes and on a dark sweater,
show the dots gone and nothing in their place: no dark hole, no grey
blotch, the noise as it is around them.

Where nothing should change, the change in levels of 255, on pixels
whose display luminance is over 0.2 (99.9th percentile, largest):
066A3439 0.12, 1.5; 5M0A0504 0.07, 1.8; 5M0A8706 0.21, 4.6; DSC00086
0.17, 4.2; 5M0A3976 0.79, 7.2. 5M0A3976, mostly dark, moves the most:
its largest changes sit on lights and gilt edges against black, where
the pedestal changes the deconvolution of the black side and the stop
of the block, and at 4x the two are not told apart. The sharpening
itself is as strong as it was: its own change (the sharpened export
less an export with the sharpen off) at edges over a display
luminance of 0.2 is 1.0003 to 1.0062 times what it was on every
base-ISO frame, and 0.993 to 1.010 in the midtones. Under the
pedestal more changes, by design: on 5M0A7337 the sharpen's change
in the blacks is 0.69 of the old one, the dots being most of the old.

The GPU against the CPU: on 5M0A7337 the two sharpens are 1.8e-7
apart at most on NVIDIA (were 1.1e-3) and bit-identical on lavapipe.
The seeded sweep draws a new scene, a high-ISO black (texture down to
black with each channel's noise about zero, from a tenth of the
pedestal to ten times it), and two fixed tests hold a noisy black and
a black at rest; the second fails with the floor read on the
luminance exactly. Seeds 1 to 30 at 200 cases pass on lavapipe; on
NVIDIA one case of the 6,000 (seed 1, case 153, no early stop, 35
iterations) is 1.16e-4 apart against the 1e-4 tolerance. The code
before this change misses the same draw too (1.01e-4), run on it by
padding its scene list so its seed makes the same pick; replayed as
it stands, the old sweep's seed 1 case 153 is a different scene and
passes. It is a pixel the unstopped iterations darken seventyfold, the
`DARKENED` case, rounding through 35 multiplicative steps, and the
early stop is never off in an edit. We left the tolerance as it is.
The new scene changes every later draw of the sharpen sweep for every
seed, so a seed and case from a log before this change replays a
different picture. Seeds 31 to 200 at
the default 30 cases pass on NVIDIA.

A CPU test makes the old failure on a synthetic black (noise about
zero in every channel, the luminance under zero at about half the
pixels, beside a blurred edge): before, a channel in the black moved
by 1,432; now by under the noise's own spread, and the edge is still
sharpened.

The review reproduced the dot counts, the base-ISO numbers for
5M0A3976 and 066A3439 exactly, the halo counts (658, 813, 729, 1,111)
on the same input, and the GPU's 1.8e-7. On a fresh develop of
5M0A3976 the halos go from 710 to 853, about a fifth more, and almost
all of them near black: over a luminance of 0.05 they go from 81 to 86.

### With the dehaze

The dehaze's `(I − A) / t + A` pushes noise at black further below
zero, which is the case the pedestal is for. With Dehaze at 60 on
5M0A7337, the CPU export had 1,387 dots before and has none now.

### Limits

- **A color with a negative luminance is black to the sharpen.** The
  clip at zero takes a pixel whose luminance is under zero as black,
  whatever its channels. No real color has one, but a camera matrix's
  error on a deep violet, a stage LED, can give one: a synthetic
  (0.05, −0.08, 0.6) beside grey is never sharpened against black, and
  the pixel beside the grey loses most of its blue at the defaults.
  The old code sent the same picture to 1.8e6 and to infinity.
- **The pedestal is in the develop's units, before exposure.** On a
  frame pushed up in the edit it is larger against the picture: on a
  synthetic edge four stops under, the undershoot at the defaults is
  2.7% of the edge's height against 2.1%; eight stops under at the
  panel's strongest, 40% against 7%.
- **A NaN stays in its pixel on the CPU** (`max` takes the pedestal
  over it); the shaders' `max` with a NaN is the implementation's
  choice, so the GPU may still spread one, as it did before.
