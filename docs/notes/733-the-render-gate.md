# §733. The render gate: kept pictures for existing edits (2026-11-22)

An edit must export the same picture in every later version, and
nothing told us when a change broke that. Tests held each op to its
reference, and the reference moved with the op. So the engine's tests
now hold a set of pictures.

**What it is.** `worker/render_gate.rs`, a test module of the engine's
worker, so it runs the export's own walk and finish.
`tests/render-gate/edits/` holds 59 edits as a sidecar writes them,
over synthetic frames (patches, a sky, a checker with a clipped disc,
an X-Trans frame, noise): every op, every demosaic, the smaller
exports, and four written as schemas 1, 3, 4, and 7 wrote them, so the
migrations render too. `tests/render-gate/kept/` holds each one's
picture, an 8-bit PNG 96 px long. No raw, and no metadata.

**The test.** `existing_edits_render_as_kept` renders each edit on the
CPU and fails when a picture differs from the kept one by more than 2
levels at any sample or 0.02 on average. The fix is then a decision:
restore the picture, or call it a new process (the number each state
will record, on the roadmap), with its changelog line.

**Blessing.** `GREYCARD_BLESS=1` writes pictures for new cases, and
the whole set when `PROCESS` is ahead of `kept/process`.
`GREYCARD_BLESS=unpinned` rewrites what moved under the same process,
for a decode or bug fix we choose not to pin. Either way the run then
fails and lists what it wrote, so a bless is never a green run. It is
refused when `CI` is set, and when `kept/process` is missing beside
kept pictures or holds no process this build knows.

**Where it is enforced.** Linux x86_64 only (`ENFORCED`). Elsewhere
the gate renders, compares, and prints, and passes, until the ignored
`render_gate_spread` (the gate at 1, 2, 3, 4, and 8 rayon threads, then
on a card) has been run on a Mac and on Windows and shows the
tolerance holds there.

**One rayon thread.** The first gate ran its cases on the rayon pool
and hung at one thread: a walk with a mask's Detail starts its planes
on a thread whose work needs the pool, and waits. The cases now run on
plain threads, as the engine's worker is one, and
`the_gate_finishes_at_one_rayon_thread` runs the small frames in a
child process at one thread. The engine itself never walks on a pool
worker.

**Frames with a lens's blur.** A two-ulp change on the raw scale moved
the first X-Trans fixture by 25 levels. Markesteijn counts a direction
homogeneous when its derivative is within 8 times the least; on a
clipped plateau every derivative is exactly 0, so an ulp decides. That
is the algorithm as dcraw and RawTherapee have it, and a real frame
meets it only at a hard edge against clipped light. The fixtures are
now sampled 16 times a pixel over a pixel and a half, as any lens
would, and the same probe moves a case by 2 levels at most.

**What it does not see.** The learned denoiser and the learned fills
(no models in the tests), the viewport, and the card: the GPU's stages
are held to the CPU by their own tests. A reviewer's mutations of
capture sharpen's `SPIKE_RATIO` and `FLAT_ENOUGH`, CA's
`SHIFT_BLUR_SIGMA`, and the hot pixel threshold by 5% passed the gate:
no fixture sits near those thresholds.

**Cost.** About 10 s of the engine's suite at four rayon threads, and
816 KB in the tree.
