# §663. Output sharpen (2026-11-13)

An unsharp mask on the finished picture's luminance: a section of its own,
OUTPUT SHARPEN, after the capture SHARPEN, with Amount, Radius, and
Threshold. Off by default; the whole picture only.

## Where it runs

Between the look table and the output transform, on the picture
display-referred in the linear working space (`finish_pixel_display` ends
there; `finish_pixel_output` takes over). After the tone curve, the point
and color curves, the grading, and the look; before the output matrix,
the AgX rail, the encoding, and the grain. Before the grain so the grain
is never sharpened; before the output matrix so the same edit sharpens
alike into sRGB, Display P3, Adobe RGB, or Rec.2020. The panel follows
the pipeline: Detail, SHARPEN, OUTPUT SHARPEN.

We read the luminance encoded: Rec.2020's, linear, through the sRGB curve
(the encoding the point curves already work in). The difference from its
blur moves the encoded luminance, and the result goes back on as a ratio
on the linear color, `decode(l + δ) / decode(l)`: every channel scaled
alike, so the hue and the saturation hold, dark halo and light alike.
Black (linear luminance under 1e-6) stays black; channels are held at
zero from below on every pixel the sharpen passes over, moved or not, so
a flat stretch outside the gamut is clipped as its sharpened edge is;
a brightening stops where the largest channel reaches white, so no
channel clips out of proportion. The first version added the
encoded delta to every channel, which saturated dark halos, washed out
light ones, and turned an orange whose blue sat at zero; we dropped it.

## Why encoded and not linear light

Measured (`output_sharpen::tests::linear_light_halos_are_lopsided`): grey
steps, radius 1, amount 0.5 encoded; the linear-light mask's amount set so
its light-side halo matches the encoded one, then its dark side read. The
ratio leaves a grey's numbers as they were, so the table holds.

| step (encoded) | encoded halo, dark / light | linear dark halo |
|---|---|---|
| 0.15 to 0.40 | 0.038 / 0.038 | 0.150 (to black) |
| 0.25 to 0.60 | 0.053 / 0.053 | 0.250 (to black) |
| 0.35 to 0.75 | 0.060 / 0.060 | 0.325 (to black) |
| 0.45 to 0.85 | 0.060 / 0.060 | 0.190 |

Linear light is lopsided: the difference across a step is mostly the light
side's, and taken off the dark side it drives it to black in a hard line.
We expected harsh bright halos from linear light; what we measured is
harsh dark ones. Same conclusion.

## The knobs

- Amount 0 to 2 (panel 0 to 200%), default 0. Zero, or the section off, is
  no op at all: the finish keeps its one pass, and the viewport its one
  draw and no targets (tested to the bit on the GPU; the CPU path is the
  same code).
- Radius 0.5 to 3, default 1: the Gaussian's standard deviation in pixels
  of the full-size picture. An export made smaller takes `radius × reach`,
  the viewport `radius × zoom`, so the viewport scales the radius as an
  export at that size does. Taps three sigma a side, nine at most.
- Threshold 0 to 0.1 (panel 0 to 10%), default 1%: a soft knee
  `d³ / (d² + t²)`, half at the threshold, falling as its square below,
  continuous everywhere, so no contour.

A pixel that is not a number (or infinite) stays as it is and is left out
of its neighbors' blurs, their weights summed over the taps read.

## The CPU's memory

The finish holds a band of 256 rows at the look table at a time, with the
blur's reach (at most nine rows) above and below it, made again for each
band, and finishes the band to the output, its grain drawn again. A band
comes out as the same rows of the whole frame, to the bit (tested across
seams of one and seven rows, a mask and grain on). Peak over the input
frame on a 45 MP synthetic frame (8256 × 5504, radius 3, release):

| | before | after |
|---|---|---|
| sharpen off | 1038 MB | 1041 MB |
| sharpen on | 2077 MB | 1041 MB |

The sharpen's own share went from a gigabyte to under the finish's
existing peak; 0.9 s against 0.6 s off.

## The GPU

With the sharpen on, the viewport draws three passes: the shader into an
`R32Float` target writing the encoded luminance and stopping, a small pass
(`osharp.wgsl`) blurring it across, and the shader again, blurring that
down and sharpening. The luminance target is the window and a margin of
the blur's reach past each side (36 pixels at 4:1, radius 3), drawn as the
window's own pixels are, so a tap is held only at the picture's real edge
and the window's border pixels read what an export has beyond them. At
the picture's edge the taps are held where the shader drew it, below.

Full floats: half floats put the ratio's sensitivity at low luminance
(saturated blues and reds) through 2.5 levels of error at amount 2. Not
filterable without a feature, so both shaders read the targets by load and
interpolate them themselves. Targets are kept one pair a size, three
sizes, the last used first, so the scopes' 512-wide draw and the
viewport's never make each other's again; they are let go of when the
sharpen goes off. The overlays-only draw under focus peaking runs no
sharpen passes (its clipping paint then reads the unsharpened picture;
the picture under it is sharpened).

**The picture's edge.** The first version held the taps at the edge plus
half a pixel, the next at the first and last texel centers on the picture.
Each was a second reckoning of the edge, made in Rust, beside the shader's
own. Where the edge fell exactly on a texel's center (a pan of half a
pixel at 3:1, or an edge on a half-pixel at 1.5:1), the shader's call of
picture or canvas came down to its float arithmetic, and the two disagreed
on one device or the other: on the RTX 5070 Ti a tap read canvas and drew
a rim of up to 74 levels along the picture's edges at fit; lavapipe did
the same at other ties. Moving the rect in by a hair broke the other side:
a texel the shader drew as picture then gave its neighbors nothing.

We dropped the rect. The luminance pass writes −1 wherever it draws canvas
(the luminance is never below zero, and a pixel that is not a number is
not −1), and its target is cleared to −1. Each blur finds the picture's
edge in what it reads: from its own texel it counts the texels of picture
out to its reach on each side (one load when the texel at the reach is
picture, as nearly all are), and its taps are held at the last of them,
where the hand-written bilinear reads that texel alone. That is the CPU's
clamp to its edge pixel, at whatever edge the shader drew. Across, a
canvas texel stays −1; down, a pixel whose own texel is −1 is not
sharpened. Near an edge the search costs up to `reach` loads a side, and
it relies on the picture being one unbroken run along each row and column,
which a convex picture is: the frame, and the turned or perspective source
within it. The same change fixes a rim on turned views: crop mode shows
the turned source whole with canvas in the frame's corners, a row there
holds picture and canvas both, and the old rect, the frame's, let the taps
read that canvas (74 levels at 45 degrees and 1:1 on the RTX 5070 Ti). A
flat 90 × 60 picture turned 3 to 60 degrees at 0.5:1 to 3:1 now moves no
pixel in 20 cases on both devices, and fails at its first case if the blur
across stops keeping the canvas's mark.

A flat picture now moves no pixel in 588 cases on both the RTX 5070 Ti and
lavapipe: seven windows, zooms 0.37 to 4 with 1.5 added, four pans
including half a pixel, and each side's edge on a whole and on a half
target position; the same test fails on the old clamp on both devices. The
edge-parity test's off-grid pans are now quarters and eighths at zooms of
powers of two: a real card's texture filter keeps bilinear weights to
about 1/256, so the view alone, with the sharpen all but off, strays from
the CPU's exact bilinear by 1.1 levels at a pan of 77/256 and 1.9 at 3:1.
That is the view's resampling, not the sharpen.

Agreement, lavapipe, eight-bit read back, every case at 0.50 levels worst,
the rounding's own floor; the viewport's tests run on the machine's own
card too:
- 1:1 over the parity frame, 27 settings and one under a mask: worst 0.50,
  mean 0.08 to 0.12 levels.
- 1:1 to 4:1 panned so the picture runs past three sides and its edge
  shows on the fourth, on the pixel grid and off it by quarters and
  eighths of a pixel: border and interior both 0.50 (0.63 on the RTX
  5070 Ti).
- Half (fit) against the CPU's finish at half reach of the frame boxed
  down two by two, as the view's footprint averages it: 0.50. Against the
  real export resized to that size (Lanczos) the fit view differs by tens
  of levels at its worst pixel, as it already does with the sharpen off:
  that is the view's resampling, not the sharpen, and is not claimed
  here. What is proven is the sharpen agreeing at 1:1 and above, and
  under 1:1 against the same 2 × 2 box.

In the app (headless weston, a 24 MP CR3 at 1:1, a 940 × 658 viewport,
amount 100%, radius 1): the viewport against the CPU export, mean 0.02
levels, at most one level on every pixel, the window's border included
(before the margin, 14 levels there). Off, the same: mean 0.02, at most
one.

Cost: the viewport's finish runs twice while the sharpen is on, the first
pass over the margined window.

## Edit schema

`Edit::output_sharpen`, left out of a sidecar at its default, so every
sidecar and develop key written before it is unchanged; no schema bump.
`overlay_changes` writes the section out whole in all three edits before
it overlays them, so a field set back to the default in culling is a leaf
and not the frame's whole section taken away. A preset and sync section of
its own ("output-sharpen"), whole, no parts, carried by default. Not
mapped from Lightroom, pp3, or a catalog. The models' preview leaves it
out, as it does the vignette and grain.

## Rejected

- Sharpening in the develop (scene-linear): the capture sharpen's place,
  and the tone curve would reshape its halos.
- After the output encoding: the result would depend on the export space.
- One encoded delta on every channel: not chroma-neutral (above).
- A hard threshold: posterizes the knee.
- The whole frame held at the look table: a gigabyte on 45 MP.
- One 2D blur in the last pass: up to 19 × 19 taps a pixel; separable is 38.
- Recomputing the finish at neighbor taps in one pass: dozens of finishes
  a pixel.

## The owner's to confirm

- The icon: `focus.svg`, the capture SHARPEN's own.
- The copy settings sheet's title "Output sharpen", a second catalog
  entry beside the panel's "OUTPUT SHARPEN". The history rows say "Output
  sharpen 60%", untranslated as the other rows are.

## Open for the owner

- The radius scales with the export's size, as Detail's bands and the
  grain do, so the sharpen an export shows is the full-size picture's
  seen smaller. A web-size export barely sharpens: at a third of full
  size, radius 0.5 is a blur of a sixth of a pixel and does nothing.
  Output sharpening is often set in output pixels instead (the export
  sheet's own Sharpen, after the resize, is); a choice between the two,
  or a radius in output pixels, is the owner's.
- With focus peaking on, the clipping warning reads the unsharpened
  picture: the overlays-only draw runs no sharpen passes. The picture
  under the paint is sharpened.
- A mask's share of it waits on the roadmap's later item.
