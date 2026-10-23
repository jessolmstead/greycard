# Changelog

What's new in each version, newest first. Each item links to the
design note behind it.

## 0.8.2, unreleased

### The window

- Settings > Editing lists the canvas, the monitor and soft proof
  profiles, and the export's format and color space beside where
  they are set; a change in either place shows in both
  ([§407](https://github.com/jessolmstead/greycard/blob/master/docs/notes/407-the-sticky-choices-listed.md)).
- A Theme setting under Interface: Charcoal, Grey, or Light; the
  picture's surround keeps its own setting
  ([§404](https://github.com/jessolmstead/greycard/blob/master/docs/notes/404-three-themes.md)).
- A menu on the Fit button, from its chevron or a right-click: Fit,
  50%, 100%, 200%, and 400%
  ([§402](https://github.com/jessolmstead/greycard/blob/master/docs/notes/402-a-zoom-menu-on-fit.md)).
- The Settings sheet is five categories down its left side: Interface,
  Library, Editing, Keys, and About, which also shows the version and
  where greycard keeps its files
  ([§401](https://github.com/jessolmstead/greycard/blob/master/docs/notes/401-settings-in-five-categories.md)).
- Ctrl on a focused slider's arrow or with the wheel moves a fine
  step, where the slider's number can show it. F puts a mat of the
  canvas color around the picture at Fit. A Settings switch opens one
  section at a time
  ([§397](https://github.com/jessolmstead/greycard/blob/master/docs/notes/397-fine-steps-a-mat-and-one-section-open.md))
- Any shortcut can be changed: Settings ends with a Keys section;
  click a key, press the new one, Esc keeps the old. A key given two
  jobs says so in red; Reset to defaults puts them all back. Kept in
  settings.json
  ([§390](https://github.com/jessolmstead/greycard/blob/master/docs/notes/390-rebindable-keys.md))
- A focused slider stays in view when a section opens above it; the
  filter, a custom aspect, a mask name, the title, and the keywords
  keep the keys while the pointer crosses the picture; Esc leaves
  the crop
  ([§391](https://github.com/jessolmstead/greycard/blob/master/docs/notes/391-keys-follow-ups.md))

### Library

- The grid's cells sit closer; Settings > Library takes the cell size
  as a number and can turn the wheel resize off
  ([§410](https://github.com/jessolmstead/greycard/blob/master/docs/notes/410-the-grids-cell-by-number.md)).
- The export sheet lists the queued sets, each with its preset or
  format, frame count, and folder, and a bin that removes one
  ([§409](https://github.com/jessolmstead/greycard/blob/master/docs/notes/409-the-export-queue-listed.md)).
- Settings can name the folder to open at start instead of the last
  one, and hide the filter bar; Sort moves to the grid's header
  while it is hidden
  ([§403](https://github.com/jessolmstead/greycard/blob/master/docs/notes/403-a-start-folder-and-the-bar-hidden.md)).
- A Lightroom catalog import reads about one fiftieth of what it did
  from each raw (1.6 MB of a 45 MB CR3), writes four sidecars at a
  time, shows a bar in its sheet, says how far it is on the command
  line, and logs its progress. It no longer memory-maps the raws, so a share that drops while the
  Mac sleeps fails the frame instead of crashing the editor
  ([§389](https://github.com/jessolmstead/greycard/blob/master/docs/notes/389-the-catalog-import-over-a-share.md))

### Masks

- A mask's white balance can follow the global: Follow global in the
  mask's section makes its temperature an offset in mired and its
  tint a difference, so a mask stays that much warmer as the global
  moves; off, it names an absolute light as before
  ([§400](https://github.com/jessolmstead/greycard/blob/master/docs/notes/400-a-masks-white-as-an-offset.md))

### Retouch

- Fill can use a second model trained for removal, offered once on a
  machine with a GPU (5.5 GB, under the CreativeML Open RAIL++-M
  license); "Another" on a fill's row makes a different one
  ([§406](https://github.com/jessolmstead/greycard/blob/master/docs/notes/406-remove-behind-fill.md)).
- A deep fill comes back with the frame's grain, measured around the
  hole on the luma and chroma axes, so a removed object no longer
  leaves a smooth patch; the retired Carve model's 208 MB file is
  removed from the store
  ([§398](https://github.com/jessolmstead/greycard/blob/master/docs/notes/398-the-frames-grain-under-a-fill.md))

### Fixes

- The click that closes a menu over the picture no longer zooms the
  picture, and a lone release no longer ends a straighten stroke or
  moves the tint ring
  ([§396](https://github.com/jessolmstead/greycard/blob/master/docs/notes/396-the-click-that-closes-a-menu.md))
- On Windows, a start that died inside a GPU driver as the window
  was made is noticed by the next one, which starts on Direct3D 12
  instead; `WGPU_BACKEND=dx12` does the same by hand
  ([§395](https://github.com/jessolmstead/greycard/blob/master/docs/notes/395-a-start-marker-for-a-driver-that-dies.md))
- The library's index pass and the thumbnails no longer memory-map
  raws, so a share that drops while the machine sleeps fails a frame
  instead of crashing the editor; a CR3's index pass reads about a
  tenth of what it did
  ([§394](https://github.com/jessolmstead/greycard/blob/master/docs/notes/394-the-index-pass-maps-nothing.md))
- An unedited ORF shows in the lightbox: the decoder reads the
  preview an ORF carries, a raw with no preview gets a quick develop
  there, and a raw+JPEG pair shows its JPEG in the lightbox as it
  does in the loupe
  ([§393](https://github.com/jessolmstead/greycard/blob/master/docs/notes/393-the-lightbox-shows-every-raw.md))
- On a laptop with two GPUs the window and the develop take the
  discrete one; `WGPU_POWER_PREF=low` keeps the other. The clipping
  toggles on the RGB scope show plainly when they are on
  ([§392](https://github.com/jessolmstead/greycard/blob/master/docs/notes/392-the-discrete-gpu-by-default.md))
- The histogram, the scopes, the clipping lamps, and the readout under
  the pointer read the export's values again when a monitor profile is
  in use; they were going through the monitor's table, and the shadow
  lamp could stay dark
  ([§388](https://github.com/jessolmstead/greycard/blob/master/docs/notes/388-the-scopes-read-the-export-not-the-monitors-table.md))

## 0.8.1, 2026-10-21

Fixed the 0.8.0 Linux package's AI library, which would not load on
a current glibc (Arch, CachyOS, Fedora), so the masks, the denoiser,
and the fill did nothing. Switching frames is faster, the fill works
at full resolution, and the sliders and tools have keys. Also an
interface scale setting, and the model downloads no longer trust the
emSign intermediates the browsers dropped.

### Install

- A Windows on ARM package, `aarch64-windows.zip`. Nobody has run it
  on a real machine yet; its AI features run on the CPU
  ([§372](https://github.com/jessolmstead/greycard/blob/master/docs/notes/372-a-windows-on-arm-package.md))

### The window

- After clicking a slider: arrows or + and - nudge it, Shift
  for more, comma and period move to the next slider, Tab between
  typed values, Escape lets go
  ([§383](https://github.com/jessolmstead/greycard/blob/master/docs/notes/383-keys-for-the-sliders.md))
- Hotkeys: R crops, W picks the white balance, M and Shift+M start a Linear or
  Radial mask. In the lightbox, J shows the badges
  ([§385](https://github.com/jessolmstead/greycard/blob/master/docs/notes/385-keys-for-the-tools-and-the-lightboxs-badges.md))
- A first open is 0.7 s quicker on eight cores or fewer: the lens
  correction runs on the GPU
  ([§386](https://github.com/jessolmstead/greycard/blob/master/docs/notes/386-the-lens-correction-on-the-gpu.md))
- Going back to a frame costs a fraction of a second: recent bases
  stay in memory, under a budget set by the machine's RAM
  ([§378](https://github.com/jessolmstead/greycard/blob/master/docs/notes/378-a-shelf-of-recent-bases.md))
- The next and previous frames are developed while you look, so a
  shoot steps in about half a second a frame. The prefetch follows
  the filter and sort, and gives way to anything you ask for
  ([§379](https://github.com/jessolmstead/greycard/blob/master/docs/notes/379-the-neighbors-prefetched-onto-the-shelf.md))
- **Scale** in Settings draws the interface at 125% to 200%, from the
  next start
  ([§377](https://github.com/jessolmstead/greycard/blob/master/docs/notes/377-a-ui-scale-setting-applied-at-the-next-start.md))
- Without a GPU the editor opens and develops on the CPU, slowly,
  instead of failing at start
  ([§374](https://github.com/jessolmstead/greycard/blob/master/docs/notes/374-a-no-gpu-mode.md))

### Retouch

- The fill works at the frame's own pixels, in a window up to 1024 a
  side, instead of a 512 square stretched back. A new export of the
  same LaMa, fetched on first use
  ([§382](https://github.com/jessolmstead/greycard/blob/master/docs/notes/382-lama-at-the-frames-resolution.md))

### Export

- `greycard-ui FOLDER --export OUT/ --all` exports every frame under
  its own edit; `--exposure`, `--develop-temperature`, and `--agx`
  apply to all
  ([§376](https://github.com/jessolmstead/greycard/blob/master/docs/notes/376-export-a-whole-folder-with-all.md))

### Fixes

- The packaged ONNX Runtime loads on Linux with glibc 2.41 or later;
  it was linked asking for an executable stack, which glibc refuses
  ([§387](https://github.com/jessolmstead/greycard/blob/master/docs/notes/387-no-executable-stack-in-onnx-runtime.md))
- `greycard develop --tint -4` parses; only `--tint=-4` did
- The `._` files macOS leaves beside raws, and every dot-named file,
  are no longer listed as unreadable pictures
  ([§381](https://github.com/jessolmstead/greycard/blob/master/docs/notes/381-hidden-files-are-no-frames.md))
- A Lightroom import with a "NaN" setting no longer writes a sidecar
  the editor refuses. Sidecars, collections, and the roots list are
  synced before the rename, so a crash leaves the old file, not an
  empty one
  ([§373](https://github.com/jessolmstead/greycard/blob/master/docs/notes/373-a-number-that-is-not-finite-and-a-sync-before-the-rename.md))
- An imported Lightroom preset no longer takes its profile's Look
  block as settings. Fuzzing the parsers also fixed four ways a
  sidecar, preset, or XMP could be misread
  ([§375](https://github.com/jessolmstead/greycard/blob/master/docs/notes/375-fuzzing-at-the-boundaries.md))

## 0.8.0, 2026-10-18

A Lightroom Classic catalog comes across whole: its folders as roots,
its edits as sidecars, its ratings, labels, flags, and collections as
ours, and a capture time corrected there kept. The window is two
views, Library and Loupe, with Cull and Develop as the Loupe's modes.
Stacks, keywords and a title typed in the window, a capture time
shift, a sort for the grid, RawTherapee profiles read beside the
raws, Landscape masks, a person picked once given to every picture,
Fujifilm X-Trans finished, a Linux AppImage, and a round of fixes
from the testers. Sidecars written by this build are refused by 0.7.x.

### Culling and the window

- Clarified Library vs Loupe view with a persistent toggle in the top left.
  Cull mode is now in the right panel and allows viewing embedded previews
  or developed frames.
  ([§359](https://github.com/jessolmstead/greycard/blob/master/docs/notes/359-two-views-and-two-modes.md))
- The grid header's Loupe button is gone; the switch at the top of
  the left pane is the one way between the views, with Enter and a
  double-click on a frame as before
  ([§371](https://github.com/jessolmstead/greycard/blob/master/docs/notes/371-one-way-between-the-views.md))
- Ctrl+A selects every frame shown, in the strip, the grid and the
  lightbox
  ([§367](https://github.com/jessolmstead/greycard/blob/master/docs/notes/367-ctrl-a-selects-every-frame-shown.md))

### Library

- Stacks: select frames of one folder and press Ctrl+G to put them
  behind one frame, with the count on a badge. S opens a stack,
  Shift+S makes a frame the top, Ctrl+Shift+G takes frames out, and
  Compare stack shows the members side by side
  ([§334](https://github.com/jessolmstead/greycard/blob/master/docs/notes/334-stacks.md))
- A camera's JPEG beside its raw is one frame, marked +JPEG: one
  rating, one move, one delete. A setting lists them apart again
  ([§332](https://github.com/jessolmstead/greycard/blob/master/docs/notes/332-raw-jpeg-pairs.md))
- A Lightroom Classic catalog imports from the window: Import a
  Lightroom catalog... under the grid's Import, a sheet that asks where
  each folder lives now, shows what the import would do, then writes.
  Ratings, flags, labels, keywords, captions, develop settings, crops,
  virtual copies, collections, smart collections on a camera or lens,
  and stacks come across; the same from the command line
  ([§329](https://github.com/jessolmstead/greycard/blob/master/docs/notes/329-importing-a-lightroom-catalog-part-1-the-command.md),
  [§337](https://github.com/jessolmstead/greycard/blob/master/docs/notes/337-importing-a-lightroom-catalog-part-2-the-sheet-the-crop-the-camera-rules-and-the-stacks.md))
- RawTherapee `.pp3` profiles beside the raws come in: rank, label
  and trash mark, and the edits that have a place here as one history
  step. A folder holding profiles is offered the import once when it
  opens, with the command's report on a sheet; frames developed or
  culled here are left alone unless you say so, and a camera JPEG
  never takes its raw's profile. The same from the command line
  ([§328](https://github.com/jessolmstead/greycard/blob/master/docs/notes/328-importing-rawtherapee-profiles.md),
  [§340](https://github.com/jessolmstead/greycard/blob/master/docs/notes/340-importing-rawtherapee-profiles-part-2-the-offer-the-sheet-and-the-aberration-by-hand.md))
- A Kind row in the filter bar (Raw, JPEG, PNG, TIFF) when a folder
  mixes them; `kind:raw` works in the field too
  ([§326](https://github.com/jessolmstead/greycard/blob/master/docs/notes/326-a-files-kind-as-a-facet-and-a-term.md))
- Import: Add in place adds any folder, with its subfolders, to the
  library where it is and opens it, beside the card copy, which is
  unchanged; the Folders pattern's new `{folder}` keeps a copied job's
  subfolders, and a copy that could not read a folder says so and
  exits 2
  ([§347](https://github.com/jessolmstead/greycard/blob/master/docs/notes/347-three-things-a-tester-did-on-every-frame.md))
- A sort for the grid: the arrow button at the right of the filter
  bar picks capture time, name, rating or date edited, oldest or
  newest first, and the header says which. Remembered per view. A
  single folder opens by name, as the camera numbered it; several
  roots by capture time, so two bodies' frames interleave; a
  collection keeps its hand order until you pick one. The strip,
  culling, the lightbox and `greycard library list --sort` follow the
  same order
  ([§339](https://github.com/jessolmstead/greycard/blob/master/docs/notes/339-a-sort-order-for-the-grid.md))
- A METADATA section on the Develop tab, under CULLING when culling,
  and beside the grid: a Title and a Keywords field, typed words
  completed from the library's own, written to the sidecar and the
  XMP over the whole selection as a rating is
  ([§350](https://github.com/jessolmstead/greycard/blob/master/docs/notes/350-keywords-and-a-title-typed-in-the-window.md))
- One Import... button in the left pane, with a menu: From a card or
  folder..., or A Lightroom catalog...
  ([§353](https://github.com/jessolmstead/greycard/blob/master/docs/notes/353-one-import-button.md))
- Shift capture time... in the frame menu: a selection moved by a
  typed difference, or one frame set to a time and the rest moved
  with it, for a second body whose clock was off. The sort by
  capture time and the Day chips follow; the export's EXIF and the
  XMP carry the corrected time; Clear shift goes back to the camera's
  ([§354](https://github.com/jessolmstead/greycard/blob/master/docs/notes/354-the-capture-time-shifted.md)).
  A capture time corrected in Lightroom comes across the catalog
  import as that shift
  ([§355](https://github.com/jessolmstead/greycard/blob/master/docs/notes/355-a-capture-time-corrected-in-lightroom.md))

### Editing

- The tone curve's black and white points slide along their edges:
  the black point dragged right sets the input black, the white point
  dragged left the input white, and the tones past them take that
  end's value. A click past a slid end adds no point, and an end's In
  can be typed. Issue 28
  ([§362](https://github.com/jessolmstead/greycard/blob/master/docs/notes/362-the-curves-ends-slide-along-their-edges.md))
- Point at the picture and the scopes show that spot: its R, G, B and
  luma levels at the top of the scope, lines through them on the
  histogram, a dot per channel at its column on Wave and Parade, and a
  ring on Vector, to see a cheek against the skin line
  ([§349](https://github.com/jessolmstead/greycard/blob/master/docs/notes/349-the-color-under-the-pointer-on-the-scopes.md))
- A Sync button at the top of the develop panel whenever several
  frames are selected, and the frame menu's paste and sync named for
  the frames they go onto. The sync sheet can shift Temperature, Tint
  and Exposure instead of setting them: each frame moves from its own
  value by the open frame's difference from its as-shot light, as
  Lightroom's Quick Develop does
  ([§346](https://github.com/jessolmstead/greycard/blob/master/docs/notes/346-batch-edits-the-keys-and-the-library-found-without-a-key.md))
- A keys sheet (? or Help > Keys) made from the keys the editor
  binds, searchable, and a Help button with the user guide. Library
  and Develop buttons at the top of the left pane switch between the
  grid and one frame, as G does
  ([§346](https://github.com/jessolmstead/greycard/blob/master/docs/notes/346-batch-edits-the-keys-and-the-library-found-without-a-key.md))
- The crop opens at the frame's own shape on a frame not yet cropped;
  Settings has the choice of Original, Last used (kept from one session
  to the next) or Free. Temperature and Tint are live under As shot and
  show the camera's white: moving either turns As shot off by itself, a
  double-click puts that slider back to the camera's value, and As shot
  puts both back
  ([§347](https://github.com/jessolmstead/greycard/blob/master/docs/notes/347-three-things-a-tester-did-on-every-frame.md))
- Presets: Import... takes several `.xmp` or `.gcp` files at once and
  says what a Lightroom preset could not bring (masks, mostly)
  ([§330](https://github.com/jessolmstead/greycard/blob/master/docs/notes/330-importing-presets-from-the-window.md))
- Rest the pointer on a look in the list to see it on the picture;
  move off and the kept look returns. A click keeps it
  ([§327](https://github.com/jessolmstead/greycard/blob/master/docs/notes/327-a-look-previewed-under-the-pointer.md))
- A "Proof: sRGB" pill on the picture while the soft proof is on; a
  click on it, or S, turns the proof off. The gamut warning is off
  until you switch it on in SOFT PROOF
  ([§352](https://github.com/jessolmstead/greycard/blob/master/docs/notes/352-the-soft-proof-named-on-the-picture.md))

### Export

- Adobe RGB as an export space beside sRGB, Display P3 and Rec.2020,
  with its own gamma in the file and the profile; the soft proof offers
  it too
  ([§336](https://github.com/jessolmstead/greycard/blob/master/docs/notes/336-adobe-rgb-as-an-export-space-and-a-curve-per-space.md))

### Masks

- Landscape: a button beside Sky runs the sky model once and offers
  what it found, largest first: Water, Foliage, Mountain, Building and
  Sky. Each is a mask edged from the picture; Foliage keeps off a
  found sky. A true sky above a sea of cloud that the Sky gate refused
  now passes
  ([§335](https://github.com/jessolmstead/greycard/blob/master/docs/notes/335-the-landscape-masks-as-built.md))
- Sky over a sea of cloud or flat overcast no longer shows square
  holes and stepped plateaus: a sure sky's noisy pixels and the
  unlabeled pockets inside it are held up, never a thing of another
  hue. Sky masks are made again once, and a frame edited with a Sky,
  Landscape or People mask gets its thumbnail remade once
  ([§345](https://github.com/jessolmstead/greycard/blob/master/docs/notes/345-the-sky-matte-over-a-diluted-map-the-sure-sky-held-and-its-pockets.md))
- People: after a paste or sync of a People mask onto several
  pictures, clicking the person on one of them offers "Use this person
  on all N pictures". Each other picture is looked at on its own and
  gets a history step of its own; where the person is not found it
  keeps asking, and the status line says how many
  ([§357](https://github.com/jessolmstead/greycard/blob/master/docs/notes/357-use-this-person-on-all-the-pictures.md))
- On a processor without AVX2, where greycard's own ONNX Runtime
  cannot load, the first AI feature offers to download Microsoft's
  CPU-only build of it, 9 MB on Linux and 79 MB on Windows, under its
  MIT license. Once it is in, the models run on the CPU, slower, and
  the status lines say so. `greycard models --fetch onnxruntime` does
  the same from the command line
  ([§358](https://github.com/jessolmstead/greycard/blob/master/docs/notes/358-microsofts-onnx-runtime-where-ours-cannot-run.md))
- Sky on a frame wider than 6144 pixels, the 100 MP bodies and the
  45 MP ones as shot, gets a raster twice as wide, so twigs, wires and
  leaf rims against the sky lose the light rim they had at 1:1.
  Smaller frames are unchanged
  ([§360](https://github.com/jessolmstead/greycard/blob/master/docs/notes/360-a-wider-sky-raster-for-the-large-frames.md))

### Cameras

- Fujifilm X-Trans raws get the hot pixel repair, Denoise and the
  chromatic aberration correction, which reads the mosaic exactly and
  runs a fifth faster than its first build. The learned denoiser is
  still not on X-Trans
  ([§331](https://github.com/jessolmstead/greycard/blob/master/docs/notes/331-x-trans-step-2-noise-hot-pixels-and-chromatic-aberration.md),
  [§343](https://github.com/jessolmstead/greycard/blob/master/docs/notes/343-x-trans-ca-the-read-the-vote-and-the-green-that-limits-both.md))
- "Fit this camera's look" on a folder on a network share surveys the
  cameras in seconds rather than minutes, reading only the ends of
  each CR3 and RAF, and says how far it has got. macOS `._` files
  beside raws are skipped
  ([§324](https://github.com/jessolmstead/greycard/blob/master/docs/notes/324-the-camera-matchs-survey-over-a-network-share.md))
- Each camera body gets a default exposure of its own, measured
  against its camera's JPEG over the library: the Fujifilm GFX 100S II
  opened about a third of a stop brighter than the Canons and now
  opens at 0.52 stops instead of 0.8; every other body is unchanged to
  the bit. Older edits keep their brightness. Sidecars written by this
  build are refused by 0.7.x, and the thumbnails of edited frames are
  remade once
  ([§348](https://github.com/jessolmstead/greycard/blob/master/docs/notes/348-the-baseline-per-body.md))
- A Lightroom import lands closer to Lightroom's own render: the
  Fujifilm GFX 100S II came in about a stop bright, measured from a
  catalog's exports and now corrected per body on import, and the
  Shadows slider's mapping is fitted from the same exports. A
  measurement for us, `greycard-ui --measure-lightroom`, compares the
  imported edits against the exports, and `--fit-lightroom` proposes
  the mapper's scales from them
  ([§333](https://github.com/jessolmstead/greycard/blob/master/docs/notes/333-measuring-the-lightroom-mapper-against-its-exports.md),
  [§342](https://github.com/jessolmstead/greycard/blob/master/docs/notes/342-fitting-the-lightroom-mapper-to-its-exports-the-bodys-stop-and-what-the-sliders-would-not-give.md))

### Install

- A Linux AppImage beside the tarball: make it executable and run it.
  It needs the kernel's FUSE, or `--appimage-extract-and-run`;
  `--cli` as the first argument runs the command line. The glibc
  floor is the tarball's. The Linux tarball no longer needs liblcms2
  on the system, since lcms2 is now built in, as on macOS and Windows
  ([§369](https://github.com/jessolmstead/greycard/blob/master/docs/notes/369-a-linux-appimage-beside-the-tarball.md))

### Fixes

- Moving between frames no longer flashes the last picture far too
  contrasty, or dark and flat, until the develop lands. The picture
  still on screen was drawn through the new frame's kind of rendering
  and under the new frame's edit, and a frame come back to never put
  its own camera picture up; each now keeps its own. The droppers wait
  for the frame's own develop instead of reading the old picture
  ([§364](https://github.com/jessolmstead/greycard/blob/master/docs/notes/364-the-picture-on-screen-keeps-its-own-kind-and-look.md))
- The window no longer dies when its surface is reconfigured while
  the develop is on the GPU. A resize or a tiling compositor's own
  reconfigure during a white balance or demosaic change, which remake
  the base, hit a race in wgpu's surface configure and took the window
  down with "Failed to wait for GPU to come idle"; the error is now
  caught, the surface configured again at once, and the scopes' read
  back made again if the race dropped it. Issue 27
  ([§363](https://github.com/jessolmstead/greycard/blob/master/docs/notes/363-the-surface-reconfigured-under-the-worker.md))
- A Lightroom preset's Tint came in backwards and too small, a
  magenta cast where green was meant
  ([§325](https://github.com/jessolmstead/greycard/blob/master/docs/notes/325-lightrooms-tint-came-in-backwards.md))
- greycard starts on processors without AVX2 or BMI2 (an Ivy Bridge
  i5, issue #23). The ONNX Runtime it ships is now loaded the first
  time a model runs, not at exec, and on a processor that cannot run
  it the AI features are off with a line saying why, everything else
  as before. The binaries shrink by 26 MB each; a build from source
  makes the runtime with `scripts/ort-dylib.sh`
  ([§338](https://github.com/jessolmstead/greycard/blob/master/docs/notes/338-onnx-runtime-at-first-use-and-the-processors-it-cannot-run-on.md))
- Open folder and every other file chooser work on a Linux session
  with no desktop portal, such as i3 started bare: greycard uses
  zenity or kdialog instead, and with neither it says what is missing
  and what to install
  ([§346](https://github.com/jessolmstead/greycard/blob/master/docs/notes/346-a-file-chooser-without-a-portal.md))
- On Linux the menu entry starts greycard when `~/.local/bin` is not
  on the session's PATH, as under a window manager started from
  `startx`: install.sh writes the editor's full path into it
  ([§347](https://github.com/jessolmstead/greycard/blob/master/docs/notes/347-the-desktop-entry-runs-the-editor-by-its-path.md))

## 0.7.0, 2026-10-15

Versions: a second edit of the same raw, beside the first in the strip,
with its own history and export. Collections and smart collections, a
lightbox, a mask with its own Detail, Fujifilm X-Trans raws, a border
at export, and a round of fixes from the first testers. Sidecars
written by this build are refused by 0.6.x.

### Versions

- New version (Ctrl+' or the frame menu) puts a copy of a frame's edit
  beside it in the strip and the grid, with its own thumbnail and
  history; rating, flag and label stay the file's. Rename, Swap with
  the original and Delete version in the menu
  ([§317](https://github.com/jessolmstead/greycard/blob/master/docs/notes/317-virtual-copies-part-2-the-index-and-the-window.md))
- Compare versions shows the original and its versions side by side
  in culling; Delete on a tile removes that version
  ([§320](https://github.com/jessolmstead/greycard/blob/master/docs/notes/320-virtual-copies-part-3-compare.md))
- A version can be put in a collection on its own
  ([§319](https://github.com/jessolmstead/greycard/blob/master/docs/notes/319-virtual-copies-part-5-collections.md))
- A version exports as `<name>-<version>.<ext>`; headless `--export`
  takes `--version-name`
  ([§313](https://github.com/jessolmstead/greycard/blob/master/docs/notes/313-virtual-copies-part-4-the-version-s-name-in-the-export.md))
- Sidecars are at schema 5 and are refused by 0.6.x
  ([§312](https://github.com/jessolmstead/greycard/blob/master/docs/notes/312-virtual-copies-part-1-the-edit-crate.md))

### Culling and the grid

- A lightbox: L over the grid, a collection or the picks shows the
  frames as edited on a dark surround, two to six across
  ([§307](https://github.com/jessolmstead/greycard/blob/master/docs/notes/307-the-lightbox.md))

### Library

- Collections: gather frames from anywhere in the library with B or the
  frame menu, drag to reorder; a frame stays in through a move. The
  filter bar's Save... keeps a filter as a smart collection
  ([§304](https://github.com/jessolmstead/greycard/blob/master/docs/notes/304-collections-and-smart-collections-built.md))
- Forget missing: drop a collection's references whose files are gone
  for good
  ([§310](https://github.com/jessolmstead/greycard/blob/master/docs/notes/310-forget-missing-built.md))
- The first pass over each folder after the update reads every raw once
  more, to learn its sensor's pattern
  ([§322](https://github.com/jessolmstead/greycard/blob/master/docs/notes/322-x-trans-step-1-markesteijns-demosaic.md))

### Cameras

- Fujifilm X-Trans raws open and develop, with Markesteijn's demosaic.
  Denoising, hot pixel repair and chromatic aberration correction are
  not on X-Trans yet
  ([§322](https://github.com/jessolmstead/greycard/blob/master/docs/notes/322-x-trans-step-1-markesteijns-demosaic.md))

### Masks

- A mask has its own Detail: Texture, Clarity and Dehaze beside its
  Light and Color
  ([§302](https://github.com/jessolmstead/greycard/blob/master/docs/notes/302-detail-in-a-mask-built.md))
- The Adjustments list stays pinned above the Masks tab's sections
  ([§300](https://github.com/jessolmstead/greycard/blob/master/docs/notes/300-the-adjustments-list-stays-pinned.md))

### Editing

- Highlights reaches into the upper midtones the way Lightroom's does,
  and Whites pivots from the frame's median. A stored highlights or
  whites opens a little differently
  ([§315](https://github.com/jessolmstead/greycard/blob/master/docs/notes/315-highlights-and-whites-by-the-frames-own-range.md))
- Tooltips on the icon-only buttons
  ([§311](https://github.com/jessolmstead/greycard/blob/master/docs/notes/311-a-word-at-the-pointer.md))
- A snapshot's row has restore and rename icons; double-click renames
  ([§305](https://github.com/jessolmstead/greycard/blob/master/docs/notes/305-a-snapshots-row-says-what-a-click-does.md))

### Export

- A border at export: width, and white, grey, black or the picture's
  own mean (#19)
  ([§314](https://github.com/jessolmstead/greycard/blob/master/docs/notes/314-a-border-at-export.md))
- The export sheet names the folder it will write to, with a Choose
  button; Export writes there with no dialog (#22)
  ([§301](https://github.com/jessolmstead/greycard/blob/master/docs/notes/301-the-export-sheet-names-its-folder.md))

### Fixes

- The window opens no taller than its screen, so the export sheet's
  buttons stay in reach on a laptop
  ([§306](https://github.com/jessolmstead/greycard/blob/master/docs/notes/306-the-window-opens-no-taller-than-its-screen.md))
- A rating, flag or label key that hides the frame behind the filter
  says so on the status line
  ([§305](https://github.com/jessolmstead/greycard/blob/master/docs/notes/305-a-snapshots-row-says-what-a-click-does.md))
- The brush and the retouch patch no longer resize a dozen times per
  touchpad scroll
  ([§303](https://github.com/jessolmstead/greycard/blob/master/docs/notes/303-the-brush-and-the-patch-size-by-the-wheels-accumulated-delta.md))
- A two-finger touchpad scroll pans the picture; a wheel click zooms a
  step; Ctrl and a scroll zooms smoothly; a trackpad pinch zooms (#17)
  ([§299](https://github.com/jessolmstead/greycard/blob/master/docs/notes/299-a-two-finger-scroll-over-the-picture-pans.md))
- A scroll over an open sheet no longer zooms the picture behind it
  (#21)
  ([§298](https://github.com/jessolmstead/greycard/blob/master/docs/notes/298-a-sheet-keeps-the-wheel.md))
- A drag anywhere on a radial or linear mask moves it (#16)
  ([§297](https://github.com/jessolmstead/greycard/blob/master/docs/notes/297-a-drag-on-a-masks-body-moves-the-mask.md))
- The color mixer's dropper puts itself down after one pick (#20)
  ([§296](https://github.com/jessolmstead/greycard/blob/master/docs/notes/296-the-mixers-dropper-is-one-pick.md))

## 0.6.0, 2026-10-10

The strip and the grid show each frame as edited, made in the background
for frames edited without being opened. The copy settings and preset
sheets offer more granularity, and Check/Uncheck all. A tint cools or warms
without turning hues through green and magenta, and a mask keeps its
own white balance. A wide round of bug fixes.

### Culling and the grid

- An edited frame shows its edit in the strip, the grid, the cull loupe
  and compare; a frame reset to the default shows the
  camera's picture again
  ([§278](https://github.com/jessolmstead/greycard/blob/master/docs/notes/278-thumbnails-from-the-edit-part-one.md))
- A frame edited without being opened (a sync, a preset, or a paste over
  the selection, a save joined from another machine, a folder opened
  with its edits on disk) gets its edit's picture in the strip and the
  grid from a reduced develop in the background, held off while you
  work on an open frame; a frame on an offline root shows its own
  edit's picture by the key the index keeps
  ([§285](https://github.com/jessolmstead/greycard/blob/master/docs/notes/285-thumbnails-from-the-edit-part-two.md))
- A pencil on a grid tile or strip cell marks an edited frame
  and a second mark one that has been exported
  ([§276](https://github.com/jessolmstead/greycard/blob/master/docs/notes/276-marks-for-edited-and-exported-frames.md))
- Cull mode shows the shot's metadata
  ([§274](https://github.com/jessolmstead/greycard/blob/master/docs/notes/274-the-shot-lines-in-cull-mode-and-the-export-choosers-memory.md))

### Color and editing

- The Look section sits at the top of the Develop tab, above White
  balance, since it's typically set first
- Fixed issues with the Tint tool
  ([§286](https://github.com/jessolmstead/greycard/blob/master/docs/notes/286-a-tint-is-a-blend-not-a-turn.md))
- The Tint section has an on/off switch, globally and in a mask, as
  every other section has; off keeps the hue and amount
  ([§294](https://github.com/jessolmstead/greycard/blob/master/docs/notes/294-the-tint-has-a-switch.md))

### Masks

- Improvements to Whole Person masking accuracy
  ([§279](https://github.com/jessolmstead/greycard/blob/master/docs/notes/279-whole-persons-edge-from-subjects-matte.md))
- A mask's overlay turns off by itself when a slider in the mask's look
  moves, so the edit is seen; feather, range, invert and the brush keep
  it showing
  ([§277](https://github.com/jessolmstead/greycard/blob/master/docs/notes/277-the-overlay-steps-aside-for-a-look-edit.md))
- A mask's white balance keeps its temperature and tint while its switch
  is off, so the switch compares the mask's light with the global's; a
  double-click resets them to global
  ([§283](https://github.com/jessolmstead/greycard/blob/master/docs/notes/283-a-masks-white-balance-keeps-its-light-while-off.md))

### Crop

- A Draw button in the Crop section: press it and the next drag anywhere
  on the picture draws a fresh crop held to the aspect, so a crop at the
  full frame can be redrawn without shrinking it first; Alt and a drag
  inside the crop does the same where the desktop leaves Alt free
  ([§295](https://github.com/jessolmstead/greycard/blob/master/docs/notes/295-a-draw-button-for-the-crop-and-alt-to-draw-inside-it.md))

### Presets and copying

- The copy settings sheet and the preset sheet list each section as a
  heading with its sliders under it (Detail: Texture, Clarity, Dehaze;
  Light: the six; the mixer's bands; each curve), so one slider is
  copied alone, with Check all and Uncheck all; "Adjustments" is now
  Masks, the frame's masks are listed by name each with its own check,
  and the pasted masks replace the target's or join them. Presets with
  parts or joining masks are new keys in the file; older builds read
  such a preset as its whole sections only
  ([§288](https://github.com/jessolmstead/greycard/blob/master/docs/notes/288-parts-single-masks-and-check-all-on-the-copy-settings-sheet.md))
- The copy settings and preset sheets work from the keyboard: Tab to a
  check, Space or Return to turn it, Escape to leave
  ([§288](https://github.com/jessolmstead/greycard/blob/master/docs/notes/288-parts-single-masks-and-check-all-on-the-copy-settings-sheet.md))

### Export

- The export chooser opens where the last export went, in the same
  session and after a restart; a preset with its own subfolder still
  wins
  ([§274](https://github.com/jessolmstead/greycard/blob/master/docs/notes/274-the-shot-lines-in-cull-mode-and-the-export-choosers-memory.md))

### Under the hood

- `cargo audit` runs in CI and weekly, every third-party Action is
  pinned by commit hash, and a test pins the import's names from a
  file's own data inside the destination
  ([§272](https://github.com/jessolmstead/greycard/blob/master/docs/notes/272-security-chores-the-audit-the-imports-names-and-the-pins.md))

### Fixes

- Color grading: clicking a wheel or its name makes it the active one,
  marked by a ring and its name in the accent color, and the Hue and
  Strength sliders follow it; they used to stay on the last wheel
  dragged. History rows name the wheel that moved
  ([§293](https://github.com/jessolmstead/greycard/blob/master/docs/notes/293-the-gradings-sliders-follow-the-chosen-wheel.md))
- Fixed a bug when pasting a mask onto another frame
  ([§292](https://github.com/jessolmstead/greycard/blob/master/docs/notes/292-a-learned-masks-raster-belongs-to-the-frame-it-was-made-on.md))
- The Crop tab's Portrait switch means one thing, the crop is tall, on
  every photo and under Original too; a ratio picked on a photo shot on
  end starts tall, and toggling the switch keeps the crop's area, so
  toggling twice gives back the crop you had instead of a smaller one
  ([§291](https://github.com/jessolmstead/greycard/blob/master/docs/notes/291-the-portrait-switch-follows-the-frame-and-a-toggle-keeps-the-crop.md))
- A snapshot can be restored after it is taken: typing its name ends
  with a click anywhere else, so the row goes back to being a button;
  it used to stay in naming mode until Enter, taking every later click
  as a rename
  ([§290](https://github.com/jessolmstead/greycard/blob/master/docs/notes/290-a-new-snapshots-name-ends-with-a-click-away.md))
- Clicking back in the History panel keeps the rows above the click,
  and a click on a greyed row goes forward to it every time; a jump that
  needed no new develop used to leave the newer state's masks and look
  behind, and the next save cut the rows above
  ([§289](https://github.com/jessolmstead/greycard/blob/master/docs/notes/289-a-history-click-keeps-the-rows-above-it.md))
- A mouse wheel's click on a slider is one step every click; it used to
  alternate one step and two, so Exposure moved by 0.05 then 0.10
  ([§284](https://github.com/jessolmstead/greycard/blob/master/docs/notes/284-a-wheel-click-on-a-slider-is-one-step.md))
- A mask's curve editor shows its histogram as soon as the mask is
  chosen, and the global editor keeps its own on the way back to the
  Develop tab; both stood empty until a slider or a point was touched
  ([§282](https://github.com/jessolmstead/greycard/blob/master/docs/notes/282-a-masks-curve-editor-has-its-histogram-when-shown.md))
- Quitting while culling no longer writes the panel's edit onto the
  frame under the loupe (a frame culled from launch got the default
  edit as a new state; a frame culled to after developing another got
  that frame's edit), and an export asked for just before culling
  starts takes each frame's own edit
  ([§280](https://github.com/jessolmstead/greycard/blob/master/docs/notes/280-a-quit-from-culling-records-nothing-from-the-panel.md))
- A frame followed onto its archive copy while its own disk is away
  keeps its last edit at quit: the copy is written before the editor
  ends, waiting at most five seconds, and a write that cannot finish
  is noted for the next window
  ([§275](https://github.com/jessolmstead/greycard/blob/master/docs/notes/275-a-followed-frames-save-at-the-quit.md))
- Picking the person for a People part while zoomed in no longer sends
  the view back to Fit; it read as the mask resetting the view when it
  landed ([§271](https://github.com/jessolmstead/greycard/blob/master/docs/notes/271-a-person-picked-on-a-zoomed-view-keeps-the-view.md))

## 0.5.0, 2026-10-09

Improvements for portrait editing: People masks are introduced, including parts.
Texture, Clarity, and Dehaze run fast on the GPU. Camera match is extended
to Nikon, Sony, and Panasonic.

### Culling and the grid

- A scroll bar under the filmstrip showing where you
  are in the folder and dragging there; Home and End go to the first
  and last frame in the filmstrip and the grid, and Shift+Home and
  Shift+End select the run to that end ([§255](https://github.com/jessolmstead/greycard/blob/master/docs/notes/255-the-strips-scroll-bar-and-home-and-end.md))

### Color and editing

- **Rename…** in the Look section renames a look's files and the name
  in every edit, preset and snapshot that uses it
  ([§257](https://github.com/jessolmstead/greycard/blob/master/docs/notes/257-renaming-a-look.md))

### Masks

- **People**: a mask of a whole person or a part of one, picked from a
  menu: face, facial skin, eyebrows, eyes, iris, lips, teeth, skin,
  hair, hands, and clothing. On a picture of
  several people you click the one you mean, or All people; pasted
  onto another picture, the mask finds the same person by their
  clothes, hair and skin, and asks when it isn't sure or they aren't
  there. The model, about 1 GB, downloads on first use ([§263](https://github.com/jessolmstead/greycard/blob/master/docs/notes/263-the-people-masks-editor-side-as-built.md))

### Camera match

- Adds a progress bar when running camera match
  ([§258](https://github.com/jessolmstead/greycard/blob/master/docs/notes/258-a-bar-on-the-camera-match-run.md))
- A fitted look no longer turns a picture's color noise into blotches
  of lightness, the "deep-fried" look on bark, fabric and skin; a look
  fitted before gives a warning in the Look section, with a Refit button
  ([§260](https://github.com/jessolmstead/greycard/blob/master/docs/notes/260-the-camera-match-keeps-lightness-off-chroma.md))
- **Fit this camera's look…** now reads Nikon, Sony and Panasonic
  raws too, grouping them by Picture Control, Creative Style or Photo
  Style; a frame shot in a scene, auto or effect mode, or with a
  setting greycard can't read, is left out of every group
  ([§265](https://github.com/jessolmstead/greycard/blob/master/docs/notes/265-nikon-sony-and-panasonic-styles-for-the-camera-match.md))

### Export

- With a subfolder typed, the export button says where the files go
  ("Export to edited", "Export 3 frames to edited") instead of
  "Choose file...", a line under the field confirms it, and a name
  the export would refuse disables the button and says why ([§262](https://github.com/jessolmstead/greycard/blob/master/docs/notes/262-export-names-and-the-subfolder-button.md))
- A name already taken gets -1, -2 before the extension, in exports
  and imports, instead of " (2)"; a name that already ends in a
  number, like DSC-0042, becomes DSC-0042-1 and is never counted on,
  and a re-exported set takes the next free names ([§262](https://github.com/jessolmstead/greycard/blob/master/docs/notes/262-export-names-and-the-subfolder-button.md))

### Speed

- Texture, Clarity, and Dehaze all now run on the GPU, making them much
  more responsive; the export is unchanged.
  ([§269](https://github.com/jessolmstead/greycard/blob/master/docs/notes/269-the-dehaze-on-the-gpu.md))
- Clarity takes half the time it did on a picture with a long edge of
  2540 pixels or more, in the editor and the export alike, its filter
  found on a grid of four-pixel blocks; below that it is unchanged
  ([§254](https://github.com/jessolmstead/greycard/blob/master/docs/notes/254-clarity-on-a-quarter-grid-as-built.md))

### Fixes

- Quitting while a picture was still developing on the GPU could crash
  the editor on the way out; so could the desktop session ending
  mid-develop, which also skipped saving settings and the open
  frame's edit. Both now save everything and exit cleanly, and the
  last edit before quitting reaches the frame's archive copy at the
  next start ([§261](https://github.com/jessolmstead/greycard/blob/master/docs/notes/261-leaving-while-the-worker-is-in-a-job.md))
- Fixed a bug in the chromatic aberration correction where a raw
  whose width fell 105 to 111 pixels past a multiple of 112
  crashed its export
  ([§264](https://github.com/jessolmstead/greycard/blob/master/docs/notes/264-the-gpu-ops-checked-at-random-settings.md))
- The viewport and the export agree next to blown highlights, where
  the chromatic aberration correction could differ between the GPU
  and the CPU by up to 37 levels on a few pixels; an export moves
  from 0.4.0's on single pixels only ([§264](https://github.com/jessolmstead/greycard/blob/master/docs/notes/264-the-gpu-ops-checked-at-random-settings.md))
- At high ISO the capture sharpen turned single pixels in the blacks
  into full-brightness magenta, blue and white dots, hundreds to
  thousands a frame at ISO 4000 and up; they are gone, and the
  sharpening elsewhere is as it was ([§268](https://github.com/jessolmstead/greycard/blob/master/docs/notes/268-the-capture-sharpen-on-a-pedestal.md))
- After a camera match run, **Fit** no longer stays on and
  highlighted over the groups it just fitted, where pressing it fitted
  the same frames again; it comes back when a group, the scope or
  Replace existing looks changes ([§267](https://github.com/jessolmstead/greycard/blob/master/docs/notes/267-fit-off-after-a-run-to-its-end.md))
- A Nikon High Efficiency raw (HE or HE*) says so when it can't be
  opened, instead of calling the file possibly corrupt
  ([§266](https://github.com/jessolmstead/greycard/blob/master/docs/notes/266-nikon-high-efficiency-raws-behind-a-build-feature.md))

### For developers

- The GPU sharpen, chromatic aberration correction and Texture and
  Clarity are each checked against the CPU at random settings, as the
  shader is, with `GREYCARD_OP_PARITY_SEED`, `_CASES` and `_ONLY` to
  replay a case, and an ignored test checks the correction on real
  frames ([§264](https://github.com/jessolmstead/greycard/blob/master/docs/notes/264-the-gpu-ops-checked-at-random-settings.md))
- A build from source with `--features nikon-he` opens Nikon's High
  Efficiency raws, through dnglab/dnglab#835's decoder with its curve
  refitted below 1000 DN; the release workflow ensures that the
  published builds leave it out, since
  JPEG XS is licensed per copy of a decoder, and the release workflow
  ([§266](https://github.com/jessolmstead/greycard/blob/master/docs/notes/266-nikon-high-efficiency-raws-behind-a-build-feature.md))

## 0.4.0, 2026-10-05

Sweeping improvements, especially to the library.
A root's folders show as a tree, the grid keeps
the library on its left, and a shoot on a NAS is culled from local
previews without reading a raw over the wire. A root can be an
archive: a shoot is backed up to it and brought back, and one frame
kept in both places stays one frame, its edits joined rather than
written over. AgX comes in as a second display curve, a mask has a
white balance of its own, and exports gain a queue, a subfolder and a
headless command line. The
[user guide](https://github.com/jessolmstead/greycard/blob/master/docs/user-guide.md)
now has a chapter for each part of the editor, besides installing on
each system, the hotkeys, and how to report a problem.

### Library

- A root's folders as a tree in the left pane, each with its count,
  shown for an offline root too; a click opens that folder alone, or
  everything under it with "With subfolders" enabled ([§209](https://github.com/jessolmstead/greycard/blob/master/docs/notes/209-a-roots-folders-as-a-tree.md))
- Added a left pane for file browsing in the grid view
  ([§211](https://github.com/jessolmstead/greycard/blob/master/docs/notes/211-the-grid-with-the-library-on-the-left.md))
- Subfolders show as tiles at the top of the grid with their counts
  ([§212](https://github.com/jessolmstead/greycard/blob/master/docs/notes/212-a-folders-folders-as-tiles-in-the-grid.md), [§246](https://github.com/jessolmstead/greycard/blob/master/docs/notes/246-the-rejects-folders-a-view-shows.md))
- The open folder is named in the pane and the grid's header, and a
  click on its name offers the last ten folders opened ([§206](https://github.com/jessolmstead/greycard/blob/master/docs/notes/206-the-open-folder-named-and-recently-opened.md))
- A folder the index knows opens without reading its
  sidecars first; an offline library root stays in view, dimmed, its pictures
  from the cache and the filter working ([§207](https://github.com/jessolmstead/greycard/blob/master/docs/notes/207-the-index-as-the-sidecars-cache.md))
- A local preview of each frame, kept in the cache, so the culling
  loupe and compare view work under an offline root and don't wait on
  a network one ([§210](https://github.com/jessolmstead/greycard/blob/master/docs/notes/210-local-previews-by-content-hash.md))
- The Settings sheet sets the network roots' poll in minutes and the
  local previews' own cache cap, 8 GB by default ([§215](https://github.com/jessolmstead/greycard/blob/master/docs/notes/215-the-rest-of-the-network-off-the-windows-thread.md))
- The loading bar says what it is reading, and names the root it is
  waiting on when the count stands still (2026-10-04)

### Archives and backup

- A root can be marked as an archive. Back up copies a folder's or a
  selection's frames not yet on it, found by content hash, with their
  sidecars; each copy is verified before it's marked done, and nothing on the
  archive is ever written over or deleted. Bring back is the reverse
  ([§216](https://github.com/jessolmstead/greycard/blob/master/docs/notes/216-archive-roots-revised-at-the-desk.md), [§217](https://github.com/jessolmstead/greycard/blob/master/docs/notes/217-archive-roots-part-one-the-mark-back-up-and-bring-back.md), [§219](https://github.com/jessolmstead/greycard/blob/master/docs/notes/219-the-backup-pairing-is-the-folders-not-the-roots.md))
- Remove rejects from an archive: the open folder's rejects are found
  on the archive and moved to a rejects folder there, or deleted;
  a move asked for while the archive is offline waits until it answers
  ([§218](https://github.com/jessolmstead/greycard/blob/master/docs/notes/218-archive-roots-part-two-remove-rejects-and-its-queue.md))
- One frame in two places: All roots lists a frame from its archive
  copy when the local one is gone; every save goes to the archive's
  copy too; a copy edited elsewhere is joined edit by edit; and the
  status line says how many edits are waiting for an offline archive
  ([§224](https://github.com/jessolmstead/greycard/blob/master/docs/notes/224-one-frame-in-two-places-listed-from-the-copy-that-is-there.md), [§243](https://github.com/jessolmstead/greycard/blob/master/docs/notes/243-one-frame-in-two-places-part-1-the-files-side.md), [§244](https://github.com/jessolmstead/greycard/blob/master/docs/notes/244-one-frame-in-two-places-part-2-the-editors-side.md))
- A library opened by this build is schema 6, which an older build
  refuses and leaves untouched ([§217](https://github.com/jessolmstead/greycard/blob/master/docs/notes/217-archive-roots-part-one-the-mark-back-up-and-bring-back.md), [§244](https://github.com/jessolmstead/greycard/blob/master/docs/notes/244-one-frame-in-two-places-part-2-the-editors-side.md))

### Culling and the grid

- Move back, the inverse of Move rejects, on the frame menu and in
  CULLING ([§225](https://github.com/jessolmstead/greycard/blob/master/docs/notes/225-move-back-the-inverse-of-move-rejects.md))
- A Rejects menu in the grid's header with the count: Move rejects,
  Move back, Delete rejects folder and Remove rejects from each archive
  ([§228](https://github.com/jessolmstead/greycard/blob/master/docs/notes/228-the-rejects-actions-on-the-grids-header.md))
- Delete rejects folder works over all the roots, a root or a folder
  with its subfolders, taking every rejects folder the view lists; an
  archive's rejects are left to the archive's own view ([§246](https://github.com/jessolmstead/greycard/blob/master/docs/notes/246-the-rejects-folders-a-view-shows.md))
- A color label tints the whole grid tile and the strip's cell ([§205](https://github.com/jessolmstead/greycard/blob/master/docs/notes/205-a-color-label-tints-the-grids-plate.md))
- The grid has a scroll bar ([§214](https://github.com/jessolmstead/greycard/blob/master/docs/notes/214-a-scroll-bar-on-the-grid.md))
- A mouse's back and forward buttons step through the grid, as the
  left and right arrows do ([§249](https://github.com/jessolmstead/greycard/blob/master/docs/notes/249-the-mouse-side-buttons-and-a-deleted-rejects-folder.md))

### Color and editing

- AgX as a display curve, Blender's wide-gamut formation with the
  "AgX - Punchy" look, behind a per-picture switch at the foot of
  LIGHT; per channel stays the default ([§230](https://github.com/jessolmstead/greycard/blob/master/docs/notes/230-agx-over-aces-the-call-from-two-sets.md), [§231](https://github.com/jessolmstead/greycard/blob/master/docs/notes/231-agx-ported-as-a-display-transform.md))
- A mask has a white balance of its own on the Masks tab: Temperature,
  Tint and Neutral, in absolute units, so it holds when the global white moves
  ([§241](https://github.com/jessolmstead/greycard/blob/master/docs/notes/241-a-white-balance-in-a-mask.md))
- A Background shape: the complement of the Subject matte ([§204](https://github.com/jessolmstead/greycard/blob/master/docs/notes/204-a-background-shape.md))
- The Masks tab is laid out to be read at a glance: one card for the
  mask, one row a shape with its own invert ([§203](https://github.com/jessolmstead/greycard/blob/master/docs/notes/203-the-masks-tab-laid-out-to-be-read-at-a-glance.md))
- A crop can be drawn by dragging outside the current rectangle ([§200](https://github.com/jessolmstead/greycard/blob/master/docs/notes/200-draw-a-crop-by-dragging-outside-todays-rectangle.md))
- Hot pixel repair is on by default, and only repairs a pixel lit in
  one color, so stars, glints and catchlights are kept ([§229](https://github.com/jessolmstead/greycard/blob/master/docs/notes/229-hot-pixels-on-by-default-the-other-colors-decide.md))
- Auto white balance reads the light off the picture's edges, so a
  frame full of sky or sea no longer comes out amber ([§245](https://github.com/jessolmstead/greycard/blob/master/docs/notes/245-auto-white-balance-from-edges.md))

### Camera match

- A fitted look keeps a table per display curve, records the curve and
  the body it was fitted for, and the Look section offers "Refit under
  AgX..." where a look has none; the look list is grouped by body
  ([§235](https://github.com/jessolmstead/greycard/blob/master/docs/notes/235-a-fitted-look-has-a-table-per-display-curve.md))
- The match sheet has a box on each body and style to choose what Fit
  runs over, and "Remove this look..." moves a look's tables to the
  trash ([§237](https://github.com/jessolmstead/greycard/blob/master/docs/notes/237-the-match-sheets-groups-chosen-and-a-look-removed.md))
- The match converges its exposure match, a smaller error under both
  curves ([§238](https://github.com/jessolmstead/greycard/blob/master/docs/notes/238-the-camera-match-under-agx-measured-on-the-same-frames.md), [§240](https://github.com/jessolmstead/greycard/blob/master/docs/notes/240-the-camera-match-converges-its-exposure-match.md))

### Export

- An export queue: Add to queue keeps a set with its edits, preset and
  destination, kept across a restart, and Export the queue runs it
  later ([§213](https://github.com/jessolmstead/greycard/blob/master/docs/notes/213-an-export-queue.md))
- A Subfolder field writes each frame into a folder beside its raw;
  under Overwrite, the files about to be replaced are named first, with
  Keep both the default ([§242](https://github.com/jessolmstead/greycard/blob/master/docs/notes/242-an-export-into-a-subfolder.md))
- An export is a line in History, so a click goes back to the look
  that was sent out ([§198](https://github.com/jessolmstead/greycard/blob/master/docs/notes/198-an-export-as-a-line-in-history.md))
- `--export` is headless: no window, no GPU, about twice as fast a
  frame, and exit codes that say whether every frame came out as its
  edit asked ([§236](https://github.com/jessolmstead/greycard/blob/master/docs/notes/236-a-headless-export.md))

### Speed

- The window no longer redraws sixty times a second at rest, and
  femtovg's pipelines are no longer rebuilt every frame, about 30 ms
  back on every frame with layers ([§208](https://github.com/jessolmstead/greycard/blob/master/docs/notes/208-a-pick-from-a-sub-menu-froze-the-window.md))
- The grid and strip make cells only for the rows on screen: a
  20,000-frame folder shows in half a second instead of two and
  scrolls about seven times faster ([§202](https://github.com/jessolmstead/greycard/blob/master/docs/notes/202-the-strip-and-the-grid-with-cells-for-the-screen-alone.md))
- The rest of the network reads are off the window's thread, and a
  share that goes quiet is set aside while the other roots carry on
  ([§215](https://github.com/jessolmstead/greycard/blob/master/docs/notes/215-the-rest-of-the-network-off-the-windows-thread.md))

### Fixes

- A rating or label picked from the frame menu's sub-menus with the
  pointer no longer leaves the window unresponsive ([§208](https://github.com/jessolmstead/greycard/blob/master/docs/notes/208-a-pick-from-a-sub-menu-froze-the-window.md))
- A frame exported from the screen is the same picture as the same
  frame exported in a set ([§199](https://github.com/jessolmstead/greycard/blob/master/docs/notes/199-the-export-s-picture-and-the-gpu-s-ca-twice-over.md))
- The viewport and the export agree over a much wider range of edits:
  divergences under stacked gains, at exact-zero channels, in masks
  and guide planes, and in near-black under a strong color shift are
  fixed on both sides ([§221](https://github.com/jessolmstead/greycard/blob/master/docs/notes/221-the-shader-checked-against-the-cpu-at-random-edits.md), [§239](https://github.com/jessolmstead/greycard/blob/master/docs/notes/239-the-random-parity-test-past-its-seeds.md))
- A raw moved or renamed by hand keeps its edits and its flag ([§223](https://github.com/jessolmstead/greycard/blob/master/docs/notes/223-a-move-seen-by-the-index-carries-the-sidecar-along.md))
- Move rejects no longer refuses a frame whose sidecar from an earlier
  cull was left in the rejects folder ([§220](https://github.com/jessolmstead/greycard/blob/master/docs/notes/220-the-folder-is-the-truth-and-the-sidecar-it-leaves-behind.md))
- The Move rejects sheet takes the keys when it opens, so Return and
  Escape work ([§234](https://github.com/jessolmstead/greycard/blob/master/docs/notes/234-the-rejects-sheet-takes-the-keys.md))
- `--export` honors `--xmp-sidecars`, no longer writes over another
  tool's change to the `.xmp`, and no longer crashes on exit after
  loading a model ([§236](https://github.com/jessolmstead/greycard/blob/master/docs/notes/236-a-headless-export.md))

### For developers

- The GPU tests run in CI on lavapipe and fail rather than skip
  without an adapter, and a random parity test checks the shader
  against the CPU over hundreds of edits a run ([§221](https://github.com/jessolmstead/greycard/blob/master/docs/notes/221-the-shader-checked-against-the-cpu-at-random-edits.md), [§239](https://github.com/jessolmstead/greycard/blob/master/docs/notes/239-the-random-parity-test-past-its-seeds.md))
- `--match-compare DIR`, `tools/compare-transforms.py` and
  `tools/transform-sheets.py` for comparing display curves on the
  same frames ([§227](https://github.com/jessolmstead/greycard/blob/master/docs/notes/227-aces-2-and-agx-on-our-frames.md), [§230](https://github.com/jessolmstead/greycard/blob/master/docs/notes/230-agx-over-aces-the-call-from-two-sets.md), [§238](https://github.com/jessolmstead/greycard/blob/master/docs/notes/238-the-camera-match-under-agx-measured-on-the-same-frames.md))
- Snapshot flags `--press` and `--keys` drive input into the window
  before a capture ([§208](https://github.com/jessolmstead/greycard/blob/master/docs/notes/208-a-pick-from-a-sub-menu-froze-the-window.md))

## 0.3.0, 2026-09-27

Quality of life improvements. A library root on a network share neither freezes
the window nor goes stale. Frames can be deleted from the disk with
confirmation. The scopes can be read for the chosen
mask alone, a slider's value can be typed, and White Balance has an
Auto. A folder of thousands of cached thumbnails opens at once. And
the fixes from the first day with 0.2.0 on a NAS root are here too.
Read the
[guide for testers](https://github.com/jessolmstead/greycard/blob/master/docs/user-guide.md)
for installing on each system, the hotkeys, and how to report a problem.

- The scopes can be restricted to the chosen mask: a Selection toggle
  beside them, off at launch, so a skin mask shows the skin's cloud on
  the vectorscope and a face its level on the waveform; the clipping
  lamps follow it too while it is on (§189)
- Delete from disk supported in all modes, with the Delete key.
  The delete runs off the window's thread (§190, §196)
- A folder of thousands whose thumbnails are all in the cache shows
  its window at once and stays responsive while they come: the pool
  delivers in batches, the rows on screen first, and rows far off keep
  their picture until scrolled near; the 20,000-frame case filled in
  1.6 s with the window drawing throughout, against 3 s of a blank
  window (§191)
- A view of the library roots that takes a while to read shows a bar with a
  count of the sidecars read, in the grid and the loupe alike, after
  the first 200 ms; a read that stops moving is given up on rather
  than left hanging (§192)
- A slider's value can be typed: click the number, Enter sets it
  clamped to the range, Escape leaves it; the point curve's selected
  point stays selected and takes a typed In and Out in 0–255 (§193)
- The camera match sheet no longer grows past the window after a run
  over many groups: its body scrolls and the buttons stay in reach
- An Auto button beside Neutral in WHITE BALANCE: grey-world over the
  frame in camera space, clipped and black pixels dropped and one pass
  re-weighting toward what is already near grey, through the same
  path as the Neutral picker, so the answer is the same whatever
  white the frame was showing and a second press changes nothing;
  frames with nothing neutral in them still fool it (§194)
- A root's chip in the library row goes off when a folder is opened
  over its view; before, the old chip stayed lit
- A library root on a network share (NFS, SMB, sshfs and the like) is
  no longer watched on Linux and macOS, where setting the watch up
  froze the editor while every folder was registered and saw only this
  machine's changes; it is passed over every 10 minutes instead
  (`network_poll_minutes` in settings.json, 0 for never), and the
  watcher on the other roots is set up off the window's thread, so
  adding, removing or starting with a root never holds the window
  (§188)
- When Move rejects takes away the frame on screen, culling now shows
  the nearest frame that is left, as the loupe already did; before, it
  kept showing the moved frame with nothing selected
- Opening the Settings sheet no longer freezes the window over a
  large root: the count of sidecars that are out of place, and the move that
  settles them, run off the window's thread, the sheet saying
  "Counting..." and "Moving..." meanwhile
- The Crop, Masks, and Retouch tabs no longer open blank after the
  Develop tab was scrolled down: each tab starts at its top
- The terminal no longer shows winit's Wayland event loop warning
  about a re-registered source, which is harmless
- The test build's dead-code lints on macOS and Windows, which turned
  the tag's CI red, are gated

## 0.2.0, 2026-09-27

The second release. The library arrived: folders added as roots and
indexed, browsed across roots with filter chips, imported from a card,
and read without freezing the window when a root is on a network
drive. Culling got its keys answered on the picture. A button fits
the camera's own look from its JPEGs. Masks gained a learned Sky.
And the editor now says when a newer release is out. Read the
[guide for testers](https://github.com/jessolmstead/greycard/blob/master/docs/user-guide.md)
for installing on each system, the keys, and how to report a problem.

- A library root on a network drive no longer freezes the editor while
  it is walked: the list, the new frames' sidecars and their rows are
  read off the window's thread, and a burst of passes is one read. A
  root that stops answering shows as offline within 3 s and no longer
  stops the list of the other roots; the indexer itself still waits on
  it, so nothing new is indexed until it answers (§187, 2026-09-27).
- The user guide says where a camera profile comes from (Adobe's
  folders, RawTherapee's set, a chart shot) and that the embedded matrix
  is what most people need (2026-09-27).
- Check for a newer release at launch, at most once a day, and offer it
  in the left pane; a switch and Check now in Settings, which now
  scrolls on a short window (§186, 2026-09-27).
- The Windows package and the Mac bundle have now been installed and run
  by testers, which closes the 0.1.0 line that named them as the parts
  not verified by running them (2026-09-27).
- A library root can have a name of its own ("Archive" for a folder
  called Photos), set from its chip's right-click menu and shown in
  place of the folder's name; the folder is not renamed or moved. Escape
  closes the name sheet and the preset sheet from their fields (§185,
  2026-09-27).
- The Develop, Crop, Masks and Retouch tab bar stays at the top of the
  panel while the sections under it scroll (§184, 2026-09-27).
- A half-copied ARW (and a CR3, once rawler stops panicking on one) is
  refused with the half-copied message instead of opening as a picture
  blank below the copied part (§182, 2026-09-27).
- Culling shows the frame's stars, flag and label on the picture, on the
  loupe and on each compare tile, and a word for each key pressed ("3
  stars", "Rejected") and for each undo and redo that fades on its own
  and is not wiped by the arrow; both with Tab on. A held culling key is
  one press (§183, 2026-09-27).
- Move on after a rating, flag or label: a switch in the CULLING
  section, off by default, kept in the settings (§183, 2026-09-27).
- Fit this camera's look: a button in the Look section surveys the
  library or the open folder by body and picture style, develops
  about forty frames a group, lays the camera's own JPEG over each
  and fits the maker's rendering as a look, written to the look
  directory under the body's and the style's name; a group under
  twenty frames borrows the style from another body or is skipped,
  and an existing table is never replaced without the switch; the
  picker says where a look was fitted when the open frame is another
  body, and picture style is a filter facet. The maker's style and
  adaptive settings join the library index by a migration in place
  (§180, §181, `docs/camera-match.md`, 2026-09-26).
- A right-click menu on a frame in the strip, the grid or the
  viewport: copy the settings of that frame, paste them onto the
  selection through the sync sheet's section list, rating, flag and
  label, reveal in the file manager, export; every item the code of
  its key. Ctrl+C and Ctrl+V do the copy and paste from the keys, and
  each pasted frame's history reads "Paste from FILE" (§171,
  2026-09-25).
- Thumbnails are made on a pool of threads of their own instead of one
  at a time behind the develop: a cold folder of 35 fills the grid in
  0.6 s instead of 3.6, and 300 in 4 s instead of 17, the first
  picture no later; a file no picture can be made of marks its cell
  "Unreadable", and a `--grid --snapshot` no longer waits forever on
  it; a folder listing takes only regular files (§172, 2026-09-25).
- Import from a card: Ctrl+Shift+I or Import... in the grid's header,
  a card found under the mount points, a destination with folder and
  name patterns (`{date}`, `{seq}`, `{camera}`, `{name}` and the rest,
  names safe on every file system), a preset laid on each frame as one
  history step, and a backup copy; every file copied to a temporary,
  hashed as read, synced, read back and renamed only when the hashes
  agree, and never a write on the card; a frame already imported is
  known by its size and its first and last 64 KiB, Verify compares
  every byte; `--import SRC --to DEST` runs one headless (§173,
  2026-09-25).
- Roots: folders added to the library, kept in `roots.json` beside
  the index; a pass over every root at launch and a watcher on them
  while the editor runs, so a file copied into a root is in the grid
  half a second later and a moved frame is found rather than flagged
  missing; an all-roots view of every file under every root with the
  filter bar over it, a root's chip narrowing to it, an unplugged
  root shown offline and left alone; the filter remembered between
  sessions; rejects moved to a rejects folder in each shoot's own
  folder; `--roots DIR,DIR` and `--all-roots` (§174, 2026-09-25).
- A Sky mask on the Masks tab: a learned prior (EoMT-S, MIT weights,
  96 MB, fetched on first use) finds the sky, a gate refuses a frame
  with none rather than paint one, SAM 2 draws the outline, and a
  color-line matte at the frame's full resolution takes the edge
  through hair and bare branches; clicks add or remove sky. Under a
  second from click to mask on the GPU at 24 MP. On the 41-frame test
  set, no false sky on the five frames without one and 31 of 36 skies
  found; sky through a window, and sky behind dense blossom, are
  refused or missed (§175, 2026-09-25).
- Undo on a clone or heal spot takes it away in one step: the source
  the develop chooses completes the step that placed it instead of
  recording a second one, and sidecars 0.1.1 wrote heal themselves
  (#7, §177, 2026-09-25).
- The Tone curve switch is gone: the base curve always applies to a
  raw, and the Light section switched off leaves the picture as it
  opens; masks' tone sliders act whatever the picture's Light says
  (#9, §177, 2026-09-25).
- Removing a preset asks first, and Settings restores any default
  preset that is missing without touching the user's own (#4, §177,
  2026-09-25).
- A linear gradient's end line turns and stretches it about its
  center; Alt moves just that end, as before (#6, §177, 2026-09-25).
- A frame turned from the Crop tab turns on screen at once instead of
  a second or two later, and without a lens profile or defringe the
  develop at the new turn takes a third of the time (#5, §177,
  2026-09-25).
- The Masks tab sets the chosen shape's controls in a card under the
  shape list, marks the chosen row, labels Add / Subtract / Intersect
  as the next shape's, and heads the look sections with the
  adjustment's name, "ADJUSTMENT 1 · LIGHT" (#10, §177, 2026-09-25).
- The left pane scrolls instead of growing the window: a long list of
  snapshots no longer takes the window out of fullscreen on Windows,
  and a list's bins sit clear of its scrollbar (#2, §177, 2026-09-26).
- The left pane, the develop panel and the strip put away one at a
  time: F7, F8 and F6, or the small arrow on each one's edge of the
  viewport; Tab still puts all three away and brings them back, and
  what was put away stays put away on the next launch (#3, §178,
  2026-09-26).
- A raw that rawler panics on, such as a CR3 still being copied, fails
  to open with a reason instead of taking the frame, or on some paths
  the editor, down with it; one whose end is zeros is called
  half-copied (§179, 2026-09-26).
- Ctrl+Z while culling takes back the session's ratings, flags and
  labels, newest first, going to each frame it changes, and
  Ctrl+Shift+Z makes them again; it no longer undoes a develop edit
  or leaves culling (§179, 2026-09-26).

## 0.1.2, 2026-09-25

- Fix JPEG, PNG and TIFF files opening brighter and more contrasty
  than the file: they took the 0.8-stop baseline and the display
  curve meant for a raw. A picture with nothing done to it now looks
  as it does anywhere else (#1, §152, 2026-09-23).
- Keep sidecars in a hidden `.greycard` folder inside the shoot's
  folder instead of beside each raw, with `sidecars_in_folder` in the
  settings or `--sidecar-folder` for a run. Both places are read
  whatever the setting says, and a frame's sidecar moves to the
  chosen place the next time it is saved. Hidden by attribute on
  Windows; XMPs stay beside the raw (§153, 2026-09-23).
- A plain `cargo test` no longer prints ERROR lines from the UI
  crate's tests: the log module's test had installed a terminal sink
  for the whole test binary (2026-09-23).
- `app.slint` split into one file a panel section under `ui/panel/`,
  the window keeping every property and callback so the Rust side is
  untouched; 4,949 lines down to 1,687, verified by whole-window
  captures against the previous build and a differential event fuzz,
  no behavior change (§154, 2026-09-23).
- Tab hides everything but the photo and brings it back; `--hide-panels`
  opens that way for a capture (§155, 2026-09-23).
- The lens profiles are offered once, unasked, the first time a picture
  opens with no lens database, with the reason; Not now is remembered
  and the LENS section keeps its button (§155, 2026-09-23).
- The PRESETS section moves to the left pane, under the navigator, out
  of the develop panel's scroll; the presets and history lists give up
  room when the pane is short of it (§155, 2026-09-23).
- A Settings sheet (Ctrl+, or the Settings... button) for where sidecars
  are written and whether XMPs are, with a "move the open frames'
  sidecars" action that reports what it renamed and what stale copies it
  removed; `--sheet settings` and `--sheet lenses` for captures (§155,
  2026-09-23).
- The release binaries are stripped: greycard-ui 112 MB to 84 MB on
  Linux, no behavior change (§155, 2026-09-23).
- Several frames at once in the strip and the grid: Ctrl+click adds
  one, Shift+click a run, Shift+arrows extend, Escape collapses; the
  rating, flag, label, reject and turn keys act on the whole set
  (§156, 2026-09-23).
- Sync settings: Ctrl+Shift+S or the Sync button lays the chosen
  sections of the frame on screen over every other selected frame
  through the preset path, one history step each; the camera profile
  only onto the body it was made for, Noise bringing the learned
  denoiser along, masks off by default; the preset and sync sheets in
  two columns so they fit a 768 px screen (§156, 2026-09-23).
- Export presets: the export sheet's choices kept by name, picked at
  the top of the sheet, saved and deleted there, and `--export-preset
  NAME` for a headless export; a metadata choice of All, No edit or
  None (§157, 2026-09-23).
- A watermark on the export: text in the system's sans-serif or a PNG,
  at one of nine positions with a margin, a size as a share of the long
  edge and an opacity, so it lands the same at every export size;
  never in the viewport; an empty or undrawable mark fails the export
  rather than letting an unmarked picture out (§157, 2026-09-23).
- Two range masks on the Masks tab: a Luminance window on the picture's
  lightness (Oklab L, 100 the sensor's white, the ends open) and a
  Color window on its hue and chroma in Oklab, each with fades, a
  dropper that centers the hue on a click and a Skin preset; sampled
  from the developed picture before the look, so a mask does not move
  under its own edit; on the CPU and in the viewport shader alike
  (§158, 2026-09-23).
- A sidecar carrying a mask shape from a later build now loads with
  that shape left out and a warning, instead of being refused whole;
  builds before 0.1.2 still refuse a sidecar with a range mask (§158,
  2026-09-23).
- The Subject and Background masks run on the GPU where WebGPU is
  available: a rewrite of the BiRefNet lite export, published by
  greycard under its MIT license, that takes 0.16 s a mask against 2.9 s
  on the CPU; fetched on first use, the original offered when it
  cannot be had (§159, 2026-09-23).
- `greycard models --fetch all` carries on past a model that fails and
  reports the failures at the end (§159, 2026-09-23).
- The library index, `greycard-library`: a SQLite index of every raw
  and picture's path, size, content hash, EXIF and sidecar meta,
  incremental, finding a moved file by hash and leaving an unmounted
  drive alone; `greycard library index DIR` and `greycard library list
  'camera:R6' 'iso>=3200' 'rating>=3'` with a filter language that
  refuses what it cannot answer. Not yet wired into the editor (§160,
  2026-09-24).
- The probe reads the lens, focal length and date, and a JPEG or PNG's
  EXIF from its head without decoding it (§160, 2026-09-24).
- A save counter in the sidecar: when a frame has a sidecar both beside
  it and under `.greycard/`, the one saved more times wins, and the
  files' times decide only when a copy carries no count, so a copy tool
  or a restore cannot make the next save drop an edit (§161,
  2026-09-24).
- A preset click lays the preset over every selected frame, one
  history step each, the way a sync does; a camera profile in a preset
  reaches only frames of the body it was made for, the open frame
  included, and the status says which it left off (§162, 2026-09-24).
- A preset applied in culling now writes the frame's sidecar; before,
  the step lived only in memory until the next save (§162, 2026-09-24).
- A thumbnail cache on disk, keyed by the file's content hash and its
  time, so a folder reopened, moved or renamed shows its thumbnails at
  once (the 35 sample files: 4 s to 0.01 s); 300 MB by default, with a
  cap field and a Clear button in the Settings sheet's new THUMBNAILS
  row; a thumbnail made while the file was still being copied is never
  kept (§163, 2026-09-24).
- The profiled denoiser's wavelet chain runs on planes a block at a
  time: the 45 MP develop with the hybrid denoise at six threads goes
  from about 26 s to 12.7 s, the 24 MP from 11.3 to 5.7, output within
  a 16-bit step of before (§164, 2026-09-24).
- Faster develop ops, output bit-identical: the lens correction 17
  percent and its vignetting four times at four threads, the CFA gains
  twice and the levels a quarter at one thread, the Gaussian blur, the
  dehaze, and the RCD demosaic 11 percent from an inlining decision
  pinned (§165, 2026-09-24).
- Name a history step by what made it: "Preset: Faded film", "Preset
  ×3: Faded film" over a set, "Sync from 5M0A3021.CR3", "Snapshot:
  Before the crop", where the rows read as the list of sections that
  moved. The words are kept with the step in the sidecar, an older
  build reads the file as before, and a `--preset` step is now written
  on quit (§166, 2026-09-24).
- Export the frames selected, not only the one on screen: each under
  its own edit, one at a time on the worker behind the develop, with
  progress on the status line, a Stop, and a finished line that counts
  what was skipped or failed. A set never writes over one of its own
  sources. `--export DIR --also ROWS` does it from the command line
  (§167, 2026-09-24).
- The library index in the editor: the open folder is indexed on a
  thread of its own and kept up to date as sidecars are saved, and
  the grid's filter grows chips for camera, lens, ISO, focal length,
  day and keyword with their counts, a text field that takes the
  filter language (`camera:R6 iso>=3200 rating>=3`), and `--filter`
  on the command line. The database is `greycard/library.sqlite`
  under the platform's data directory, or `--library PATH` (§168,
  2026-09-24).
- Fix a click with the color range, white balance or defringe
  dropper zooming the view as well, and a drag with one panning it
  (§169, 2026-09-24).
- Drop the color mask's Skin button: a new color window starts at the
  skin already, and the button only put the sliders back there (§169,
  2026-09-24).
- Offer the GPU Subject model where the store holds the original and
  it is on record as falling back to the CPU on this card, with the
  reason on the sheet; an offer accepted mid-session is used at once,
  and one declined leaves the original working (§169, 2026-09-24).

## 0.1.1, 2026-09-22

- Fix the macOS package failing at launch with "Library not loaded:
  liblcms2.2.dylib" on any Mac without Homebrew's little-cms2. lcms2 is
  now built into the app, and packaging refuses a Mac build that links
  a library outside the system and the bundle (notes §151).

## 0.1.0, 2026-09-22

The first test build of greycard, a raw editor for Linux, macOS and
Windows. It is a pre-release: some things are missing and some are
wrong, and the reason it is out is to hear about both. Read the
[guide for testers](https://github.com/jessolmstead/greycard/blob/master/docs/user-guide.md)
before you start; it covers installing on each system, the keys, and
how to report a problem.

**What you can do with it**

- Open a folder of raws and move through it in a filmstrip, a grid or
  a single frame, with the camera's own JPEG shown at once and the
  develop replacing it a moment later.
- Cull from the camera's JPEG without developing anything: ratings,
  picks and rejects, color labels, a filter over all of them, compare
  two or four frames, and move the rejects into a folder of their own.
- Develop: exposure and tone, a parametric and a point curve, a color
  mixer, grading, black and white, texture, clarity and dehaze, grain
  and vignette; white balance and camera profiles; lens corrections
  from lensfun; demosaicing, highlight reconstruction, chromatic
  aberration correction, deconvolution sharpening and two denoisers.
- Mask with gradients, radial shapes and a brush, or let a learned
  model find the subject or an object; heal, clone and fill.
- Crop, straighten and correct perspective.
- Use presets, including Lightroom's, and 3D LUT looks.
- Export JPEG or TIFF in sRGB, Display P3 or Rec.2020, with the
  profile and the EXIF embedded.
- Keep every edit in a `.gcd` file beside its raw, and share ratings,
  labels and keywords with Lightroom and darktable through XMP.

**Known rough edges**

- The default look and the Highlights, Whites and Shadows sliders are
  close to Lightroom's but not there yet; that is the next release.
  Tell us when a frame looks too dark, too flat or wrong in a way you
  can name.
- Fujifilm X-Trans bodies and Nikon's High Efficiency raws do not open.
- On Windows and macOS the display is taken as sRGB unless you choose
  your monitor's ICC profile by hand.
- On a Mac, a double-click in the Finder launches greycard without
  opening the file. The app is not signed, so each system warns once
  before it first opens; the guide says what to click.
- The best denoisers take tens of seconds on a large frame.

Report a problem from the button at the foot of the left panel: it
fills in the version, your system and your GPU, and shows you the log
to attach.

## Development history before 0.1.0

- The file's name, the shot and the scopes stay at the top of the
  panel while the sections scroll under them, so the histogram is in
  sight of any slider (§147, 2026-09-22)
- A short guide for testers, `docs/user-guide.md`: installing on each
  system, the basics, culling, the keys, and how to report a problem
  (2026-09-22)
- Whites aims at white: +1 makes the tones a stop under the raw's clip
  white and -1 puts white a stop further out, where since the brighter
  default it was aiming at a pale grey (§149, 2026-09-22)
- Highlights pulls about as hard as Lightroom's: -2 is Lightroom's
  -100, strongest in the bright mid-tones and easing off towards
  white, where it used to pull the very top twice as hard. Lightroom
  presets bring their Highlights across at the same scale (§146,
  2026-09-22)
- What the sensor clipped is white: the display curve reaches white at
  the raw's clip instead of 0.9 of the way there, so a blown sky is
  white as the camera and Lightroom show it, and the mid-tones do not
  move (§145, 2026-09-22)
- Blacks lifts the shadows instead of laying a grey veil over the
  picture: up to 1.2 stops in the deep shadows, fading out through the
  mid-tones, where Lightroom's +100 lands. Lowering it is unchanged.
  Lightroom presets bring their Blacks across at the same scale, and
  Warm Negative's faded black now comes from its curve (§144,
  2026-09-22)
- The default develop is 0.8 stops brighter, where the camera's JPEG
  and Lightroom put a picture; the exposure slider at zero is that
  baseline and moves from it as before, so edits already written come
  up 0.8 stops brighter too (§141, 2026-09-22)
- A lens profile measured on a smaller sensor than the camera's no
  longer darkens the corners it was never measured over: the
  vignetting is held at its value at the calibration's corner, and
  the LENS panel and `greycard lenses` say when that is happening.
  A full-frame lens on a GFX gets +0.21 EV more at the corner tip
  (§142, 2026-09-22)
- Report a problem… at the foot of the left panel opens the bug form
  in the browser with the version, the OS and its version, and the GPU
  filled in, and shows the log selected in the file manager beside it,
  its path written without the account name; the form's fields are
  now OS and the log rather than a Linux distro and terminal output
  (§138, 2026-09-22)
- The grid has its own Open folder button, first in its header, where
  before only Ctrl+O reached one there; and its background is a dark
  grey sheet rather than the near-black behind the loupe (§139,
  2026-09-22)
- CI builds and tests on Windows beside Linux and macOS, so a change
  that breaks the Windows build shows on a push rather than at a tag
  (§140, 2026-09-22)
- On Linux the file manager offers greycard for `.orf`, which the
  browser has read all along (§140, 2026-09-22)
- On the Mac the `.gcd` is a document type of greycard's own with its
  own icon, and the raw types offer greycard without taking a
  default; a double-click launches greycard but does not yet open the
  file (§140, 2026-09-22)
- The editor no longer opens a console window beside itself on
  Windows. It is built for the GUI subsystem now, where a Rust binary
  is a console one by default and Windows hands every console binary a
  window whether it wants one or not. The log is unaffected; what goes
  is a terminal waiting for the editor it launched, so
  `greycard-ui --version` comes back after the prompt does. `greycard`,
  the command line, is unchanged (§137, 2026-09-22)
- There is a Windows package, `greycard-<version>-x86_64-windows.zip`,
  beside the Linux and macOS tarballs: both binaries, Dawn and the two
  DXC libraries it loads by name, the five Visual C++ runtime libraries
  so a machine that has never carried a redistributable still starts,
  and the icon and version in the executables themselves. `install.cmd`
  puts it under `%LOCALAPPDATA%\Programs\greycard` with a Start menu
  entry and the command line on the Path; `register.cmd` gives `.gcd`
  its own document type and icon and offers greycard in Open with for
  every raw extension the browser reads, taking no default away;
  `uninstall.cmd` takes all of it back out. A `.zip` rather than a
  tarball, since Windows 10's Explorer opens one and does not open the
  other, and reproducible the same way the tarballs are (§137,
  2026-09-22)
- The Windows build runs. `cargo fmt`, `cargo clippy --all-targets`
  and all 697 tests are clean on x86_64-pc-windows-msvc; the editor
  comes up on a discrete NVIDIA card through Vulkan and develops a
  45 MP Canon frame in 1.66 s with the CA correction and the sharpen
  both on the GPU; the log lands in `%LOCALAPPDATA%\greycard\logs`.
  The only thing above info in a first run is the storage-buffer limit
  BiRefNet already hits everywhere else (§137, 2026-09-22)
- Choosing a frame no longer flashes a dark viewport before its
  camera preview, and a frame with no camera JPEG, a picture file, or
  a develop that fails keeps the last picture on screen instead of a
  dark window; the wait before the preview grows with the raw's size,
  which is why 90 MB Fujifilm and Sony files showed it and Canon
  files did not (§135, 2026-09-21)
- Switching frames shows the new frame's camera JPEG at once, the way
  culling draws it, oriented as the develop will be, with a "camera
  preview" word and no edit overlays, and the developed picture
  replaces it when it lands; on a 45 MP raw the first picture comes
  in about 110 ms where nothing changed for 1.6 s. The worker is
  joined on the way out, so a close during an export finishes the
  file (§134, 2026-09-21)
- The three-way Show is a filter on the whole meta: rating chips
  read as at-least or exactly, flag and label chips any-of within
  the row, words matched against the file name and the keywords,
  each chip counting the frames that carry it among what the other
  rows leave, in the grid header and the CULLING section; Ctrl+F or
  `/` reaches the field, Esc clears it; a change that hides the shown
  frame hands the selection to the nearest frame still shown (§133,
  2026-09-21)
- The Original crop aspect keeps a frame's own way up: an upright
  frame cropped to Original is upright, where before it went straight
  to landscape unless Portrait was ticked. A sidecar written before
  this reads under schema 4 and is brought across once the frame's
  shape is known, so what it rendered is what it still renders (§132,
  2026-09-21)
- A mask's lines and handles stay on the picture: the viewport is
  clipped, so a gradient's outline no longer draws over the navigator,
  snapshots, history or filmstrip, and a handle off the picture no
  longer takes their clicks (§132, 2026-09-21)
- Under a filter, a rating, flag or label badge lands on the thumbnail
  it was set on rather than the one at that row number (§132,
  2026-09-21)
- Eighteen decisions moved out of the panel wiring into pure modules
  with their tests, three of them new (`files`, `wheel`, `zoom`); no
  pure module imports from the wiring any more (§132, 2026-09-21)
- `main.rs` split into one module a panel section under `panel/`,
  each with its state-to-UI bridge, its callbacks and its tests, and
  the headless window helpers shared so any panel can drive its own
  callbacks; 11,008 lines down to 627, reviewed as a moved-line diff,
  no behavior change (§130, 2026-09-21)
- A frame the camera got the wrong way up turns a quarter with `[`
  and `]`, in culling, the loupe and the grid, with buttons in the
  CULLING section and under the Crop tab's rotations. The turn sits
  beside the edit in the sidecar on top of the camera's tag, so undo,
  presets and snapshots never touch it; the crop, every mask, every
  repair patch and every history state and snapshot turn with the
  picture, measured by the frame's own size; the camera's preview is
  redrawn through a matrix so a culling turn costs about a
  millisecond; `tiff:Orientation` is written to and read from the
  XMP sidecar composed with the camera's tag (§131, 2026-09-21)
- A repair just drawn no longer stays selected, so the retouch Size,
  Feather and Opacity sliders and the wheel set the next stroke
  instead of resizing the last one; click a repair to select and
  change it, Esc or the empty picture to let it go (§129, 2026-09-21)
- The profiled denoiser's non-local means run about 1.6x quicker on a
  synthetic frame and 2.3x on a photograph, with the output held to
  the reference within a few last bits: the per-pixel weight is
  arithmetic instead of a library call, the box sums walk rows, and
  the tiles write into the frame. A 45 MP develop with the hybrid
  denoise at six threads goes from 28.3 s to 22.6 s. §90's reading
  that the means scaled badly was wrong: they scale with cores; the
  base develop and the wavelets are the memory-bound ones (§128,
  2026-09-20)
- A `look` section: a 3D LUT from a `.cube` or HaldCLUT PNG in
  `~/.local/share/greycard/looks`, applied after the tone curve in
  the table's own encoding with tetrahedral interpolation and a
  strength, the same in the viewport and the export; a LOOK panel
  section lists the tables found; three film presets shipped under
  honest names (Muted Slide, Warm Negative, Red-Filter Mono), built
  from the sliders and tested on what they render (§127, 2026-09-20)
- The chromatic aberration correction runs on the GPU in the editor:
  the green interpolation, the per-tile votes, the resample and the
  color-shift guard as compute, the median, gate and polynomial fit
  on the CPU from a small readback, checked tile by tile against the
  reference; the op takes about 100 ms at 45 MP where it took 300, and
  every base develop is quicker by that. An export after a GPU
  develop remakes its base on the CPU reference, so the exported file
  is the reference's picture (§126, 2026-09-20)
- Focus stacking in core and on the command line: `greycard stack`
  registers the frames neighbor to neighbor, weights each by its
  local sharpness and blends them through a Laplacian pyramid, then
  writes a linear DNG beside the sources with the reference frame's
  color tags and EXIF, or a TIFF; a frame that will not register is
  dropped and reported, never stacked wrong; about 0.6 s a frame at
  24 MP (§125, 2026-09-20)
- XMP sidecars for the meta: a frame's rating, label, keywords, title
  and caption are read from an `IMG.xmp` or `IMG.CR3.xmp` another
  tool wrote, taken once and again only when that file changes, never
  by comparing clocks; with `xmp_sidecars` on in settings (off by
  default) or `--xmp-sidecars`, they are written back into it with
  everything else in the file kept byte for byte; a reject is the
  Bridge and darktable rating of -1; opening a folder writes nothing
  into it (§124, 2026-09-20)
- Culling from the camera's JPEG: C enters a loupe on the embedded
  preview through the monitor profile only, no develop; the arrows
  move with a dozen frames decoded ahead either side, a switch in
  under 15 ms; the rating keys work there; Space or a click shows 1:1
  for focus; V compares two or four frames side by side; a filter
  shows picks or hides rejects in and out of the mode; Enter or any
  develop control develops the frame and the picture swaps to ours
  when it lands; Move rejects… puts the rejected frames and their
  sidecars in a `rejects` folder beside the shoot, never a delete
  (§123, 2026-09-20)
- Camera profiles: a DCP file's matrix pair, forward matrices and
  hue/saturation/value map are read and applied after the camera
  matrix, so a body's own profile from Adobe or a maker renders its
  colors instead of the file's matrices alone; the choice lives in the
  edit as `camera.profile`, embedded by default, and a CAMERA PROFILE
  section lists the profiles in `~/.local/share/greycard/profiles`
  that fit the camera; `greycard develop --camera-profile` on the
  command line. The look table and tone curve inside a DCP are kept
  for a later look line (§122, 2026-09-20)
- The defringe acts on a hue, not on every color: a purple window and
  a green window, each a center, a width and an amount, gate the pass
  on the direction a pixel's chroma departs from its neighborhood's,
  so a red berry on a grey wall is left alone; defaults measured on
  fringed frames (purple 310, green 130, 120 wide); a dropper in the
  LENS panel centers the nearer window on a clicked fringe, with the
  pass switched off while it is in hand so the fringe shows;
  `--defringe-all-hues` on the command line is the old pass (§121,
  2026-09-20)
- On Linux a `.gcd` sidecar is a greycard edit file to the desktop:
  the tarball carries a shared-mime-info type and an icon for it, the
  installer registers them, and opening a sidecar from the file
  manager opens its raw with the edit (§120, 2026-09-20)
- Registration in core: a pyramidal Lucas-Kanade fit of a translation,
  a similarity or an affine between two frames, exposure-invariant,
  sub-pixel to a thousandth on synthetic warps and 0.02 px on a real
  45 MP frame, 150 ms at 24 MP; a residual that tells a failed fit
  from a good one; `greycard register A B` prints what it found. The
  piece the focus stack and the handheld HDR merge wait on (§119,
  2026-09-20)
- The sharpen runs on the GPU in the editor: a new `greycard-gpu`
  crate holds the capture sharpening as WGSL compute in the CPU
  reference's own tiles and order, checked against it to a millionth;
  a slider move reaches the frame in 55 ms at 24 MP and 100 ms at
  45 MP where it took 330 and 580 ms. The export still develops on
  the CPU reference. A GPU error falls back to the CPU for the rest
  of the session; `--cpu-ops` turns the GPU ops off (§118, 2026-09-20)
- A frame carries a rating, a pick or reject flag, a color label,
  keywords, a title and a caption in a `meta` section of its sidecar,
  beside the edit and never inside it, so undo, presets and snapshots
  leave them alone. In the browser and the loupe, 1 to 5 rate and 0
  clears, P picks, X rejects, U unflags, 6 to 9 set red, yellow,
  green and blue and the same key again clears; a badge on the
  thumbnail shows what a frame carries. A sidecar whose meta this
  build cannot read costs that field and never the edit (§117,
  2026-09-20)
- The first Subject mask in a session takes about four seconds where
  it took eight to ten: the CPU provider skips its warm-up run, and a
  provider that failed to load or run a model is remembered in
  `providers.json` beside the model cache, keyed by the model file,
  the adapter and the build, so the WebGPU attempt that fails on
  BiRefNet is paid once and not every launch. Delete the file to
  probe afresh; the log names it (§116, 2026-09-20)
- A press on a slider's handle no longer nudges the value: the handle
  is grabbed where it is and moves only once the pointer has travelled
  past a 4 px dead zone, then tracks the pointer at the grabbed
  offset; a press on the track still jumps, and a press on the label
  or the value text only takes focus, where it used to slam the value
  to an end of the range (§114, 2026-09-20)
- Building from source written out for someone who has neither Rust
  nor a compiler: `docs/building.md` walks the libraries, rustup, the
  clone, the build and the run, with Debian, Fedora and Arch package
  lines and `xcode-select --install` for the Mac, and says the things
  that are not guessable — the Dawn symlink into ONNX Runtime's
  download cache, git being needed to build and not only to clone,
  and the two variables the `#[ignore]`d tests want before they mean
  anything (§113, 2026-09-20)
- Logging done properly: every crate speaks through the `log` or
  `tracing` facade and each binary installs one subscriber. The
  editor's keeps `greycard-ui.log` at info and shows the terminal
  warnings, `-v` and `-vv` say more, `RUST_LOG` decides exactly; the
  file says the version and machine, the GPU and driver, the settings
  path, the display profile and where it came from, the model store
  and the lens database, then per picture what the decoder found
  (camera, size, mosaic, black and white levels), the lens matched,
  and one line per develop with the denoiser's provider and each
  stage's seconds. Silent fallbacks warn: a file with no white
  balance or color matrix, a provider that would not load, a cache
  or settings write that failed, a sidecar that would not parse. A
  panic in any thread lands in the file with its backtrace, and a
  panic in a worker job becomes the failed outcome the editor was
  waiting on instead of a spinner. The command line's `-v` shows
  what a develop found, which used to print unasked; its output is
  unchanged. Windows gets the same file. The stderr tee is gone
  (§112, 2026-09-20)
- The editor keeps a log, `greycard-ui.log` under
  `~/Library/Logs/greycard` on a Mac or `~/.local/state/greycard` on
  Linux, the previous run kept as `.1`, so a tester with no terminal
  open still has something to attach; and a tester's guide for the
  Mac, `docs/testing-on-a-mac.md`, that walks the security dialog
  (§111, 2026-09-20)
- A macOS release: the tarball rolled on a Mac holds `greycard.app`,
  both binaries and Dawn in the bundle with an icon and an ad-hoc
  signature, and `sh install.sh` puts it in `~/Applications`, clears
  the download's quarantine mark that Gatekeeper would otherwise stop
  on, and links the command line into `~/.local/bin`; `package.sh`
  rolls the same reproducible tarball with GNU tar or bsdtar, and the
  release workflow builds on ubuntu-24.04 and macos-15 and publishes
  both, with CI on both too (§110, 2026-09-20)
- Native file dialogs where there is no portal: one request that the
  desktop portal answers on Linux and `rfd` answers everywhere else,
  built and awaited on the event loop's thread since AppKit wants its
  panels there, with the filters' globs taken apart into the
  extensions a native dialog matches on. zbus is a Linux dependency
  now and colord with it, so a Mac build carries no D-Bus stack and no
  failed bus at startup (§109, 2026-09-20)
- The §90 develop table taken on an M-series Mac: a 24 MP base develop
  in 1.9 s where the estimate said 4 to 6, the learned denoiser's
  three tiers in 5.7, 8.7 and 16.8 s where the fast tier's estimate
  said 25, and the profiled denoiser the one op that would rather have
  the wide machine (§109, 2026-09-20)
- The tree builds and runs on macOS: zbus names the `async-io`
  feature it had been getting on Linux alone, from Slint's tray crate
  through Cargo's feature unification, and the CLI's and the bench's
  rpath is `@executable_path` there like the UI's since §106. The
  window comes up on Metal and develops a 102 MP GFX 100S II RAF;
  colord finds no session bus and the output falls back to sRGB
  (§108, 2026-09-20)
- The Contrast slider's neutral at the center of its track, as the
  other Light sliders' zeros are: a logarithmic track on the slider
  control, the value and the edit unchanged (§105, 2026-09-19)
- Dehaze slider in the Detail section: the dark channel prior after
  darktable's haze removal, the transmission fitted on a quarter-size
  grid and closed with each pixel's own luminance, bounded by the
  pixel's own dark channel with the slider's end at a strength of 0.8
  so no channel goes to zero, negative putting haze back, 0.04 s at
  24 MP; Lightroom's Dehaze comes across; `--dehaze` and
  `--dehaze-map` on the CLI (§104, 2026-09-19)
- Texture and Clarity sliders in the Detail section, clearly separate
  from sharpening: one local-contrast op from a guided filter on log
  luminance, Texture the fine band at a few pixels and Clarity the band
  between that and a radius of the long edge over forty, mid-tone
  weighted, faded at the clip, a gain on all three channels so hue
  holds, 0.13 s at 24 MP; a DETAIL section above a SHARPEN section;
  Lightroom's Texture and Clarity come across; `--texture` and
  `--clarity` on the CLI (§103, 2026-09-19)
- The Fill retouch tool works but you can't see where you have drawn:
  the chosen patch's shape is outlined on the picture with its feather
  line inside, contoured from the stroke's coverage so a scribble is
  one ring; the status says when the fill model is at work, missing or
  failed, a missing model is offered, and a fetched one fills at once;
  a `--patch` flag for snapshots (§102, 2026-09-19)
- Separate parametric curve from the usual flexible curve: a
  Parametric mode in CURVES with Highlights, Lights, Darks and
  Shadows and three draggable splits, baked ahead of the point curves
  so the viewport, the export and the masks read one table;
  Lightroom's seven parametric keys come across; a `--panel-scroll`
  flag for snapshots of a section below the fold (§101, 2026-09-19)
- Can't read the defringe slider labels, they get cut off: Radius and
  Threshold under the Defringe toggle, as the sharpen names its own
  (§100, 2026-09-19)
- Add resolution to metadata display: a third line under the shot,
  "5464 × 8192 · 45 MP", the developed picture's size as it is shown,
  so a JPEG reads the same way as a raw (§100, 2026-09-19)
- Slint 1.18 and wgpu 30: the viewport's render module on the new
  major, one real API break applied, the export and the 1:1
  screenshot identical to 1.17's; the filmstrip's `viewport-*` names
  to `content-*` (§91, 2026-09-19)
- Need to be able to scroll through the filmstrip: wheel, drag, the
  selection kept in view, the visible frames' thumbnails made first
  (§89, 2026-09-19)

- Zoomable grid view of all photos, accessible with keybind G: a
  contact sheet over the window, cells from 96 to 512 px on
  Ctrl+wheel or +/-, arrows in two dimensions, Return, Escape, G or a
  double click back to the loupe, thumbnails rendered for the visible
  rows at up to 360 px (§96, 2026-09-19)
- The Tint tool needs a more precise way of choosing the hue: a hue
  ring in the section sharing the grading wheels' picture and press
  arithmetic, angle for hue and radius for amount, the Hue slider in
  tenths of a degree (§95, 2026-09-19)
- The highlights slider does not reach far enough towards the
  mid-tones: its ramp starts a stop under mid grey instead of at it,
  so a region a stop over grey takes six tenths of the slider where
  it took a third; highlights, shadows and whites run ±2 EV instead
  of ±1.5 and ±1, the shadows ramp half a stop wider to keep the
  curve monotonic (§98, 2026-09-19)
- Ability to dial in the strength of the B&W color filters: a
  `strength` on the section, 0 to 3, today's look at one, on the CPU
  and in the shader alike; the shaders parsed and validated in the
  test suite (§87; §94, 2026-09-19)
- Enabling the B&W tool should disable the color mixer: the picture's
  mixer stands down while the conversion is on, its settings kept,
  the section drawn superseded (§94, 2026-09-19)
- Clear display with shot metadata: camera, lens, focal length,
  aperture, shutter and ISO under the file name, the same words from
  the CLI's `info`; ISO and shutter join `Shot` in core (§93,
  2026-09-19)
- `--sharpen` on a JPEG, PNG or TIFF does nothing in the CLI, since
  `run_develop_picture` takes no develop settings; the editor
  sharpens a picture when the section is on. `--exposure` on a
  picture's preview and `--ai-denoise` on a picture were silently
  dropped the same way (§88; fixed §92, 2026-09-19)
- Open Folder says "the desktop offered no file chooser" when
  clicked: an empty filter sent to the portal (§88, 2026-09-19)
- The CLI sharpens inside `develop`'s `finish`, before the lens
  correction; the worker sharpens after it. With a profile that
  moves pixels and the sharpen on, the CLI and the editor do not
  make the same file (§74; moved behind the lens, §88, 2026-09-19)
- The editor opens a blank window for a path that is not there:
  `list_files` checks the extension, not the file (§88, 2026-09-19)
- `--snapshot`, `--screenshot` and `--export` hang forever when the
  first file will not develop: they wait on an image, and a failed
  open only sets the status text. Print the message, exit non-zero;
  a file that cannot be written fails the run too (§88, 2026-09-19)
- A failed folder chooser reaches the user only on stderr; the
  window should say so in its status (§88, 2026-09-19)
- An X-Trans frame is refused with a `{:?}` of the CFA pattern;
  say the mosaic is not supported yet, in words, and at the door
  rather than after preparing the frame (§88, 2026-09-19)
- `greycard models [--fetch TIER|all]` beside `lenses`, so a headless
  machine can get the denoiser weights and the mask models with
  their licenses shown (§88, 2026-09-19)
- The editor has no `--version`, and its help calls it a spike (§88,
  2026-09-19)
- The README's install section, source build, run-time needs and
  how to report a bug; an issue form (§88, 2026-09-19)
- CD: a `v*` tag builds the tarball with the Dawn library, the
  desktop entry, the icon and an installer, and attaches it to a
  GitHub pre-release (§88, 2026-09-19). Untested until the first tag

- In general the Light sliders (highlights, shadows, whites, not blacks)
  are a bit too limited I think. Like the whites really only affect the
  very, very top of the histogram. Highlights don't seem to do nearly
  enough to recover a photo with a lot of DR. Actully, the shadows
  adjustment might be about right? Whites do basically nothing.
  (Highlights and shadows by region through a guided plane, whites a
  white point pivoted at mid grey, the monotonic guard back: §85,
  2026-09-19)
  - A valid alternative could be implementing Dynamic Range Compression

- Need to be able to add a color (HSV?) to the mask area: a Tint,
  hue and amount, on any look and so on a mask (§83, 2026-09-19)

- Need to be able to toggle off individual masks: a switch on every
  shape in a mask's list (§81, 2026-09-19)

- Strong black-and-white support, not just desaturate: a BLACK &
  WHITE section, eight band weights and the classic filters, on
  both paths (§77, 2026-09-18)

- The dual demosaic's VNG4 cannot be skipped, its blend weight is
  nowhere zero on a real frame, but its gradients, its green planes
  and the mask's blur are cheaper, bit for bit: the dual demosaic
  895 to 625 ms at 45 MP (§76, 2026-09-18)

- Perspective: a guided tool, two strokes along lines that should
  be vertical giving the tilt and the turn, or two horizontals for
  the horizontal keystone; the Crop tab in CROP, ROTATE and
  PERSPECTIVE sections (§49, §75, 2026-09-18)
- Perspective adjustments need to have their own subsection, it's
  not immediately clear what they are without dragging them
  (2026-09-18)

- Defringe: axial chromatic aberration and purple fringing, a
  different tool from the lateral correction; RawTherapee's, in
  Oklab, after the lens correction (§13, §61, §74, 2026-09-18)

- The sharpen's tiles widened to 128 with the early stop kept at
  the old 32-pixel grain: 72% border overhead to 16%, a 45 MP
  sharpen 590 to 435 ms and a sharpen slider 0.65 to 0.50 s (§73,
  2026-09-18)

- The denoiser's weights published to the registry's Hugging Face
  address, so the tiers work off a fresh install (§70, 2026-09-18)

- The grain's tonal weighting revisited: it sits in the shadows
  and is absent in a sky, where film grain shows most (§61; §69,
  2026-09-18)

- Warning/dialog when an export will overwrite a file. Add option
  to increment name, overwrite, skip (§68, 2026-09-18)

- The learned denoiser's default blend lower at base ISO, or from
  the measured noise: "best" at full takes fabric weave and
  surface texture off an ISO 100 frame (§61; from the ISO, §67,
  2026-09-18)

- There should be an option to change the canvas color from black to
  different greys (§66, 2026-09-18)

- Main app interface to choose folder to open (§65, 2026-09-18)
- editor should jump to last open photo on launch (§65, 2026-09-18)

- Space bar and Z keys don't zoom, only clicking does. Space should
  zoom 1x, Z for 3x (§64, 2026-09-18)
- Clicking off the canvas shouldn't zoom, it should no-op (§64,
  2026-09-18)
- We should show the zoom amount in the interface (§64, 2026-09-18)
- Key combo for exporting, maybe ctrl+shift+e (§64, 2026-09-18)
- Possible to have an actual dropper for a cursor when using a picker?
  (drawn at the pointer, §64, 2026-09-18)

- The mixer weighted by chroma, its hue and weight read from a
  local mean of a and b: near-grey noise reads as random hue and
  a band's luminance lift prints it as speckle (§61, §63,
  2026-09-18)

- Desperately need global saturation and vibrance sliders (§60,
  2026-09-18)

- The monitor's profile from the panel: colord's for a chosen
  monitor, sRGB, Adobe RGB, Display P3, or a file (§59, 2026-09-18)

- General speedup in the editor: the sharpen's tile blur, the CA
  correction's color-shift guard, the noise estimate and the
  float-to-half conversion; a 45 MP develop from 3.4 s to 2.0 s
  (§56, 2026-09-18)

- White balance (temp and tint sliders) should have color gradients
  showing which way is which (2026-09-18)

- Need more range on the Vignette amount, more than 2EV (five stops,
  2026-09-18)

- The crop frame stuck against an edge; the Object tool drew no
  boxes or picks; a long adjustment name pushed the panel's
  content out; "Show mask" was forced on while a tool was in
  hand; no gamut warning proofing sRGB from an sRGB output; the
  view snapped to the center after a develop with a crop; a hover
  over a history row with another crop bounced the panel (§62,
  2026-09-18)

- An object mask with two or more boxes, or boxes and clicks, got no
  mask: the decoder answers one mask a box in a single call. Each
  box is now its own call, with the clicks inside it, and the masks
  are joined (§58, 2026-09-18)

- Choosing another file from the strip put the new file's look over
  the old picture until the develop landed; the viewport now holds
  the old picture under its own look until the new one arrives
  (§57, 2026-09-18)

- Droppers: Neutral in WHITE BALANCE reads a temperature and tint
  off a click; Pick in CURVES puts a point at the tone under the
  pointer and drags it; Pick in COLOR MIXER chooses the band
  under the pointer and drags its saturation and hue (§55,
  2026-09-17)

- The navigator's rectangle missing after a zoom by key or click:
  a conditional element under the clip's rounded corners, which
  the renderer's cached layer never saw appear (§54, 2026-09-17)

- Edit history in the left panel: every step named for what it
  changed, a click to make it current, a hover to see it; and
  snapshots, whole states kept by name in the sidecar, restored as
  a step (§53, 2026-09-17)
- Presets: named parts of an edit as `.gcp` files under the config
  dir, a PRESETS section to apply, save and import them, a
  Lightroom `.xmp` import with the sections it can map, and
  `greycard presets` to list, import and apply in batch (§52,
  2026-09-17)

- What an export says about itself: an XMP packet with the source's
  name, what the sheet asked for and the whole edit, and
  `OriginalRawFileName` in the EXIF (§51, 2026-09-17)

- JPEG, PNG and TIFF open in the editor and the CLI, through their
  embedded matrix/TRC profile or as sRGB, with their EXIF and lens
  profile; the raw-only sections say so (§50, 2026-09-17)

- Perspective: vertical and horizontal keystone as the camera's
  tilt, a homography in the geometry stage on both paths (§49,
  2026-09-17)

- The panel's sections fold on their headers, remembered between
  runs; each with an effect has a switch on its header that undoes
  it in the picture and keeps its settings; the edit schema is
  version 2 for the NOISE section's own switch (§48, 2026-09-15)

- The sharpen's mask in the viewport: Show mask in DETAIL paints
  the blend red where the sharpen acts; `--sharpen-mask` for a
  screenshot (§47, 2026-09-15)

- Custom export resolution: a typed long edge on the sheet with
  the size it gives, kept in the settings; `--long-edge` beside
  `--export` (§46, 2026-09-15)

- Manual chromatic aberration: red and blue radius scales in the
  LENS section and on the CLI, folded into the profile's model so
  the picture is resampled once (§45, 2026-09-15)

- Navigator, in a new left panel: the frame with the view's
  rectangle, click or drag to move; `--snapshot` writes the whole
  window (§44, 2026-09-15)

- Output sharpening for downsized exports: the capture sharpening's
  deconvolution after the resize, Off, Low, Standard and High on the
  sheet (§43, 2026-09-15)

- Soft proofing: a profile (the export's spaces, or a file) with an
  intent and a gamut warning, S to toggle; the viewport and the
  display table in the export sheet's space (§42, 2026-09-15)

- Clipping: marks at the histogram's corners lit by the channels
  that clip, each a toggle for its warning over the picture, J for
  both; kept in the settings (§41, 2026-09-15)

- EXIF in exports: the source's tags carried into JPEG, PNG and
  TIFF, the size, orientation, software and time the export's own;
  the 16-bit TIFF written whole, LZW under the predictor (§40,
  2026-09-14)

- Lens corrections: distortion, chromatic aberration and vignetting
  from the lensfun database, fetched on first use, matched from the
  file's tags, interpolated to the shot; a manual distortion and
  the scale beside them; `greycard lenses` and `--lens` in the CLI
  (§39, 2026-09-14)

- CI: fmt, clippy and the tests on every push (§40, 2026-09-14);
  green on its first run, 2026-09-15
- The learned denoiser's answers kept as linear DNGs under the
  cache dir, keyed by what the network saw, 8 GB budget (§37,
  2026-09-14)
- The Denoiser's default tile down to 1024 and a plausibility check
  on every tile's answer, since the WebGPU provider returns garbage,
  not an error, when VRAM runs out on the integrated GPU (§37)

- Scopes: waveform, RGB parade and a vectorscope with the skin-tone
  line, off the histogram's compute pass (§38, 2026-09-08)
- Settings file: the export sheet's choices and the scope, kept
  between runs (§38, 2026-09-08)
- The mixer's hue slider shows which color each way goes to (§38,
  2026-09-08)
- Pre-push hook in `hooks/`: fmt, clippy and the tests (§38,
  2026-09-08)

- The eraser: Fill patches made up by LaMa, beside heal and clone
  (§36, 2026-09-07)

- Retouch: heal and clone spots and strokes, sources chosen by the
  engine, pins to move them (§36, 2026-09-07)

- Learned rasters cached as PNGs under the cache dir, keyed by file,
  model and shape (§34, 2026-09-07)

- Orientation from the EXIF tag: portrait frames develop upright
  (rawler leaves it Normal) (§35, 2026-09-07)
- Filmstrip from the camera's preview JPEG, turned as the develop
  is; 40 to 110 ms a file (§35, 2026-09-07)

- Filmstrip pictures follow their files' turns and mirrors; a subject
  found shows its mask at once (2026-09-07)

- `greycard-ai`: `ort` runtime with providers tried in order, model
  registry with hashes and licenses, store fetching on first use
  (§34, 2026-09-07)
- AI masks: Subject (BiRefNet) and Object by click, right-click or
  box (SAM 2.1), guided-filter refined into the brush raster on both
  paths, model sheet before a download (§34, 2026-09-07)

- Grain: scattered crystals from a hash, cubic or tabular, in frame
  units, on the encoded output, the same on both paths (§33,
  2026-09-07)
- Vignette: amount in stops, midpoint, feather, roundness, over the
  frame as shown (§32, 2026-09-07)
- Quarter turns and mirrors, in the geometry, exact on both paths
  (§31, 2026-09-07)
- Masking: brushes that add, subtract or erase, with feather and
  flow, sized by the
  wheel, joined with the gradients; strokes in the sidecar, one
  raster on both paths (§30, 2026-09-07)
- Masking: handles to move, reshape and turn a placed gradient (§29,
  2026-09-07)
- Zoom keys: space to 1:1 and back to fit, Z to 3:1; a click without
  a drag zooms to 1:1 there, another fits (2026-09-07)
- Mask infrastructure: local adjustments carrying a whole look
  (curves and mixer included), linear and radial gradients placed
  by drag, added, subtracted or intersected, named, blended in
  parameter space on both paths (§27, 2026-09-07)
- Color grading: shadows, mid-tones and highlights wheels with
  balance, into the curves' color table (§26, 2026-09-07)
- Color curves against lightness, R/G and B/Y, in Oklab, beside
  the point curves (§25, 2026-09-07)
- Straighten and crop: angle, level tool, aspect presets with 65:24
  and custom, portrait toggle (§24, 2026-09-06)
- Color mixer: hue, saturation and luminance by band, in Oklab
  (§23, 2026-09-06)
- Point curves: a master curve and one per channel, drawn on the
  histogram; the sliders of §19 are the parametric curve (§22, 2026-09-06)
- Capture sharpening, RawTherapee's, with the worker's first cached
  stage (§21, 2026-09-06)
- Export sheet: format, quality, size, color space, profile, the
  desktop's file chooser (§20)
- Tone sliders: highlights, shadows, whites, blacks (§19)
- Edit schema and sidecars (§16), display color management (§17),
  export with the CPU reference (§18)
- Design tokens and the control set, first pass (§14)
- Toolkit: Slint; the egui spike is not needed (§14)
- Create a repo remote
- AI denoise: our own raw-domain UNet trained on the §13 noise model
  (the profiled denoiser stays the path with no model). Data, network,
  training, `Denoiser`, CLI and bench (§37); three tiers beat the
  hybrid in every cell of the bench, base ISO included: v19 (1.2 M,
  2.4 s per 24 MP develop), v15 (2.3 M, 3.3 s), v20 (5.3 M, 4.2 s),
  and hold on Sony, Panasonic and Nikon frames they never trained on.
  In the editor since 2026-09-14: the NOISE panel's tier and blend,
  the registry entries, the CLI's tier names; the answers cached as
  linear DNGs since the same day; the weights published at the
  registry's Hugging Face address (§70, 2026-09-18)
