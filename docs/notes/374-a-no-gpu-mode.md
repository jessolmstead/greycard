# §374. A no-GPU mode (2026-10-19)

A tester on an Ivy Bridge i5-3470, whose Mesa Vulkan driver wgpu does
not take, could not open the editor: Slint's check for a hardware
adapter failed the start with "no GPU-backed WGPU adapter is
available ... Set SLINT_WGPU_CPU=1". With the variable set the window
opened on lavapipe, and the engine took lavapipe too, running every
develop op as shaders interpreted on four cores: minutes a frame at
400% CPU. With `--cpu-ops` beside it a 6000 x 4000 X-Trans frame
developed in 13.7 s and exported. Slow is the honest outcome on such
a machine; what we owed it was opening, and saying why, without the
two switches.

Slint draws through Vulkan here (Metal on a Mac, DX12 or Vulkan on
Windows) and never through GL, so a GPU that only GL can drive counts
as none. The Ivy Bridge is one: its OpenGL works, its Vulkan does not
pass wgpu. "No GPU" is therefore never what we say; "no GPU greycard
can draw on through Vulkan" is.

## Select first, then look

Slint's `select` is called as before. Only when it fails, and the
adapter was not asked for by hand, do we probe the adapters, on the
backends Slint's own check looks on (what `WGPU_BACKEND` names or the
default, less GL everywhere, Metal off Apple, and DX12 off Windows,
unless that leaves none: Slint's `mask_backends` and
`default_backends_to_avoid`, which it does not export, copied). The
choice is a pure function of the device types found and whether the
adapter was asked for:

- any adapter that is not a CPU one: Slint's failure was something
  else, and its error goes up as it did;
- only CPU adapters (lavapipe, WARP): software. We set
  `SLINT_WGPU_CPU=1`, build a fresh selector with its own window-fit
  handler and a clone of the settings, and select again. A failed
  select returns before it sets the platform, and a selector's drop
  acts only on one never selected, so the second select is clean;
- none at all: the start fails with "no Vulkan adapter wgpu can use,
  not even a software one (Mesa's lavapipe gives one)". Slint's own
  cause, which names a variable that would do nothing there, goes to
  the log at info, off the terminal;
- `SLINT_WGPU_CPU` or `WGPU_ADAPTER_NAME` set by whoever started us:
  as asked, with no probe and no retry.

A first version probed before every select. Slint already makes a
throwaway instance and enumerates the adapters before it selects, so
ours doubled that on every launch, 67 to 117 ms on a machine with
NVIDIA's, RADV, and lavapipe installed. Selecting first makes the
mode free wherever a GPU is: the probe runs only on a start that
would otherwise have failed, 2 ms with lavapipe alone and 60 to 83 ms
with all three drivers loaded and only lavapipe's device shown.

## Manual, or the variable

Slint 1.18's `WGPUConfiguration::Manual` takes an instance, adapter,
device, and queue we make, and looked like the clean way to hand it
the software adapter without touching the environment. It is not
enough on its own: the selector's fail-fast check runs for `Manual`
as well, enumerates the adapters of the instance it is given, and
refuses one with only CPU adapters unless `SLINT_WGPU_CPU` is set. So
the variable is needed either way, and `Manual` would add the device
making Slint does now (the limits, the features) and lose the one
thing `Automatic` does that we cannot before the window exists:
choosing the adapter with the window's surface as the compatible
one. We kept `Automatic` and set the variable.

The `set_var` is unsafe in this edition. It runs on the window's
thread before the event loop, the worker, or any pool of ours starts;
the drivers' own threads end with the instances that made them,
Slint's failed check's and our probe's, both dropped by then. A
breakpoint on `setenv` for this variable found the main thread alone,
under lavapipe alone and again with all three drivers loaded
(`VK_LOADER_VENDOR_ID_FILTER` showing lavapipe's device only).

The variable we set would reach the commands we start, and a greycard
started from one (a file manager we opened, and a raw opened from it)
would take it as asked for by hand and run its ops as shaders. So
we set a marker beside it, `GREYCARD_SET_SLINT_WGPU_CPU`, and a
greycard that finds both does not count the first as asked for by
hand. That covers every way out, `webbrowser::open` included, which
starts the browser for the report form and the release page and
takes no environment of ours. The reveal commands (`xdg-open`,
`open`, `explorer`) and the bug report's file manager, which do take
one, are started without either. Such a child's Slint still reads
the inherited variable, so on a machine with no GPU its first select
succeeds on the software adapter, and it says "drawing in software
on llvmpipe" rather than "no GPU greycard can draw on"; its ops go
to the CPU all the same.

## What the engine does

Whether the engine's ops leave the window's device is decided by the
device Slint made, not the probe: a window on a CPU adapter, unless
the adapter was asked for by hand, keeps the device from the worker,
so the ops run on their CPU paths exactly as under `--cpu-ops`. A
pure `lend_device(in_software, cpu_ops)` holds the rule. A GPU
machine whose window still lands on a software adapter (one Slint
passed over for the surface) is therefore software too, and told so
differently: there the status line names the adapter, "drawing in
software on llvmpipe and developing on the CPU, which is slow", and
not "no GPU". `--cpu-ops` keeps its meaning on any machine.

## Asked for by hand

`SLINT_WGPU_CPU=1` from the user means what it meant: a CPU adapter
allowed beside the hardware ones, and the engine's ops on whichever
adapter the window takes. `WGPU_ADAPTER_NAME`, which wgpu reads and
Slint honors before its own choice, is the same kind of request and
is treated the same way: naming llvmpipe on a machine with an RTX put
the window and the ops on llvmpipe, with nothing said of a missing
GPU. Either is the way to run the window on lavapipe with the ops as
shaders there, which a check of the shaders in the window wants. No
hidden flag was added. The cargo tests are untouched: they build
their own devices and run Slint on its testing backend, never through
the selector, and CI gets lavapipe by `WGPU_BACKEND=vulkan` and its
installed drivers, not by the variable.

## What the user sees

At the start the status line says "no GPU greycard can draw on
through Vulkan: drawing in software and developing on the CPU, which
is slow", and the log warns with the adapter's own line. The open at
start replaces that line within a moment, so every develop's line in
software carries the reason after its time: "6240x4160, developed in
2.34 s on the CPU, no usable GPU, ...", or ", drawing in software"
where a GPU is there. The user guide's "Before you start" says a GPU
is for speed and what happens without one; the README's install line
likewise.

## Verified here

- Unit tests: the choice (a hardware adapter beside a CPU one is a
  GPU, CPU adapters alone are software, none is nothing, either
  variable by hand is as asked), the lines (no "no GPU" where the
  window is on a software adapter beside a GPU), the device lent only
  on a GPU without `--cpu-ops` and never in software, the probe's
  backends against Slint's, the variable taken off a command only
  once we set it, and the status and develop lines in the testing
  backend.
- `greycard_gpu::adapters` finds the adapter a request on the usual
  instance gets, on NVIDIA and on lavapipe.
- Release build, a 26 MP X-Trans frame, a headless weston, a
  `--snapshot` each:
  - lavapipe alone (`VK_DRIVER_FILES` naming its manifest only): the
    first select failed, the probe found llvmpipe in 2 ms, the second
    select opened the window on it, the worker was never handed the
    device, and the frame developed on the CPU in 2.34 s with the
    reason in the snapshot's status line;
  - all three drivers loaded with only lavapipe's device shown: the
    same, the probe in 60 to 83 ms;
  - `VK_DRIVER_FILES` naming a file that does not exist: exit 1 with
    "no Vulkan adapter wgpu can use, not even a software one (Mesa's
    lavapipe gives one)", Slint's cause in the log only;
  - every driver: no probe, the window and the ops on NVIDIA as
    before, 1.65 s;
  - `WGPU_ADAPTER_NAME=llvmpipe` beside the RTX: the ops as shaders
    on llvmpipe, 6.29 s, nothing said of a missing GPU.
- The window fit on the second selector: on weston's Xwayland with a
  1280 x 720 output and a 4000 x 3000 size asked, the retry path's
  window was fitted to 1200 x 580, as the GPU path's was.
- On this 16-core machine lavapipe's shaders are only slower than the
  CPU paths (6.3 s against 2.3 s); on the tester's four cores the gap
  was minutes against 13.7 s.

## Not verified

- No Ivy Bridge, nor any machine whose only hardware adapter wgpu
  refuses, is at hand; lavapipe alone through `VK_DRIVER_FILES` stands
  in for it.
- Windows with WARP as the only adapter, the same path by reading.
- Whether wgpu's GL backend would have given that machine hardware
  drawing: Slint leaves GL out everywhere, and we did not try it.
- Where the models run in software is not touched: ONNX Runtime's
  WebGPU provider makes its own adapter through Dawn. On a processor
  without AVX2, as the tester's is, the AI features need Microsoft's
  CPU runtime anyway, and run on the CPU.
