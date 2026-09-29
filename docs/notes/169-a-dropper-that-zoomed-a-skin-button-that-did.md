# 169. A dropper that zoomed, a Skin button that did nothing, and the GPU Subject model offered (2026-09-24)

Three roadmap items, fixed on one worktree, then a round of review fixes.

**A dropper's click no longer zooms.** The viewport's `TouchArea`
decided whether a release was a plain click (and so zoomed) by
reading `picking` fresh at release time. The Curve and Mixer droppers
keep `picking` set through the whole gesture, so that read was
correct for them; the White, Range and Defringe droppers are
single-shot and let go of `picking` from inside their own
`pick_pressed` handler, before the release ever arrives, so the
release's `picking != ""` check was always false for them and fell
through to `toggle-zoom`. The fix latches a `picked-down` flag on the
touch area at press time — before any handler has a chance to clear
`picking` — and the release checks that flag instead of the live
property. This fixes every current and future single-shot dropper at
once, without changing when each one puts itself down. The drag
handler is gated on the same flag, so a press-drag with one of these
three no longer pans the view either, which the release fix alone
left open (a drag reads `picking` too, and it is just as empty by
then). A dropper left in hand over the placeholder (no develop to pick
a color from) still lets a click zoom, as a click there always did;
nothing latches `picked-down` in that branch, on purpose — the
placeholder's own comment already said the zoom keeps working there.
A UI test drives the compiled window's pointer events directly
(through `crate::testing::click`) with fake `pick_pressed`/
`toggle_zoom` handlers standing in for the real ones (which need a
GPU-sampled picture to run), so it reproduces the exact bug: reverting
the `.slint` fix makes it fail.

**The Skin button is gone.** It read as doing something (a labeled
button beside Pick) but only ever set the sliders back to the same
window a new Color shape already starts at — Hue 55, the vibrance
protection's hue, is `Shape::skin()`'s default. Since the button did
not do anything a slider drag could not already do, and a skin found
by a model is its own later roadmap item, it is dropped rather than
renamed. `Shape::skin()` itself, and the "Esc to keep the skin" status
hint, stand: a new Color window still starts there. The removed
button's test now sets the Hue slider to `SKIN_HUE` by hand and checks
the shape comes back to `Shape::skin()`; it does not drive the
slider's own double-click-to-default gesture, which is a `.slint`
interaction with nothing behind it worth a Rust-level test.

**The GPU Subject model is offered when the original already falls
back.** `subject::pick` picked the store's original whenever the store
had it, with no way to ask whether that original had ever actually
run on WebGPU for this card. An install from before the WebGPU
rewrite shipped has only the original in its store; on a card the
onnx-community export cannot run whole on, that original silently
falls back to the CPU's three seconds a mask, and `pick` had nothing
to notice that with — the only record of it, `providers.json`, was
read by `runtime::open` alone. The fix adds a `remembered_failure`
query in `runtime.rs` (the same `plan` decision `open` makes, exposed
so a caller can ask it without opening a session) and threads an
`original_failed_on_webgpu` predicate through `subject::pick` and the
mask panel's `step`: with WebGPU on offer, the original in the store,
the rewrite not, and a failure on record for this adapter and build,
the rewrite is offered — still subject to `unavailable`, so a declined
or failed rewrite fetch leaves the original in use rather than waiting
forever. With no record of failure, the original stands, since a card
that runs it needs no second download of the same weights. The offer
sheet's note says why when this is the reason for the offer, rather
than reading as an unexplained second Subject download.

A review round found the worker side of that offer had not been
carried along. `panel::mask::step` decides which model to use with the
session's declines and the providers record in hand; the worker did
not have either, and `Subject::load`'s own `model_for(store, providers,
&[])` — always an empty `unavailable` — could pick a different file
than `step` just did, most sharply once a declined rewrite left the
store with only the original and a failure still on record: the panel
asked for the original, and the worker went looking for the rewrite,
which was not there, and the mask failed with "is not in the model
store". `Job::Mask` now carries the model `step` chose; `Ai::raster`
and `cached_path` use it directly rather than asking again, so the two
sides cannot land on different files. A worker-level test proves a
raster already cached under the original's slot comes back with no
model load at all, so it needs no real weights to run anywhere.

Three smaller things came out of the same round. The adapter probe and
a `providers.json` read were running from `ask_for`, which fires on
every render frame a Subject want is still waiting on; both are now
asked once and cached for the run (`ai::subject_original_failed_on_
webgpu`), since the first probe alone opens a `wgpu::Instance` and
blocks on `request_adapter`. An offer accepted mid-session had no way
to reach a Subject already loaded in the worker: `Outcome::Fetched`
now tells the worker to drop it, so the next mask picks up the new
file rather than the session's first choice until a restart. And
`step` had started waiting on a busy offer sheet even where the
store's original was ready to use right now with nothing to offer
this round (the sheet busy, or the rewrite already declined or
failed) — a mask must never wait on an offer when a working model is
already at hand, so that case asks for the original immediately and
leaves the offer for whenever the sheet is free again.

**The review.** Items one and two held on the first pass (the latch
cannot stay set, and the test fails with the slint hunk reverted).
The third had the blocking bug the paragraph above records: the
reviewer built a test against a copy of the user's own store, the
original only and its providers record, and watched a declined
rewrite leave the worker asking for a file that was not there. It
also found the adapter probe inside the render callback, the
accepted offer that would not be used until a restart, the sheet
that could hold a ready model waiting, and "113 MB" for 113,778,088
bytes. One round, then two rebases as the export set and the filter
bar landed under it.
