# §372. A Windows on ARM package (2026-10-19)

A tester with a Windows on ARM laptop asked to try greycard. The
release now builds a fourth package,
`greycard-<version>-aarch64-windows.zip`, natively on GitHub's
`windows-11-arm` runner, which is free for a public repository and
carries Visual Studio 2022 with the ARM64 toolset and Git Bash, so
the Windows leg's steps run there as they are. A workflow of its
own, `arm-package.yml`, run by hand from the Actions tab, builds the
same package from any branch and keeps it as an artifact for a week,
so a tester gets one before a release; GitHub lists it once the file
is on master.

## What the package carries

The editor and the command line, the sidecar icon, the installer
scripts, and the Visual C++ runtime, which `scripts/package.sh` now
takes from the `arm64` redistributable directory beside the `x64`
one: four DLLs there rather than five, since `vcruntime140_1.dll`
exists for x64 alone, and Microsoft's own ARM64 ONNX Runtime
imports only the four. No ONNX Runtime, no Dawn, and no DXC: pyke
publishes an `aarch64-pc-windows-msvc` archive of ms@1.28.0 with
DirectML but none with WebGPU, which is the provider we run the
models on, so `crates/ort-dylib` has nothing to make our runtime
from and refuses there, as it already did, and the release skips
that step on this leg. The skip is a field on the matrix row
(`runtime: none`) rather than a test of `runner.arch`, so the row
says what its package carries. A guard step on both Windows legs
fails when rustc's host is not the leg's target, so an emulated
x86-64 toolchain on the ARM image would fail with its cause plain
rather than at the package script.

## The models on the CPU, fetched

The models run on Microsoft's own CPU-only release of the same
version, `onnxruntime-win-arm64-1.28.0.zip`, a third entry in the
registry beside the two x86-64 ones (§358), fetched into the model
store on first use exactly as the x86-64 builds fetch it for a
processor without AVX2: the same sheet, the same `greycard models
--fetch onnxruntime`, the same hash checks on the archive and on the
three members kept, the DLL, the MIT license, and the third-party
notices, which are the x86-64 zip's files byte for byte. Each
registry entry now says what it is for, and the note written beside
the files says that instead of naming AVX2 on every platform.

The decision gained one fact, whether greycard ships a runtime for
this platform at all, true exactly where `ort-dylib` has an archive:
Linux on x86-64, macOS on Apple silicon, and Windows on x86-64.
Where it does not, there is no shipped step: the order is the
library named by `GREYCARD_ORT_DYLIB`, a fetched one, and off. A
library beside the executable is not taken there, since none was
shipped and one found there is nobody's to trust. Off is a new kind,
fetchable where the registry has a runtime for the platform: its
line opens with the fetch command, as the processor's does, and its
reason, which the sheet's Why line shows, is that no ONNX Runtime of
greycard's own is made for this platform. Where the registry has
none either (Linux on ARM, which has no package), the line says
`GREYCARD_ORT_DYLIB` can name one, instead of the old advice to run
`scripts/ort-dylib.sh`, which refuses there too. Nothing on x86-64
or on the Mac changes.

There is no GPU path for the models on this platform; the providers
are the CPU alone, labeled "CPU (slower)" as on an older x86-64
processor. pyke's DirectML archive is the candidate for one, and the
roadmap holds it until the package has run somewhere.

## A leg that may fail

The leg is new and nobody has run its package yet, so it carries
`continue-on-error` through a matrix field (`tentative: true`): a
failure there leaves the other three packages to be released, and
the release's globs need only one zip to match. It is not in the
push-time CI, which runs on three runners already; the workflow run
by hand builds it when asked.

## Verified, and not

Verified here: the registry's numbers against a downloaded copy of
the zip (size, hash, and each member's size and hash), twice, by the
author and the reviewer; our own zip reader taking exactly the three
members out of it; the DLL's machine type (ARM64) and its imports;
the decision's order on a platform that ships nothing, and its
lines, by test; the window's sheet offering the ARM64 runtime with
the platform as its reason, by test; the package script's choice of
layout and redistributable directory for each host triple, the three
others unchanged. A check of greycard-ai for
`aarch64-pc-windows-msvc` from Linux stops at `ring`'s build script,
which wants MSVC's headers, so the runner is the first build.

Not verified: that the runner builds it at all, or that every crate
builds for the target; that the arm64 redistributable has the four
DLLs the script copies (it fails, naming the file, if one is
missing, and the toolset step prints the directory's contents); that
rustup's `llvm-tools` component exists for the ARM64 Windows host,
which the "loaded at exec" check needs. Nobody has run the package
on a Windows ARM machine: not the window, whose wgpu has to find a
Vulkan or Direct3D 12 adapter on a Snapdragon's drivers, not the
viewport, and not the fetched runtime loading and running a model.
