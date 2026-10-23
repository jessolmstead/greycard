# §412. GPU passes under the watchdog, and a lost device

*2026-10-23*

Two testers lost their GPU under a develop. On a Renoir iGPU with
2 GB (issue #35) a pass on a 32 MP frame ran past amdgpu's 10 s
ring timeout and the reset took the device; on a GTX 1060 under
Windows (issue #41) the sharpen's Iterations slider at 50 on a
20 MP frame ran past Windows' 2 s watchdog, the device was lost
from under the worker, and the window's thread then panicked on a
viewport buffer the loss had invalidated. The sidecar held the
edit, so every restart of that file died the same way.

## Why the sharpen

The sharpen ran every Richardson–Lucy iteration of every tile
batch inside one compute pass of one submission. The bench on the
tester's frame gives 58 ms for 20 iterations on an RTX 5070 Ti; a
GTX 1060 is about ten times slower, so 50 iterations is 1.5 to 3 s
in one piece, which Windows' default 2 s timeout kills. The other
passes were read and left alone: the lens correction submits once
per band of rows, the CA once per iteration, dehaze is one
dispatch, and the local contrast a bounded chain.

## Bounded submissions

The iterations are cut into submissions sized to a budget of
250 ms, an eighth of Windows' timeout and a fortieth of amdgpu's,
short enough that a slider drag stays live. A timed submission
gives a rate in seconds per padded pixel per iteration, kept per
context so the second develop starts right and scaling to a frame
of any size; a context with no rate begins with a one-iteration
probe, and a submission grows to at most twice the last one timed,
so a chunk measured fast on settled tiles cannot overshoot. The
rate counts the window's frames on the lent device, which only
makes it read slow, and it is retaken at every submission. The
cut is bit-identical to one submission: the per-batch uniform slot
and the early stop's buffers live on the GPU across submissions,
and a test with the atlas budget forced to one band per batch
checks every batching against the whole, planes, mask, and stats,
with the early stop on and off. Cost: 6 percent at one submission
per iteration; on this card a 50-iteration sharpen still runs in
one submission after the first develop, and the tester's file logs
three of at most 85 ms.

## A lost device

wgpu panics inside a poll on a lost device. greycard-gpu catches
that into an error of its own, marks the context lost, and every
later op answers without touching the device; the worker's per-op
fallbacks finish the develop on the CPU, and a loss found after a
develop landed drops the context and runs the on-screen develop
again on the CPU. The window's renderer is Slint's and owns the
device, which Slint cannot replace, and the crash in the log is
femtovg's own buffer mapping, out of reach. So the window's
device-lost callback saves everything on the normal quit path and
starts greycard again with the same arguments and `--cpu-ops`, the
open file swapped in, the AppImage used only when the running
binary lies under it; a run already on CPU ops quits saved instead,
so a loss cannot loop, and a capture run fails rather than restart.
The user sees the window close and reopen on the same file within
a second, the edit intact, engine ops on the CPU for that run.
Checked under weston with a patch that destroyed the device.

## Not done

- One iteration over a tile batch is the smallest unit; a GPU too
  slow for that alone is not split by tiles.
- The third start has GPU ops on again, so a file that still loses
  the device restarts every other start.
- Not tried on a real driver reset or on Windows; the tester's
  machine is the test.
