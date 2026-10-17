# §328. Importing RawTherapee profiles (2026-10-16)

The first half of the roadmap's `.pp3` item: the mapper and a command.
Reading the profile when a folder opens is the second half and waits
on this one.

RawTherapee keeps a frame's edit in a `.pp3` beside it,
`IMG_0001.CR3.pp3` by default (`IMG_0001.pp3` when told to replace the
extension; both are looked for, the first first). We read it the way
§79 reads a Lightroom catalog: exact metadata, approximate edits, said
plainly, a dry run first. Every mapping below was checked against
RawTherapee's own source (`procparams.cc`, `iptransform.cc`,
`colortemp.cc`, its coarse panel and its language file).

**The file.** A GLib key file: `[Section]` lines, `Key=value` under
them, whole-line comments. We parse it ourselves in
`greycard-edit/src/pp3.rs` (no INI crate is in the lockfile, and the
format is forty lines): `#` and `;` comments, a byte-order mark,
GLib's escapes (`\s`, `\n`, `\t`, `\\`), a pair of quotes around a
value for a profile written by hand, a section named twice merged and
a key given twice its last value. A number must be finite: `nan` and
`inf` parse as floats in Rust, and either carried into an edit wrote a
sidecar that would not read again. A file with no section at all is
refused as not a profile; nothing else is. Lookups walk the list,
which is quadratic in principle and some sixty sections of a few dozen
keys in practice, read once. RawTherapee writes every tool it has into
every profile, most of them switched off, so the mapper reads a profile
for what it sets and not for what it names.

**The version.** `[Version] AppVersion` and `Version` (the profile's
own number, 353 at RawTherapee's current development head) are read
and said. We wrote the reader against 340 to 353 (2018 onward); a
profile outside that, or with no version, is read all the same and the
report says which.

**What lands where.**

| RawTherapee | Here | How |
| --- | --- | --- |
| `[General] Rank` 0-5 | meta rating | exact; out of range named, not held |
| `[General] ColorLabel` 0-5 | meta label | exact: none, red, yellow, green, blue, purple |
| `[General] InTrash` | meta flag | true a reject; false clears a reject, leaves a pick |
| `[Coarse Transformation]` Rotate, HorizontalFlip, VerticalFlip | geometry turns and mirror | exact |
| `[Rotation] Degree` | geometry angle | exact, ±45 |
| `[Crop]` Enabled, X, Y, W, H, FixedRatio, Ratio | geometry crop, aspect, portrait | approximate |
| `[Common Properties for Transformations]` AutoFill, Scale | how the crop is unzoomed | approximate; a stretch named |
| `[Exposure] Compensation` | Light: Exposure | approximate |
| `[Exposure] Black` above 0 (0 too, forced) | Light: Blacks | approximate; a negative one named |
| `[Exposure] Saturation` | Color: Saturation | approximate |
| `[White Balance] Setting=Camera` | white balance as shot | exact |
| `[White Balance]` Custom or a named preset, Temperature, Green | white balance custom | approximate |
| `[LensProfile] LcMode=lfauto`, UseDistortion, UseVignette, UseCA | Lens: profile and its three switches | exact (lensfun both sides) |
| `LcMode` lfmanual, lcp with a file, metadata | the same, lensfun's | approximate |
| `[Distortion] Amount` | Lens: manual distortion | approximate |
| `[CACorrection]` Red, Blue | Lens: chromatic aberration by hand | exact |

The meta goes into the sidecar's `meta`, the edit's settings into a
preset of parts (Light: Exposure, Light: Blacks, Color: Saturation,
Lens: Profile and so on) with White balance a whole section, laid over
the frame's current edit, and the geometry replaced when it is
carried, but for two things a profile has nothing to say about: the
keystone (`vertical`, `horizontal`), which RawTherapee's Perspective
does by a model of its own and which is named rather than mapped, so
the frame's own stays; and a crop the profile could not place because
the frame's size would not read, where the frame's own crop, aspect
and way up stay rather than being cleared by an empty one. Parts, so
that nothing of the frame's edit the profile does not speak to moves. The result is recorded as one state of the
original's history labeled "Imported from RawTherapee"
(`history::import_label`, for the Lightroom catalog's import to use as
well), through `Sidecar::record_as` and saved with `Sidecar::save_in`,
the same writer `presets --apply` uses; the command's new-sidecar code
moved into one helper the two share. The frame's other versions are
left as they were, as a preset laid over the frame leaves them.

**How much is carried.** Over a frame with no history here we carry
what the profile sets off RawTherapee's neutral, as a preset carries
what it sets (`Carry::Set`): nothing else could differ from this
engine's default. Over a frame developed here, which waits for
`--force`, we carry every setting a section we read speaks to, the
neutral ones too (`Carry::Every`): an exposure set back to 0 in
RawTherapee sets this frame's back to 0, a lens profile of `none`
turns the profile off, an empty geometry clears a crop (unless the
profile's own crop could not be placed, when the frame's stays, as
above). A forced
re-import should follow RawTherapee, and the report lists those under
"neutral". Two things are written in either mode that the profile may
not set: the camera's white balance, carried whenever the profile says
`Camera`, so a frame given a white here takes the camera's back; and
the hand-set chromatic aberration, which shares its part with the
profile's switch for it and so is written, at zero when the profile
has none, whenever a lens profile is carried.

**Why each is what it is.**

- The turns: RawTherapee turns its picture clockwise by `Rotate` and
  mirrors the turned picture (its rotate buttons reverse when one
  mirror is on, which is how we know the order); here the source is
  mirrored and then turned counter-clockwise. A mirror after a turn is
  a mirror before the reversed turn, so we build the geometry with
  `turned(-quarters)` and then `flipped`, the editor's own mirror, and
  a test walks a corner through `to_source` to check it.
- The angle: RawTherapee's transform samples the source at
  `x cos - y sin, x sin + y cos` of the output, the matrix
  `Geometry::matrix` uses with the same sign, so `Degree` is our
  `angle` as it is.
- The crop is in RawTherapee's pixels of the frame the way up its
  orientation tag and its own turn stand it, measured on its decode,
  which can be a few pixels larger or smaller than ours; we take
  fractions of our frame's size (`stance_path`'s `shown_size`) and
  hold them to the plane, never closer to an edge than the smallest
  crop allows (a crop held to 0.98 of the way across once asked
  `clamp` for a minimum over its maximum, in f32, and panicked).
  RawTherapee's crop is on its picture as zoomed: with AutoFill on (its
  default) by the fill that leaves no corner empty under a fine angle,
  with it off by `1/Scale` and, across and down, by `Scale horizontally`
  and `Scale vertically` over that (`ascale / scale_h` in its
  transform); ours is on the plane unzoomed, so we scale the crop
  about the center by the same. Each scale is held to its slider's 0.1
  to 10 there: one written by hand as next to nothing (1e-300) made
  the unzoom infinite and a crop edge at the center NaN, which wrote
  `null` into the sidecar. Its fill also counts the distortion
  and the lens profile, which ours does not; approximate under those.
  Its fill also waits on a perspective "render" switch, which is a
  runtime flag and never in a profile. A zoom with no crop to carry it
  is named, and so is a stretch, which this engine cannot make.
- The crop's way up: a named ratio is written landscape and
  `portrait` turns it, so a tall crop is portrait. The image's own
  ratio (`As Image`, RawTherapee's default crop) is the plane's shape
  and `portrait` turns that (`Aspect::ratio`), so there portrait
  means the crop stands against the frame: a tall crop of a tall frame
  is not portrait. Reading every crop's portrait off its own shape
  turned every vertical frame's default crop wide.
- A frame whose size will not read takes the angle and the turns,
  names the crop and keeps its own. A frame turned here (the sidecar's quarter turns) is
  a picture RawTherapee never saw: its crop, angle and turns would land
  on the wrong one, so all three are named instead.
- The exposure is stops on both sides, over different tone pipelines.
- The black point: RawTherapee subtracts `Black` of 65535 after the
  exposure, which is our crush of that fraction of white, divided by
  mid grey. Its negative is a lift by a rule of its own and is named.
- Saturation: ±100 with -100 grey there; our chroma scale's -1 is grey.
- The white balance's kelvin is put where RawTherapee puts it
  (`temp2mulxyz`): on the black body's locus up to 4000 K, on the CIE
  daylight locus above, which sits about 0.003 Duv over the black
  body's between 5000 and 6500 K. We take the daylight chromaticity at
  that temperature and read its kelvin and Duv back through rawcolor
  (now a dependency of greycard-edit, not only a dev one). Its `Green`
  divides the white's green channel (`temp2mul`, `gmul /= green`), so
  above one the picture goes greener, which is a magenta white point,
  negative Duv here. Near daylight the white moves about 0.05 Duv for
  each unit of `1 - 1/green` (worked through sRGB to CIE 1960 uv), so
  we add `-0.05 (1 - 1/green)`, held to the slider's ±0.05. The white
  still reaches the camera through RawTherapee's matrices there and
  ours here, and a named preset with a spectrum of its own there (a
  fluorescent, a lamp) is taken at its kelvin. An auto white balance
  (`autold`, `autitcgreen`) has no counterpart here and is named, as
  is `Equal`, the blue-red equalizer, and a white balance switched
  off.
- The manual distortion: RawTherapee's source radius is the output's
  times `1 - a + a r`, `r` in half diagonals; ours, lensfun's poly3,
  `1 - k + k ρ²`, `ρ` in half shorter sides. Linear against quadratic,
  so no `k` agrees everywhere; we take the one whose corner stands to
  its center as RawTherapee's does,
  `k = (A - 1) / (A - 1 + 1/c²)`, `A = 1/(1 - a)`, `c` the shorter
  side over the diagonal. The sign agrees: positive corrects a
  pincushion on both sides.
- The chromatic aberration by hand: RawTherapee scales each of red's
  and blue's sampled radius by `1 + amount`, and so do we.
- The lens profile: RawTherapee's `lfauto` and our lookup are the same
  database looked up from the same file. Its manual lensfun choice,
  an Adobe LCP, or the raw's own corrections become our lensfun
  lookup, approximate. An LCP mode with no file named is no profile
  there (`useLcp`) and none here. `none` is RawTherapee's default and,
  unforced, is left alone: the frame keeps this engine's default
  lookup.

**Not carried.** The tone curves (`Curve`, `Curve2`, in any mode) are
named rather than mapped. RawTherapee's curve does the tone mapping a
raw gets from the base curve here, and our point curves sit after that
base curve, so a curve carried across would apply the contrast twice;
the auto-matched curve RawTherapee's default profiles use is an
imitation of the camera's JPEG, the base curve's job here outright.
Contrast, Brightness and HighlightCompr under `[Exposure]` are named.
Every other section is named by RawTherapee's name for it when its
`Enabled` is true; `Perspective` and `Vignetting Correction`, which
have no switch, when a value in them is not zero; a section we do not
know and that has no switch, always, since nothing says it does
nothing. Never named: `Version` and `General` (read); the transforms'
shared settings, whose AutoFill and scales are read with the crop and
whose zoom or stretch is named there when it cannot be carried;
`Color Management`, `RAW`, `RAW Bayer`, `RAW X-Trans` and
`RAW Preprocess WB` (the raw's own processing is this engine's choice,
not an edit; the last has no switch and RawTherapee writes it into
every profile); `HLRecovery` and `PostDemosaicSharpening`, switched on
in every profile RawTherapee 5.11 writes and always done here, so
naming them said nothing (a dry run over the author's archive, 4,436
profiles, named both on every frame before this); and `MetaData`,
`Exif`, `IPTC`.

**The command.** `greycard rawtherapee <files or folders>
[--write] [--force]`: a folder is its own raws and pictures, not its
subfolders'; a file named that is neither (a `dir/*` that caught the
profiles and the sidecars) is passed over and said, where it once made
a sidecar of a sidecar. Per frame with a profile, the profile and its
version, then a line each for what came across exactly, what was
approximated, what was carried at its neutral, and what has no place
here, then what would be written, or was. A frame whose sidecar has a
history already (a state before the current one, on any version) is
held back unless `--force`. A frame whose sidecar will not read, or
will not write, is that frame's line and the run goes on; a write
that failed makes the command's exit an error at the end. Then the
totals: frames found and with a profile, files passed over, meta
carried, edits carried as a state, frames held back, unreadable and
failed, and each name passed over with the number of frames it was
set on. Without `--write` nothing is written.

**Open.**

- Whether the tone curve should come across after all, for someone
  who used RawTherapee's curve for a look rather than for the tone
  mapping. A Standard curve could be carried into the RGB point curve
  with the base curve's own contrast taken out first; that is a
  measurement, not a mapping, and wants a few real profiles.
- `[RAW] CA` (RawTherapee's automatic raw-level chromatic aberration
  correction) is treated as the raw's own processing and not named;
  ours is the profile's or the hand-set one. Naming it may be fairer.
- RawTherapee's `Green` changed its calculation in 5.9 (testers
  report a 0.04 offset); we read the number either way.
- The hand-set aberration written at zero with a carried profile: a
  part of its own for the two amounts would leave a frame's hand-set
  ones alone under an unforced import.
- RawTherapee can keep profiles in its cache instead of beside the
  raw; those are not looked for.
- The second half: reading the profile on opening a folder, in the
  window, with the same report as a sheet.
