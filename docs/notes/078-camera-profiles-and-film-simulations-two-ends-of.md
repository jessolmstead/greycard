# 78. Camera profiles and film simulations: two ends of the pipeline (2026-09-18)

The roadmap had "Film simulations?" and §5 rule 6 had "camera profiles
as first-class data". They are two different things, and the mistake
to avoid is building them as one. A camera profile says what the
sensor saw; a film simulation says how the picture should look.
Lightroom blurs them: "Adobe Color", "Camera Standard" and the
creative profiles are all one DCP format carrying matrices, a hue
map, a look table and a tone curve, and its users learn that
"profile" means "the starting look". Here the two sit at opposite
ends of the pipeline and each is honest about what it is: the input
profile after the white balance at the matrix, the look after the
tone curve before the output transform, and everything between them
scene-referred as it is now.

**Camera profiles: the input side.** What the engine has is the DNG
dual-illuminant matrix pair from the file (`profile_from_frame`)
through Bradford: the "Adobe Standard matrix" level of accuracy,
colorimetric and slightly dull, and the reason every raw editor hears
that its colors are off against the camera's JPEG.

- *DCP loading first.* A DCP is a TIFF of DNG tags: the matrix pair,
  forward matrices, a hue/saturation/value map per illuminant, a
  look table, a tone curve and a baseline exposure offset. The hue
  map is the part that buys accuracy. It is defined in HSV of linear
  ProPhoto after the matrix, so the stage is camera to XYZ to
  ProPhoto HSV, apply the map interpolated between the two
  illuminants as the matrices are, back to Rec.2020. RawTherapee's
  `dcp.cc` is the reference to port, GPL, with attribution in the
  header as the license note requires. Adobe's DCPs cannot be
  bundled; users have them from the DNG converter and some makers
  ship their own. The look table and tone curve inside a DCP are
  treated as a look, not as part of the profile, so the accurate
  half can be taken without the Adobe rendering.
- *The choice lives in the edit.* A `camera.profile` field:
  "embedded" by default, else a named DCP from the user's profile
  directory under `~/.local/share/greycard/profiles/`. In the edit
  because it changes the picture and a preset should carry it. Core
  gains the map on `CameraProfile` and nothing else; it still knows
  no schema.
- *ICC input profiles* are darktable's older route. Skipped until
  someone asks.
- *User-measured profiles* from a chart shot are the real
  differentiator and come last. dcamprof (Anders Torger, GPL) is the
  reference for solving a matrix and a hue map from a ColorChecker.
  A tool of its own.

**Film simulations: the output side.** Two tiers.

- *Parametric looks are presets the editor already has.* A film
  preset is a curve, the mixer, grading and grain in a `.gcp` (§52).
  Classic Chrome is roughly a desaturation, a hard shoulder and cyan
  shadows. It covers most of what people ask for, every parameter
  stays editable, and it costs nothing to build. A handful shipped
  under honest names, not the makers' trademarks.
- *3D LUT looks* for the rest: a `look` section with a LUT by name
  and a strength. Read `.cube` and HaldCLUT PNG, the format
  RawTherapee uses and the one the Pat David film collection is in,
  some three hundred stocks under CC-BY-SA. A LUT is made for a
  particular input encoding, nearly always display-referred sRGB, so
  it goes after the tone curve: gamut-map to the LUT's space (an
  sRGB LUT clips Rec.2020), encode, look up with tetrahedral
  interpolation, decode back to working linear, blend by strength,
  then the display transform. The sampling shader already takes a
  3D LUT for the monitor (§17), so a second slot in the same path is
  small; the CPU reference comes with it as every op's does.

**The idea worth doing that nobody else does: fit the camera's own
rendering from the file.** Every raw carries the maker's JPEG with
the picture style or film simulation applied, same frame, same
geometry. Develop the raw through the accurate profile at low
resolution, register it against the embedded JPEG (the decoder
already hands the preview over for the filmstrip, §35), and fit a
low-order model of the difference: a matrix, a curve and a small
LUT. Across a few dozen of the user's own files per camera and
style, that is "Provia" or "Canon Standard" as a look, learned from
the user's data, with no license problem and no reverse engineering.
Color and tone only, at a size where the maker's sharpening and
noise reduction do not show. One mechanism answers "match the
camera" for the profile crowd and "give me the film sims" for the
Fujifilm crowd.

**No network in the camera match.** The target is the wrong shape
for one. The maker's color and tone rendering, minus its spatial
processing, is a per-pixel map from three numbers to three: smooth,
deterministic, low-dimensional, with millions of samples a frame. A
matrix, three curves and a 3D LUT are already a universal
approximator on that cube, and a LUT is data (§5 rule 8) that can be
inspected, hand-edited and exported, where a network is a runtime
dependency that can be none of those. The hard parts are elsewhere:
registration (the JPEG is cropped, scaled and lens-corrected against
the raw; fit on block means over a coarse grid after aligning, which
also averages the maker's sharpening and noise reduction out);
censored highlights (the JPEG is 8-bit sRGB and clipped; fit only
where both are unclipped and regularize the shoulder rather than
learn it); spatially varying processing (Canon's Auto Lighting
Optimizer, Fujifilm's dynamic range modes, every maker's local
contrast; a per-pixel model cannot fit them, so a frame that fits
poorly is down-weighted and the user is told to fit from frames shot
with them off); the illuminant (the maker's look is applied after
its white balance, so fit in balanced space and group frames by
white balance setting); and coverage (a few dozen frames do not
cover the cube, so the LUT needs a smoothness prior to interpolate
the unseen regions). On that last point a three-input MLP is a
smooth function of the cube by construction and some color
transform fitting uses one as the regularizer before sampling it
into a LUT and discarding it; thin-plate splines or radial basis
functions do the same with fewer knobs and no dependency, so those
first, the MLP only if the spline fit rings. The fit runs once per
camera and style, offline, so a slow classical solver costs nothing.

**The order.** DCP reading with the hue map and forward matrices and
a picker in the panel, embedded as the default. Then the look
section, LUT files with strength and film presets as ordinary
presets. Then the camera match fitted from the embedded JPEG. Then
chart-based profile making. The roadmap carries the four.
