# §761. An export batch, timed, and the CPU's arena (2026-11-28)

A tester's export of a few hundred frames took days (discussion 59).
We timed one: 234 frames from 20 to 61 MP, written by the program
itself with every kind of edit, exported by `greycard export` in a
release build.

**On a discrete GPU it is seconds a frame.** 868 s for the 234 on an
RTX 5070 Ti, 3.7 s a frame: a plain 45 MP frame 2.6 s, the learned
denoiser 6.8 to 11.8 s by tier, everything together 9.4 s. Nothing is
built again for each frame: the device, the pipelines, the lens
database, and the model sessions are made once a run, 5.5 s of loads
in the 868. A third of the time is the CPU's finish and the encode
after each develop, one frame at a time, the GPU idle through them.

**With no GPU the learned denoiser is the cost.** On the CPU,
Balanced takes about 12 s at 24 MP and Best 40 s and more at 45 MP,
on sixteen fast cores. That, on a smaller machine, is hours; days
needs more, and we found one such thing.

**The arena.** ONNX Runtime's CPU provider keeps an arena for each
session, which holds the session's largest run for as long as the
session lives. A batch with no GPU rested at 13 GB between frames
and peaked at 15.6 GB, which on a 16 GB machine is swap. The CPU's
sessions now run without it: 1.7 GB at rest and 6.5 GB at the peak,
the files the same. The peaks fall too, the parts' model from
24.6 GB to 9.1 GB. Only sessions made on the CPU provider change; a
GPU session's fallback operators keep theirs, being small.

It costs the learned denoiser 6 to 9 percent on the CPU, measured on
a quiet machine, the two ranges apart: it runs its session once a
tile, and each tile's buffers are now allocated afresh. The models
that run once a frame are as quick. We take the cost: a machine with
no GPU is the one short of memory. The arena kept for the denoiser
alone, shrunk after a frame's last tile (the run option
`memory.enable_memory_arena_shrinkage`), would buy the time back for
1.5 to 3.3 GB more at the peak, and is to be measured before it is
built.

An export also logs its finish and its encode and write, at info,
so a tester's log names the slow stage.

**Not measured:** the window's own queue, a small or integrated GPU,
a machine of few cores, macOS, and Windows.
