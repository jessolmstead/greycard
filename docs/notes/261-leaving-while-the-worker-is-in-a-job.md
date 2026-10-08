# 261. Leaving while the worker is in a job (2026-10-07)

Quitting while the worker was in a job could kill the editor with
SIGSEGV in NVIDIA's driver. The bug was on the roadmap from §255, seen
17 times in scripted runs since 2026-10-03.

**The cause.** After the window closes, `startup::main` asks the worker
to stop and waits `LEAVING`, 5 s. A develop that outlasts the wait (a
debug build, a large frame, a mask, the learned denoiser) was left
running, and `main` returned. Returning runs the C library's exit
handlers, and NVIDIA's (`libGLX_nvidia`) tore the driver down while the
worker sat in `vkWaitSemaphores` under `wgpu::Device::poll`, reading
back a CA or sharpen result. The core dumps show both threads there.
Debug runs crashed 2 times in 60; with `LEAVING` forced to 0 in a
release build, 27 in 30 (one of them an abort on heap corruption
inside the driver, the same teardown).

**Leaving by `_exit` when the worker is busy.** A GPU wait cannot be
interrupted from outside, and joining the worker with no limit would
hold a closed window open for as long as a job takes. So the window now
does what the headless export does (§236), but only in this case:
`Worker::stop` says whether the worker ended, a worker that outlasted
the wait keeps its thread handle, and at the very end of `main`, after
the last save, a worker still running sends the process out through
`headless::leave`, which flushes the standard streams and calls
`_exit`. No exit handler runs; the kernel ends the worker where it is.
A worker that has stopped leaves the process to end as before.

Nothing the quit writes is lost by skipping the handlers. The saves
come first in their usual order (the import's file in hand, the index
closed, settings, the open frame's sidecar), and each is a write that
has returned. The log is an unbuffered file. Every file the editor
writes goes through a temporary name and a rename, and the index is
SQLite in WAL mode, closed explicitly before the exit. The only `Drop`
implementations in our crates set a stop flag or decrement a counter.
What the worker had in hand is lost, as a quit always lost it.

After the change: 0 crashes in 60 runs in debug and 0 in 60 with the
wait forced to 0; a review reproduced it, 11 in 18 before and 0 in 18
after.

**A window lost under the run.** The same crash had a sibling. When the
compositor goes away mid-develop (a logout, the shell crashing), winit
ends the event loop with an error, and `main` returned on it at once:
past the whole quit sequence, so nothing was saved, and through the C
library's exit, which crashed 10 times in 36. Now an error from the
loop is logged, the quit runs as for a closed window, and the error is
returned at the end (by `_exit` if the worker is still busy). After the
change, 0 crashes in 36, every run exiting 1 with its settings written.

**The quit's archive write.** In the same pass we found that the quit's
own sidecar save had always come after the index reader closed, so its
write to the frame's archive copy (§243) was deferred and then dropped
with the process: a change made in the last moments before quitting
never reached the copy. A newer write queued behind one still out was
dropped the same way. Those unsent writes are now noted waiting in the
index as "saved as the window closed", as a headless save is (§236),
and the next window's catch-up writes them from the sidecar on disk.

Two gaps stay open. A frame followed onto its archive copy while its
own root is away writes its sidecar only through a job, so its last
save before quitting is never written anywhere; it is on the roadmap.
And a job still out at the quit could, landing in the last
milliseconds, clear the row noted for a newer write under the same key;
that window is narrow, and the loss is the one every write had before.

**§236's teardown.** §236 says the window always ends by a crash in
Dawn's static destructors after a session that loaded a model. We
could not reproduce that: six window sessions with the learned
denoiser or a Subject mask, on NVIDIA 615.71.09, all exited 0. So the
window keeps the C library's exit when the worker has stopped, and
`_exit` stays the busy case only; ending every quit by `_exit` would
skip thread-local and static destructors with no crash to justify it.
