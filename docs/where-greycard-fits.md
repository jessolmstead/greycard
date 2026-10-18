# Where greycard fits

A raw editor that works on the light the camera caught, keeps your
folders in charge, and runs on Linux, macOS and Windows. This page
says what the other editors chose and what greycard does differently,
and ends with what is built and what is planned. Updated for 0.7.0,
October 2026.

## The short version

Two choices shape every raw editor: whether the numbers in the
pipeline mean light in the scene or brightness on a screen, and
whether the truth about your photographs lives in your folders or in
the program's database.

greycard chooses light and folders. The pipeline is scene-referred
and color managed from the decoder to the display, so a stop of
exposure is a stop and highlights keep their color. It can learn your
camera's own rendering from the JPEG inside your raws and lay it on
as a look. The library indexes your folders without owning them and
keeps working when a drive is unplugged or a shoot lives on a NAS. It
is GPL-3.0-or-later, so the best published algorithms are ported with
attribution, and its learned tools run on your machine.

It is not a Lightroom clone. It is the editor we wanted and could not
find: Lightroom's library and culling pace, Capture One's color,
DxO's denoising, darktable's pipeline, RawTherapee's demosaic and
detail work, in one, for free. We edit our own shoots in it, so what
gets fixed first is what gets in the way of a real edit. It is not
the most complete editor yet.

## Two ideas behind the field

**Scene-referred or display-referred.** Scene-referred numbers stand
for light: mid grey is 0.18, a highlight can be forty, and doubling
is one stop everywhere. Exposure is a multiply, white balance and
highlight recovery work on real light, and the curve comes last.
Display-referred numbers have the curve already applied, so every
control after it works on bent values: exposure misbehaves above the
midtones and saturation shifts hue with brightness. Display-referred
was the right call in 2003, when the reference was a print, and the
editors from that era stay on it because every saved edit must keep
rendering the same. The editors that started after 2015 are
scene-referred, and their problem is making a correct curve feel as
immediate as the sliders people learned on. That is the gap greycard
works in.

**The catalog as truth, or as cache.** Lightroom's catalog is a
database beside a folder tree, and so is darktable's. In Lightroom
the database wins when they disagree: a file moved outside the
program gets an exclamation mark, and ratings, keywords and edits
live only in the catalog unless you opt in to writing them out. The
other way keeps the folders as the truth and the database as an
index that can be rebuilt, with edits and metadata in a sidecar
beside the file. It is harder to build, because files move while you
look at them, but it removes the failure people hate most.

## The editors

**Lightroom Classic.** The reference, and the library nobody has
matched. Display-referred and tuned for two decades, so its sliders
feel right even where they are physically wrong. Adobe's color, which
is where "it doesn't look like my camera" comes from. The catalog is
the truth. greycard takes its library's shape, the filmstrip, the
culling keys and the develop panel's order, so a Lightroom hand finds
its way in an hour.

**Capture One.** The choice for tethering and for color, with
in-house profiles widely held to render skin best. Display-referred
and the highest price. greycard takes the standard it holds color to;
the camera-match look is how greycard reaches the camera's own
rendering.

**DxO PhotoLab.** The denoiser the others are measured against, and
lens corrections measured per camera and lens. Slow, few local tools,
a weak library. greycard takes the idea: denoise the raw sensor data
with a learned model, before demosaic, on your machine.

**darktable.** Scene-referred since 2019, with proper chromatic
adaptation, a database plus sidecars, and more modules than anyone
uses. Its controls never felt like anything a photographer
recognized, and culling is slow. greycard takes the pipeline.

**RawTherapee.** The best demosaicing and detail tools anywhere, and
no library beyond a file browser. greycard takes the most: the
demosaic, the capture sharpen, the defringe and the profiled denoiser
are ports, with attribution in each file.

**RapidRAW.** A Rust and React editor with AI masks and a good-looking
interface. Its pipeline is sRGB throughout and applies white balance
in the wrong space; its
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

## Who it is for, and not yet

Photographers who want correct color without learning darktable, and
would rather their editor read their folders than own them. Anyone
who wants the camera's own rendering as a starting point rather than
Adobe's. People who print, since the display path is color managed
with soft proofing. Developers who want a raw engine as a library
with no interface toolkit, tone-mapping choice or edit schema in it.
Tested with Canon, Fujifilm GFX and X-Trans, Sony, Panasonic and
Nikon bodies.

Not yet for anyone on an Intel Mac or Windows on ARM, since the ONNX
runtime ships no WebGPU build for either, or anyone whose camera
rawler does not decode; support goes upstream to rawler, not into a
fork. A phone app, cloud sync and a preset marketplace are not
planned.

## Where it stands

Built, as of 0.7.0:

- **Pipeline.** rawler decoding. Linear Rec.2020 from the camera
  matrix to the monitor profile, soft proofing, AgX as a second
  display curve. RCD, AMaZE and dual demosaic, Markesteijn for
  X-Trans. Highlight reconstruction, chromatic aberration and
  defringe, DCP profiles, a profiled denoiser and a learned one,
  deconvolution sharpening, lensfun corrections. Every operation has
  a CPU reference and a test.
- **Editing.** Exposure, tone, curves, a color mixer, grading, black
  and white, texture, clarity, dehaze, grain, vignette. Crop,
  rotation, guided perspective. Heal, clone, learned fill. Scopes.
  Presets, including Lightroom's, 3D LUT looks, and a look fitted
  from the camera's own JPEG. A named history, snapshots, and
  versions of a frame in one sidecar.
- **Masks.** Gradient, radial, brush, by lightness and by color, each
  with its own light, color, white balance and detail. Subject,
  background, sky, object and people's parts by model, on your
  machine.
- **Library.** Folders watched as roots, a tree, an index that fills
  the filter with chips and takes a filter language. Ratings, flags
  and labels in the sidecar and in XMP, so they travel to Lightroom
  and darktable. A share that stops answering never holds the window.
  Archives backed up by hash and joined edit by edit. Collections
  that follow a picture through a move. Import from a card with
  renaming and a verified backup. A Lightroom Classic catalog brought
  across whole. RawTherapee's profiles offered when a folder opens.
- **Culling.** Strip, grid and lightbox, each frame shown as edited.
  From the camera's JPEG or a local preview when the drive is
  offline. Compare, versions side by side, stacks, a rejects folder
  with a way back, delete to the trash.
- **Export.** Embedded profiles and EXIF, presets, watermark, border,
  a queue that survives a restart, a headless `--export`. Focus
  stacking to a linear DNG from the command line.
- **Around it.** Packages for Linux, macOS and Windows. A Report a
  problem button that fills in the version, OS and GPU. A release
  check that can be turned off.

Planned, in order: the tone controls made to feel right; a generative
fill; HDR merge and panoramas; tethering. The learned denoiser for
X-Trans alongside these.

The reasoning behind every choice is in `docs/notes/`, one numbered
section a file with `docs/notes.md` as the index, and the plan is
`docs/roadmap.md`.
