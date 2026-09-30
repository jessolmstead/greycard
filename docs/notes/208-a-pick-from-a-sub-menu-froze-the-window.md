# 208. A pick from a sub-menu froze the window, and what was found on the way (2026-09-29)

Setting a rating or a color label from the frame menu's sub-menus,
with the pointer, froze the editor: no key, no click, no redraw,
until it was killed. The keys did the same change without trouble,
and so did the menu's top-level items, and a pick made from the same
sub-menu by arrow keys and Enter. Three defects came out of the
chase. One was the freeze; the other two had been costing every
frame since the editor was written.

**What the freeze was.** A menu takes the keyboard focus from the
window's key scope while it is up, and gives it back when it closes;
§171's frame menu read "the menu is up" off exactly that, a
`focus-lost` with the popup-activation reason setting `menu-up`, a
`focus-gained` clearing it, so that the click which closes a menu is
delivered to what is under the pointer and let go by there. A pick
from a sub-menu with the pointer closes two popups in one turn, and
Slint 1.18 does not give the focus back to the scope it took it from
in that case: `menu-up` stayed true, every surface let every press go
by as the closing click, and the keys had no focus to take. The window
was alive and idle in the event loop, the sidecar written, and deaf.
Escape would have woken it. The fix is a one-tick timer started from
every menu pick (`menu-done` in `app.slint`) that puts the focus back
on the keys, after the popups have closed; giving it back in the
activation handler itself did not hold, since the closing moved it
once more.

**Why it looked like a hang.** Until tonight the window redrew sixty
times a second whether or not anything changed, and each of those
frames recompiled its shaders (below), so a deaf window still had a
busy thread: the first stack traces landed inside a frame's render,
in the driver's queue submit and in naga's shader compaction, and read
as a frame that never ended. They were frames that never stopped
coming. Only after the two costs below were gone did a trace show the
thread waiting on the event loop with nothing to do, which is what a
focus bug looks like.

**The idle redraw loop.** In `BeforeRendering` the viewport's texture
was handed to Slint as a new `Image` every frame, and the scopes'
bins, the curve and the clipping marks with it, since `analyze`
returned its latest bins on every call. Each new image marks the
window dirty, which schedules the next frame. The viewport also drew
its picture on the GPU every frame. Now `Renderer::render` keeps the
size and the `View` it drew for and draws again only for another, or
after a setter changed what is underneath (`redraw`, set beside every
`image_changed`); it says whether it drew, and only then is the image
set. `analyze` hands the bins out once per readback (`fresh`). In the
loupe at rest: 28 frames for a run that had 700, and the window thread
at 0%.

**femtovg's pipeline cache.** femtovg 0.27's wgpu renderer ends every
flush by dropping each render pipeline that was not bound during that
flush. Slint flushes once per offscreen layer, so each layer's flush
threw away the main pass's pipelines and the main pass threw away the
layers', and the next frame compiled all of them again: measured
through the frame menu, 9,555 compiles across 702 flushes in eight
seconds, about a millisecond each on the RTX 5070 Ti, some thirty
milliseconds of the window's thread per frame with layers. A pipeline
is now kept until it has gone 1,024 flushes unbound; the same run
compiles 38. The key space is a few dozen either way. The fix is on
`jessolmstead/femtovg`, branch `pipeline-cache-0.27` off the v0.27.0
tag and `pipeline-cache` off master for the pull request, and pinned
here by revision as rawler is.

**Ruled out, and two leads for upstream.** The AI runtime's Dawn
device and the engine's wgpu device were not it: the freeze came with
`--cpu-ops` too. femtovg makes a fresh wgpu bind group for every draw
whose image differs from the last, about 300 a flush here, in a
millisecond; not the freeze, but a cache would help a slower GPU.
Slint's `draw_image_impl` retries in an unbounded `loop` when an image
asked to be colorized comes back uncolorized; the retry never fired
here, but the loop has no exit, and a bounded retry is worth
proposing. Slint's nested popup focus restore is the other.

**How it was found.** The release binary is stripped, so the first
trace named only driver frames; a second build of the same commit
with `strip = false` and line tables, in a target directory of its
own, gave traces with names. Two flags landed beside `--menu` and
`--sheet` for the reproduction: `--press KEY` sends a meta key once
the picture is up, and `--keys` a sequence of key and pointer tokens
(`down`, `enter`, `f8`, `move:X,Y`, `click:X,Y`, `rclick:X,Y`,
`wait`), a fifth of a second apart, before the snapshot, or without
one in an ordinary session. The dead state reproduced with them: a key
and a click after the synthetic pick did nothing, and do now.
`GREYCARD_UI_TIMING` says per frame whether the viewport was drawn.
No unit test covers the fix: the testing backend has no popups and no
popup focus.
