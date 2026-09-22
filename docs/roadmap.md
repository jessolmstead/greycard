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

- [ ] `assets/greycard.desktop` lists no `image/x-olympus-orf`,
      although the browser has read `.orf` since it had a list, so
      a Linux file manager does not offer greycard for one (§137)

## v0.1.0: browse, develop, export

A user on Linux, macOS or Windows can install it,
open a shoot, get to any frame, edit it, export it, and report what
went wrong.

- [ ] There should still be the Open Folder button in grid mode
- [ ] Grid mode background should probably not be pure black, it feels unpolished
- [ ] The `.gcd` type on the Mac: document types and an exported type
      in `Info.plist` with an `.icns` from the same SVG, so the Finder
      offers greycard for sidecars and raws (§110 notes it declares
      none). Windows' half landed in §137
- [ ] CI on Windows too: the third line in `ci.yml`'s matrix, so a
      change that breaks the Windows build is caught on a push rather
      than at a tag. The release workflow is the only thing that
      builds there today (§137)
- [ ] Report a problem… in the editor: opens the issue form with the
      version, the OS version and the GPU filled in, and says where
      the log is (§111)
- [ ] The repo public, issues on, a `bug` label for the issue form
- [ ] The first tag, which is the release workflow's first run, on all
      three runners; the Windows one is the only part of §137 not
      verified by running it (§88, §110, §137)

## v0.2.0: the feel

The default develop matches the camera JPEG's brightness and the
sliders feel like the ones people learned on.

- [ ] Reference frames for the feel: eight frames with a Lightroom edit
      each and the greycard values that match, a check of the default
      develop's brightness against the embedded JPEG, a slider response
      weighted to the first third of the travel, an Auto from the
      histogram (§85)
- [ ] The shoulder's own white: scene white renders at 231 of 255 where
      every camera JPEG puts it at 255 (§85). A change to the default
      look; waits on the reference frames
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
- [ ] Key binding to hide everything but the photo for review

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
- [ ] Luminance masking
- [ ] A color range mask: a hue and chroma window in Oklab with a
      feather, seeded by clicking a spot, sampled before the look so
      it does not move under the edit; a skin preset at the 55 degree
      center the vibrance protection uses
- [ ] Scopes weighted by the active mask: a "selection" toggle on the
      scopes panel, each pixel's bin weighted by the mask's coverage,
      in the shader and the CPU reference alike, so a skin mask on the
      vectorscope shows the skin cloud against the line and a face on
      the waveform shows its level
- [ ] Export presets and a watermark: the sheet's choices kept by name,
      a text or image mark placed at a corner with opacity and scale,
      rendered on the export only

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
- [ ] Collections and smart collections in one file under
  `~/.local/share/greycard/`, referencing files by hash with the
  path as a hint. Waits on roots
- [ ] Virtual copies: named versions in the same sidecar, each with
  its own edit and history; a collection references hash plus
  version. Waits on collections
- [ ] Stacks, in the collections file. Waits on collections
- [ ] Sync settings across a selection: copy chosen sections of one
  picture's edit and paste them onto every selected frame, as a
  preset is laid over an edit. Waits on multi-select in the browser
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
- [ ] Multi-select in the browser and a merge action with progress; the
      new DNG appears beside its sources (§71)
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
- [ ] Settings menu including custom keybindings
- [ ] Resizeable / hideable panels
- [ ] Right click on photo for options. Set label, copy develop settings, etc
- [ ] Select multiple photos for copying settings to them etc
- [ ] `strip = true` under `[profile.release]`: the released
      greycard-ui binary is 110MB unstripped (109.5MB with a full
      symtab), 82MB stripped, for no behavior change
- [ ] Split `app.slint` the same way, 4,436 lines into one file a
      panel section (§130)


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
