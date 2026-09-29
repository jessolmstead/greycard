# 109. Native dialogs, and the first Apple numbers (2026-09-20)

§108 got the tree building on a Mac; this makes it usable there. The
file chooser was the whole of what was missing, and with the machine
in front of us the §90 estimates could stop being estimates.

**One request, two choosers.** The three calls (a save, a file to
open, a folder to open) each build an `Ask` — a title, the folder to
open on, the name a save dialog offers, a filter, and whether it
wants a directory — and hand it to `ask`, which is the portal on
Linux and `rfd` everywhere else. The portal path is what it was, one
thread of its own for a blocking D-Bus call, now reading the request
instead of six positional arguments; `named` went with them, since a
name offered and a save dialog were always the same thing, and the
folder is carried outright rather than as the parent of a path with
an `x` joined on.

**The dialog goes where the window is.** AppKit wants its panels on
the thread the `NSApplication` is on, and rfd says as much: spawn on
the main thread, await where you like. So the non-Linux arm posts to
Slint's loop with `invoke_from_event_loop`, builds the dialog there,
and awaits it in `spawn_local` on that same loop — which also means
the window behind the panel keeps drawing, where a blocking dialog on
the loop's thread would freeze it. `done` is answered exactly once,
from a slot both arms can reach: posting to the loop takes ownership
of the callback, and if there is no loop to post to (no window yet)
this thread has to be able to take it back and report that, the way
a missing portal is reported.

**Filters come apart.** The portal takes patterns, `*.jpg`; a native
dialog takes extensions, `jpg`. `native_extensions` strips them,
folds case so `*.xmp` and `*.XMP` are one entry, and drops what is
left of a pattern with no extension in it. The portal's own filter
test stays, under its cfg, and this has one beside it.

**The tree splits at the dependency.** zbus is a Linux dependency now
and rfd a not-Linux one, so a Mac build carries no D-Bus stack at all
and §108's borrowed `async-io` question is moot off Linux. colord
goes with it: off Linux `colord_monitors` says it knows no monitor,
which is what the missing session bus amounted to, without the error
line at every start. ColorSync and WCS are the real answers and are
not written yet.

**The numbers.** An M5 (10 cores, 16 GB), release build, CLI develop
with the decode and the PNG in each figure, beside §90's 32-thread
Zen 5 and the Air it extrapolated to:

| develop                           | Zen 5, 32 thr | §90's Air | M5     |
|-----------------------------------|--------------:|----------:|-------:|
| 24 MP R6 II, sharpen              |         1.8 s |  4 to 6 s | 1.9 s  |
| 24 MP, sharpen + profiled denoise |         5.4 s |         — | 7.7 s  |
| 24 MP, learned denoise, fast      |    2.4 s (§37)|      25 s | 5.7 s  |
| 24 MP, learned denoise, balanced  |             — |         — | 8.7 s  |
| 24 MP, learned denoise, best      |             — |      50 s | 16.8 s |

§90 was pessimistic by about three times. An Apple performance core
against 128-bit NEON was the right worry, but the base develop is
memory-bound past a point on the wide machine, so the M5 meets a
32-thread Zen 5 on it rather than trailing by 1.5x the 6-thread
column. The learned tiers run on WebGPU through Dawn on Metal — ort
registers no CoreML provider yet, which is the 0.3 item — and their
CPU time is a quarter of their wall time, so the GPU is the wait. The
profiled denoiser is the one row that goes the other way, 1.4x the
Zen 5's, which fits §90's reading of it as the op that only pays with
many threads. In the editor a 102 MP GFX 100S II frame develops in
8.3 s, and the model store fetched its three tiers into
`~/Library/Caches/greycard/models`, which is §106's directories
working.

No 45 MP frame was to hand, so that row of §90 is still the Zen 5's
alone.
