# §333. Measuring the Lightroom mapper against its exports (2026-10-17)

§323 split fitting the Lightroom mapper into two steps, measurement
before fit. This is the first step: a command that pairs a catalog's
images with Lightroom's own exports, develops each raw under the
catalog's settings as the import maps them, and says how far apart the
two pictures are, per frame, by profile and by slider. It changes no
scale in `lightroom.rs`; the numbers below are what the fit, when we
build it, has to work with.

## The pairing, as we found it

**The export history is the record.** The catalogs have no export
table: `AgLibraryExportHistory` does not exist in schema 1500000 or
1504001, `AgOutputImageAsset` and the publish tables are empty, and
`AgLastCatalogExport` is empty. What Lightroom does keep is a develop
history step per export, named `Export - Hard Drive (<local time>)`,
with its `dateCreated` in seconds from 2001 and its `text` holding the
settings of that moment. The step's `text` is not the Lua table the
current settings are: it is four bytes of length and the table
deflated, the form the XMP packets have, so it compares unequal to the
current settings byte for byte while being the same table once
inflated. Inflated, the last export step held the image's current
settings in 78 of 79 exported images in one catalog and 59 of 61 in
another.

**The file's time names the step.** In a 2026 catalog of about 280
images, every one of the 74 files in its export folder has an export
step of its image stamped 0.1 to 22 seconds after it was written, most
within two; a few steps were stamped up to a second and a half before
their file. Lightroom stamps the step once the file is out. So the
step that wrote a file is the earliest of its image's export steps
stamped from five seconds before the file to a minute after it, and
the pair's settings are that step's: confidence `history`. Not the
nearest step, and not the latest one before the file: one image was
exported twice 28 seconds apart with different settings, and its
second file has the first export's step 27.6 seconds before it and its
own 0.4 seconds after. A file found by name alone is `name`, with the
current settings assumed, or `name, exported elsewhere` when the
image's history has export steps and none at the file's time, which
says the file came from some other export. An image Lightroom counts
as never developed (`hasDevelopAdjustmentsEx` -1, one history step) is
not paired by name at all: the import writes nothing for it, so a file
of its name is not a render of anything the import carries.

**Where the exports sit.** In the two catalogs that have exports, the
export folder is named `export` and sits beside the raws' folder (raws
in `<shoot>/<shoot>/`, exports in `<shoot>/export/`) or inside it
(raws in `<shoot>/`, exports in `<shoot>/export/`), and the catalog is
in neither (beside the raws' folder, or in a folder of its own under
the shoot). So the command looks in folders named `export`, `exports`,
`exported`, `jpeg`, `jpegs` or `jpg` under each raw's folder, the
folder above it, the catalog's folder and the folder above that, and
in any `--exports DIR`. The other two catalogs (about 560 and about 155
images) record 61 and 26 exported images in their history, and no
export of them is anywhere in the archive under any name: those two
pair nothing.

**A master and its copy.** Lightroom names a second file of a name
`<stem>-2.jpg`, and a virtual copy exported with its master is often
that second file, written in the same second. The time cannot tell
them apart; the shape usually can, since each image's export has its
crop's proportions. The crop's edges are two opposite corners of the
cropped rectangle as shares of the file's size before any turn, and
`CropAngle` turns the rectangle about the corner at left and top: the
width runs along (cos, sin) and the height along (-sin, cos), down
positive. That reads the cropped sizes the catalog keeps beside the
settings (`croppedWidth`, `croppedHeight`) to the pixel at -0.24 and
1.1 degrees. In the catalog of about 280, all 20 images left in doubt
after the shape were a color master and its black and white copy of
one shape, and the files' pixels settle those: of the files a color
image and a black and white one both could be, the less colorful (the
median spread between a pixel's channels) is the black and white
one's. The toned black and white exports read 6 to 8 levels there and
their color masters 27 to 30, so the comparison is not close; it is
relative rather than a threshold so that a strongly toned black and
white still reads as the greyer of the two. Where the two images have
one file between them there is nothing to compare, and the pair stays
in doubt.

**What pairing found.** The catalog of about 280: 79 images exported
by their history, 74 files, 73 pairs, all by history; 4 exported
images with no file in the folder; one master and its black and white
copy with a single file between them, in doubt. The catalog of about
350: 10 images exported by their history, 42 files, 12 pairs by name
(3 `name`, 9 `name, exported elsewhere`) and none by history, 24 files
of images never developed in it, 3 of a developed image's name and
another shape. Its files are dated a week after the catalog's last
export and their crops are not the catalog's: they were rendered from
another catalog of the same shoot. Its pairs are the name rule working
as designed and are the weakest numbers below.

The times, the channel spreads and the dates above stand on our run
alone: the review read the catalogs and the code but could not read
the archive's export folders.

## The path

**In the editor's binary, as `greycard-ui --measure-lightroom
CATALOG`**, beside `--match-compare`, with `--root CATALOG=HERE` (the
import's mapping), `--exports DIR`, `--limit N` and `--out
report.json`. Not `greycard library measure-lightroom`: the develop
and the finish the camera match uses (`FrameDevelop`, the export's
finish with its look, lens and AI plumbing) live in the editor's
worker, the `greycard` command does not link the editor, and the
measurement tool that already develops frames for a comparison
(`--match-compare`) is a flag of the editor for the same reason.
Nothing is written but the report: no sidecar, no look, the catalog
opened immutable.

The pieces:

- **The reader** is the import's. `lightroom_catalog` now exposes its
  open (the write-ahead handling and the schema check), its read and
  its root mapping to the crate, and `imported_edit(raw, settings)`:
  the settings through `read_table` laid over the frame's starting
  edit, which is exactly what the import records for a frame with no
  sidecar. `lightroom_exports` adds the history steps, the sizes, the
  export folders and the pairing.
- **The develop** is the camera match's: the raw developed once under
  the mapped edit, finished as a 16-bit sRGB picture with no output
  sharpening. The finish's long edge grows with the crop so that the
  cropped part comes out at about the match's 2048.
- **The look.** Where the catalog's profile is a camera-matching one
  ("Camera Faithful", "Camera REALA ACE"), the look store is searched
  for the body's table of that style: the file named for the body as
  the panel names it, then the style, compared with case, spaces and
  punctuation dropped and Lightroom's "/Standard" and the like cut off
  (so "PROVIA/Standard" is "Provia" and "ACROS+G Filter" is "Acros + G
  Filter"). The table the picture's curve reads must be there. Adobe's
  profiles and anything else go without a look and are grouped apart.
- **The crop.** The import does not carry the crop yet, and nearly
  every export is cropped, so the render is cut as Lightroom cut it:
  taken back to the file's own way up, the turned rectangle sampled out
  bilinearly, then turned to the export's shape (the raw's own
  orientation first, then the other quarter turns whose shape is the
  export's, since Lightroom keeps an orientation of its own).
- **The comparison** is the camera match's: `register` lays the export
  over the cut, both are averaged over the match's 24-pixel blocks with
  the textured and clipped blocks dropped (which is what takes the
  export's resize and output sharpening out), and the mean ΔE in
  Oklab, its lightness and chroma parts (signed and as sizes, §260's
  split) and the exposure offset (`offset_stops`) are taken at the
  mapped exposure. Then the brightness is matched through the Exposure
  slider with the match's own secant (`iterate`), and the ΔE taken
  again, so a frame's brightness error reads apart from its color.
- **The report** prints in the CLI's column style and writes JSON with
  `--out`: per frame the image's id in the catalog, the error, its
  parts, the offset, the matched error, the profile and the look, the
  confidence and the export step whose settings were used, whether
  Lightroom counts the image developed, the process version of those
  settings (not the image's as it stands), the temperature and tint it
  arrived with, the sliders off
  their default with Lightroom's values, and the masks, lens blur,
  spots, Enhance and crop; then by profile, global edits and local
  ones apart; then by slider, each slider's frames off their default
  against the rest, once over the global edits and once over every
  frame. A slider is off its default when not zero; the temperature
  and the tint when they moved from what the image arrived with (its
  first history step), which is its as-shot balance; the tone curve
  when a point leaves the diagonal or a parametric amount is set.
  Masks and lens blur (an empty `LensBlur = { }` is on every image and
  is not one) and an old process make a frame local; spots and Enhance
  do not, since neither moves a block's mean much.

**What a frame costs.** About 20 seconds in release under `nice` on a
GFX 100S II or R5 Mark II frame, develop, finish, two to four more
finishes for the brightness, the registration; 73 frames in 21
minutes. The run takes one frame at a time.

## What the real catalogs said

Two catalogs measured: 68 of 73 pairs in the catalog of about 280, 11
of 12 in the catalog of about 350. Six frames did not register
(correlation 0.67 to 0.88 against the 0.9 the match asks), every one
masked; the 0.67 is a name pair from the weaker catalog with a turned
crop, and 20 frames with turned crops registered across the two.

**Almost nothing is a global edit.** 66 of the 68 frames measured in
the catalog of about 280 carry masks (people, sky, objects, brushes,
gradients); 6 of the 11 in the other. The global-edit tables §323
wanted to fit on hold 2 frames in one catalog and 5 in the other, and
those 5 are name pairs of images exported elsewhere. The first finding
is that a photographer's delivered frames are local edits, and a fit on
global edits alone will have a few frames per shoot, not hundreds.

**By body and profile** (mean ΔE; lightness ours minus Lightroom's;
offset is Lightroom over us in stops, mean and spread; matched is the
ΔE with the brightness matched):

| catalog | body | profile | look | frames | ΔE | L | C (mean, mean size) | offset | matched |
|---|---|---|---|---|---|---|---|---|---|
| ~280 | R5 Mark II | Camera Faithful | fitted | 23 (all masked) | 0.038 | +0.010 | -0.007, 0.009 | -0.05 ± 0.14 | 0.032 |
| ~280 | R5 Mark II | Adobe Standard | none | 5 (1 global) | 0.056 | +0.043 | +0.005, 0.006 | -0.34 ± 0.23 | 0.033 |
| ~280 | R5 Mark II | Camera Monochrome | none fitted | 1 | 0.062 | -0.039 | +0.009, 0.010 | +0.08 | 0.058 |
| ~280 | GFX 100S II | Camera REALA ACE | fitted | 15 (1 global) | 0.107 | +0.100 | +0.002, 0.008 | -0.86 ± 0.47 | 0.028 |
| ~280 | GFX 100S II | Camera PROVIA/Standard | fitted | 13 | 0.116 | +0.101 | +0.002, 0.012 | -1.19 ± 0.34 | 0.059 |
| ~280 | GFX 100S II | Camera ACROS | none fitted | 7 | 0.120 | +0.115 | +0.004, 0.007 | -1.32 ± 0.39 | 0.038 |
| ~280 | GFX 100S II | Camera ACROS+G Filter | none fitted | 3 | 0.070 | +0.063 | +0.002, 0.006 | -0.80 ± 0.37 | 0.037 |
| ~280 | GFX 100S II | Camera Monochrome | none fitted | 1 | 0.104 | +0.103 | +0.009, 0.010 | -0.57 | 0.035 |
| ~350 | R5 Mark II | Camera Faithful | fitted | 5 global, name (elsewhere) | 0.060 | -0.006 | +0.008, 0.014 | +0.18 ± 0.16 | 0.056 |
| ~350 | R6 Mark II | Camera Faithful | fitted | 6 masked, name | 0.095 | +0.094 | +0.005, 0.010 | -0.81 ± 0.21 | 0.039 |

What it says about the mapper:

- **The error is brightness first, and per body.** On the R5 Mark II
  under its fitted Faithful look the import lands within a twentieth of
  a stop of Lightroom on average and within 0.038 of ΔE, masks and all.
  On the GFX 100S II it is too bright under every profile, -0.57 stops
  on its one Camera Monochrome frame and -0.8 to -1.3 stops on the
  rest, and that is nearly all of its error: matched, the Reala Ace
  frames fall from 0.107 to 0.028, the same as the R5 Mark II. The
  Reala Ace frames are two sets, not one spread: three sit within a
  tenth of a stop of Lightroom at ΔE 0.018, and the other twelve at
  -0.63 to -1.44 stops; what the three share that the twelve do not is
  for the fit to find, and the mean offset is not a constant to take as
  it is. The R6 Mark II reads about 0.8 stops bright, on name pairs
  alone from the weaker catalog, so that one is a lead, not a number.
  This is §150's open item 2, the baseline per camera, measured on real
  edits rather than on a test set: one exposure constant for every body
  is wrong by most of a stop on the GFX. The
  lightness error is positive (ours lighter) wherever the offset is
  negative and nearly zero once matched, so it is a level, not a
  curve's shape.
- **The chroma is small.** Its signed mean is under 0.01 on every row,
  with a look or without (+0.002 to +0.009 on the rows with none
  fitted); its mean size is 0.006 to 0.012 in the history pairs and
  0.010 to 0.014 in the name pairs. So the profile's color is not
  where the import falls short; the fitted looks carry it, and the
  black and white profiles need little. The Provia table is the
  exception once matched (0.059 against Reala Ace's 0.028), and it is a
  fit-1 table from before §260; it wants refitting before anyone reads
  that row as the mapper's.
- **The sliders cannot be told apart in these catalogs.** Within a
  shoot the photographer pastes one look across the frames and nudges
  the exposure: in the catalog of about 280, highlights, shadows and
  whites are off their default on 68 of 68 frames, the tone curve on
  66, so every slider's "off" group is the whole set and its "at" group
  is empty or the few Adobe frames. The grouped tables §323 asked for
  are in the report and say nothing here, which is itself the answer:
  §323's guess that a shoot is one look over hundreds of frames holds,
  and a per-slider fit needs either catalogs with more varied edits or
  a regression across frames within a body. Within the R5 Mark II's 29
  frames, regressed one slider at a time, the offset leans on
  highlights (r -0.72: the stronger Lightroom's pull, the brighter
  Lightroom sits against us, about 0.6 stops across -25 to -84) and
  on shadows (r 0.61). The sliders move together, so this says a
  regression is worth running, not which of them is off.
- **Masked frames are not much worse where the body is right.** The R5
  Mark II's 23 masked Faithful frames sit at 0.038; the masks are
  mostly people and sky, local in area, and the blocks average over
  the frame. The local frames are measured and listed apart, as §323
  asked, but they are the bulk of what a photographer would bring.

**Looks.** The user's look store has fitted tables for the R5, R5
Mark II (Faithful, Standard), R6, R6 Mark II, R6 Mark III, R7 and R8
Faithful, and the GFX 100S II's Provia and Reala Ace. Used: R5 Mark II
Faithful (fit 1), R6 Mark II Faithful (fit 2), GFX 100S II Provia and
Reala Ace (fit 1). With no table: the GFX 100S II's Acros, Acros+G
Filter and Monochrome, and the R5 Mark II's Monochrome; 12 frames
measured, every one black and white. A look for the monochrome styles
is worth fitting from the camera JPEGs before the fit uses those
frames. The Adobe Standard frames (5) have no look by design.

## Open

- The baseline per body comes before any slider: the GFX's stop would
  swamp every slider's fit. Either a per-body exposure constant
  measured here (the offsets above, from exports rather than from one
  DNG per body) or the DNG's `BaselineExposure` as §150 proposed.
- The slider fit, if it comes, is a regression within a body over
  every frame, masked ones included, with the body's offset taken out
  first; frames with a global edit are too few to fit on alone.
- The frames that did not register were all masked, at 0.85 to 0.88
  but for one; 20 turned crops registered, so the cut is right. We kept
  the camera match's 0.9: a frame that misses it is reported, not
  measured on a guess.
- Pairing by name trusts the current settings; the catalog of about
  350 shows how wrong that can be (exports from another catalog of the
  same shoot). Name pairs are reported as such, those of images
  exported at other times apart, and the fit should take `history`
  pairs only.
- A master and its black and white copy with one file between them
  stay in doubt; the time does not tell them apart (the copy's step
  0.3 seconds after the file, the master's 2.5).
- The export step is found by its English name, `Export - `; a
  Lightroom in another language names it otherwise.
- Two catalogs' exports were not in the archive at all; with them the
  measurement would have twice the frames and two more shoots, which is
  what the held-out-by-shoot test §323 wants.
