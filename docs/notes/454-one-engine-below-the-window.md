# §454. One engine below the window

*2026-10-29.* An outside review of the layering found the direction
clean (core knows nothing, gpu on core, render between edit and the
window as §361 meant) and one debt: the develop engine still lives
inside the window's crate. The job queue, the base cache, the GPU
substitution per op, the model lifecycle, and the export, headless
or not, are all under greycard-ui, which is why the editor binary
carries `--export` and the measurement flags, and why the command
line cannot render an edit. People have asked for a command-line
export. This section is the design; the parts are on the roadmap's
engine track and run after 0.9.1 has been out a while, since the
move touches what testers exercise most.

## What is there now

Three develops walk their own lists. The worker's `develop_job`
runs decode, prepare (balance, highlights, CA), demosaic or the
learned denoiser, core's `finish` (matrix, DCP map, orient), lens
and defringe, the guide plane, the retouch and the guide again, the
masks' detail bands, local contrast, dehaze, capture sharpen, then
the per-pixel finish of the render crate, in the viewport's shader
or on the CPU for an export. The reduced develop bins instead of
demosaicing and skips sharpen, denoise, and the fill. The command
line's develop has no edit at all: flags into core's ops, then a
linear DNG or an sRGB preview that is not the editor's finish. Core's
`finish` still carries dehaze and sharpen after orient, before the
lens, and every caller nulls them; a new caller that passes an
edit's settings straight in gets the wrong order silently. "Finish"
names two stages, core's after the demosaic and render's at the end.

The seam already exists. The worker takes jobs through its methods
and hands outcomes to a `Deliver` closure; the window wraps that in
`invoke_from_event_loop`, headless export in a channel. No Slint
type and no `State` crosses it. What does cross it lives in window
modules: the thumbnail batch, the export set, the AI keys, the
version key, the edited frame's ISO. Headless export builds a whole
worker, idle pool threads included, never sets a GPU, and reads the
sheet's subset of settings.json through window helpers.

## Decided

**The stage list, once.** An engine crate, greycard-engine, between
render and the window, owns the list of stages a develop walks,
derived from the edit and the purpose (the viewport, an export, a
reduced picture). Core's `finish` loses dehaze and sharpen; they are
stages of the list, after the lens, where every caller already puts
them. Core's post-demosaic stage is renamed "prepare" and "finish"
is the render crate's alone. The worker, the reduced develop, and
the command line walk the same list; the reduced develop is the list
with its bin and its skips stated, not a second pipeline.

**What moves down, as is.** The worker's modules (the job, the
outcome, the base, decode, develop, export, masks, keep, shelf,
prefetch, thumbs, the engine loop, the handle), the reduced develop,
the thumbnail pool, the previews, the engine half of the edited
pictures, the AI side, the queue and the names, the export sheet's
settings, and the version key. Their tests go with them; the test
helpers get a home in the engine crate's testing module as the
render crate's have.

**Delivery stays a closure.** `Deliver` is already the trait; a
trait would rename it. The window keeps its match over outcomes.

**The GPU is lent, never made.** The engine takes an optional
context from whoever owns a device; the window lends Slint's
surface device as now, the command line lends none and runs on the
CPU as headless export does since §236. The wgpu pin stays the one
shared line. The restart after a lost device stays in the window;
only the loss flag and its poll go down with the renderer.

**The engine without its collaborators.** The worker splits into
the engine core and an optional background side (the pool, the
previews, the reducer, held during a develop through a `hold`
trait), so neither headless export nor the command line spawns idle
threads.

**The viewport shader is held to its reference.** The shader, its
Rust, and the parity tests move to a viewport crate over gpu and
render, with the few Slint pixel types replaced by plain buffers
converted in the window, the scope's binning split from its
drawing, and the two `include_str!` of the shader from the render
crate pointed there.

**`greycard export`.** The command line gains an export subcommand
that runs the engine's synchronous export entry (what headless
export calls today) with the sheet, the presets, and the overrides
read by one serde subset reader of settings.json that the window,
headless export, and the command line share, in place of the three
readers now. The editor's `--export` stays as an alias for a
release, then goes. The sidecar read, write, and migrate helpers
(`load_sidecar_said`, the rows' `from_disk`, the files' list and
migrate, the export record, the sync note) move down first, as a
pure move in the style of §316 and §318, since the reduced develop
and the masks already reach up for them.

**Smaller things decided with it.** The Subject model's chooser
becomes a pure function in the engine that takes the session's
declines as data, so the command line and the window pick the same.
The learned-mask cache is keyed by the base it was made from, so an
export's masks are the viewport's (§236's one known difference). The
log's own-crate list takes the engine and viewport crates (it missed
the render crate from §361 to 2026-10-29). A command-line export that
loaded ORT's WebGPU provider leaves through `_exit` as headless
export does.

## Order

1. Pure moves that shrink the surface: the sidecar helpers down, the
   edited pictures split at their engine half, the version key out
   of the versions module, explicit imports in place of the glob
   imports of the crate root in each file touched.
2. The engine crate: the stage list and the worker moved, core's
   `finish` trimmed, the reduced develop on the list. The viewport
   and headless export unchanged in behavior, gates green, exports
   byte-equal on the parity set before and after.
3. The viewport crate.
4. `greycard export`, the settings subset reader, and the README
   line.

Each is one landing with its own review. Part 2 is the big one and
takes a design pass of its own on the stage list's type before the
move begins.
