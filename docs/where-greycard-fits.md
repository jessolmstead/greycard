# Where greycard fits

A raw editor built to be correct, yet familiar, on Linux
first and then everywhere.
This document says what that means, how the other editors made their
choices, and what greycard does differently. It is written for anyone
deciding whether to use it, contribute to it, or wait. Written
September 2026, when the engine is built and the library is planned;
the last section says which is which.

## The short version

Every raw editor answers two questions. What do the numbers in the
pipeline mean: light in the scene, or brightness on a screen? And
where does the truth about your photographs live: in your folders, or
in the program's database? The field divides on both, and most of
what people love and hate about each editor follows from those two
answers.

greycard answers: light in the scene, and your folders. It is
scene-referred from the decoder to the display transform, color
managed at every boundary, in physical units, with a CPU reference
and a test for every operation. Its library, when built, will index
your directories without owning them. It is GPL-3.0-or-later so that
the best published algorithms can be ported with attribution instead
of reinvented worse, and it runs its learned tools on your machine.

It is not the most complete editor and will not be for some time. It
is the one whose foundations are right. It is developed on Linux,
where the alternatives are darktable and RawTherapee, and its stack
carries to Windows and macOS, where the alternatives cost money and
own your catalog.

## Two ideas that explain the field

### Scene-referred against display-referred

Scene-referred numbers stand for light. One is a reference white, a
cloud can be four, a specular highlight forty, and mid grey sits at
0.18. Doubling is one stop, exactly, everywhere. Nothing is clipped
and nothing is bent to fit a screen until the last step.

Display-referred numbers stand for how bright a pixel should be on
the display. One is the brightest the display can show. The scene's
range has already been through a tone curve to fit, and there is no
light above white any more.

The choice decides how every control behaves. In a scene-referred
pipeline, exposure is a multiply, white balance and highlight
recovery work on real light, color mixes as light mixes, and the
tone curve comes last. HDR output is a different curve at the end. In
a display-referred pipeline the curve is applied early and every
later control works on bent values: exposure becomes a curve
adjustment that looks like a stop in the midtones and behaves oddly
above them, saturation shifts hue as brightness changes, and HDR
output means rebuilding the pipeline.

Display-referred was the right engineering call in 2003, when
Camera Raw shipped. Files were six megapixels, GPUs did not do float
arithmetic, and the reference for what a photograph should look like
was a print. The editors that started then are still on it, because
every edit their users have ever saved must keep rendering the same.
The editors that started after 2015 are scene-referred, and they have
a different problem: making four sliders on a physically correct
curve feel as immediate as the ones people learned on. Nobody has
fully solved the second problem yet. That gap is what greycard is
for.

### The catalog as truth against the catalog as cache

Lightroom's catalog is a SQLite database beside a real folder tree,
and so is darktable's. The difference is what happens when they
disagree with the disk. In Lightroom the database is the truth: a
file moved outside the program gets an exclamation mark, the
catalog must be backed up, and one catalog is open at a time.
Ratings, keywords and edits exist only in it unless you opt in to
writing them out.

The alternative keeps the folders as the truth and treats the
database as an index that can be thrown away and rebuilt. Metadata
and edits live in a sidecar beside the file. Collections, which
cannot be derived from disk, live in one small file. The index is
fast and disposable. This is harder to build than it sounds, because
files move while you look at them, but it removes the failure mode
people hate most.

## The editors

### Lightroom Classic

The reference everyone compares against, and the library nobody has
matched. Its develop module is display-referred with the tone curve
baked in, tuned for two decades against what photographers reach
for, which is why its sliders feel right even where they are
physically wrong. Its color rendering is Adobe's own, fitted from
chart shots under two illuminants and corrected by hand, and it is
the source of the complaint that Lightroom's colors do not look like
the camera's. Its AI denoise and masks are good and run in the cloud
or on a large local model. Its catalog is the truth, with all that
brings. It needs a subscription, runs on Windows and macOS, and
supports a new camera on the day it ships. It is a product with an
ecosystem: presets, plugins, a phone, tutorials, and the muscle
memory of most working photographers.

### Capture One

The professional's choice for tethered studio work and for color.
Its camera profiles are made in-house per body and are widely held
to be the best rendering of skin from any editor. Its tethering is
the reference. Its layers and color editor are strong. It is
display-referred, keeps sessions as well as catalogs, costs the
most, and runs on Windows and macOS. Its catalog at scale has a
worse reputation than Lightroom's.

### DxO PhotoLab

The best denoiser and the best lens corrections, and not much
library. DeepPRIME is the denoiser the others are measured against,
and DxO's optics modules are measured per camera and lens pair rather
than fitted from a database. It moved to a wide-gamut working space
recently. It is slow, has few local tools, a weak library, a
perpetual license, and runs on Windows and macOS.

### darktable

The one that got the pipeline right. Scene-referred since 2019, with
filmic and later sigmoid as the display transform, color
calibration in a proper chromatic adaptation, OpenCL acceleration, a
library with a database and sidecars, tethering, and more modules
than anyone uses. It runs on Linux and is GPL. Its problem is the
other half: the controls did not feel like anything a photographer
recognized, the project spent years in public argument about it, and
the module count makes the first hour hostile. Culling is slow. It
proves that correct is the easy half.

### RawTherapee

The best demosaicing and detail tools anywhere: AMaZE and RCD, a
capture sharpening built on deconvolution, careful chromatic
aberration and defringe work. It reads DCP camera profiles and ships
film simulations as HaldCLUTs. It has no library beyond a file
browser, got local adjustments late, moves slowly, and its pipeline
is not scene-referred end to end. GPL, on Linux. greycard's demosaic,
sharpen, defringe and profiled denoiser are ports from it and from
darktable, with attribution in each file.

### RapidRAW

A Rust and React editor with AI masks and a good-looking interface
that gained a large following fast. Its pipeline assumes sRGB
throughout, applies white balance in the wrong space, and has
fourteen render paths that disagree with each other. Its license
(AGPL, one owner) means its algorithms cannot come from darktable or
RawTherapee and cannot go anywhere else. The lessons from its
color path are the first design rules in greycard's notes.

### Others

digiKam is a library without a developer. Photo Mechanic and
FastRawViewer are cullers. ON1 and Luminar sell looks. ART is a
RawTherapee fork with a cleaner interface. None is a whole answer on
Linux.

## Side by side

Where a cell says planned, the design is in `notes.md` and the item
is in `roadmap.md`; nothing planned is counted as done.

| | Lightroom | Capture One | DxO | darktable | RawTherapee | greycard |
|---|---|---|---|---|---|---|
| Pipeline | display | display | wide gamut, display | scene | mixed | scene |
| Working space | ProPhoto | proprietary | DxO wide | Rec.2020 | ProPhoto | linear Rec.2020 |
| Color managed to the monitor | yes | yes | yes | yes | yes | yes, with soft proofing |
| Camera color | Adobe's | in-house, the best | DxO's | matrix | matrix, DCP | matrix; DCP and a fit from the camera's own JPEG planned |
| Demosaic | Adobe's | proprietary | proprietary | RCD, AMaZE | AMaZE, RCD, dual | AMaZE, RCD, dual |
| Denoise | AI, cloud or large local | conventional | DeepPRIME | profiled | profiled | profiled, and a raw-domain network that runs locally |
| Masks | AI people, sky, objects | layers | limited | parametric, drawn | local adjustments | gradient, radial, brush, subject and object by model |
| Retouch | remove, generative | heal, clone | limited | retouch | spot removal | heal, clone, learned fill |
| Library | the catalog is the truth | sessions and catalogs | weak | database plus sidecars | file browser | folders as truth, index as cache; planned |
| Culling speed | fast, embedded previews | fast | slow | slow | slow | from the embedded JPEG; planned |
| Tethering | basic | the reference | none | gphoto2 | none | planned |
| Stacking | HDR, panorama | none | none | none | none | HDR, focus, panorama; planned |
| License | subscription | subscription or perpetual | perpetual | GPL | GPL | GPL-3.0-or-later |
| Linux | no | no | no | yes | yes | yes, first |
| Windows and macOS | yes | yes | yes | yes | yes | yes |
| Tests per operation | unknown | unknown | unknown | some | some | every operation has a CPU reference and a test |

## What greycard is for

Photographers who want correct color without learning darktable,
and who would rather their editor read their folders than own them.
On Linux, where there is nothing else like it, and on Windows and
macOS as an alternative that costs nothing and does not hold your
catalog hostage. People who shoot Fujifilm GFX and Canon bodies, which are
what it is developed against. Anyone who wants the camera's own
rendering as a starting point rather than Adobe's, which the planned
camera match is built to deliver from the JPEG already in every raw
file. People who print, since the display path is color managed
with soft proofing. And developers who want a raw engine as a
library, with no interface toolkit, tone-mapping choice or edit
schema in it.

## What it is not for, yet

Anyone on an Intel Mac or on Windows for ARM: ort ships no WebGPU
ONNX Runtime for either, so there is no build. Anyone with a hundred
thousand rated and keyworded frames in Lightroom, until the library
and the catalog import exist. Anyone whose camera rawler does not
decode; support goes upstream to rawler, not into a fork. Anyone who
wants a phone app, cloud sync, or a preset marketplace, which are not
planned.

## Why it was possible

It would be fair to ask why a scene-referred editor with a color
managed pipeline and tested operations could be built quickly when
Adobe's is still display-referred. Three reasons, none of them
cleverness.

**Starting in 2026.** GPUs do float arithmetic everywhere, a
45 megapixel float image fits in memory, and the scene-referred
approach has been worked out in public by the ACES community and by
darktable's developers over ten years of argument. greycard's design
rules are a summary of what they learned.

**No installed base.** Every rule in the design is free because no
saved edit anywhere has to keep rendering the same. Adobe's greatest
asset is the thing that stops them.

**The GPL.** The demosaic, the profiled denoiser, the deconvolution
sharpen, the defringe: each is a port of years of someone else's
published work, with attribution, in a day or two each. Most of the
engine's quality was borrowed from people who solved the problem and
published the answer, which is what the license is for.

## Where it stands

Built, as of September 2026: decoding through rawler; the color
pipeline from camera matrix to monitor profile with soft proofing;
RCD, AMaZE and a dual demosaic; highlight reconstruction; chromatic
aberration correction, on the GPU in the editor, and a defringe that
acts on a fringe's hue;
camera profiles from DCP files; a profiled denoiser and a learned one
with published weights; deconvolution sharpening, on the GPU in the
editor; lens corrections from lensfun; exposure, tone, a parametric
and a point curve, a color mixer, grading, black and white, texture,
clarity, dehaze, grain, vignette; gradient, radial, brush, subject and
object masks; heal, clone and a learned fill, each outlined where it
was drawn; crop, rotation and a guided perspective tool; scopes;
presets, including Lightroom's, and three film looks among them;
3D LUT looks from `.cube` and HaldCLUT files; export with embedded profiles and
EXIF; a filmstrip and a zoomable grid from the camera's previews,
with ratings, flags and color labels kept in the sidecar and set from
the keyboard, and a folder filter on all of them with a count on
every chip, and read from and written to XMP sidecars so stars,
labels and keywords travel to Lightroom and darktable (picks and
rejects do not, since XMP has no field for them); culling from the camera's JPEG, with compare and a
rejects folder, and a quarter turn for a frame the camera got the
wrong way up, in every view; the shot's settings under the file name, with its
size;
frame registration and a focus stack merged to a linear DNG from
the command line; the camera's own picture in the viewport the
moment a frame is chosen, replaced by the develop when it lands;
and a package for each of Linux, macOS and Windows that installs
where that desktop expects, with the file types registered; and a
Report a problem button that opens the bug form with the version,
the OS and the GPU filled in and the log beside it; several frames
at once, with the settings of one synced across them, a preset laid
over all of them and an export of all of them; export presets and a
watermark; masks by lightness and by color; the Subject mask on the
GPU; a history that names a step by the preset, sync or snapshot that
made it; a right-click menu on a frame, with copy and paste of
settings onto the selection; thumbnails made in parallel on a pool of
threads, a cold folder of hundreds filling in seconds; an import from a card with
renaming, a preset and a verified backup; roots, folders the library
watches, with a view of every file under them and the filter
remembered between sessions; and a library index that fills the grid's filter with chips
for camera, lens, ISO, focal length, day and keyword, with a filter
language behind the text field. Nine hundred tests.

Planned, in order: the tone controls made to feel right; sky and people masks and a
generative fill; a look fitted from the camera's JPEG; the library's roots
and collections; a Lightroom catalog import; HDR merge and panoramas, and the
merges in the browser; tethering.

The reasoning behind every choice is in `docs/notes.md`, numbered by
section, and the list is `docs/roadmap.md`.
