# §714. Windows ends without teardown once a model is loaded (2026-11-18)

**What was wrong.** On Windows the editor crashed about a second after its
window closed, three times across a tester's reports: an access violation
in the graphics driver's Vulkan library once, and in the shader compiler's
library twice at the same offset. The reports carry no dump, so the thread
is not known; the timing puts it in the exit. The first was read as a crash
of the learned denoiser in OpenGL. It was neither: on that vendor's driver
the Vulkan and OpenGL drivers are one file, and nothing of ours makes a GL
context in the editor.

**Why.** `ort` releases ONNX Runtime's environment from a TLS callback at
process detach. On Windows a return from `main` runs `ExitProcess`, which
kills every other thread where it stands and then runs each library's
detach on the exiting thread, so the WebGPU provider (Dawn, D3D12, the
shader compiler) is torn down under a wgpu instance held in a static and a
window's device still alive, perhaps with a worker killed inside a call.
On unix we already ended by `_exit` for this class of fault; the Windows
arm returned, and its comment said the crash had not been seen.

**Decided.** Every ending that can follow a loaded model goes through one
function, `runtime::end`: flush Rust's and C's streams, then `_exit` on
unix and `TerminateProcess` on our own process on Windows, so no library
detach, TLS callback, or atexit handler runs. The C runtime's `_exit`
would not do there, since it still goes through `ExitProcess`. The window's
quit, `--export`, the CLI, and the measuring start-up modes all end this
way once ONNX Runtime is loaded.

Nothing is lost that was kept before. The saves (sidecars, settings, the
open frame's edit, the writes still waiting, the library log's fold, the
GPU marker's ending) all happen before the end, as on unix; and
`ExitProcess` was already killing the background threads before any
cleanup ran, so no writer relied on it.

**Known limits.**
- A panic that unwinds out of `main` after a model is loaded still ends the
  old way, on every platform.
- A thread that loads ONNX Runtime just after the main thread has checked
  ends the old way too.
- Not run on Windows: that `TerminateProcess` stops the fault is reasoned
  from the reports and `ort`'s source. The check is a denoise at a 256 px
  tile, then closing the window, which crashed two times of two.
- We tried leaving GL out of our own wgpu instances on Windows and took it
  back out: DX12 always lists the software adapter, so a fallback to GL
  never ran and a machine with GL alone lost its GPU export.
