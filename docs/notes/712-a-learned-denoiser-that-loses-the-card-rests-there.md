# §712. A learned denoiser that loses the card rests there (2026-11-17)

**What was wrong.** On a tester's 2 GB card (Windows, Vulkan), Fast or Best
lost the window's device about two seconds into the learned denoiser's run.
The restart under `--cpu-ops` moved only the engine's ops; the model went
back on the same card, lost it again, and that loss quit, since a loss under
`--cpu-ops` was taken for the window's own drawing. The run was at 384, as
§646 decided: the second "denoiser tile 512" line was the reload's choice
from the card's whole, not the run's tile.

**Decided.** A model is blamed on evidence only: its run was under way on
the card when the window's device was lost (the callback reads the runs held
in memory), or the run itself failed with a runtime's word for a lost device
(Dawn's `device_lost`, "is lost.", "Device lost"; D3D's
`DXGI_ERROR_DEVICE_REMOVED`, `_HUNG`, `_RESET`). It goes in a record of its
own, `off_card.json` beside the provider record in the model store, keyed by
the adapter and driver and by the build: the provider set, version, ONNX
Runtime's build, and the binary's label, which carries the commit for a
source build and is the plain version for a release or a build with no git.
A file of its own, so a load on another card or driver, or an older
greycard, which rewrite the provider record's one slot for a file, cannot
wipe it. While any tier is there for this card, every tier is refused at
`Denoiser::from_store`, which every caller passes (the window, an export, a
merge, merge again, the CLI): not on WebGPU, and not on the CPU after it
(§646 cut that rung). The window rests the tiers with "learned: not on this
graphics card"; an edit with a tier, a merge's frame, an export develop as
on X-Trans: no denoise, said on the status line and in an export's line, a
merge's recipe with no tier. "Denoise the frames and merge again" is not
offered, and declines up front in its own words if asked. A merge whose
frames asked for a tier and went without it says so after its finished line,
"no learned denoise on this graphics card" (or "on X-Trans"; the card's
wins when both), joined by a format-only "{line}; {why}" a translation can
reorder, and as one CLI note; the reason is a value on the merge's report,
not read from a warning. A frame whose plan kept the tier but whose load
the card refused says the same note, not a "stood down" warning. Only the window's panorama reaches it:
the window's HDR and every focus stack walk no tier, the CLI's HDR and
panorama pass none, and merge again declines a tier up front.

The restart: the quit, off the callback, writes the record and reads it
back. Under `--cpu-ops` it starts again only when a record landed; the start
carries the hidden `--after-model-loss` (a plain launch still, so it claims
the single instance), and a loss in that run starts no other. A loss with no
model's run and no recorded run failure is as before.

The way back is **Try again**, beside the hint: it takes this card's and
build's entries off the record (another card's, driver's, or build's stay;
the write is atomic), lets the tiers back on in this process, and develops
the open frame again. It also lets go of the denoisers this process put
off the card, so a later loss with none under way is not taken for theirs
under `--cpu-ops`, and a record read begun before the press (the models
probe's) sets nothing after it. A loss after it is put on record again as
any is. A
new driver, another card, a new release, or a source build at another commit
also clear it; a build of the same commit keeps it.

The card's whole memory is now its largest device-local Vulkan heap, as the
free figure already was: summed, NVIDIA's BAR window (~224-256 MiB) read a
2 GB card as 2.2 GiB. Cards now read lower by their BAR heap: NVIDIA
without resizable BAR (correct now), and cards whose heaps were disjoint and
the sum right (RADV without ReBAR, Intel Arc with a small BAR, possibly
AMD's Windows driver), now 256 MiB under. No threshold moves for a common
card: 6 GB stays over `CARD_FROM` 5.5, 8 GB over the remove's 6.3; a 2 GB
AMD card without ReBAR takes the past-card give-back at 512, with the same
result. The remove's greyed figure changes on NVIDIA (4.2 to 4.0 on a 4 GB
card). Unified memory, Apple, and lavapipe are unchanged.

`Denoiser::run` logs the tile it runs at. Where the window or an export
would have said the card's reason as a failure with the engine's denoise in
its place, it says it as the no denoise it is; a merge's "stood down"
warning keeps its old wording. A
later launch logs the record's path when it finds this card on it, and a
record that will not parse says so (the next write drops its entries).

**Known limits.**
- A healthy card that loses its device during a denoise (a sleep and
  resume, another program's reset) rests every tier until Try again.
- Two windows writing the record at once: the last rename wins and may drop
  the other's entry; a lost entry is written again at the next loss, so no
  lock.
- The CLI's `--ai-denoise <path>` names a file, not a tier, and rests as
  the tiers do; a loss under it is still not written to the record.
- The first read of the record loads ONNX Runtime and names the adapter,
  so the window's hint only reads the flag the worker's read has set.
- The cause is unproven between the watchdog and memory: both give the same
  adapter-wide loss (paging is what makes a dispatch slow enough for TDR).
  The 384 estimate was measured on one large card under Vulkan; the tester's
  Dawn runs on D3D12.
- A smaller tile chosen by measured speed could keep the model on such a card.
- Masks and fills are not wired: a loss under them is as before.
- New strings, approved: the hint "learned: not on this graphics card", the
  reason "the learned denoiser is not on this graphics card", the button
  "Try again", and the merge's "no learned denoise on this graphics card"
  and "no learned denoise on X-Trans". The .pot needs the window's four and
  the format-only "{line}; {why}".

**Tests and the record.** A test process never reads the user's store, ONNX
Runtime, or an adapter for the record: each crate's tests say they are tests
(`runtime::off_card_for_tests`: from the engine's own checks in its tests,
a constructor at the window test binary's start, the CLI's load in its tests), and then the card
and store are only what a test names on its thread. A binary, debug or
release, with the test seam built in or not, runs the real path.

**Tests.** The ladder at the tester's figures; the nine pairs at
`from_store`, the stage skip (window, export, merge frame), and merge
again's check; a merge skipped and not made again; a run under way put off
and a run ended not; a run failing on each lost-device word, ordinary
errors not; a store that cannot be written puts nothing off and a loss
under `--cpu-ops` then quits; the record per card, driver, and build, kept
across another card's and driver's loads; a run after a model's loss never
starts again, read from its own arguments; the plain launch with the
once-only argument; the record written by the quit, not the callback; the
installed callback reads the runs under way; the held run let go in
`Denoiser::run` (fixture, CPU, run with ONNX Runtime); the window's rested
tiers; the largest device-local heap as `adapter_memory` reads it; the
adapter's identity carries its driver; a record written later in the
process seen by the flag; Try again found by its element id, taking this
card's entry alone, the hint and button gone, a second loss on record again;
the card's reason said as no denoise in the window and an export, and the
old wording kept for every other reason; a merge's tail for each reason and
kind that asks a tier, the card's first, and none with no tier or the tier
run, in the window and the CLI; a test reads no record unless it names its
card; the first ask reads the store, once; a read begun before Try again
sets nothing after; Try again lets go of what the process put off; no
button on X-Trans; a press asks a develop; no tail on a recorded card with
no tier (engine and window); the card's refusal at the load is the merge's
note.
