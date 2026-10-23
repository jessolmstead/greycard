# §395. A start marker for a driver that dies

*2026-10-22*

On a Windows 11 machine with an Intel UHD 630 on the last driver its
line will get, beside an RTX 5060 Ti, greycard-ui died at every start
with an access violation inside igvk64.dll, Intel's Vulkan ICD, as
Slint created its wgpu instance (issue #34). The log ended after the
window-fit line. Nothing in the process can catch a crash inside a
driver during instance creation. `WGPU_BACKEND=dx12` avoids it, since
the Vulkan loader then never loads the ICD, and Slint reads that
variable when its settings are built.

## The marker

A start that cannot catch its own death can leave a note for the next
one. Before Slint's select, the start writes `<cache>/greycard/
gpu-setup` beside the thumbnail cache: a comment, `setup: default`
or `setup: dx12` for the backends under way, and `died:` with the
backends earlier starts died on. Right after the select returns, the
note is updated: removed when the default backends worked, or kept as
`died: default` when the start had gone to DX12 and DX12 worked, so
later starts go straight there with an info line naming the file. A
start that finds `setup:` still there with no owner alive knows the
last one died in the select, and on Windows sets `WGPU_BACKEND=dx12`
with a marker variable of ours, scrubbed from commands we start, as
the software adapter's and the interface scale's are. A value the user
set by hand is honored and the marker only logs. Off Windows there is
no second backend Slint draws with, so the marker warns and the start
goes on.

Two things keep it honest. A lock file is held for the setup window,
so a second greycard started beside a live one (Explorer's multi-file
Open with starts one a file) reads the held lock as alive, counts no
death, and writes nothing. And the window closes right after Slint's
select, about 80 ms into a start, because the select is where the
enumeration that loads every ICD happens; a kill, a logoff, or an
out-of-memory later in startup is then no death. That gives up a
crash while the real device is made after the select, and the
`SLINT_WGPU_CPU`-by-hand path, where the select skips enumeration.

## Not looping

Both backends can die. When `setup: dx12` is found with `died:
default`, the start goes back to the default backends, warns, and
stays there; nothing alternates. A DX12 select that comes back as an
error drops the pin and retries the default backends in-process
before the software probe, which otherwise would see DX12 alone, find
WARP, and leave the machine drawing in software. A DX12 start whose
event loop ends with no device made writes `died: dx12` for the same
reason; an early return after the select for any other cause (an
unknown preset, a failed open) leaves the pin, since a typo must not
turn into a crash loop.

greycard-ai's adapter probe, which builds an instance of its own to
remember which provider a model ran on, honors `WGPU_BACKEND` the
same way, and skips the probe when the variable names none of
Vulkan, Metal, or DX12 rather than widen past the user's choice.

## Open

- Untested on the machine it is for: the user's Windows box has the
  check (a first crash, a second start on DX12, the third straight
  there, a learned mask running, the file deleted bringing the cycle
  back).
- Dawn, which ONNX Runtime's WebGPU provider runs on, chooses its own
  adapters; whether it loads the Intel ICD at the first model run on
  that machine is unknown, and the marker cannot cover it.
- A window closed within the first frame on a pinned machine counts
  DX12 as failed; the log names the file to delete.
- Direct3D 12 has never drawn greycard: the ops' shaders go through
  naga's HLSL and dxcompiler there, untried.
