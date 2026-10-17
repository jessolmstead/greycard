# §338. ONNX Runtime at first use, and the processors it cannot run on (2026-10-17)

Issue #23: on an Intel i5-3450, an Ivy Bridge with AVX but no AVX2 or
BMI2, both binaries died at exec with `Illegal instruction`, before
any of our code ran. The ONNX Runtime `ort` 2.0.0-rc.13 links
statically is pyke's prebuilt, which is built for x86-64-v3 (AVX,
AVX2, BMI1, BMI2, F16C, FMA, LZCNT, MOVBE), and BMI2 sits in its
ordinary code (abseil's hash, `std::vector<bool>`) and in the static
initializers that run before `main`. Dawn, beside the binary as a
library the loader opened at exec, is built the same way. Microsoft's
own releases are baseline x86-64 with run-time dispatch, but carry no
WebGPU, and pyke ships no shared library at all. So the editor starts
on such a processor only if nothing of ONNX Runtime or Dawn is in the
process at exec, and that is what we built.

## Loaded at first use

`ort` runs with `load-dynamic` and none of its defaults: no download,
no copied libraries, no linking. `runtime::open` loads the library
the first time a model is opened (`ort::init_from`) and nothing
before; `Provider::available` answers without loading it, as the
window asks it on its render path. Before the load, `runtime::status`
decides, once a process and from nothing but CPUID and the disk:

- `GREYCARD_ORT_DYLIB` set: that file, taken on its word and without
  the processor check, since the check is about the library we ship
  and a library named by hand (Microsoft's CPU-only build, which the
  roadmap has as the follow-up) is the user's call. Named and not
  there is off, said so, rather than falling back to ours. Not
  `ORT_DYLIB_PATH`, `ort`'s own variable: it is anybody's, may be set
  in a user's environment for some other program, and taken on its
  word it would hand a v3 library to a processor that cannot run it.
  We never read it, and `ort` reads it only when nothing was loaded
  through `init_from`, which ours always is.
- Otherwise the processor: `is_x86_feature_detected!` for the eight v3
  additions; AArch64 needs nothing. Any missing is off, and the line
  names them and the processor, by its CPUID brand string (the same
  text `/proc/cpuinfo` shows, on every platform, no file read).
- Otherwise the library beside the executable, which is where a
  package puts it and where `target/<profile>/` holds it for a build;
  for a test in `target/<profile>/deps` or an example in `examples/`,
  the profile directory above as well. The executable is followed to
  the file it is first: on a Mac `current_exe` answers with the path
  the process was started by, and the installer links
  `~/.local/bin/greycard` into the bundle, where the library is (on
  unix only: Windows links nothing, and its `canonicalize` answers
  with a `\\?\` path that would be in every line and handed to
  LoadLibraryExW). Not there is off, and the line names every place looked and that a
  build from source makes it with `scripts/ort-dylib.sh`.

A library that is there and will not load is off too, from then on,
and the line carries the loader's own reason: `ort`'s error says only
that `dlopen` failed and keeps the reason inside the libloading error
it holds, so we unwrap it ("dlopen failed: libwebgpu_dawn.so: cannot
open shared object file", with Dawn moved away).

Off is one plain line, the same everywhere it is said:

    the AI features are off: this processor (Intel(R) Core(TM)
    i5-3450 CPU @ 3.10GHz) has no AVX2, BMI2, FMA, ... or MOVBE, which
    greycard's ONNX Runtime is built for

(or "no libonnxruntime.so beside greycard (looked in A and B); a
build from source makes it with scripts/ort-dylib.sh",
"GREYCARD_ORT_DYLIB names PATH, which is not there", "ONNX Runtime at
PATH would not load: ..."). The editor warns it once at startup,
before any of its modes branch, so the log, the terminal and a
headless `--export` all carry it. `runtime::open` answers
`Error::Unavailable` with it before any provider is tried. The
provider-outcome record (§97) is left alone: it is about what a
provider did with a model file, nothing was tried, and every path in
the editor asks `status` before it ever reaches `open`, so a line kept
there would almost never be written; the log and the caller have it.
In the window,
every model offer goes through `offer_model` (`offered` says whether
the sheet opened, for the Whole person's edge offer), which instead puts the
line in the status line and takes the model as declined for the
session (a People part waiting on the model is let go), since a
download would only fill the disk; with the model already in the
store, the mask, the People run, the fill and the denoiser each check
first and fail with the line, so it reaches the status line as
"mask: ...", "People mask: ...", the develop line's "the learned one
failed (...)" and the fill's. A raster already in the disk cache is
still had without the runtime. The CLI's `--ai-denoise` stops with
the line before decoding anything, once the tier or path it names is
known to be one (a mistyped tier is still named as such), and
`greycard models` says it on stderr and still lists and fetches, to
carry the files elsewhere.

## The way out

Loaded rather than linked, the runtime's teardown moved. `ort`
releases its environment from an exit handler (a `.fini_array` entry
on Linux, `__cxa_atexit` on a Mac), the release destroys the WebGPU
provider's Dawn instance, and Dawn's Vulkan instance's destructor
calls through libvulkan into a driver the loader has by then let go
of: one run in 29 of `greycard -v develop <CR3> --ai-denoise fast`
wrote its PNG and died with SIGSEGV, exit 139, in `_dl_fini`, where
master's linked build ran 23 of 23 clean. The headless `--export` had
met the same crash with the linked build and already ended by `_exit`
(`headless::leave`). The fix we wanted was to release the environment
ourselves before `main` returns, but `ort` 2.0.0-rc.13 gives no way
to: the environment is a private static, and a `ReleaseEnv` of our
own through the C API would be released a second time by that
handler. So `runtime::leave` ends the process by `_exit`, after
flushing stdout, stderr and the C streams, whenever a model was
opened, and the command line's `main` now ends through it (an error
said as `main` returning it would say it); the window's quit takes the
same way out once a model has been opened, as it already did with a
worker still in a job. No exit handler runs; the kernel takes back
the driver's state. On Windows, where Dawn is on D3D12 and the crash
has not been seen, the process returns as before. After the change,
100 runs of the command line's learned denoise and 100 of the
editor's headless export of an edit asking for it, each with a fresh
cache so the model loads and runs every time: 100 of 100 exited 0 for
each, none 139.

`greycard-bench` still returns from `main` as before and so keeps the
rare crash on its way out after a learned denoise; it is a tool of
ours, not shipped, and a crash after its numbers are printed costs a
re-run at most.

The quick way out had a cost the window's quit found. Its index
connections close during the quit's `stop`, but a reader on a thread
of its own (a folder's look, the tree's counts) can still hold a
connection to the library for some milliseconds, and the last
connection to close is the one that folds SQLite's write-ahead log
into the file and removes it with its index. Ended by `_exit`, the
kernel closed that reader's for it, with no fold: in six of six quits
with a model opened the `-wal` and `-shm` were left beside
`library.sqlite`, where the return path, slower to end, had always
given the reader time. Before it leaves, the quit now opens a
connection of its own, checkpoints with `TRUNCATE` and closes it, and
again every 10 ms until the log is gone, for at most a second. A
reader that still holds a snapshot after that keeps the checkpoint
from folding the frames past it, so the `-wal` and `-shm` are left
and the library's next open recovers them, as after any end that
skipped the last close; the file without them is then not whole (a
copy of it alone fails `integrity_check`), and nothing here pretends
otherwise. 20 of 20 quits with the Subject mask made left nothing
beside the library.

A capture (`--snapshot`) of a frame whose edit has a learned mask,
with the runtime off, waited for ever: a shape the worker answered by
failing stayed among the shapes asked and not answered, so the frame
was never quiet. The failure is kept beside it now, by key, shape and
file, and counts as answered for the capture's wait, while the shape
stays asked so it is not asked again and again; the capture is taken,
with the line in the status bar.

The binaries carry no runpath any more: nothing they load at exec sits
beside them. ONNX Runtime is opened by its full path, and it finds
Dawn through its own runpath, `$ORIGIN` (`@loader_path` on a Mac).

## The library, made from pyke's archive

`scripts/ort-dylib.sh [profile ...]` runs the `ort-dylib` crate, since
unpacking a raw LZMA2 stream and finding MSVC's `link.exe` are not
things a shell does alike on three hosts. It fetches the archive
`ort-sys` would have fetched for `webgpu` (its URL and SHA-256 copied
from ort-sys's `dist.tsv`; a new `ort` means new rows), checks the
hash, unpacks it into `target/ort-dylib/<hash>/`, links one library
exporting `OrtGetApiBase` alone (the one symbol `ort` looks up), and
links it with Dawn into `target/<profile>/`, `deps/` and `examples/`,
the three places `ort`'s `copy-dylibs` put Dawn before (copies on
Windows). A second run with nothing changed does nothing; a stamp
with a recipe version relinks when the recipe changes.

- Linux: `c++ -shared`, the archive pulled in from `-u OrtGetApiBase`,
  a version script that hides everything else, Dawn on
  `-lwebgpu_dawn` with a runpath of `$ORIGIN`, `-z defs` so a missing
  symbol fails the link. pyke's Linux archive is compiled by Clang
  but against libstdc++ (its symbols are the `__cxx11` ABI, none
  libc++'s `__1`), so `-lstdc++` as ort-sys linked it. Its glibc floor
  is the build host's, as the binaries' is: linked here, against glibc
  2.44, it asks for GLIBC_2.43 (libm's float functions, `sinhf`,
  `acosf` and the like, took new versions there); the archive itself
  wants no more than 2.38 (`__isoc23_strtol`). What ships is linked on
  CI's Ubuntu 24.04, whose 2.39 sets the floor of the package, the
  same as before.
- macOS: `c++ -dynamiclib`, `-u`/`-exported_symbol _OrtGetApiBase`,
  install name `@rpath/libonnxruntime.dylib` and `@loader_path` for
  Dawn's own `@rpath` name, libc++, libobjc, Foundation,
  CoreFoundation and CoreML (the archive is the coreml,webgpu one),
  built for 13.4, Dawn's floor and Info.plist's.
- Windows: `link.exe /DLL` found as `cc` finds it, a `.def` exporting
  `OrtGetApiBase` and `/INCLUDE:OrtGetApiBase` as the root, Dawn's
  import library, and the six system libraries the archive's objects
  call (kernel32, advapi32, setupapi, dxgi, dbghelp, pathcch, read off
  its undefined symbols); the C++ runtime comes from the objects' own
  `/DEFAULTLIB` (`/MD`, so the five redistributable DLLs package.sh
  already ships cover it). The toolset floor of 14.51.36231 moves
  from the binaries' link to this one.

Not `--whole-archive`: pyke's Linux archive holds some members twice
(ONNX's `onnx-data.pb.cc.o` among them) and taking every member fails
on duplicate definitions. Rooted at `OrtGetApiBase`, the linker takes
the same members the binaries took when `ort` linked the archive into
them, which is the set already known to work.

CI builds the library into `target/debug` before `cargo test`, so the
tests that open the runtime run as before; the release workflow
builds it into `target/release` after the build and then checks that
neither binary names ONNX Runtime or Dawn as a library to load at
exec: readelf on Linux, otool on macOS, and on Windows the
llvm-readobj rustup ships in `llvm-tools` (Git Bash has no dumpbin),
which must also find kernel32 so an empty answer is not a pass.

The helper's stamp holds a hash of the link's program, arguments and
exports, so a changed flag, `$CXX` or `MACOSX_DEPLOYMENT_TARGET` links
again without bumping a version by hand, and the fetch has a
ten-minute limit. The tests that open the runtime say "skipped, no
ONNX Runtime (run scripts/ort-dylib.sh)" and return where it cannot
be had, or check the line instead, so a fresh clone's `cargo test`
passes before the script has run. libtest keeps a passing test's
stderr to itself, so that skip is silent, and a library that landed
where the tests do not look would leave CI green with the runtime
paths unchecked: `GREYCARD_REQUIRE_ORT`, set in CI's test step as
`GREYCARD_REQUIRE_GPU` is, turns every one of those skips into a
failure (`runtime::absent_in_test`). `package.sh` ships it beside
the binaries as it ships Dawn, lists its libraries, and the macOS
stray-library check covers it; `uninstall.sh` takes it out again
(its list is written by hand and had only Dawn).

## What we measured

On this machine (Ryzen 9 9950X3D, Linux, glibc 2.44), release builds:

| | before | after |
|---|---|---|
| greycard-ui, stripped | 106.2 MB | 79.9 MB |
| greycard (CLI), stripped | 50.1 MB | 23.8 MB |
| libonnxruntime.so | (in each binary) | 26.2 MB |
| libwebgpu_dawn.so | 12.1 MB | 12.1 MB, loaded with the runtime |
| a Linux package's four files | 168.4 MB | 142.0 MB |
| `greycard-ui --help`, exec to exit, median of 30 | 2.2 ms | 1.7 ms |
| headless `--export` of a 24 MP CR3 to 1200 px, median of 5 | 1482 ms | 1417 ms |
| the library's load at the first model | (at exec) | 4 to 14 ms |
| first Subject mask, BiRefNet on WebGPU, open to settled | | 3.74 s |
| first Sky model on the CPU provider, with the load | | 0.12 s |

The start was never slow; what moved is that it no longer runs pyke's
code at all. The runtime once in a shared library rather than twice
in two binaries makes the package 26 MB smaller. The load itself is
4 ms; the first model's seconds are the session build and the
warm-up, as §97 measured. The Sky prior on WebGPU through the loaded
library agrees with the CPU to 4.1e-5 in the class logits and on
every pixel's label (`the_sky_prior_is_the_same_on_the_cpu_and_on_webgpu`).

With the library absent, `readelf -d` on both binaries names neither
ONNX Runtime nor Dawn (nor libstdc++ any more), and `objdump -d`
finds 628 BMI2 instructions in greycard-ui where there were 4,341:
257 in rawler's `multiversion` clones (`..._avx_avx2_bmi1_bmi2_...
_version`) and 371 in ring's assembly, both chosen at run time by
CPUID. None is outside a dispatched path. We had no qemu or Intel SDE
to run it on an Ivy Bridge model, and glibc's hwcaps tunables do not
stop a processor executing BMI2, so the start on such a CPU is shown
by the binary's contents and by the decision's tests, not by a run;
the window started under weston with the library moved away, warned
the line, and put it in the status line when a People part was
chosen.

## Left out

- The `cuda` feature: pyke publishes no archive with both WebGPU and
  CUDA, so `ort-dylib` makes only the WebGPU library; a CUDA build
  names its own library with `GREYCARD_ORT_DYLIB`.
- The thumbnail pool's own log line for an edit whose learned mask is
  not made still says the model is not downloaded when the runtime is
  off; it is an info line, not shown to the user.
- The macOS and Windows link recipes are written from the archives'
  contents (their undefined symbols, their directives, Dawn's install
  name) and checked only by CI's runners, not on a Mac or a Windows
  machine here.
- The CPU-only fallback the roadmap carries, Microsoft's build fetched
  as a model is, rides on `GREYCARD_ORT_DYLIB`'s path through the
  decision: a fetched library put where that names would be taken
  without the processor check.
