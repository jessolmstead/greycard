# §379. The neighbors prefetched onto the shelf (2026-10-19)

The shelf (§378) makes the return to a frame cheap. The walk forward
through a shoot, which is what the tester of issue 30 does, still
paid a fresh base on every right key. Now the worker, when it has
nothing else to do, makes the base of the next frame and then the
previous one and puts them on the shelf, so the key lands on "base
from the shelf". On this machine the key to the picture on screen
is about 0.51 s against 1.7 s before, with the develop itself 0.24
to 0.39 s.

## How the worker learns the neighbors

The window sends them. `browser::open_frame` calls
`Worker::neighbors(generation, list)` right after it sends the open,
each neighbor carrying the path, the edit the cell's line holds (the
one an open of it would send), the sidecar's turn, whether its seed
is still to come, and which side it is on. The window holds every
edit and turn already, so the worker asking would be a round trip
for nothing. The neighbors are what the right and left keys reach:
the strip's rows in filter and sort order (`beside`), stepping from
the nearest row when the open cell is hidden by the filter. Under
the index most frames are stand-ins whose sidecar is not read until
something picks them (§207), so a stand-in whose row says it holds no
edit and no versions is sent under the stand-in's default edit, with
the turn undone from the row's shown turns, which is what the read
will give; a wrong guess only misses the shelf. A stand-in whose row
says it is edited is left out and logged, as is a mirrored row and an
offline root: reading those sidecars ahead would queue before the
user's own pick and raise the window's read bar. A first version
left every stand-in out, so the prefetch did nothing in an indexed
folder, a collection, or a view of the roots, which is the tester's
setup; the review caught it, since the measurement had been made in
a fresh configuration with no index. The list is kept off
`Job::Open`, so the other
builders of an open (the tests, the headless export) are untouched
and, never sending neighbors, never prefetch.

The queue keeps the list with its open's generation, and the worker
records the generation of the last open whose develop gave a base.
A list for an older generation is dropped; one for a newer waits for
its open.

## When it runs

`prefetch::due` is asked only at the bottom of the worker's wait
loop, so nothing is queued: no develop, open, keep, mask, people, or
landscape job, no export, no person-on. It further needs a frame open
in the editor (leaving for culling or the grid drops the list), the
list's generation to be the landed one, a base in hand, no save
waiting on a develop, and the window quiet for the reduced develop's
idle, 1.5 s (`edited.idle_for`, touched on every open or develop
sent and at the end of every develop). That alone did not keep a
slider's pauses clear: with 2 s between moves every move met a
prefetch in its demosaic, 778 ms a move against 230 with the
prefetch off, the work thrown away each time. So each time a
prefetch gives way the list's quiet doubles, up to eight times, and
the next open starts again at the idle; measured over eight moves
with 2 s pauses, the first move took 781 ms and every move after it
218 to 240, a mean of 300. The quiet is added to when a neighbor is
ready; the loop waits with a timeout meanwhile. The reduced
develops' pool is held while a prefetch runs or waits
(`prefetch::claim`): the flag goes up before the pool is asked what
it has in hand, so one begun meanwhile is seen, and if one is in
hand the prefetch waits behind it at the front of its list, the flag
still up, so the pool begins no other and the prefetch runs next. A
first version lowered the flag while waiting, and since the pool
begins its next reduced develop in the pass that ends the last, the
prefetch would have waited for the whole reduced queue. Touching the
window instead would have pushed every reduced develop and every
next neighbor past each prefetch. Leaving the editor for culling or
the grid lets the list go; coming back to the same frame without
reopening it sends the list again (`neighbors_again`), taken only
when none waits and the landed open is that file, so it never
attaches to an open in flight. A trip while the open's own develop
is still running loses that list, and the first step after is not
prefetched. One neighbor at a time, the
next first, and the decoded input is dropped before the base is
shelved, so at most one extra frame is decoded at any moment.

## Giving way

The engine has no stop flag, so `engine_base_or_stop`, which
`engine_base` now calls with a closure that never stops, asks before
the demosaic and before the finish; the prefetch asks after the
decode and before the lens step as well. On any request it gives
way, logs "abandoned before its STAGE", and keeps nothing; if the
list is still the open's, the neighbor goes back to the front and is
made again from the decode once things are quiet. The one exception
is an open of the very frame being prefetched (same path, turn, seed,
and edit base, with at most the left frame's saves also queued): the
prefetch goes on, says so, and the open takes the base from the
shelf, since finishing is sooner than starting over. Measured, a key
during the previous frame's demosaic showed the picture 963 ms after
the key against about 515 idle: one demosaic. Keys faster than the
quiet period start no prefetch and cost what they cost before.

## What is skipped, each logged

A frame whose edit asks for the learned denoiser: the model runs for
seconds with no point to stop at, and its pair would hold a gigabyte
on the shelf. A picture file, whose base is its decode. An
unreadable file. Another version of the open file under the same
base. A base already on the shelf (`Shelf::holds`, the take's test on
path and stat before any decode; a neighbor whose seed is still to
come takes the shelved base's strength, so a never-developed frame is
not made twice). A base the shelf refuses as larger than its whole
budget, which `put` now reports. And every neighbor when the shelf has room for fewer
than two bases the size of the open one beside it: with room for one,
a prefetch would push out the frame just left. That rules out an 8 GB
machine at 45 MP and `GREYCARD_SHELF_MB=0`; with room for exactly
two, walking forward still works, since the previous frame is the one
just left and already on the shelf.

## The base it makes

The prefetched base goes through the shelf's `put` and `fit` as a
left base does: the same key (the file's stat before the decode, the
frame's identity after), the same budget, no learned pair, the
profile resolved. The CA runs on the GPU when there is one, through a
`CaRun` now shared with `develop_job`; a GPU CA error inside a
prefetch falls back to the CPU for that frame and leaves the context
alone, so a prefetch can never turn the GPU off for the session. The
GPU's working textures are not released (that would drop the open
frame's sharpen textures); a shelved base holds nothing on the GPU
anyway. A panic in a prefetch is caught, logged, and drops the list,
the shelf untouched; a real-queue test holds a prefetch between
stages through a test-only hook and shows the open of its own frame
landing from the shelf, a newer list replacing it, and the panic
letting the list go. `base_settings`, `ca_out_of_prepare`,
`resolved_profile`, `lens_step`, and `guide_step` are shared with
`develop_job`, so a prefetched base is made by the code an open's is.

While a prefetch runs, the camera JPEG stand-in appears 150 to 260 ms
after the key instead of 117 to 140, since the placeholder's decode
competes for the CPU. Nothing in the window says any of this; the
log has "prefetching the next frame", the landed line with its stage
times and the shelf's fill, the abandon, and the skips. A measuring
knob, `GREYCARD_TIME_SELECT_GAP_MS`, sets `--time-select`'s gap
between steps.

## Verified

- Unit tests: due only once the develop has landed, no save waits,
  and the window has been quiet; next, then previous, then nothing; a
  newer open drops the old list; the budget rule at one base, zero
  bytes, just under two, and exactly two; gives way to every request
  kind and goes on only for its own frame's open; the landed line's
  text; abandoned at each of the four checks on real files, keeping
  nothing; a prefetched base taken back by the open, not made again,
  bit for bit a fresh open's, and a second prefetch of it skipped; a
  learned frame and the open frame's own base skipped; the worker end
  to end (open, next and previous prefetched in order, the next
  opened from the shelf); a worker handed neighbors that never opens
  prefetches nothing; `beside` under a filter, at both ends, from a
  hidden cell, with one or no frames; the neighbors carry the cell's
  own edit, turn, and seed.
- Release build, headless weston, four 45 MP raws with the lens
  database present, `--time-select`, 7 s between keys, mean of three:
  515 ms with no index and 516 ms with the four listed from the
  index's rows and no sidecar read, "base from the shelf" on every
  step; about 1.7 s before.

## Not verified

- The no-GPU and `--cpu-ops` window paths headless; the prefetch's
  CA runs on the CPU there and the develops are unchanged.
- The GPU CA inside a prefetch in a unit test (the test files are
  linear, not Bayer); seen only in the headless runs' lines.
- The 8 GB skip on such a machine; tested through budget values.
- The hold on the reduced develops' pool, which has no test, and no
  run had edited frames for the pool to develop.
- An indexed walk onto rows that say they are edited, which are
  skipped and cost what they did before.
- A frame whose edit was reset keeps its history, so its row says
  not edited while its line, once read, is not "never developed":
  the prefetch seeds the learned strength where the open does not,
  and the base misses the shelf. Wasted work, never a wrong picture.
- A frame under a network root is prefetched like any other, so its
  decode, which cannot stop, can take seconds over a slow share and a
  slider move meanwhile waits for all of it; the backoff limits how
  often. The reduced develop runs such a frame alone; a skip under
  network roots is the small change if a tester meets it.
- Windows and macOS.
- A filter or sort change while a frame is open does not resend the
  neighbors; the ones sent with the open are prefetched, and stale
  ones just miss.
- An edit changed on a neighbor after its prefetch (a sync, a paste)
  leaves a base that never matches until the budget evicts it.
- Peak resident memory during a prefetch.
