# Architecture

The map of the code: the crates and what each may depend on, the
develop stage by stage, the window, what lives on disk, and how it is
tested. It is written from the code as it stands; the notes
(`docs/notes.md`) are the why, cited as §N. Where the two disagree,
the code wins and the sentence says which note it supersedes. When a
crate or a stage moves, this page moves with it.

Start here, in this order:

1. `crates/greycard-engine/src/stages.rs`: what a develop runs.
2. `crates/greycard-engine/src/walk.rs`: how it runs, and what it keeps.
3. `crates/greycard-engine/src/worker/mod.rs`: the jobs and outcomes.
4. `crates/greycard-ui/src/panel/startup.rs`: the window built, the
   worker with it.
5. `crates/greycard-ui/src/panel/deliver.rs`: the window's answer to
   each outcome.

## 1. The crates and the layer rule

Eighteen crates under `crates/`, and the fuzz targets in `fuzz/`.
"Depends on" is the crate's normal dependencies among our own crates,
as its `Cargo.toml` has them; test-only dependencies are left out.

| Crate | Owns | Depends on |
|---|---|---|
| `greycard-core` | Decode (rawler behind `decode::Decoder`), every develop op's CPU reference, color, DCP profiles, the linear DNG writer, EXIF, registration and stacking, and the HDR merge | none |
| `greycard-edit` | The edit schema in physical units, its sections, migrations, the sidecar and its history, versions, sync, and the `Loader` trait for profiles and looks | core |
| `greycard-edit-io` | Other tools' files read into the edit (Lightroom, RawTherapee, XMP), preset import, the profile and look stores (`Stores`), and a frame's sidecar as an open reads it (`on_disk`) | core, edit |
| `greycard-lens` | The lensfun database and Adobe's `.lcp` files, fetched and matched to a frame | core |
| `greycard-gpu` | The device versions of core's ops (CA, demosaic, lens, local contrast, dehaze, sharpen), each held to its reference, and the `Context` a device's owner lends the engine | core |
| `greycard-ai` | ONNX Runtime, the model store and registry, the learned masks, the learned denoiser, and the fill | core |
| `greycard-match` | The camera match's fit: the maker's look from the JPEG a raw carries | core |
| `greycard-render` | A developed frame made into a picture or a file: the CPU finish the shader is held to, AgX, the profile tone curve, the output sharpen, the masks' detail bands, geometry, the border, the watermark, and the export (JPEG, PNG, 16-bit TIFF, and lossless WebP) | core, edit |
| `greycard-library` | The SQLite index, the roots, the thumbnail cache, collections, stacks and their carried copies, the catalog import, and a sidecar held or noted from disk | core, edit, edit-io |
| `greycard-viewport` | The viewport's shader and its renderer, the scopes' binning, the monitor's display table, and the device's loss flag; no Slint type | core, edit, gpu, render |
| `greycard-engine` | The stage list and the walk, the worker and its jobs, the reduced develop, the thumbnail pool, the previews, the edit's pictures, the models' side, the export queue, sheet, and headless run, the settings subset, and the camera match's runner and the measurement runners over the export's develop | core, edit, edit-io, lens, gpu, render, ai, match, library |
| `greycard-slint` | The `.slint` files compiled to Rust, a crate of its own so a change to the editor's code does not recompile them | none |
| `greycard-version` | The build's version and its label, the commit stamped by the build script, a crate of its own so a new commit recompiles only the two binaries' crates | none |
| `greycard-i18n-probe` | A test of the window's translation mechanism on a catalog of its own: one `.slint` component, one French `.po`, select and read; never shipped (`publish = false`) | none |
| `greycard-ui` | The editor: the window, its `State`, the panels and sheets; a library target of its windowless modules beside the `greycard-ui` binary | core, edit, edit-io, lens, gpu, render, ai, library, match, engine, viewport, slint, version |
| `greycard-cli` | The `greycard` binary: `develop`, `export`, `merge`, `library`, `models`, `lenses`, and the rest | core, edit, edit-io, lens, gpu, ai, library, engine, version |
| `greycard-bench` | The demosaic benchmark and the denoiser's training data | core, ai |
| `ort-dylib` | A build tool: makes the ONNX Runtime shared library `scripts/ort-dylib.sh` installs | none |
| `greycard-fuzz` | The fuzz targets over every reader of outside text, a workspace of its own (`fuzz/`) | core, edit, edit-io, library |

The layers, each crate naming only crates on rows below its own:

```
5   greycard-ui                               greycard-cli
4   greycard-viewport                         greycard-engine
3                       greycard-library
2   greycard-render     greycard-edit-io
1   greycard-edit   greycard-gpu   greycard-ai   greycard-lens   greycard-match
0   greycard-core   (rawler, rawcolor)        greycard-slint (Slint)   greycard-version

    tools: greycard-bench (core, ai), ort-dylib, greycard-i18n-probe,
           fuzz/ (core, edit, edit-io, library)
```

rawler and rawcolor are core's, but not core's alone: edit-io reads
rawcolor directly, and the library takes rawler under its `fixtures`
feature, for other crates' tests.

The rules, in prose:

- **Core knows nothing.** No UI toolkit, no tone-mapping choice, no
  edit schema (CLAUDE.md's rule); and no wgpu, no ONNX Runtime, no
  SQLite. rawler is its decoder and DNG writer, never its developer.
- **Edit, lens, gpu, ai, and match stand on core alone.** None knows
  of another; a consumer joins them.
- **Render sits between the edit and the window**: an edit and a
  developed frame in, a picture or a file out, with no device (§361).
- **The library stands on core, edit, and edit-io**, and never on a
  UI (§72).
- **The engine is over render and lent a device, never makes one.**
  Whoever owns a wgpu device and queue hands them over
  (`Worker::set_gpu`), and the worker builds its `greycard_gpu::Context`
  on them on its own thread (`Context::from_device`); without one,
  every stage runs on the CPU (§454).
- **The viewport is over gpu and render** and holds no Slint type:
  what the device gives back is a plain `Rgba8`, and the window makes
  Slint's picture of it (§509). Nothing outside it reads the WGSL.
- **The window is on everything below it**, and the only crate that
  names Slint besides `greycard-slint`.
- **The command line is on the engine and has no window**: no
  viewport, no Slint.

A test, `crates/greycard-core/tests/layering.rs`, holds the rule: it
fails when `cargo metadata` and the table above part ways in either
direction, and when the window names core's develop outside its tests.

A crate the log lists do not name (`OURS` in
`crates/greycard-ui/src/log.rs` and `crates/greycard-cli/src/log.rs`)
logs at warn only; one whose info lines matter joins the lists.

The root `Cargo.toml` patches three projects to forks of ours, rawler
(§13), femtovg (§208), and Slint (§377), each until an upstream
release carries the fix; the comment above each says which.

**The camera match and the runners.** The camera match's runner
(`camera_match.rs`: the survey, the plan, and the fit run) and the
measurement runners (`match_compare.rs`, `measure_baseline.rs`,
`measure_crop.rs`, `measure_lightroom.rs`, `fit_lightroom.rs`) are the
engine's, beside the frame develop they call
(`worker::FrameDevelop`). The window keeps the sheet
(`panel/camera_match.rs`), its progress, and the calls, and reads the
runners' flags into their options (`runners.rs`).

## 2. The develop, stage by stage

Every develop of an edit is one list of steps,
`greycard_engine::stages::stages`, derived from the edit, the frame's
facts, and the `Purpose` (the viewport, an export, the reduced
picture, what the models see, a prefetch, a merge's frame). `greycard_engine::walk::walk`
runs it. `walk::plan` places a step on the device only for the
viewport, the prefetch, and an export with a device lent: the CA, the
lens, and a demosaic with a device version
(`Stage::in_memory_on_device`), the local contrast, the dehaze, and
the sharpen (`Stage::device`), and the viewport's `Finish::Shader`.
An export's run of the Detail section, the dehaze, and the sharpen
writes full floats and is read back for the CPU's finish. Everything
else, and an export with no device, runs the CPU reference.

The decode comes first and is not a step: `greycard_core::decode`
reads a raw through `RawlerDecoder` into a `RawFrame`, or a picture
through `greycard_core::picture`; `stages::needs` says what the decode
must give, and the list is made from its `Facts`.

| Step | What | Crate and code | Device twin and its test |
|---|---|---|---|
| `Prepare` | Levels, crop, the DNG's mosaic opcodes (gain maps, bad-pixel lists), hot pixels, the noise measured, white balance as gains in camera space, and highlight reconstruction | core, `develop::prepare_with` (`develop/gain_map.rs`, `develop/bad_pixels.rs`, `develop/hotpixels.rs`, `develop/highlights.rs`) | none |
| `Ca` | Lateral chromatic aberration on the mosaic | core, `develop/ca.rs` | `ca.wgsl`; `crates/greycard-gpu/tests/ca.rs` |
| `Demosaic` | Mosaic to camera RGB | core, `demosaic_prepared` (`rcd.rs`, `amaze.rs`, `vng4.rs`, `xtrans.rs`, `dual.rs`) | `amaze.wgsl`, `rcd.wgsl`, `vng4.wgsl`, `dual.wgsl` (Bayer; X-Trans stays on the CPU); `crates/greycard-gpu/tests/amaze.rs` and `demosaic.rs` |
| `Bin` | In place of the demosaic, for the reduced develop | core, `develop/reduced.rs` | none |
| `Learned` | In place of the demosaic: the learned denoiser's network and a plain demosaic beside it | ai, through the walk's `Models` (`worker/base.rs`) | ONNX Runtime's own providers |
| `Denoise` | The profiled denoise on camera RGB | core, `develop/denoise.rs` | none |
| `HighlightFill` | The learned highlight fill, where the edit names a version this build has, on a Bayer raw of the camera's own: fields made from the plain picture through the matrix, applied to it and a learned pair's other half, in the mosaic's frame; at a bin, look-scaled. The frame's range is carried to `Tap::Guide` as a shift from before the fill. The fields are kept (`KeptFill`, in `Kept`, counted in its bytes and shelved only with a filled picture) and applied again by a walk of the same steps in the same places; the walk asks its hold between the fill's parts whether to give way | core, `develop/highlight_fill/camera.rs`; weights from ai's `highlight`; the stage in the engine's `highlight_fill.rs` and `walk::fill_op` | none |
| `HighlightStandIn` | The window's alone, never in a list an edit makes: the last fields applied through this walk's white balance, standing in for `HighlightFill` while a white balance or the profiled denoise's strength moves (`stages::stands_in_for`); walked on a copy of the frame's kept points, which keep none of it | engine, `highlight_fill::stand_in` over core's `CameraFill::through` | none |
| `Warp` | A DNG's own lens warps, on the camera's geometry | core, `develop/warp.rs` | none |
| `IntoWorking` | The matrix into linear Rec.2020, the camera profile's map, the orientation | core, `develop::finish` | none |
| `Blend` | The learned pair blended by strength | engine, `walk::blend` | none |
| `Orient`, `Reduce` | A picture input's turn and its reduction | core `develop::orient`, render `reduce.rs` | none |
| `Lens` | Distortion, vignetting, and TCA, from greycard-lens's match | core, `develop/lens.rs`; matched in the engine's `lens.rs` | `lens.wgsl`; `crates/greycard-gpu/tests/lens.rs` |
| `Defringe` | Purple and green fringes | core, `develop/defringe.rs` | none |
| `Tap::Masks` | The learned masks' rasters, made, asked for, or read from the cache | ai, through the engine's `worker/masks.rs` | ONNX Runtime |
| `Retouch` | Clone, heal, and fill | edit's `retouch.rs` over core's `develop/retouch.rs`; fills from ai through `Models` | none |
| `Tap::Guide` | The tone equalizer's guide plane | render, `finish::guide_plane` | none; the shader reads it |
| `Tap::Planes` | The masks' detail bands | render, `detail.rs` | none; the shader reads them |
| `Tap::SharpenGuide` | The picture before the local contrast, dehazed with it, that the sharpen reads what is flat from; only where the local contrast runs and a sharpen follows | engine, `walk.rs` (`SharpenGuide`) | the device's dehaze and sharpen read it there |
| `LocalContrast` | The Detail section's local contrast | core, `develop/local_contrast.rs` | `local_contrast.wgsl`; `crates/greycard-gpu/tests/local_contrast.rs` |
| `Dehaze` | Dehaze; its fit stays on the CPU | core, `develop/dehaze.rs` | `dehaze.wgsl`; `crates/greycard-gpu/tests/dehaze.rs` |
| `Tap::MaskPicture` | The picture a refined mask's edge is made against | render, `refine.rs` | none |
| `Sharpen` | Capture sharpen | core, `develop/sharpen.rs` | `sharpen_apply.wgsl` and two more; `crates/greycard-gpu/tests/sharpen.rs` |
| `Finish` | Tone (or the camera profile's own tone curve in the display curve's place), color, the look, the masks' adjustments, the output sharpen on the display-referred luminance, and the output space: `Finish::Shader` for the viewport, `Finish::Cpu` then the border and the mark for a file | render, `finish.rs`, `profile_tone.rs`, `output_sharpen.rs`; the engine's `finish.rs` reads both ends off a walk | `viewport.wgsl`, and `osharp.wgsl` for the output sharpen, in greycard-viewport; `crates/greycard-viewport/src/render/tests.rs` (`the_shader_is_the_cpu_over_random_edits`, §221; `a_profile_tone_curve_on_the_gpu_is_the_cpus`; `the_output_sharpen_on_the_gpu_is_the_cpus`) |

A DNG's mosaic opcodes run inside `Prepare`, on the cropped mosaic
before the noise is measured (§513). Core's `develop::finish` keeps
its name for the move into the working space; §454 had it renamed,
but only its other half landed: dehaze and sharpen left it and are
stages after the lens.

**Kept points.** A list carries `Keep` marks (`Camera`, `Pair`, `Base`,
`Patched`, `PreSharpen`, `Developed`); at each, a walk stores the
picture, what it measured, its taps, its steps, and where each ran, in
a `walk::Kept`. The next walk resumes from the last point whose steps
make the same picture as the start of its own list (keeps skipped, a
mask tap read alike whatever its purpose) and whose places its plan
allows, so a walk without a device never starts from a picture the
device touched (an export runs under the device when one is lent,
§600). The open frame holds a `FrameKept` (`worker/base.rs`); the
shelf and the prefetch hold the same. The open file's export resumes
from a copy of it that shares every picture by `Arc`, so the window's
device points survive the export (§501, §505, §506).

**The highlight fill's rounds** (§719 §3, `worker/develop.rs`). A
window's first round whose walk would make the fill delivers its
picture without it: the list without the fill (`Steps::without_fill`),
or with the last fields standing in, the camera picture kept where the
fill would run (`Keep::Camera`, `Steps::kept_before_fill`; the
stand-in's walk hands that one point to the frame). The frame says the
fill-off edit until the next round (the masks' edit and print, the
develop delivered), and its edit's pictures wait (`filling`). The next
round (`Job::Fill`, `walk::walk_fill_round`) walks from the camera
picture, gives way to a newer open or develop until the fill is made,
leaving the frame as it was. It takes the frame's picture without the
fill (its kept points, base, bands, and guide) into the round and lets
go of all of it once the fill is made, before the filled base is; a
round that fails after that leaves the frame only its fields, so the
next develop makes the base anew. Through, it lets go of the camera
picture. Exports, the prefetch, and the reduced develops make the fill
in their one walk.

**The reduced develop is the same list.** `Purpose::Reduced` (the
edit's pictures of a frame not open) bins the mosaic by its cell
(`Facts::bin_under`) in place of the demosaic, or reduces a picture,
refuses fills, reads masks from the cache only, and finishes to
exactly 2048 on the long edge; its `Steps::skipped` names the learned
denoiser, the profiled denoise, and the capture sharpen, when the
edit asks for them, with `Skip::Reduced`. `Purpose::ModelsSee` is
that list without the retouch, the guide tap, and the planes tap,
ending after the dehaze. `Purpose::Prefetch` ends at `Base` (§454, §488).
`Purpose::MergeInput` is a merge's frame: the prepare with nothing
clipped after the gains, the CA, the demosaic (or the learned
denoiser), and a DNG's warps, ending at camera RGB, all on the CPU (the engine's
`merge.rs`, which `greycard merge hdr` and the window's Merge to HDR
run). A
camera thumbnail is not a develop: it is the file's embedded preview,
or a picture file itself (`worker/thumbs.rs`).

**One develop still outside the list.** `greycard develop`
(`crates/greycard-cli/src/develop.rs`) calls core's ops from its own
flags to write a linear DNG or TIFF for another editor, as it always
has; `greycard export` walks the list (§514). §454 had the command
line walk the list too; that holds for `export` alone.

## 3. The window

`greycard-ui` is two targets. Its library
(`crates/greycard-ui/src/lib.rs`) holds the modules that need no
window, no Slint type, no wgpu, and no `State` (§467, §471). Its
binary (`main.rs`) holds the window: one `State` on the UI thread, its
fields grouped by owner in `crates/greycard-ui/src/owners/`.

| Owner | Holds |
|---|---|
| `Launch` | What the command line asked of this run: captures, timings, and actions at the first open |
| `Window` | The Slint app handle, the settings file, the session's policies (sidecar writes and placement, deletes, and batch) |
| `Sheets` | The sheets without a struct of their own: deletes, look rename and remove, model and lens offers, a preset's save, and sync and paste |
| `Work` | The exports and their queue, Edit in, the archive copies' writes, and the saves |
| `Culling` | Culling, the lightbox, compare, and the camera's pictures held and fetched ahead |
| `Views` | The library views: the index and its filter, roots, collections, stacks, the tree, the selection, and the thumbnails |
| `Develop` | The open frame and its edit, the develop landed, the `Renderer`, zoom, scopes, and the tools on the picture |

Beside the owners, `State` keeps the file list, cells, sidecars, and
the sheets with structs of their own (import, Lightroom, RawTherapee,
camera match, archive).

**The worker and its handles.** The engine's `Worker` takes `Job`s
through its methods on one thread and hands each `Outcome` to a
`Deliver` closure; delivery stays a closure, not a trait (§454). The
window builds `Worker::batched` in `panel/startup.rs`, whose closure
posts each outcome through `slint::invoke_from_event_loop` to
`panel::deliver::deliver`, the window's match over outcomes. The
window lends Slint's device through `Worker::set_gpu`. The engine's
`headless` run builds `Worker::headless`, which delivers to a channel
and starts no `Background` (the pool and the reducer, held during a
develop through `walk::Hold`). The restart after a lost device stays
in the window (`gpu_lost.rs`); the loss flag it reads is the
viewport's (`lost.rs`).

**The thumbnail pool's three tiers.** The engine's `thumbpool` makes,
in order: the camera thumbnails, the local previews behind them
(`previews.rs`, 2048 on the long edge, for the culling loupe and an
offline root), and the edit's pictures (`edited_pictures.rs`, the
reduced develop). All three share the library's thumbnail cache,
keyed by content hash. A cache miss is made only near the screen, the
rows on view and as many again either side; while culling, a
thumbnail or a preview is made within the reach ahead as well (§452,
§495, §512). The window's half of the edit's
pictures, the notes to the list and the refreshes, is `edited.rs`
(§464).

**One instance.** A launch with a path, or none, first asks a running
editor of the same user and configuration folder to take it: a Unix
socket (`instance/socket.rs`) or a Windows named pipe
(`instance/pipe.rs`). Exports and runs with flags (but `--cpu-ops`,
`--verbose`, and `-v`) never take part (§504).

## 4. On disk

**The sidecar.** `<file>.gcd` beside the frame, or under `.greycard/`
beside it as the placement setting says: JSON of the edit, its
history, its versions, its meta, and its exports (`Sidecar` in
`crates/greycard-edit/src/sidecar.rs`). The schema version is
`VERSION` in `crates/greycard-edit/src/lib.rs`; every field has a
default and is left out of a state at it, no default changes within
a version, and a changed meaning bumps the version. A newer sidecar
is refused only past its read floor; past its write floor it opens
read-only (§734). The migrations are greycard-edit's (`migration.rs`); the step
that waits on the frame's shape runs at an open, in edit-io's
`on_disk.rs`, which also merges an `.xmp` beside the frame (§16,
§463, §464). Revisions and the join of two machines' copies are §243,
§244, and §734.

**The index.** `library.sqlite` under the data folder, owned by
greycard-library, a cache the files can rebuild (§72). Its version is
SQLite's `user_version`, `SCHEMA_VERSION` in
`crates/greycard-library/src/lib.rs`. An older index is brought up in
place when every step from its version is in `MIGRATIONS` (each adds
columns or tables a pass can fill), else dropped and rebuilt; a newer
one is refused. Its tables:

- `files`: one row a file: path, size, mtime, content hash, EXIF, and
  the sidecar's meta.
- `keywords`: a file's keywords, one row each.
- `versions`: a file's versions with their develop keys.
- `meta`: the index's facts about itself by name.
- `archive_writes`: the sidecar last written to each archive copy.
- `pending`: archive writes waiting on the archive.
- `looks`: a frame's look signature and focus, for bursts.

The roots are `roots.json` beside it, since the index is rebuilt from
them.

**Collections and stacks.** Each machine's own files are under
`collections/` beside the index (`<id>.json`, `<id>.smart.json`), and
`stacks/` within it. Every save also writes a copy into
`.greycard/sets/` of each folder a set touches and into its backup;
a deletion leaves `<id>.gone.json`. Lineages decide which copy wins
(`crates/greycard-library/src/carry.rs`; §493, §511).

**The model store.** Models under the cache folder's
`greycard/models`, fetched after their license is shown and checked by
hash (`crates/greycard-ai/src/store.rs`, `registry.rs`).
`providers.json` beside the store records each execution provider's
outcome on this adapter and build, and the settled race between Core
ML and WebGPU (§97, §466; `runtime/record.rs`).

**settings.json.** Under the configuration folder (`XDG_CONFIG_HOME`
first). One file, two readers that agree by construction:
`greycard_engine::settings::Shared` reads what a run with no window
needs (the export sheet and presets, sidecar switches, the last file),
and the window's `Settings` flattens `Shared` into itself, so every
name is defined once (§514).

**The export record.** An export is noted in the frame's sidecar
(`Sidecar::record_export`); for a frame no window holds, the library's
`export_record.rs` writes it on disk.

## 5. Testing

- **Every op has a CPU reference with a test**, and a device version
  is held to it: the ops in `crates/greycard-gpu/tests/`, the
  shader in `crates/greycard-viewport/src/render/tests.rs`, the scopes
  against `scope.rs`. An export is always the reference's.
- **GPU tests on lavapipe** (§221). CI's Linux job installs Mesa's
  Vulkan drivers and runs with `GREYCARD_REQUIRE_GPU=1` and
  `WGPU_BACKEND=vulkan`, so a test that finds no adapter fails rather
  than skips; the second because wgpu ranks a GL adapter above a CPU
  Vulkan one. Locally, the same three: `VK_DRIVER_FILES` naming
  lavapipe's manifest alone, `GREYCARD_REQUIRE_GPU=1`, and
  `WGPU_BACKEND=vulkan`.
- **The gates.** CI's `check` job runs, on Linux, macOS, and Windows
  (`.github/workflows/ci.yml`), `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings`, `scripts/ort-dylib.sh debug`, and
  `cargo test` with `GREYCARD_REQUIRE_ORT=1`; its `fuzz-check` job, on
  Linux alone, runs `cargo check --locked --manifest-path
  fuzz/Cargo.toml`. Locally: `scripts/ort-dylib.sh debug`, then `cargo
  fmt`, `cargo clippy --all-targets`, and `cargo test`, with `git
  config core.hooksPath hooks` once so `hooks/pre-push` checks the
  first two.
- **No bare folder removal in a test** (§502). Tests remove scratch
  through `greycard_core::testing::remove_dir_retry`, which on Linux
  waits for and names any SQLite file or map still open under the
  folder, the failure Windows would give. A guard test,
  `crates/greycard-core/tests/scratch_removal.rs`, scans every crate
  for a bare `remove_dir_all` in test code.
- **Test hooks across crates** sit behind a `testing` feature (core,
  edit, render, engine, viewport) or `fixtures` (library), turned on
  by the consumer's dev-dependency; `cfg(test)` alone does not reach
  another crate's tests (§478).
- **Ignored tests** want files the repo does not carry: run them with
  `--ignored`. The common two are `GREYCARD_MODELS`, naming a model
  store, and `GREYCARD_SAMPLES`, a folder of raws; some suites read
  their own (`GREYCARD_TEST_RAWS`, `GREYCARD_RAW`, and more, named in
  each test's comment). Without them they pass at once and prove
  nothing.

## 6. Where the notes are

`docs/notes.md` indexes the design record, one file a section under
`docs/notes/`; §5 is the engine's principles. This page is the map and
the notes are the why: §5 predates the ai, render, gpu, engine, and
viewport crates. Its CLI as the first front end is superseded by
§7's window, and its no learned features at first by §34. Its one
render graph for everything holds in part: every develop of an edit
is the stage list, but a camera thumbnail is no develop and `greycard
develop` stays outside the list.
