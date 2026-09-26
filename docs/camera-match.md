# Camera match: fitting the maker's look from the embedded JPEG

A trial plan for the roadmap's v0.6.0 line "Camera match: a look fitted
from the maker's embedded JPEG against the accurate develop, a matrix,
a curve and a small LUT per camera and style" (notes §78). This file
carries the trial's design, its results as they come in, and the
decision it leads to. The reasoning behind the idea itself stays in
§78; what follows assumes it.

## The claim under test

Every raw carries the maker's JPEG with the picture style applied to
the same frame. The maker's color and tone rendering, minus its spatial
processing, is a smooth per-pixel map from three numbers to three. So
a matrix, three curves and a small 3D LUT, fitted over a few dozen of
the user's own frames per body and style, should reproduce "Canon
Standard" or "Provia" as a look, learned from the user's data, with no
license and no reverse engineering.

The trial asks two things:

1. Is the residual small enough that the look reads as the style, not
   as something near it?
2. Does a fit hold on frames it was not fitted on? If not, the LUT
   memorized the frames and is not a look.

A third answer comes for free: which of the maker's processing is
spatial, and so cannot be a per-pixel map at all. Those frames show up
as the ones that fit worst.

## Nothing in the engine changes for the trial

Both ends already exist.

- The export path renders the default develop to a 16-bit TIFF, in
  sRGB, at a chosen long edge. `tools/reference/render.sh` drives it
  from a copied raw and a written sidecar, so no real sidecar is
  touched and no hand is on a slider.
- The look section (§127) applies a `.cube` after the tone curve, in
  the table's own encoding, with a strength. That is the slot the fit
  targets, so the fit's output is a `.cube` dropped into
  `$XDG_DATA_HOME/greycard/looks` and picked in the panel.
- `tools/reference/measure.py` already pulls the largest embedded JPEG
  out of a raw and reads both sides back as linear luminance.

The trial is therefore a Python script beside those two, and a shoot.

## The script

One run per body and style. The steps:

1. **Render.** Each raw through the default develop, lens corrections
   on (the camera's JPEG has them), everything else at the default.
   Export sheet set to TIFF 16-bit, sRGB, long edge about 2000.
2. **Extract.** The camera's JPEG from the raw, as `measure.py` does.
3. **Register.** The JPEG is cropped a few percent and scaled against
   the raw, and may sit a pixel or two off. Solve a similarity
   transform on luminance (phase correlation for the shift, a scale
   and crop from the two frames' sizes and the maker's crop tag where
   there is one). Then average both pictures over a coarse grid of
   blocks, 32 pixels or so a side at the export size.
   - Drop blocks whose internal variance is high on either side: edges
     and texture, where the maker's sharpening and noise reduction
     live. What remains is color and tone.
   - Drop blocks where either side clips: the JPEG is 8-bit and
     censored at white, and the develop's own shoulder clips too.
4. **Per-frame exposure first.** The camera JPEGs scatter by about
   ±0.8 stops across scenes (§143), and that is the camera's
   scene-adaptive brightness, not the style. Solve a single exposure
   offset per frame from the block pairs, re-render with it through
   `render.sh`'s `EXP`, and fit the shared model over the re-rendered
   frames. The look then carries the style; brightness stays with the
   Exposure slider, where it belongs.
5. **Fit in stages**, reporting the residual after each as ΔE in
   Oklab, per frame and overall:
   - a 3×3 matrix in linear light;
   - then a per-channel curve;
   - then a 17³ LUT in the encoded domain, fitted as a thin-plate or
     radial-basis regression with a smoothness prior and a pull toward
     identity where the frames put no samples. §78 says why no network:
     the target is a smooth low-dimensional map with millions of
     samples, and a LUT is data the user can inspect and export.
6. **Hold one out.** Fit on all frames but one and measure on that
   one, round the set. The gap between fitted and held-out residual is
   the answer to the second question.
7. **Write** the LUT as a `.cube` with `# encoding: srgb` above the
   table, into the looks store, and look at it in the editor beside
   the camera preview that culling mode draws (§123).

Then the check that matters: the same frame, greycard's develop with
the look at full strength against the camera's JPEG, side by side and
as a ΔE map. The strength slider gives a half-match for free.

## What has to be shot

The sample folder does not hold a usable set:

| Body | Frames | Why it falls short |
|---|---|---|
| R6 II | 11 | all Picture Style Auto, which is scene-adaptive itself; 3 also have Auto Lighting Optimizer on |
| R5 II CR3 | 6 | 5 Faithful, 1 Standard; only 3 Faithful with ALO off |
| R5 II DNG | 4 | the embedded preview is Adobe's, not the camera's |
| GFX 100S II | 3 | all Reala Ace at DR100, consistent but three frames |

The trial wants 20 to 40 frames on one body, one fixed style, with
every adaptive setting off. The shot list is in `docs/test-frames.md`;
the settings per maker:

- Canon: Picture Style Standard (not Auto), Auto Lighting Optimizer
  off, Highlight Tone Priority off. The tags are
  `Exif.CanonPr.PictureStyle`, `Exif.CanonLiOp.AutoLightingOptimizer`,
  `Exif.CanonLiOp.HighlightTonePriority`; exiv2 prints them.
- Fujifilm: one film simulation, DR100, highlight and shadow tone at
  zero, `Exif.Fujifilm.FilmMode` and `Exif.Fujifilm.DynamicRange`.
- Nikon: one Picture Control, Active D-Lighting off.
- Sony: one Creative Style, Dynamic Range Optimizer off.

Across daylight, shade, tungsten and skin; a few frames deliberately
bright and dark, so the fit sees the whole tone range. Group by white
balance setting if the light is mixed, since the maker's look is
applied after its white balance.

**One style proves the mechanism; the rest come free.** Each style is
its own fit, but nothing ships per style: the product version fits
whatever styles the user's library holds. And the makers' own software
renders any style from a raw with the camera's engine, Canon's Digital
Photo Professional, Fujifilm's in-camera conversion and X Raw Studio,
Nikon's NX Studio, so one shoot on Standard yields every other style as
a target. The script fits against any JPEG that shares the raw's
geometry. The in-camera route is exact; a desktop render is checked
against one embedded JPEG before it is trusted.

A second style is still worth fitting in the trial, because it
separates "the fit found Standard" from "the fit found the camera". If
Standard and Faithful come out as different LUTs whose difference is
what Canon says those styles do, the fit is reading the style. If they
come out nearly the same, it is mostly fitting the sensor and tone.
Two styles need care: Monochrome discards hue, so its fit is trivial
and the look belongs to the black and white section; Fine Detail
differs from Standard mostly in sharpening, which the block averaging
discards, so it should land on nearly Standard's LUT, a check rather
than a problem.

**A ColorChecker is useful here, not required.** It adds no truth
about the style: the camera's JPEG of the chart is already that truth.
What it adds is a clean set of samples, edge-free, well exposed, and
covering the saturated patches ordinary frames miss, which is the
coverage gap named below. Colorful subjects, fruit, fabric, paint
chips, do most of the same job. Where the chart becomes required is
the next roadmap item, the profile made from a chart shot after
dcamprof, which needs known reflectances; the 24-patch Classic or the
Passport is the one every tool carries reference data for. Since it is
wanted for that anyway, a handful of trial frames with it in is cheap.

## What to expect

The color half should work, and others already do a version of it.
RawTherapee's auto-matched tone curve fits a curve from the embedded
JPEG per frame and lands close. Adobe's camera-matching profiles were
built by fitting against the maker's rendering. The Fujifilm film-sim
LUTs in circulation were made by shooting charts through the camera.
For a fixed style with the adaptive settings off the map is
deterministic and smooth, so the matrix and curves should carry most
of it and the LUT should close the hue-specific twists, Canon's skin
and Fujifilm's simulations. Held-out generalization on that data
should hold.

Where it is expected to fall short, and what each means:

- **Highlights.** The develop's shoulder clips before the look slot,
  so wherever the render has reached white nothing applied after it
  can bring the camera's soft roll-off back. This shows as residual
  in the top band. The fix is an engine change, fitting before the
  tone curve or rendering with headroom; the trial says whether it is
  needed.
- **Tone across scenes.** With the adaptive settings off a camera
  still varies brightness, and sometimes contrast, by scene. The
  per-frame exposure offset absorbs the first. A per-frame contrast
  difference it cannot, and that shows as held-out error on tone while
  color stays good. That outcome is a success for the look and a note
  for the tone curve.
- **Shadows.** The JPEG is 8-bit, so the darkest tenth is a few levels
  of data. The fit is least trustworthy there, and the LUT is held
  near identity below about the 10th percentile rather than trusted.
- **Coverage.** Ordinary frames rarely hold saturated blue, magenta or
  deep red, so the LUT interpolates there from the prior, and a
  saturated subject is the first thing that will look off. The chart
  and colorful subjects in the shoot are the answer.

The likely result: a look convincing on the frames people shoot most,
slightly wrong on saturated colors and clipped skies. Enough to ship
as a look with the strength slider, and a clear list of what to fix
second.

## What the answers lead to

If the held-out residual is small, the productized version is the
same fit run from inside the editor over the user's own library, one
look per body and style, named after the style the tag reports and
kept in the looks store like any other. The roadmap line stays as
written.

If the fit holds only with the LUT and not with the matrix and curve,
the look needs the LUT and the panel needs nothing more than it has.
If the held-out gap is wide, the frames were memorized: more frames or
a stronger prior, and the trial repeats before anything lands.

If the frames with adaptive processing on fit badly, as §78 expects,
the editor tells the user which settings to turn off before fitting,
by the same tags the table above names.

## Log

- 2026-09-25: plan written. Nothing run yet; waits on the shoot.
