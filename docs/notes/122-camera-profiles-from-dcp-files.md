# 122. Camera profiles from DCP files (2026-09-20)

DCP camera profiles, the first of §78's four: the file read, the
hue/saturation/value map applied where the matrix leaves off, the
choice in the edit, a picker in the panel. The look table and the tone
curve a DCP also carries are parsed and applied by nothing; they are
Adobe's rendering, and they belong to the look line, not to this one.

**The reader is ours, not rawler's.** A DCP is a TIFF whose magic
number is `RC` (0x4352) where a TIFF carries 42, and whose single IFD
holds DNG's profile tags. rawler's `GenericTiffReader` refuses that
magic outright, and even patched past it, it would go looking for the
sub-IFDs, the chained directories and the image data a profile does
not have. What a DCP needs is one flat directory and six of TIFF's
value types, bounds checked: 250 lines in `greycard-core/src/dcp.rs`,
against a dependency on a decoder's internals for a file that is not a
raw. The writer beside it (`Dcp::to_bytes`) is what the tests build
their profiles with, and is the shape the chart-based profile maker
will want when its turn comes.

Read: `ProfileName`, `UniqueCameraModel`, `CalibrationIlluminant1/2`,
`ColorMatrix1/2`, `ForwardMatrix1/2`, `ProfileHueSatMapDims`,
`ProfileHueSatMapData1/2`, `ProfileHueSatMapEncoding`,
`ProfileLookTableDims/Data/Encoding`, `ProfileToneCurve`,
`BaselineExposureOffset`, `ProfileEmbedPolicy`, `ProfileCopyright`,
`ProfileCalibrationSignature`. The last two are read and honored by
nothing — nothing here embeds a profile in anything yet, and the
engine builds no `CameraCalibration` for a signature to match — and
say so where they are declared, rather than looking like something
that works. A file that is not a DCP says which thing it is not: a
DNG or a TIFF is named as such (the magic number), a text file is told
it has no byte order mark, a truncated one that a tag points past its
end. Every offset is checked against the file's length before it is
read and added with `checked_add`; a table's axes are capped at 1024
nodes, so a file claiming four million an axis is turned away rather
than wrapping its own size and indexing past its data. The pair is
ordered warm first on the way in, whichever order the file wrote it,
because that is the order rawcolor interpolates in, and a calibration
this build cannot use — an illuminant code it does not know, a matrix
that will not invert — is dropped so the other one can carry the
profile; only a profile with neither is refused.

**The forward matrices needed nothing.** rawcolor's `Calibration`
already takes one and `camera_to_xyz` already prefers it over
inverting the color matrix, so a DCP's forward matrices go straight in
and are used as a DNG's own are.

**The map hangs beside `CameraProfile`, not on it.** rawcolor is a
published crate of its own and knows nothing of DNG's profile tables,
so the map went on a new `color::Profile`: rawcolor's `CameraProfile`,
the map pair, and the profile's name, camera, copyright, embed policy
and baseline exposure offset. `DevelopSettings` gained
`profile: Option<Arc<Profile>>` and lost `Copy` with it — a hue map is
a hundred kilobytes and is shared between the develops of one file
rather than copied into each.

**Where the map runs.** DNG defines it in HSV of linear ProPhoto after
the matrix; the engine's matrix lands in linear Rec.2020, and Rec.2020
to ProPhoto through Bradford undoes exactly the D50-to-D65 adaptation
the camera matrix did on the way in, so the stage is a matrix there, a
lookup, and a matrix back. It sits in `finish`, straight after
`apply_matrix` and before the orientation: the last of the camera's own
color, and the one place the viewport, the export and the learned
denoiser's path all pass. A map that is the identity is not run at all.

Nothing is clamped going into HSV, and the saturation and the value
come out unclamped too, which is the one place this departs from the
specification. A color outside ProPhoto has a channel below zero,
which the usual max/min formulation reports as a saturation above one,
and the way back puts it exactly where it was; Adobe clips both to 1
because its pipeline is display referred by then, and clipping here
would gamut-map every such pixel and throw away reconstructed
highlights in the middle of a scene-referred pipeline. RawTherapee
leaves them unclipped for the same reason. Only a pixel whose largest
channel is at or below zero is left alone, having no hue to shift.

`ProfileHueSatMapEncoding` is the value axis's and nothing else's: the
hue and the saturation are read in linear space whatever it says, the
value is encoded, the table indexed with it, the scale applied there
and the result decoded, and a map with no value axis is not encoded at
all. Encoding the RGB before the conversion, which is the obvious
reading and the wrong one, moves every lookup's hue and saturation as
well, and turns a 2.5D map's `v * scale` into `f⁻¹(f(v) * scale)`. An
identity map cancels the encoding either way, so the tests that hold
this down are a saturated color against a map that varies with
saturation, and a 2.5D map that halves the value.

The two illuminants' maps are blended entry by entry by the same
weight the matrices are — DNG's linear blend in reciprocal temperature
— which meant copying rawcolor's `primary_weight`, three lines it does
not expose; it comes out again if it ever does.

**BaselineExposureOffset is read and reported, not applied.** The
engine never applied a file's own `BaselineExposure` either (it writes
one on its DNG output, and reads none), and it is scene referred: a
profile asking for a third of a stop is saying something about Adobe's
rendering, which is the look line's business.

**Listing is not reading.** The panel lists what the profile
directory holds, and a directory of a dozen profiles with 90x30x30
look tables is megabytes of floats to parse on the thread that draws
the panel. A listing reads each file's header — its name, its camera,
its copyright — and stops; the profile that is chosen is the only one
read in full. The directory is read again when the section is opened,
so a profile dropped in while the editor is running shows up without
reopening the file, and that re-listing is also when what was read
before is let go of.

**The choice is `camera.profile` in the edit**, "embedded" by default,
else a DCP's file name resolved against
`$XDG_DATA_HOME/greycard/profiles` (`dirs::data_dir` elsewhere, as
§106 has it). No schema bump: the field has a default, so every
sidecar written before it loads unchanged and reads as embedded, and
`VERSION` stays 3 — a version is for a field that changes meaning, not
for one that arrives. A name that is a path, or climbs out of the
directory, is refused before anything is opened. The file is read in
`Edit::settings()`, where every consumer's develop passes, and kept in
a small cache keyed by path, size and mtime: the viewport and the
export are handed the same `Arc`, and a profile that will not read —
or is not there at all, which is its own entry in the cache — warns
once rather than on every develop and falls back to the file's own
calibrations. The cache holds eight at most, since one edit names one.
A develop never fails for a profile.

A preset can carry the profile — `Section::Camera` — but not by
default, as the white balance, the lens and the demosaic are not:
a profile belongs to a body, and a look preset saved from one camera
should not quietly put that camera's profile on another's file.

**The panel** lists Embedded first, then the profiles the directory
holds that fit the open frame. The fit is the DCP's
`UniqueCameraModel` folded to its letters and digits, against the
frame's make and model folded the same way, and against the model
alone (some decoders leave the make on the front of the model).
Equality, not containment: "CANON EOS R6" would otherwise fit an EOS
R6 Mark II. A profile whose maker writes the model its own way does
not fit and is not listed, with a line saying how many are there for
other cameras; a profile the edit names is always listed, chosen, and
carries a warning under the list saying what it was made for. That
covers the case the filter cannot: a preset or a sidecar from another
body.

**The white balance has to agree with the profile.** The temperature
and tint the panel shows are solved through the camera's matrices, so
the moment a DCP replaces them, anything that converts a white point
has to use the DCP's too. The neutral dropper and the panel's live
preview of a temperature change were both still going through the
file's own, which meant clicking a neutral patch put a temperature on
the slider that did not neutralize it. Both now resolve through the
profile the develop uses, resolved once per choice and kept, and the
preview matrices are thrown away when it changes.

**What it costs.** The map stage is about 55 ms at 24 MP (6000x4000,
32 threads, the 90x30x1 map from RawTherapee's Canon EOS R6 profile),
against a 0.7 s develop of a 20 MP R6 frame: the trilinear lookup and
two HSV conversions per pixel. The blend of the two illuminants' maps
is 8100 entries once per develop and does not register. Reading a
profile and building its maps is under 10 ms even for a megabyte file
with a 90x30x30 look table, and is done once per file and kept; a
header read for the listing is a fraction of that.

**What it does to a picture.** RawTherapee's Canon EOS R6 profile
against the file's own calibrations on an R6 frame: 0.4% RMSE over the
8-bit rendering — both matrices descend from Adobe's, so what the map
is worth is the difference, not a new picture. That is the honest
scale of it for one camera; the point of the line is that a user who
has the DNG converter's profiles, or the maker's, can now use them.

**Left out.** ICC input profiles (§78 already says: until someone
asks). Applying the look table or the tone curve, which is the look
line. Embedding a profile in an exported DNG, which `ProfileEmbedPolicy`
is read for and nothing yet honors. A profile per camera remembered
across files: the edit carries it per picture, and a preset is how it
is carried further. No Adobe DCP is committed; the round trip is
tested on profiles the tests build, and on a real one behind
`GREYCARD_DCP` (RawTherapee's `rtdata/dcpprofiles`, GPL, is where the
one used here came from).
