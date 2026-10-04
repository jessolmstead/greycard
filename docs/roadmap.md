# Roadmap

The list to glance at and add to. Next is the tag about to be cut: one
sentence a tester can verify and the short list that makes it true,
chosen from the tracks. A tag with fixes only moves the patch number;
one with new features moves the minor. A track is a theme with a
sentence of its own as the test of when it is done, and a track's
sentence coming true is one reason for a minor bump. Everything
else waits in the pool by theme, and an item's "waits on" line decides
when it can join. Bugs sit at the top and belong to Next. Ticked items
move to `docs/changelog.md` under their version, so this file only
holds what is not done. One line an item, the notes section in
brackets, and for a blocked item what it waits on and nothing more;
the reasoning lives in `docs/notes.md`.

## Bugs

- [ ] Auto white balance seems to lean way too warm. (tbf lightroom does too)
- [ ] The Settings sheet's previews cap can show the file's value, not
      the live one: `on_settings_asked` takes the thumbnail cache with
      `try_lock` and falls back to `Settings::load()` while the cache's
      count thread holds it; the prefs test of the field fails under load
      for the same reason

## Next: 0.4.0

A large archive on a NAS opens at once, is browsed by folder from
the left pane and culled without reading a raw over the wire, a shoot
is backed up to it and brought back by hash, and the last folders
opened are a click away; a crop can be drawn, an export is a line in
History, and the Masks tab reads at a glance.

- [ ] One frame in two places, what is left after §224 listed a frame
      from the copy that is there: its sidecar written to both copies
      when both are there, and the frame on screen following to the
      archive's copy when its local root goes; designed in §233, built
      in two parts: the file's side in `greycard-edit` (state ids,
      revisions, meta field times, the comparison and the join), then
      the editor's (the queued write, the pending list, the catch-up
      pass, the status line)

## Tracks

A track is done when its sentence is true. None is ordered before
another; the feel track's measured items come first within it.

### The feel

The default develop matches the camera JPEG's brightness and the
sliders feel like the ones people learned on.

Three of these are measurements with a target and go first: the
baseline exposure per body, the default brightness against the
embedded JPEG, and the slider response fitted to the reference set.
The rest are taste, and taste is not decided alone: each ships both
ways behind a switch, testers say which they keep, and then the
switch goes and the default follows. "Feels like Lightroom" means the
same direction and a similar size in the first third of the travel,
within a tenth of a stop; past that it is a look, and looks are the
Look section's job.

- [ ] A file opened from the Finder opens in the editor: the delegate's
      `application:openURLs:` is written but has not run on a Mac;
      try a double-click, Open With and a Dock drop, at launch and
      while it runs (§148)
- [ ] The default's saturation and the reds' skew, measured under AgX
      once it is the default (§230): the per-channel curve's slope
      added chroma in the mid-tones that read as over-saturated beside
      ACES 2.0 and AgX Punchy (§227), and its yellow skew of bright reds
      is what the eye expects where AgX holds them and they go pink.
      Measure the default develop's chroma by lightness band against
      the camera JPEG and a Lightroom export on the reference set, and
      the reds' hue the same way; Punchy's saturation and the camera
      match's fitted table are the two places to put the answer, not a
      skew of our own. Waits on the AgX port
- [ ] AgX's contrast and white as the feel track's controls (§231): the
      port ships Blender's slope over a range shortened to put white at
      the sensor's clip, about 19 percent more contrast per stop than
      Blender; a second construction keeps Blender's contrast per stop
      (`GREYCARD_AGX_SOFT`, no user control), and the user could not
      pick between them on 33 frames. Decide whether contrast, white
      and the inset's rotation (the reds' warmth, the lanterns) are
      sliders, a preset's, or fixed, by the track's rule
- [ ] Blender's sRGB and P3 guard rail lerps the luminance toward the
      opponent-compensated one by its 0.08 power; that step is Eary
      Chow's and his generator carries no license, so the port ships
      darktable's form of the rail, which departs from Blender's sRGB
      table by a 99th percentile of 18 levels on saturated colors
      inside Rec.2020 (§231). Waits on a license from its author, or on
      a derivation of our own from the method
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
      decide before the track closes. The frame's range is what would
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

### Speed

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
- [ ] The non-local means' symmetric-offset halving, §128's leftover:
      the means are the larger half of the hybrid denoise again (about
      4.4 s of the 45 MP denoise at six threads against the chain's
      2.3), and it is the next second (§164). The whole 45 MP develop
      with the hybrid denoise is 12.7 s at six threads, the number a
      tester can watch
- [ ] An AVX2-and-FMA path for the wavelet chain's taps behind a
      feature check: about as much again on a desktop, nothing on the
      Mac; the first x86 assumption, declined twice (§128, §164)

### AI masks and fill

A sky or a person's face is masked from a menu, and a removed object
is filled from what stood around it.

Decided (§34): ONNX Runtime through `ort`, CPU floor with CUDA, DirectML
or CoreML when found; models downloaded on first use with their
licenses, never bundled; a `greycard-ai` crate that core never sees.

- [ ] AI masks, after Sky (§175): a learned sky matte for dense canopies and twigs
      against glare, where the color-line matte fails (needs clean
      data we assemble); a 4096-wide raster for Sky so a 100 MP
      frame's twigs are not averaged fourfold; more hard negatives
      for the gate (sky-blue walls, a lake reflecting sky with none
      above it, a snowfield filling the frame); the bimodal WebGPU
      prior. Then Water, Mountain, Vegetation, Ground and Building
      shapes from the same label map; then the body's parts as shapes
      of their own, each a mask to put a look on: facial skin, body
      skin, hair, eyebrows, eyes and the iris, lips, teeth, and
      clothing by name (top, dress, coat, trousers, shoes, hat), on
      EasyPortrait's face parser, MediaPipe's landmarks and multiclass
      model, and SAM 2 from landmarks (the CelebAMask-HQ, LIP, ATR and
      DeepFashion2 families are research-only; SAM 3 ruled out for
      now, gated weights and a 1.4 GB text encoder, §34 addendum).
      The parts wait on two calls of the user's: the ImageNet-backbone
      reading of the license rule, and hosting a converted
      EasyPortrait file ourselves rather than fetching from its
      publisher's bucket (SberDevices, a Sberbank company)
- [ ] Generative fill behind the patch layer

### Color

A camera's own profile and look are a picker away, and a chart shot
makes a new one. This and the library can swap order on what testers
ask for first.

- [ ] AgX made the default display curve, after the camera match below
      is refitted under it and §141's baseline and §143's medians are
      re-measured under it: sidecars written before the flip read as
      per channel (a schema rule, so no edited picture moves), a
      picture with no sidecar gets AgX; the viewport timed on an
      integrated GPU and lavapipe first (§231). The match under AgX is
      10 to 20 percent worse than under per channel on the same frames,
      all of it per-frame lightness, and better than today's per channel
      once the exposure match converges (§238). Waits on the one below
      and a refit under it
- [ ] The camera match's exposure match converged: a secant step from
      the match's own two finishes, the slope held to a quarter to four
      times as steep and a step to a stop, at most eight finishes, about
      two more a frame than today (+0.5 s); held-out error 0.0161 to
      0.0131 per channel and 0.0195 to 0.0154 under AgX (§238)
- [ ] A per-frame contrast term in the camera match's fit, measured
      first with `--match-compare`: the offset and slope oracle puts the
      held-out ceiling at 0.0131 per channel and 0.0146 under AgX (§238)
- [ ] The camera match's black cut on the camera's side only, or scaled
      to the curve, so a table fitted under AgX sees the toe it crushes:
      a quarter of the deep-shadow blocks per channel keeps are lost
      under AgX (§238)
- [ ] The comparison tool's baseline from one pipeline: render per
      channel from the same linear develop the OpenColorIO renders
      start from, so a difference is the transform's and not the
      export's lens profile, crop or look (§227, §230)
- [ ] The scene-referred tail's remaining parts, after the AgX port
      (§226 for the design): (2) the master point curve on a norm with
      the channels scaled by one ratio, the R, G and B curves per
      channel; (3) a gamut compression at constant hue after the curves
      and the look table, before the output matrix, in place of the
      clamps at the curve, the look table and the output, measured as
      §226 measures the hue turn in sRGB; (4) the clamp before the
      curves dropped once the curve's range is guaranteed, a final clamp
      to the encodable range kept. Each behind the comparison over the
      archive sets and its own notes section. Waits on the AgX port
- [ ] The color curves' lightness axis near black: the shift is looked
      up by Oklab lightness, a cube root whose slope is unbounded at
      zero, so a few parts in a billion of light at a clipped channel
      read as a lightness near 1e-3 and a curve with a point under 0.05
      turns them into a cast of whole levels; the random parity test
      draws its points from 0.05 for that reason. Decide the axis's
      floor or shape so the curves are stable at black, and lift the
      draw back to the panel's 0.01. §239's fade at black (a shift reads
      light under 2e-6 as black) may be that floor; try the 0.01 draw
      over the seed sweep
- [ ] Camera match follow-ups (§181): a group's error held out over
      every frame rather than four (the four-frame figure is off by up
      to 0.004 and too noisy to size a gap, §238), and that error deciding whether a
      new fit of the same body from another folder replaces the table
      there, where today more frames wins; the per-lens radial report fed
      into the vignetting line below; `style:` in the filter text;
      Nikon, Sony and Panasonic styles once files with the settings
      varied make exiv2 a clean oracle for them (§180)
- [ ] Camera match Refine: the frame on screen, or the selection,
      pinned into a group's sample and the group fitted again, with the
      pinned frames' error and the rest's said before and after; a
      pinned frame that will not register, or has an adaptive setting
      on, refused with the reason. Fast because each frame's block
      pairs are cached under the cache directory, keyed by the frame's
      content hash and a version of the default develop, so a fit over
      a group develops only the frames it has not seen; the held-out
      error over every frame (above) reads the same cache with one
      frame left out each time, and its per-frame list is what says
      which frames to refine with. Never a nudge of the table toward
      one frame: a refined table is still one fit over a sample, so
      the replacement rule still counts frames
- [ ] A progress bar on the camera match run: the sheet shows only a
      line of words today ("starting...", then the group in hand); give
      it §192's bar, filled by frames developed over frames to fit
      across the groups, with the group's name and the frame count
      under it, and the same bar on the fit itself where its iterations
      are counted
- [ ] Rename a look from the Look section: the look's own file and
      every `<name>.<curve>.cube` of it (§235) renamed together in the
      looks folder and the name rewritten in the open folder's
      sidecars, in presets and in snapshots that carry it, so no edit
      goes "(missing)"; the TITLE line inside a .cube is the label and
      can be changed by hand today without breaking anything
- [ ] Profile making from a chart shot, after dcamprof (§78)
- [ ] RawTherapee's DCP profiles offered for a fetch: they are made by
      that project under the GPL, so they can be hosted by us as the
      Sky model is and fetched into the profile directory from the
      Camera section for the bodies they cover, a one-click profile on
      Linux before the chart shot exists (§78); with attribution and
      their license shown as lensfun's is. Waits on a tester asking,
      since it covers a fraction of bodies and the chart shot is the
      real answer
- [ ] The monitor's own profile on Windows and macOS: read it from
      WCS and from ColorSync, so "System" means the display rather
      than sRGB. Today `colord_monitors` returns nothing off Linux
      and the viewport falls back to sRGB unless an ICC file is
      picked by hand, which a wide-gamut monitor makes wrong
      (display.rs:565, §137)

### The library

Directories stay the truth; the catalog is a rebuildable index (§72).
A moved shoot is found, not flagged missing, and a Lightroom catalog
comes across with its ratings and collections.

- [ ] The filter bar's last two chips: a "none" chip for frames with
  no value for a facet (decide it with a library of phone JPEGs in
  hand), and an error row in the index for a file whose hash fails so
  it stops showing under every chip (§168). The all-roots view and
  the filter remembered between sessions are there since §174
- [ ] Edits synced between a shoot and its archive copy by the
  sidecars alone, no database crossing machines: the comparison and
  the join of §233 run in Back up and Bring back, behind every save
  when both copies answer, and as a pass when a root returns. Waits
  on one frame in two places
- [ ] The network assumed slow and sometimes hung: a timeout on every
  read of a root over a network mount so a sleeping NAS does not hold
  the pool; the indexer's reads several at once on such a root,
  latency being the limit; check what the content hash reads of a
  file, since a whole-file hash over 200 GB is a first pass measured
  in hours. The polling fallback for the watcher landed in §188
- [ ] Thumbnails from the edit: the strip's and the grid's picture of a
  frame with an edit is the developed frame, not the camera's JPEG,
  made by the pool from the edit at the thumbnail's size and remade
  when the edit is saved, with the camera's JPEG standing in until it
  lands; what the lightbox needs too
- [ ] Lightbox over a collection, or the picks until collections
  exist: the grid with everything that gets in the way of judging
  consistency taken out. Tiles at a chosen size up to a handful
  across, the developed picture in each rather than the camera's JPEG
  so what is compared is what will export, no badges, names or chips,
  a dark surround, Tab hiding the rest; the sync sheet reachable from
  it so a frame that stands out is matched to its neighbors; one key
  into the loupe on a tile and one back. Needs thumbnails rendered
  from the edit at a larger size
- [ ] Collections and smart collections in one file under
  `~/.local/share/greycard/`, referencing files by hash with the
  path as a hint. To settle first: two identical files sharing a
  hash, a file whose hash changes when another tool rewrites it, and
  whether a collection keeps an order of its own
- [ ] Virtual copies: named versions in the same sidecar, each with
  its own edit and history; a collection references hash plus
  version. Waits on collections
- [ ] Stacks, in the collections file. Waits on collections
- [ ] Faces in the library: detection and grouping by a clean-license
  model, names as keywords in the meta section, a facet in the filter
  bar. The index is there since §160
- [ ] Lightroom catalog import: the `.lrcat` read as SQLite, ratings,
  flags, labels, keywords and captions to the meta section exactly,
  collections and translatable smart collections to the collections
  file, develop settings through the XMP mapper as a named first
  history state, roots remapped by asking; the older process
  versions (PV2003, PV2010), whose keys and slider meanings differ
  from the 2012 ones the mapper reads, mapped or reported as
  approximate, their keys checked against a real old catalog; a
  command with a dry-run report first, then a sheet (§79). Waits on
  collections; the capstone, its own release if it grows
- [ ] A map: the frames' EXIF GPS on tiles, a click to select them, a
  position given to a frame by hand. Low priority

### Merge

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
- [ ] A Compress slider in the Light section: the guide plane of §85
      pulled toward mid grey by a strength, so bright regions come down
      and dark ones up together and the texture inside each rides on
      top untouched (Durand and Dorsey's base and detail, with the
      guided plane as the base; no new pass, the plane is cached
      already); an Auto from the frame's range in stops. One slider,
      not a filmic module with a shoulder and a toe. Wanted most for a
      merged bracket written with headroom; waits on the guided plane's
      halo measurement in the feel track, since one slider moving the whole
      plane makes any halo count
- [ ] Float samples in the DNG writer, for brackets deeper than 16 bits
      hold. Waits on someone needing it (§71)

---

## Pool

Not yet in a track or in Next. An item joins one when its "waits on"
clears or a tester asks for it.


#### Editor

- [ ] The learned mask and fill caches keyed by the develop they were
      made from: the window's Export reuses the mask its viewport made
      from a base whose CA ran on the GPU, where `--export` makes it
      from the export's CPU base (AE 174 on a Subject frame), and the
      disk cache of masks is keyed by file, model and shape, so the
      first run to make one decides it for every later edit (§236)

- [ ] Home and end should go to the beginning/end of the grid/filmstrip
      when it is selected.
- [ ] The panel's look, refined as the editor grows (§14); in develop
      order on Develop, Crop, Masks and Retouch tabs since 2026-09-14
- [ ] Choose output color space/gamut for export: sRGB, Display P3 and
      Rec.2020 since §20; the viewport shows the chosen one since §42
  - ProPhoto as a working space: no (§84). It stays an output space and
    an intermediate inside the operations defined in it
- [ ] The develop sections in an order of the user's: a list of
      section names in settings, the panel a repeater over it with each
      slot instantiating the section its name says, the Masks tab
      following the same order, White balance pinned at the top and
      Monitor, Proof and Demosaic at the bottom, and up and down
      buttons for it in the Settings sheet. Goes with the develop
      panel's move out of app.slint, since that is the same rewiring
- [ ] Resizable side panes: drag the left pane's and the develop
      panel's inner edge, today's widths the minimum, about 500 px the
      most, each kept in the settings; first see what a viewport
      resize costs per pointer move, and hold the redraw to the drop
      if it is a develop (hiding landed, §178; the strip stays fixed)
- [ ] A software GPU when there is none: the worker and greycard-gpu
      ask wgpu for a high-performance adapter and stop when there is
      no adapter at all (a headless box, a VM, a remote desktop), so
      ask again with `force_fallback_adapter` and take lavapipe or
      WARP, slow but right; the develop already runs on the CPU, the
      viewport does not. A tester on such a machine decides whether
      the viewport wants a CPU path too
- [ ] A scene mode for the R, G and B curves: the color curves on
      log-encoded scene values before the display curve, in stops about
      mid grey, as a grading tool (what Resolve's log controls do),
      beside the display-referred curves and not in their place; waits
      on the scene-referred tail above, which decides where the curve
      sits
- [ ] Export naming patterns (a suffix, a sequence number, the date)
      on the sheet and in an export preset, for a set (§167)
- [ ] Export naming patterns and a destination folder on the sheet,
      and so in an export preset; a list of marks so text and a logo
      go on one export; a plate or shadow behind text; a bundled font
      so exports match across machines (§157)
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
- [ ] Remember the last chosen export location when exporting again
- [ ] Rebindable keys: a sheet in Settings to change any shortcut,
      saved in settings.json, with a conflict warning and a reset to
      defaults


#### Library

- [ ] Folders opened outside every root looked at again as a root's
      folders are: opening a folder indexes it, but nothing passes over
      it afterwards, so when it is deleted its rows are never marked
      missing and stay in the index for good (§160's rule covers only
      what a pass reaches); a light look at those folders on the
      launch pass or the network poll, a gone folder's rows marked
      missing
- [ ] "Forget missing frames" on the Settings sheet, with how many and
      the oldest's date, so the missing rows go without the CLI's
      `--prune` (§160)

#### Engine

- [ ] `stack.rs` and `register.rs` warps: rows written the way the lens
      correction's were, with captured models re-read per pixel; the
      lens gain (§165) says they are the next place to look
- [ ] X-Trans demosaic. Waits on an X-Trans frame to test against
- [ ] Per-camera hot pixel defect map. Waits on a place for per-camera
      data and a way to take dark frames in (§13s)
- [ ] A per-photo switch for the hot pixel repair in the edit schema,
      since a misfire (a colored point light one site across on busy
      texture) has no escape in the editor; only the CLI turns it off
      (§229)
- [ ] The hot pixel others test counts a neighbor as lit by ratio and
      sigma alone, so a defect's own bleed into the next photosite
      (about three percent of its excess) spares it; ask the excess to
      be a fraction of the candidate's, measured on the lanterns' one
      recurring site (§229)
- [ ] The sharpen's corner radius offset. Waits on a per-lens notion (§21)
- [ ] Segment highlights' rebuild modes. Waits on a frame that shows the
      need (§13o)
- [ ] Exact denoiser tiling, only if a memory budget demands it (§13p)
- [ ] The 5x5 patch at high ISO, only if a print prefers it (§13v)
- [ ] A tool for a broad out-of-focus color cast: §61's lavender bokeh
      discs, which the defringe cannot reach because the cast is wider
      than any local mean it takes (§74)
- [ ] Nikon High Efficiency raws (the Z9, Z8 and Z6 III's default
      setting; 64 of the 87 Z6 III frames on hand): the codec is JPEG
      XS and a Rust decoder for rawler sits in dnglab/dnglab#835,
      tested on a Z6 III only. Run it on our Z6 III set against the
      lossless frames and report on the PR; review it against the
      standard; the shipping call is separate, since the JPEG XS
      patent pool charges per copy and exempts nothing (§176). A build
      feature off by default, or the macOS system decoder, are the
      ways to carry it without that

#### Immich

- [ ] Check that Immich reads greycard's XMPs from an external library:
      which fields land (keywords above all) and whether it writes back
      into the sidecar; an Immich in a container over a copied shoot
      (§195)
- [ ] A post-export command in an export preset, run with the exported
      paths, so `immich upload` (or any other tool) takes the developed
      picture; the destination folder for a watched library is the
      pool's naming-and-destination item. Waits on the XMP check (§195)
- [ ] A paragraph in `where-greycard-fits.md` on greycard as the raw
      developer for an Immich library. Waits on the XMP check (§195)

#### Nice-to-have

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

#### Platform

- [ ] The window's own monitor for the profile, without choosing it.
      Waits on the Wayland color-management protocol reaching Slint's
      winit backend (§17)
- [ ] HDR output. The same wait; check each Slint release (§5, §17)
- [ ] Wide-gamut output: a profile away once the compositor says which
      monitor (§17)
- [ ] Self-updating: when the check (§186) finds a newer release, an
      Update button on the Settings sheet downloads the build for this
      platform from the release, verifies it against the release's
      checksum, swaps it in beside the running binary and relaunches,
      so nobody downloads and unpacks by hand again; opt-in, with the
      release page a click away as it is now. Waits on signed builds
      (notarization on the Mac, Authenticode on Windows) so a swapped
      binary is not refused by the gate, and on each platform's way of
      replacing a running binary (rename and relaunch on Windows, the
      bundle swapped on the Mac, the tarball's folder on Linux); §186's
      reasons against it stand until those are in hand

#### Upstream

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
- [ ] rawler's TIFF reader follows the next-IFD pointer with no cycle
      check when given no chain limit, so a file whose IFD0 points at
      itself grows memory until killed; its CR3 path parses the CMT
      boxes that way (§180). Report upstream with the 26-byte
      reproducer in `decode/style.rs`'s test; a visited set and a
      default chain limit is the fix
- [ ] rawler 0.8 on a CR3 with a zero-size box in `moov` loops
      until it runs out of memory, and its RW2 decoder divides by
      zero on a zero height (§160). Filed as dnglab/dnglab#848 and
      #850; until the first is fixed a damaged CR3 on a card is the
      one way an index pass can be brought down
- [ ] ort rc.13 ignores every `ep::WebGPU` option (the key prefix is
      applied twice); greycard sets them through a config entry
      instead (§159). Fixed on ort's main in pykeio/ort@bc23566;
      drop the workaround when a release carries it
- [ ] The GPU Subject model on Mac and Windows adapters: its remaining
      Splits need nine storage buffers; an adapter allowing fewer
      falls back to the CPU. Check on the machines (§159)
- [ ] The camera's own vignetting correction is about half of
      lensfun's on the R6 II with the RF 50mm f/1.2: measured against
      the embedded JPEG by the camera match trial
      (`docs/camera-match.md`). Decide whether the profile, a
      strength, or the maker's own table is the answer; ties to the
      next line
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
