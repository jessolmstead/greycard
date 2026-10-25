# §422. The Core ML provider, in

*2026-10-24*

We put Core ML in as planned, on Linux, for the M5 to confirm. The
open question first: pyke's macOS arm64 archive, which `ort-dylib`
links our dylib from, is the `+coreml,webgpu` build. Its
`libonnxruntime.a` defines `OrtSessionOptionsAppendExecutionProvider_CoreML`
and `CoreMLExecutionProvider` and carries every option key we set, and
the dylib already links the CoreML framework. No rebuild is needed.

Under `load-dynamic`, `ort`'s `coreml` feature only compiles the
registration, so it is on by default everywhere and the provider is
offered only on macOS; release.yml is untouched. `Provider::CoreMl`
asks for ML Program, every compute unit, static input shapes, and a
compile cache under greycard's cache, one folder a model file (path,
size, modification time, and fixed dimensions), since ONNX Runtime
recompiles on a changed graph but not on changed weights.

`open` kept the first provider whose warm run passed. Core ML can pass
and still run most of a graph on the CPU, so where a GPU provider
follows it both are opened, a second warm run is timed on each, and
the faster is kept. The price is a second session at each load on a
Mac; remembering the winner would save it, and waits on the Mac.

The denoiser's free `h` and `w` are fixed at load through ORT's
free-dimension override, on Core ML only, so one weight file serves
every provider. The side follows the caller's tiling (608 packed at
the default; the bench's `--tile` loads for its own). Checked here:
with the override the session is [1, 4, 608, 608] in and [1, 3, 1216,
1216] out, no symbol left, and a 304 tile is refused. With Core ML on
offer every provider warms up on a whole tile, so the race compares
like with like.

Sky and Subject, static as exported, take the provider through the
same list. LaMa, SAM, and SAM 3 stay off it, each by a providers
function of its own: LaMa's free size would fall to the CPU under
static shapes, and each would pay two sessions a load for a race it
has no reason to win. The remove's UNet stays on WebGPU.

The probe takes `PROBE_EP=coreml`, `coreml-ane`, or `coreml-gpu` and
`PROBE_FIX`, and Core ML prints its compute plan.

A limit: the compile folders are never cleaned. Each model file and
tiling leaves one under `greycard/coreml/`; a sweep waits until we
know their size.

## Review

A fresh review found Core ML reaching every model through the window's
provider list, LaMa and SAM 3 among them, so each paid two sessions and
four warm runs a load for a race LaMa would lose under static shapes;
the three keep-off functions above are the fix. It also found the
bench's `--tile` refused once Core ML won, which is where the side
following the caller's tiling came from, and the Mac test sheet's
compute-plan grep matching nothing, since the device prints as an
object. The Mac has not run any of it yet; §090 names what it must
confirm, and target/captures-2026-10-24/coreml/mac-test.md holds the
commands.
