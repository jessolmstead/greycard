# §406. The remove behind Fill

*2026-10-23*

§405's engine gets its caller. Fill stays one tool; which model
makes a fill is the engine's choice, and a tester sees a second
download, a button, and better fills.

## Which engine

A Fill patch drawn on a machine that has the remove model and a
WebGPU adapter is marked as the remove's. Its fill is made by RORem
when its window (the hole plus LaMa's context) is at most 1024 a
side, and by LaMa's tiles above that; an unmarked patch is LaMa's,
as every patch was. The mark is in the sidecar so an export on a
machine without the model, or without a GPU, leaves the area as it
was and says why, instead of quietly making a different picture
than the screen showed; the LaMa precedent (§36) already left a
fill it could not make. The price is that a patch drawn before the
model arrived stays LaMa's until it is drawn again.

Both engines share the window, the sRGB encoding at a middle-grey
gain, and the grain pass after (§398); the remove's output is
smooth, so the frame's grain always goes back over it.

## Variations

A fill is a seed, kept in the patch (default 0, left out of the
sidecar at 0, no schema bump). An "Another" button on a remove-made
Fill's row steps the seed through history, so undo is the previous
fill back from the cache in no time; the row reads "Fill 1 varied".
Lightroom shows three at once; one at a time with a step costs no
layout and the same clicks.

## The offer

The second model is 5.5 GB under RAIL++-M, so it is never fetched
unasked: the first time Fill is chosen on a machine with WebGPU and
LaMa present, the model sheet offers it with its size and license
from the registry; "Not now" keeps LaMa for the session. A CPU
machine is never offered it. The registry entry stays unlisted
until the files are published, so the CLI's fetch-all does not pull
it.

## Memory, measured properly

Part 1 asked ONNX Runtime for its lazy-release buffer cache through
ort's builder option, which the pinned release candidate drops on
the floor (§159: the key is prefixed twice), so part 1's memory
figures were the default cache's. Set through the session config
entry it takes effect, and inside the editor with the denoiser
holding the context first a 1024 remove peaks 9.4 GB over the
desktop by default against 8.3 GB with lazy release, 7.3 GB of it
the remove's own; the saving is the denoiser's cached buffers. The
mode is one per process, and lazy release slows the other models
5 to 43 percent (SAM's embedding 0.056 to 0.080 s). The default
stays. A card under 10 GB wants the tiled VAE the roadmap names.

## Honest answers, second pass

The correlation check that closes §405's gap is taken on 8×8 block
means over blocks at least half outside the hole, where grain
averages out: on a grainy flat sky a smooth answer correlates with
nothing per pixel and would have been refused. A flat answer and a
shifted one on structured content are refused below 0.5.

## Not done

- A fill that fails is remembered by patch, seed, and window for
  the open file, so a slider move does not rerun a 13 s remove; a
  new seed, an edit to the patch, or another file tries again. The
  cache of fills and the failures with it are keyed without the
  file, which the review flagged; it holds because a set's export
  and the background mask jobs each run on an engine state of their
  own, and the open file's is cleared at every open.
- The harness against Lightroom's Generative Remove (§380) before
  the release that ships this.
- A tiled VAE for 8 GB cards; the 512 window for small holes is the
  present answer there.
- The upload of the published files, and the entry moved into the
  listed models after it.
