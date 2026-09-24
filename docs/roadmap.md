# Roadmap

The list to glance at and add to. A release is one sentence a tester
can verify, then the short list that makes it true. Everything else
waits in the backlog by theme, and an item's "waits on" line decides
which release it can join. Bugs sit at the top and belong to the next
release. Ticked items move to `docs/changelog.md` under their
version, so this file only holds what is not done. One line an item,
the notes section in brackets, and for a blocked item what it waits
on and nothing more; the reasoning lives in `docs/notes.md`.

## Bugs

None at present

## v0.1.0: browse, develop, export

A user on Linux, macOS or Windows can install it,
open a shoot, get to any frame, edit it, export it, and report what
went wrong.

- [ ] The first tag, which is the release workflow's first run, on all
      three runners; the Windows package, the Mac's UTI spellings and
      sidecar `.icns` are the parts not verified by running them (§88,
      §110, §137, §140)

## v0.2.0: the feel

The default develop matches the camera JPEG's brightness and the
sliders feel like the ones people learned on.

- [ ] A file opened from the Finder opens in the editor: the delegate's
      `application:openURLs:` is written but has not run on a Mac;
      try a double-click, Open With and a Dock drop, at launch and
      while it runs (§148)
- [ ] Reference frames for the feel: eight frames with a Lightroom edit
      each and the greycard values that match, a check of the default
      develop's brightness against the embedded JPEG, a slider response
      weighted to the first third of the travel, an Auto from the
      histogram (§85). Seven frames in and measured (§143); the
      baseline landed from them (§141). Still wanted: Lightroom bases
      for the forest, the torii, the pagoda and the flowers. Start from
      §150: the set, `tools/reference/`, where each control stands and
      what is open
- [ ] The baseline exposure per camera, from Adobe's `BaselineExposure`
      per body: the Canons land within a tenth of a stop of Lightroom,
      the GFX half a stop over (§141, §143). Waits on one DNG exported
      from Lightroom per body (R6 II, R5 II, R8, GFX 100S II)
- [ ] Highlights and whites by the frame's own range or in fixed stops:
      fixed stops for now (§146); ask testers which feels right and
      decide before this release. The frame's range is what would
      reach under the median, give the watch its full pull and bring a
      clipped sky under white as Lightroom does
- [ ] Whites and shadows reshaped against the set: whites moves only
      the top where Lightroom's moves the mid-tones, and wants two more
      frames of whites exports; shadows over-lifts the mid-tones of a
      low-key frame; the tops still sit 0.1 to 0.2 stops under
      Lightroom's where they are not the sensor's clip (§143, §145,
      §149, §150). Waits on the fixed-or-range decision
- [ ] The guided plane's halo at a hard edge, measured; darktable's
      exposure-independent guided filter if it shows (§85)
- [ ] The contrast filters weighted by chroma through the whole range,
      so a filtered sky keeps its gradient instead of going down as one
      flat band (§87). A change to the default look; waits on the
      reference frames
- [ ] The black and white base grey: Oklab lightness, where every other
      editor mixes the channels, so None starts flatter across hues than
      people expect (§87). A change to the default look and to every
      mono edit written; waits on the reference frames
- [ ] A save counter in the sidecar, so `find` chooses between a
      beside and a folder copy by what was saved last and not by the
      files' times, which a copy tool or two clocks can get wrong;
      the time stays the fallback for sidecars written before the
      field. The wrong choice loses an edit at the next save (§153)

## v0.3.0: speed

On a fanless laptop the fit view follows a slider without a wait,
the sharpen included, and the first Subject mask is there in ten
seconds.

- [ ] A proxy develop for the fit view, as a placeholder and not a
      mode: 2x2 Bayer quads binned to one RGB pixel, shown only while
      the full develop is pending and replaced by it when it lands, so
      the fit view at rest is still the export (§90, §115). The
      camera's JPEG now stands in for the switch (§134), so this is
      only worth building if the Mac timing says the first develop
      itself is the wait; waits on that timing
- [ ] The CoreML provider, so the learned denoiser runs on the Neural
      Engine and the M5's GPU neural accelerators: ort's `coreml`
      feature, a `Provider::CoreMl` with the ML Program format and a
      model cache dir, the free dimensions fixed at 608x608 (every tile
      is that size already), and the compute plan printed from the
      probe before believing any timing, since a node the provider
      declines runs on the CPU with a copy each way (§90)
- [ ] BiRefNet on WebGPU: the decoder's fifty sixteen-way Splits
      rewritten as Slices, so the shader stays under Dawn's sixteen
      storage buffers; our own registry entry with its hash (§97)
- [ ] The profiled denoiser's wavelet chain: three quarters of the
      denoise at six threads now that the means are quick (§128), and
      it scales like the base develop, memory-bound, so more cores
      will not help it; the 45 MP develop with the hybrid denoise is
      22.6 s at six threads, the number a tester can watch

## v0.4.0: cull and rate

A shoot can be culled without developing a frame.

- [ ] Camera, lens and ISO chips on the folder filter; waits on a
      folder index or a cached probe, since reading every raw's EXIF
      on open is twenty seconds on a wedding (§133)
- [ ] Lightbox to see a whole collection and look for consistency

## v0.5.0: AI masks and fill

A sky or a person's face is masked from a menu, and a removed object
is filled from what stood around it.

Decided (§34): ONNX Runtime through `ort`, CPU floor with CUDA, DirectML
or CoreML when found; models downloaded on first use with their
licenses, never bundled; a `greycard-ai` crate that core never sees.

- [ ] AI masks: Sky (a small clean-license model, or SAM 2 seeded) and
      People parts (MediaPipe landmarks driving SAM 2); SAM 3 ruled out
      for now, gated weights and a 1.4 GB text encoder (§34 addendum)
- [ ] Generative fill behind the patch layer

## v0.6.0: color

A camera's own profile and look are a picker away, and a chart shot
makes a new one. This and the library can swap order on what testers
ask for first.

- [ ] Camera match: a look fitted from the maker's embedded JPEG
      against the accurate develop, a matrix, a curve and a small LUT
      per camera and style (§78)
- [ ] Profile making from a chart shot, after dcamprof (§78)
- [ ] The monitor's own profile on Windows and macOS: read it from
      WCS and from ColorSync, so "System" means the display rather
      than sRGB. Today `colord_monitors` returns nothing off Linux
      and the viewport falls back to sRGB unless an ICC file is
      picked by hand, which a wide-gamut monitor makes wrong
      (display.rs:565, §137)
- [ ] Scopes weighted by the active mask: a "selection" toggle on the
      scopes panel, each pixel's bin weighted by the mask's coverage,
      in the shader and the CPU reference alike, so a skin mask on the
      vectorscope shows the skin cloud against the line and a face on
      the waveform shows its level

## v0.7.0: the library

Directories stay the truth; the catalog is a rebuildable index (§72).
A moved shoot is found, not flagged missing, and a Lightroom catalog
comes across with its ratings and collections.

- [ ] `greycard-library` crate: a SQLite index of path, size, mtime,
  content hash (first 64 KB plus size), the filterable EXIF, and
  the sidecar's meta; a CLI listing with a filter expression as
  the testable surface
- [ ] The filter bar across roots and large folders with the EXIF
  facets the index holds: camera, lens, ISO, focal length, date,
  keyword; the folder filter's fields become the index's. Waits on
  the index
- [ ] Thumbnail cache keyed by content hash rather than path, so a
  moved shoot does not re-render. Waits on the index
- [ ] Roots: folders the user has added, an all-roots view, an inotify
  watcher on open roots and an mtime pass on launch; a moved file
  found by hash rather than flagged missing. Waits on the index
- [ ] Archive roots, for a NAS or a mounted cloud folder: Back up
  copies a shoot's new and changed files and sidecars to the archive,
  hash-verified and never deleting there; Remove rejects finds the
  archive's copies by content hash, shows the list, and on confirm
  moves them into a rejects folder on the archive, as culling moves
  them locally; a removal queued while the root is offline. Cloud
  through a mount or a configured command, never a provider's API.
  Waits on roots
- [ ] Collections and smart collections in one file under
  `~/.local/share/greycard/`, referencing files by hash with the
  path as a hint. Waits on roots
- [ ] Virtual copies: named versions in the same sidecar, each with
  its own edit and history; a collection references hash plus
  version. Waits on collections
- [ ] Stacks, in the collections file. Waits on collections
- [ ] Faces in the library: detection and grouping by a clean-license
  model, names as keywords in the meta section, a facet in the filter
  bar. Waits on the index
- [ ] Lightroom catalog import: the `.lrcat` read as SQLite, ratings,
  flags, labels, keywords and captions to the meta section exactly,
  collections and translatable smart collections to the collections
  file, develop settings through the XMP mapper as a named first
  history state, roots remapped by asking; a command with a dry-run
  report first, then a sheet (§79). Waits on collections; the
  capstone, its own release if it grows
- [ ] A map: the frames' EXIF GPS on tiles, a click to select them, a
  position given to a frame by hand. Low priority
- [ ] Import from a card: copy, rename, apply a preset, a backup copy.
  Last

## v0.8.0: merge

A bracket, a focus stack or a panorama becomes a linear DNG beside
its sources. The plan and the sizes are in §71; the brackets to shoot
for testing are in `docs/test-frames.md`.

- [ ] HDR merge of a tripod bracket: exposure-scaled weighted average,
      clipped samples dropped, written with headroom; develop skips
      highlight reconstruction and the white-level clip for it (§71)
- [ ] HDR merge handheld: alignment and reference-frame deghosting
      (§71)
- [ ] A merge action over the selection, with progress; the new DNG
      appears beside its sources (§71). The selection is there since
      §156
- [ ] Panorama stitching: feature matching and RANSAC for the coarse
      homography, registration for refinement, multi-band blend from
      the focus stack (§71)
- [ ] Float samples in the DNG writer, for brackets deeper than 16 bits
      hold. Waits on someone needing it (§71)

---

## Backlog

Not yet assigned to a release. An item joins one when its "waits on"
clears or a tester asks for it.

### Editor

- [ ] The panel's look, refined as the editor grows (§14); in develop
      order on Develop, Crop, Masks and Retouch tabs since 2026-09-14
- [ ] Choose output color space/gamut for export: sRGB, Display P3 and
      Rec.2020 since §20; the viewport shows the chosen one since §42
  - ProPhoto as a working space: no (§84). It stays an output space and
    an intermediate inside the operations defined in it
- [ ] Resizeable / hideable panels
- [ ] A software GPU when there is none: the worker and greycard-gpu
      ask wgpu for a high-performance adapter and stop when there is
      no adapter at all (a headless box, a VM, a remote desktop), so
      ask again with `force_fallback_adapter` and take lavapipe or
      WARP, slow but right; the develop already runs on the CPU, the
      viewport does not. A tester on such a machine decides whether
      the viewport wants a CPU path too
- [ ] Right click on photo for options. Set label, copy develop settings, etc
- [ ] Export over the selection: a queue on the worker with progress
      and the naming policy; today Export takes the frame on screen
      (§156)
- [ ] Export naming patterns and a destination folder on the sheet,
      and so in an export preset; a list of marks so text and a logo
      go on one export; a plate or shadow behind text; a bundled font
      so exports match across machines (§157)
- [ ] A preset laid over the selection, and settings copied and pasted
      from a right-click, both the sync's loop with a different source
      (§156)
- [ ] Geometry in a sync, mapped through each frame's aspect and turn
      as masks already are (§156)
- [ ] The range masks' sample from before the Detail section, so dehaze
      stops moving a luminance window; needs a second full-size texture
      on the GPU (§158)
- [ ] The 5x5 color mean at a fixed scale, so the viewport and a
      smaller export agree on a color window's edge and the mixer's
      hue (§158)
- [ ] A dropper for the luminance window's edges and a swatch of the
      color window beside its sliders (§158)
- [ ] The vibrance protection's skin window (§60, 55 degrees plus or
      minus 15) against the 10 to 59 degrees a pale face measured
      (§158): widen it or not; a change to existing edits


### Engine

- [ ] Every op that writes tiles straight into a frame: check that
      the accumulation does not see the frame's `&mut` rows, since a
      reference read out of a slice carries no `noalias` and the
      inlined write-out cost the means 25 percent until the sums were
      kept out of line (§128)
- [ ] X-Trans demosaic. Waits on an X-Trans frame to test against
- [ ] Per-camera hot pixel defect map. Waits on a place for per-camera
      data and a way to take dark frames in (§13s)
- [ ] The sharpen's corner radius offset. Waits on a per-lens notion (§21)
- [ ] Segment highlights' rebuild modes. Waits on a frame that shows the
      need (§13o)
- [ ] Exact denoiser tiling, only if a memory budget demands it (§13p)
- [ ] The 5x5 patch at high ISO, only if a print prefers it (§13v)
- [ ] A tool for a broad out-of-focus color cast: §61's lavender bokeh
      discs, which the defringe cannot reach because the cast is wider
      than any local mean it takes (§74)

### Nice-to-have

- [ ] Flexible dual monitor support
- [ ] Reference view
- [ ] Different frameline options when cropping
- [ ] Camera tethering. Low priority.
- [ ] Idea: AI sharpening
- [ ] Idea: AI color grade matching to a reference
- [ ] Idea: AI highlight recovery

## Blocked

Work that waits on someone outside the repo: a protocol reaching
Slint, a runtime exposing what it knows, a rawler release. It joins a
release when the wait clears.

### Platform

- [ ] The window's own monitor for the profile, without choosing it.
      Waits on the Wayland color-management protocol reaching Slint's
      winit backend (§17)
- [ ] HDR output. The same wait; check each Slint release (§5, §17)
- [ ] Wide-gamut output: a profile away once the compositor says which
      monitor (§17)

### Upstream

- [ ] The Denoiser's tile sized from the device's memory, so a short
      allocation on a discrete card is not a crash in Dawn and a
      unified-memory Mac is not a swap storm (§90). Waits on ort
      exposing the adapter's memory
- [ ] The "request to hide window failed" line on every quit: Slint
      1.18.0's winit backend holds the window while it dispatches the
      close, so the check fires wrongly and the window stays
      registered; fixed in slint-ui/slint#13507 on 2026-09-19. Waits
      on the release after 1.18.0 (§99)
- [ ] Drop the rawler `[patch.crates-io]`. Waits on a rawler release
      carrying dnglab/dnglab#840 (§13j)
- [ ] The makers' embedded lens corrections, exact where a file carries
      them. Waits on rawler surfacing them (and the Panasonic lens name)
- [ ] lensfun calibrates the Sigma 50mm f/1.4 DG HSM Art with a
      distortion k1 of exactly zero (made on a Canon 6D), so it gets no
      distortion correction where Adobe's profile straightens it; measure
      it and contribute the entry to lensfun (§142)
- [ ] rawler's lens database lacks the Sigma 28mm F1.4 DG HSM Art on
      the Canon mounts and the Sony FE 24-70mm F2.8 GM II, and finds
      two definitions for one Canon ID; each decode of such a file
      warns and asks for an upstream issue. Contribute the entries
      to rawler; until then those warnings are kept out of the log
- [ ] Nikon High Efficiency raws (the Z9, Z8 and Z6 III's default
      setting, intoPIX TicoRAW; 17 to 19 MB files where the lossless
      ones run 26 to 32 MB) do not decode: 64 of the 87 Z6 III frames
      on hand. A codec to contribute to rawler, not a table entry, and
      it decides whether the editor opens most Nikon shooters' folders
