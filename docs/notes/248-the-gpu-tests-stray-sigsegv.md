# 248. The GPU tests' stray SIGSEGV: NVIDIA's driver and instances made at once (2026-10-04)

Now and then a GPU test binary died with SIGSEGV and passed when we
ran it again: `greycard-gpu`'s `tests/ca.rs`, and `greycard-ui` as
`render::tests` started. We saw it during reviews, in crates the
change under review didn't touch.

**What it was.** systemd-coredump had kept six of those crashes (three
`ca`, three `greycard_ui`). All six are the same crash. The faulting
thread is at PC 0. The return address on its stack is the same offset
in the Vulkan loader (`libvulkan.so.1` + 0x22d5e). That is the
loader's add-ICD path, right after it `dlsym`s
`vk_icdNegotiateLoaderICDInterfaceVersion` from `libGLX_nvidia.so.0`
and calls it. In the same process, a second test thread is inside that
same NVIDIA function, connecting to the X display (`XOpenDisplay`,
then `_XReply`). One thread came from
`vkEnumerateInstanceExtensionProperties` and the other from
`vkCreateInstance`. Both were under `wgpu::Instance::new`.

The driver's code explains the crash. The exported negotiate function
checks a global flag. If the flag is set, it tail-jumps through a
function pointer. If not, it calls an init routine. The init routine
sets the flag first. Only then does it open the X display and fill the
pointer table. There is no lock. So a second thread that arrives while
the first is still in init sees the flag set, jumps through a pointer
that is still null, and the process dies. The loader negotiates on
every fresh load of the driver, and a fresh load happens whenever an
instance is made while no other instance holds the driver. The window
is therefore open on every instance creation, not only the first one
in a process. Seen on NVIDIA 615.71.09, Vulkan loader 1.4.357.

The test binaries hit this because each test makes its own instance
(`Context::own`, or a test's own `device()` / `context_with`), and
libtest runs the tests on parallel threads. The window is short, which
is why the crash was rare in ordinary runs and more common under load.
The software and AMD drivers don't have this bug. A run with only
lavapipe (`VK_DRIVER_FILES`) never loads the NVIDIA driver.

**Reproducing it.** Looping the suites as they were didn't reproduce
it: 200 runs of `ca` on the default adapter, 200 on lavapipe, and 61
runs of `render::tests`, with no crash. Starting threads together
behind a barrier didn't either, even with one CPU or a slowed X
server. Starting them together lines them up in the same phase, so
they wait on each other rather than overlap.

What did reproduce it was two threads making instances back to back
with no barrier, so that over a few seconds every phase offset gets
tried. On the NVIDIA machine that crashed 30 runs of 30 at five
seconds each, and 20 of 20 at two seconds, with the same loader offset
and the same second thread inside the driver's X connection. With
lavapipe only, the same program passed 30 of 30.

**What changed.** `greycard_gpu::instance()` makes a `wgpu::Instance`
while holding a process-wide mutex, and everything of ours that makes
an instance goes through it: `Context::own`, the `ca` limits test, and
the device helpers in `greycard-ui`'s `render`, `worker` and
`headless` tests. Each caller still gets its own instance, adapter and
device. Only the making takes turns, which costs little: an instance
takes tens of milliseconds to make.

We first tried a single instance shared by the whole process. That
stopped the Vulkan crash, but it brought out another failure: adapter
enumeration on a shared instance from two threads failed in wgpu's GL
backend with `EGL_BAD_ACCESS` in 29 runs of 30, and crashed once.
Separate instances made one at a time have neither problem.

The bug is NVIDIA's; this is the narrowest workaround on our side. The
lock covers only the instances we make. Slint makes its own instance
on the UI thread, and greycard-ai makes one to identify the WebGPU
adapter (once a process, behind a `OnceLock`). Either could in
principle race one of ours. In the editor, the worker uses Slint's
device rather than making an instance, so the overlap there would need
the WebGPU probe to run while Slint's window was coming up. We haven't
seen that crash in the editor. ONNX Runtime's WebGPU provider (Dawn)
makes its own Vulkan instance too, on the worker, after the window is
up. What keeps the editor safe is that Slint's instance lives for the
whole run: while any instance is alive the driver stays loaded, and a
later creation does not negotiate again. A third instance held alive
stopped the two-thread crash, 0 of 10 runs against 10 of 10 without
it.

**What the tests show.**

- A new test, `greycard-gpu/tests/instance.rs`, runs two threads
  making instances back to back through `instance()` for two seconds.
  On NVIDIA it passed 30 of 30 runs. With the lock taken out, the same
  loop crashed 20 of 20.
- `ca` with `--test-threads=2` (the configuration of the recorded
  crashes): 200 of 200 on the default adapter, and 100 of 100 on
  lavapipe.
- `render::tests`: 12 of 12. The full `greycard-gpu` and `greycard-ui`
  suites pass, and clippy and fmt are clean.
