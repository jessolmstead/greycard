# §358. Microsoft's ONNX Runtime where ours cannot run (2026-10-18)

§338's follow-up. The ONNX Runtime we ship is pyke's build, compiled
for x86-64-v3 so that it carries the WebGPU provider and runs the
models on the GPU; on a processor without AVX2 it cannot load, and
since §338 the editor runs there with the AI features off and a line
saying why (issue #23). Microsoft's own release of the same version,
1.28.0, picks its kernels at run time and runs anywhere, on the CPU
only. So on such a processor we fetch it, as a model is fetched.

## What is fetched, and where it stands

Linux x64 and Windows x64 only: the Mac package is arm64, which
lacks nothing ours needs, and Microsoft publishes no Intel Mac
build. The archive is downloaded into the model store, its hash
checked, and three members taken out by exact path and hash, the
library, the MIT license and the third-party notices, each written
to a part file and renamed only once all three have passed, so a
failure leaves the good copy from before or nothing at all; the
archive is deleted after. The tgz is read with tar and flate2, the
zip by a small reader of our own on flate2's deflate, stored and
deflated members, zip64 and encryption refused, every read capped
at the declared size. A failed download removes its part file and
the empty directory.

The decision gained one step, in order: the library the environment
variable names, with no processor check; ours, if the processor has
all of x86-64-v3; a fetched one, asked for only when the decision
gets that far, since it hashes the library; off, with a line naming
the fetch command when a runtime is on offer for this platform. The
decision sits in a lock that can be asked again, so the fetched
library loads in the same process; one library is ever loaded in a
process. With the fetched runtime the providers are the CPU alone,
labeled "CPU (slower)" wherever a provider is named.

## The offer

The first thing that wants a model offers the runtime through the
same sheet a model download uses: why, the name, that it runs the
models on the CPU only and slower, the size from github.com and the
size kept, the license, and where it goes. Every model offer, a mask
whose model is already in the store, and a learned denoise or fill
that failed on such a machine all offer it; a batch run never does.
Yes downloads on the worker with a model's progress and failure
paths, then decides again, loads, and goes on with what was held up:
the model offer, the mask asked for, a pending People pick or
Landscape, a develop for the denoiser or fill. Not now declines it
and the held-up model for the session. A library that downloaded
but will not load is said so, removed from the store again, and
offered afresh at the next start, which is the honest state; in the
running process the failure stands and nothing offers a model.

Headless `--export` never fetches: the frame is written without its
learned masks, the line names the runtime and the fetch command, and
the exit code is 2, as for a missing model. `greycard models --fetch
onnxruntime` fetches it on request, loads it before saying the AI
features run, restores a deleted license file, refuses the other
platform's id by name, and says beforehand when another library
comes first on this machine so this one will sit unused.

## Measured here

On this AVX2 machine, Microsoft's build named through the
environment variable ran the Sky model on the CPU in the same 0.13
to 0.25 s as ours on the CPU, against 0.03 s on WebGPU, with the
same labels on the same frame. "Slower" is against the GPU, and a
processor without AVX2 will be slower again.

## Not verified

Nobody has yet run it on a processor without AVX2. QEMU user mode
is not installed here and no such machine is at hand; the roadmap
keeps the line until it is run under an emulated pre-AVX processor
or on a real one. Windows was checked by reading: the DLL's imports
are the VC runtime the package already ships, and a rename over an
existing file replaces it there, but a re-fetch from the CLI while
the window holds the DLL loaded fails on the rename and restores
nothing. When the removal of a library that would not load fails
itself, which the loaded DLL can cause on Windows, the lines still
say it was removed and the next start trusts it by its hash. After
a runtime that would not load, a mask that was waiting replaces the
status line within a frame with its own failure, which carries the
same reason.

## Tests

The decision order, named then shipped then fetched then off, and a
short processor fetching and then loading; the registry's two
entries with their license; a runtime in the store had by its hash
and not fetched again, a gone license refetched, a failed download
leaving no part file or directory; the library and its license out
of a tgz and a zip and nothing else, a member that fails its hash
leaving the good copy, an archive error naming the archive; the CLI
naming the runtime by word or id and refusing the other platform's;
the window offering it with its license, Not now keeping the plain
line, a batch run not offered, a runtime that would not load said so
and not offered again.
