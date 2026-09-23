# Changelog

What shipped, under the version it shipped in, newest first. An item
moves here from `docs/roadmap.md` when it lands, with its notes
section and its date; the reasoning stays in `docs/notes.md`. The
release workflow puts a version's section into the GitHub release.

## 0.1.0, unreleased

- A short guide for testers, `docs/user-guide.md`: installing on each
  system, the basics, culling, the keys, and how to report a problem
  (2026-09-22)
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
  balance or colour matrix, a provider that would not load, a cache
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
