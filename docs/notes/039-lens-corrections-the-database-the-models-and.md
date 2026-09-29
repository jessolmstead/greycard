# 39. Lens corrections: the database, the models, and where they run (2026-09-14)

The first of the roadmap's new Next up. A modern mirrorless lens is
designed to be corrected in software, so until now the camera's own
JPEG drew straighter lines and evener corners than the export. Three
corrections from one profile: distortion, lateral chromatic
aberration, vignetting; and a distortion by hand beside them.

**Where the profiles come from.** The lensfun database
(lensfun.github.io, CC BY-SA 3.0): the one open, maintained
collection, the one darktable and RawTherapee read, some 1 570 lenses
and 1 050 bodies in its version 2. The alternatives were the
corrections the makers embed in their files (Sony, Fujifilm, Olympus
and Panasonic carry distortion and vignetting tables; DNG carries
opcodes), which rawler surfaces only for DNG and which would have
been a decoder contribution per maker, and Adobe's LCP files, whose
license forbids it. lensfun's XML is small (5 MB unpacked, 430 KB as
its tarball) and its models are documented, so `greycard-lens` reads
it itself (`roxmltree`) rather than binding liblensfun: no C
dependency, and the matching is ours to see. Consistent with §34's
rule for models, nothing is bundled: the tarball is fetched on first
use from the database's own site (its maintainer's mirror second),
unpacked to `~/.cache/greycard/lensfun/version_2` with a license note,
after a sheet that says what and whence; a system lensfun's copy in
`/var/lib/lensfun-updates` or `/usr/share/lensfun` is read when there
is no fetched one. The CLI has `greycard lenses [--fetch] [FILE]` for
the same, and to say what the database finds for a file.

**The models, in the engine.** `develop::lens` in core has the models
as the database publishes them, by their names: distortion `poly3`,
`poly5`, `ptlens` and Adobe's `acm`, CA `linear` and `poly3` (red and
blue as radial scalings of the green), vignetting `pa` (and Adobe's,
the same polynomial). Two facts from lensfun's `modifier.cpp` that
the manual leaves implicit and that everything depends on: for
distortion and CA a radius of one is half the picture's shorter side
(the middle of the long edge); for vignetting it is half the diagonal
(the corner). A calibration made on another sensor is used through
`radius_scale`, the ratio of the two sensors' shorter sides in
millimeters from their crop factors and shapes (the database's
`aspect-ratio`, three by two when unsaid), and `vignetting_scale`,
the ratio of the crop factors. The body's crop factor comes from the
database's camera entry; when the body is unknown the lens's own
format is assumed and the panel says so. Correction is one cubic
resample of the working image, each channel read at its own place
(`TCA(Distort(p))`, as lensfun composes them), the vignetting gain
taken at the place read; a profile with vignetting alone changes the
gain and moves nothing. The scale is by default the smallest
magnification that leaves no edge empty, found by bisection on the
output's border; a fixed one is a slider. Tests: the models' fixed
radii, a ramp read back where the model says, a corner's gain, CA
moving red and blue apart from green, the auto scale fitting just.

**Matching.** Names as bags of tokens, letters and digits apart, case
and a few noise words aside, so "RF24-70mm F2.8L IS USM" as Canon
writes it meets "Canon RF 24-70mm F2.8L IS USM". A lens needs every
number on each side to appear on the other (a 24-105 F4 is not the
24-105 F4-7.1), must fit the body's mount or one it takes, must cover
the focal length shot, and must not be a fisheye; a body needs the
same tokens or a strict subset of the file's worth three quarters of
them, so "EOS R6" does not stand in for "EOS R6 Mark II". Checked on
the frames here: the R6 Mark II with the RF 50 F1.2, the A7 IV with
the FE 24-70 GM II, the R5 and R5 Mark II with the RF 24-70 F2.8, all
found with their bodies. The S5 II's RW2 records no lens name in the
tags rawler reads (it is in the maker note), so it gets nothing until
rawler surfaces it. Calibrations are interpolated: distortion and CA
between the two focal lengths about the shot's when they share a
model, else the nearer; vignetting at each of those focal lengths
between the apertures about the shot's, at the calibration distance
nearest by ratio to the file's (ten meters when it records none),
then between the focal lengths.

**Where in the pipeline.** On the worker's base: after the engine's
`finish` (matrix and orientation), before the retouch and the sharpen,
so a patch drawn on a corrected picture stays where it was drawn and
the sharpen sees the resampled picture. `Edit::same_base` includes
the lens, so a toggle costs a develop, not a sharpen. The edit holds
choices, not the profile: which of a profile's corrections, a manual
`poly3` k1 on top, auto or fixed scale; the profile is looked up from
the file each open, so an edit moves between machines and database
versions. The vignetting gain is applied after the denoise rather
than on the mosaic, where RawTherapee puts it: simpler, and the
denoiser measures noise on what the sensor recorded; the corners
come out with their noise amplified by the gain, at most 1.7x on the
RF 24-70 wide open. On the R5's 45 MP frame the whole correction is
0.21 s in release.

**The panel.** A LENS section on Develop between NOISE and DETAIL:
the profile found (or why none), a button to get the profiles when
there is no database, switches for the profile and each of its
corrections, the manual distortion, auto scale and the scale. The
fetch sheet grew a `fetch-note` so it serves the profiles as well as
the models. The viewport screenshot is the texture, not the window,
so the section is checked by its bindings compiling and not by eye.

**Not here.** The makers' embedded corrections, which would be exact
for the lenses that carry them (a rawler contribution per maker).
Fisheye projections. Manual CA. The sharpen's radius is measured on
the mosaic and is not adjusted for the resample, which at a scale
near one changes it by a few percent. Adobe's `acm` CA model, rare in
the database, is skipped. Perspective is its own op, as §24 said.

**Addendum, the same evening: third-party names.** The Sigma 28mm
F1.4 DG HSM Art was not found. Canon bodies write Sigma's year code
into the name ("28mm F1.4 DG HSM | Art 019"), and the rule that every
number in the file's name must be in the database's read 019 as a
focal length or an aperture the entry lacked. A three-digit number
with a leading zero is now a series code and not held against the
entry. And when no lens fitting the body's mount matches, the name
is tried across all mounts: the database lists the mounts its
calibrators had, and a third-party lens is the same glass in each
(this Sigma turned out to list Canon EF as well; the rule stands for
the ones that do not). `greycard lenses --lens NAME [--camera MAKE
MODEL]` tries a name without a file, which is how the four spellings
a body might use were checked.

---
