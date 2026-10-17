# Where greycard fits

A raw editor that works on the light the camera caught, keeps your
folders in charge, and runs on Linux, macOS and Windows. This page
says how the other editors made their choices and what greycard does
differently, for anyone deciding whether to use it, contribute to it,
or wait. The last section says what is built and what is planned.
Updated for 0.7.0, October 2026.

## The short version

Two choices shape every raw editor. What the numbers in the pipeline
mean: light in the scene, or brightness on a screen. And where the
truth about your photographs lives: in your folders, or in the
program's database.

greycard chooses light and folders. The pipeline is scene-referred
from the decoder to the display and color managed at every step, so a
stop of exposure is a stop and highlights keep their color. It can
learn your camera's own rendering, from Canon's Faithful to Fujifilm's
Reala Ace, from the JPEG inside your raws and lay it on as a look.
The library indexes your folders without owning them and keeps
working when a drive is unplugged or a shoot lives on a NAS. It is
GPL-3.0-or-later, so the best published algorithms are ported with
attribution rather than reinvented, and its learned tools run on your
machine.

It is not a Lightroom clone, and not a clean-room answer to one
either. It is the editor we wanted and could not find: the
best part of each existing tool, in one. Lightroom's library and its
culling pace, Capture One's care for color, DxO's denoising, darktable's
pipeline and RawTherapee's demosaic and detail work, with the parts
each of those gets wrong left out. Oh, and for free. It is built by photographers who
edit their own shoots in it every day, so what gets fixed first is
what gets in the way of a real edit, not what looks good in a
screenshot. It is not the most complete editor yet. It is developed on
Linux, where the alternatives are darktable and RawTherapee, and runs
the same on Windows and macOS.

## Two ideas behind the field

### Scene-referred and display-referred

Scene-referred numbers stand for light: mid grey is 0.18, a cloud can
be four, a specular highlight forty, and doubling is one stop
everywhere. Nothing is clipped or bent to fit a screen until the last
step. Display-referred numbers stand for brightness on the display:
one is the brightest it can show, the tone curve has already been
applied, and there is no light above white.

The choice decides how the controls behave. Scene-referred, exposure
is a multiply, white balance and highlight recovery work on real
light, and the curve comes last; HDR output is a different curve at
the end. Display-referred, every control after the curve works on
bent values: exposure looks like a stop in the midtones and behaves
oddly above them, and saturation shifts hue with brightness.

Display-referred was the right call in 2003, when Camera Raw shipped
and the reference was a print. The editors from that era are still
on it, because every edit their users ever saved must keep rendering
the same. The editors that started after 2015 are scene-referred, and
their problem is the other one: making a physically correct curve
feel as immediate as the sliders people learned on. That is the gap
greycard works in.

### The catalog as truth, or as cache

Lightroom's catalog is a database beside a folder tree, and so is
darktable's. The difference is what happens when they disagree with
the disk. In Lightroom the database wins: a file moved outside the
program gets an exclamation mark, the catalog must be backed up, and
ratings, keywords and edits live only in it unless you opt in to
writing them out.

The other way keeps the folders as the truth and treats the database
as an index that can be thrown away and rebuilt. Edits and metadata
live in a sidecar beside the file; collections, which cannot be read
off the disk, live in one small file. It is harder to build, because
files move while you look at them, but it removes the failure people
hate most.

## The editors

**Lightroom Classic.** The reference everyone compares against, and
the library nobody has matched. Display-referred with the curve baked
in, tuned for two decades against what photographers reach for, which
is why its sliders feel right even where they are physically wrong.
Adobe's own color, which is where "Lightroom doesn't look like my
camera" comes from. Good local AI denoise and masks; generative
remove in the cloud. The catalog is the truth. Subscription, Windows
and macOS, new cameras on launch day, and an ecosystem of presets,
plugins and tutorials. From it greycard takes the library's shape,
the filmstrip, the culling keys and the develop panel's order, so a
Lightroom hand finds its way in an hour.

**Capture One.** The choice for tethered studio work and for color:
in-house camera profiles widely held to render skin best, and the
reference tethering. Display-referred, sessions as well as catalogs,
the highest price, Windows and macOS. From it greycard takes the
standard it holds color to; the camera-match look is greycard's way
of reaching the camera's own rendering.

**DxO PhotoLab.** The best denoiser (DeepPRIME is what the others are
measured against) and lens corrections measured per camera and lens
pair. A wide-gamut working space since recently. Slow, few local
tools, a weak library, perpetual license, Windows and macOS. From it
greycard takes the idea of denoising the raw sensor data with a
learned model, before demosaic, running on your machine.

**darktable.** Scene-referred since 2019, with filmic and sigmoid as
the display transform, proper chromatic adaptation, OpenCL, a
database plus sidecars, tethering, and more modules than anyone
uses. GPL, on Linux. Its controls never felt like anything a
photographer recognized, and the module count makes the first hour
hard. Culling is slow. From it greycard takes the pipeline: scene-
referred, color managed end to end, with a proper chromatic
adaptation.

**RawTherapee.** The best demosaicing and detail tools anywhere: AMaZE
and RCD, deconvolution capture sharpening, careful chromatic
aberration and defringe work, DCP profiles, film simulations as
HaldCLUTs. No library beyond a file browser, local adjustments came
late, and the pipeline is not scene-referred end to end. GPL, on
Linux. From it greycard takes the most: the demosaic, the capture
sharpen, the defringe and the profiled denoiser are ports, with
attribution in each file.

**RapidRAW.** A Rust and React editor with AI masks and a good-looking
interface that found a following fast. Its pipeline is sRGB
throughout and applies white balance in the wrong space; its
[README](https://github.com/CyberTimon/RapidRAW/tree/b4b2bbeaa77f24c3bd54522171a4ed79d98b3a95)
says it is "not for users who seek absolute, perfect color accuracy".
AGPL with one owner, so its code can neither take from darktable and
RawTherapee nor go anywhere else.

**Others.** digiKam is a library without a developer. Photo Mechanic
and FastRawViewer are cullers. ON1 and Luminar sell looks. ART is a
RawTherapee fork with a cleaner interface.

## Side by side

Where a cell says planned, the design is in `notes.md` and the item
is in `roadmap.md`; nothing planned is counted as done.

| | Lightroom | Capture One | DxO | darktable | RawTherapee | greycard |
|---|---|---|---|---|---|---|
| Pipeline | display | display | wide gamut, display | scene | mixed | scene |
| Working space | ProPhoto | proprietary | DxO wide | Rec.2020 | ProPhoto | linear Rec.2020 |
| Color managed to the monitor | yes | yes | yes | yes | yes | yes, with soft proofing |
| Camera color | Adobe's | in-house, the best | DxO's | matrix | matrix, DCP | matrix, DCP, and a look fitted from the camera's own JPEG |
| Demosaic | Adobe's | proprietary | proprietary | RCD, AMaZE | AMaZE, RCD, dual | AMaZE, RCD, dual; Markesteijn for X-Trans |
| Denoise | AI, local | conventional | DeepPRIME | profiled | profiled | profiled, and a raw-domain network that runs locally |
| Masks | AI people, sky, objects | layers | limited | parametric, drawn | local adjustments | gradient, radial, brush, by lightness and color; subject, object, sky and people's parts by model |
| Retouch | remove, generative | heal, clone | limited | retouch | spot removal | heal, clone, learned fill |
| Library | the catalog is the truth | sessions and catalogs | weak | database plus sidecars | file browser | folders as truth, index as cache; browsable offline, archive backup; collections by content, surviving a move |
| Culling speed | fast, embedded previews | fast | slow | slow | slow | fast, from the embedded JPEG, or a local preview for an offline or network drive |
| Tethering | basic | the reference | none | gphoto2 | none | planned |
| Stacking | HDR, panorama | none | none | none | none | focus, from the command line; HDR and panorama planned |
| License | subscription | subscription or perpetual | perpetual | GPL | GPL | GPL-3.0-or-later |
| Linux | no | no | no | yes | yes | yes, first |
| Windows and macOS | yes | yes | yes | yes | yes | yes |

## Who it is for

Photographers who want correct color without learning darktable, and
who would rather their editor read their folders than own them. On
Linux, where there is nothing else like it; on Windows and macOS as
an alternative that costs nothing and does not hold your catalog.
Anyone who wants the camera's own rendering as a starting point
rather than Adobe's. People who print, since the display path is
color managed with soft proofing. Developers who want a raw engine as
a library with no interface toolkit, tone-mapping choice or edit
schema in it. It is tested with Canon, Fujifilm GFX and X-Trans,
Sony, Panasonic and Nikon bodies.

## Who it is not for, yet

Anyone on an Intel Mac or Windows on ARM: the ONNX runtime ships no
WebGPU build for either. Anyone with a large rated and keyworded
Lightroom library, until the catalog import exists. Anyone whose
camera rawler does not decode; support goes upstream to rawler, not
into a fork. Anyone who wants a phone app, cloud sync or a preset
marketplace, which are not planned.

## Why it could be built now

Three reasons. GPUs do float arithmetic everywhere, a 45 megapixel
float image fits in memory, and the scene-referred approach has been
worked out in public by the ACES community and darktable's
developers; greycard's design rules summarize what they learned.
There is no installed base, so no saved edit anywhere has to keep
rendering the same. And the GPL: the demosaic, the profiled denoiser,
the deconvolution sharpen and the defringe are each a port of years
of someone else's published work, with attribution, which is what the
license is for.

## Where it stands

Built, as of 0.7.0:

- **Pipeline.** Decoding through rawler. Linear Rec.2020 from the
  camera matrix to the monitor profile, with soft proofing, and AgX as
  a second display curve. RCD, AMaZE and dual demosaic, Markesteijn
  for X-Trans. Highlight reconstruction, chromatic aberration
  correction and a hue-based defringe, DCP camera profiles, a profiled
  denoiser and a learned one with published weights, deconvolution
  sharpening, lensfun corrections. Every operation has a CPU reference
  and a test; the GPU runs the editor's.
- **Editing.** Exposure, tone, parametric and point curves, a color
  mixer, grading, black and white, texture, clarity, dehaze, grain,
  vignette. Crop, rotation and a guided perspective tool. Heal, clone
  and a learned fill. Scopes, readable for a mask alone. Presets,
  including Lightroom's, 3D LUT looks from `.cube` and HaldCLUT
  files, and a look fitted from the camera's own JPEG per body and
  picture style. A history that names its steps, snapshots, and
  versions of a frame in one sidecar, each with its own edit, history
  and export.
- **Masks.** Gradient, radial, brush, by lightness and by color, each
  with its own light, color, white balance and detail. Subject,
  background, sky, object and people's parts by model, on your
  machine; a person picked from a group is found again when the mask
  is pasted.
- **Library.** Folders the library watches as roots, a tree in the
  left pane, an index that fills the filter with chips and takes a
  filter language. Ratings, flags and labels in the sidecar and in
  XMP, so stars and keywords travel to Lightroom and darktable. A
  share that stops answering never holds the window. Archives: a
  shoot backed up by hash, each copy read back before it counts,
  edits written to both, a copy edited elsewhere joined edit by edit.
  Collections and smart collections that hold a picture or one
  version of it by content and follow it through a move. An import
  from a card with renaming, a preset and a verified backup.
- **Culling.** The strip, a zoomable grid and a lightbox, each frame
  shown as edited and made in the background. Culling from the
  camera's JPEG or a local preview when the drive is offline, compare,
  versions side by side, a rejects folder with a way back, delete to
  the trash.
- **Export.** Embedded profiles and EXIF, presets, a watermark, a
  border, a named folder or a subfolder beside each raw, a queue that
  survives a restart, several frames at once, and a headless
  `--export`. Frame registration and focus stacking to a linear DNG
  from the command line.
- **Around it.** Packages for Linux, macOS and Windows that install
  where each desktop expects. A Report a problem button that fills in
  the version, OS and GPU. A check for a newer release, which can be
  turned off.

Planned, in order: the tone controls made to feel right; a generative
fill; the Lightroom catalog import; HDR merge and panoramas, and the
merges in the browser; tethering. The learned denoiser for X-Trans
alongside these.

The reasoning behind every choice is in `docs/notes/`, one numbered
section a file with `docs/notes.md` as the index, and the plan is
`docs/roadmap.md`.
