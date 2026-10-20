# Building greycard from source

From a machine with nothing on it to a running editor. Linux, an
Apple silicon Mac, or Windows. Half an hour the first time, most
of it the compiler working.

If you only want to run it, there are tarballs on the [releases
page](https://github.com/jessolmstead/greycard/releases) and
[testing-on-a-mac.md](testing-on-a-mac.md) walks a Mac through
one. Build from source to change it, to run it on a distribution the
tarball does not suit, or to get a fix before it is released.

## What the machine needs

- **A GPU.** Vulkan on Linux and Windows, Metal on a Mac. The
  viewport is wgpu through Slint and there is no software renderer
  behind it.
- **Disk.** A debug and a release build of the workspace together
  come to around ten gigabytes under `target/`.
- **Network, once.** The build fetches around 700 crates, and
  `scripts/ort-dylib.sh` a prebuilt ONNX Runtime. After that it
  builds offline.

## 1. A compiler and the libraries

### Debian, Ubuntu

```sh
sudo apt-get install build-essential curl git pkg-config \
    liblcms2-dev libfontconfig1-dev libwayland-dev libxkbcommon-dev \
    mesa-vulkan-drivers
```

### Fedora

```sh
sudo dnf install @development-tools curl git pkgconf-pkg-config \
    lcms2-devel fontconfig-devel wayland-devel libxkbcommon-devel \
    mesa-vulkan-drivers
```

### Arch

```sh
sudo pacman -S base-devel curl git pkgconf \
    lcms2 fontconfig wayland libxkbcommon
```

Arch installs the Vulkan driver with the graphics stack: `vulkan-radeon`
for AMD, `vulkan-intel` for Intel, `nvidia-utils` for NVIDIA.

lcms2 does the color profiles, fontconfig and wayland are Slint's
backend, xkbcommon is its keyboard. If you run X11 only, the wayland
package is still wanted at build time.

To check the GPU side before you spend the compile: install
`vulkan-tools` and run `vulkaninfo --summary`. It should name your
card. If it names nothing, install the driver for it first.

### macOS

Xcode's command line tools, and nothing else:

```sh
xcode-select --install
```

Full Xcode is not needed. lcms2 builds from the source its crate
carries, so there is no Homebrew step. macOS 13.4 or newer, and an
Apple silicon Mac — there is no Intel build.

### Windows

The MSVC build tools, and a new enough toolset:

```powershell
winget install Microsoft.VisualStudio.BuildTools ^
  --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

The toolset version matters. `scripts/ort-dylib.sh` (step 4) links
`onnxruntime.dll` out of pyke's prebuilt static `onnxruntime.lib`,
and a toolset older than the one that library was compiled with fails
at the very last step — `link.exe` reporting unresolved `__std_*`
symbols, which are MSVC STL vector algorithms renamed between
toolsets. As of `ort 2.0.0-rc.13` the floor is **14.51.36231**, which
is the VS 2026 build tools; VS 2022 17.14's 14.44.35207 is too old,
and the newest toolset installed is the one used. To read the floor
out of the library itself, under `target\ort-dylib\<hash>\src\`:

```sh
grep -ao "VC.Tools.MSVC.14\.[0-9.]*" onnxruntime.lib | sort -u
```

lcms2 builds from the source its crate carries, so there is no
vcpkg step, and nothing needs pkg-config. Windows 10 or newer.
The script puts `onnxruntime.dll`, `webgpu_dawn.dll`, `dxcompiler.dll`
and `dxil.dll` into `target/<profile>/` for you, copied rather than
linked; the loader looks beside the executable.

## 2. Rust

greycard needs Rust 1.98 or newer, which is stable. Install
[rustup](https://rustup.rs):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Take the default answers. Open a new terminal afterwards so
`~/.cargo/bin` is on your `PATH`, then check:

```sh
rustc --version
```

If your distribution's Rust is older than 1.98, use rustup's rather
than fighting it. If you already have rustup and it is old,
`rustup update`.

## 3. The source

```sh
git clone https://github.com/jessolmstead/greycard.git
cd greycard
```

Git is needed for the build too, not just the clone: three
dependencies are pinned to git revisions until releases carry what
they add. rawler's carries a white-level fix, the Nikon High
Efficiency decoder below, and a fix of ours to that decoder's black
level; femtovg's carries a rendering fix; Slint's carries a fix to
the window's size under a scale of its own.

## 4. Build

```sh
cargo build --release
scripts/ort-dylib.sh
```

On Windows the script runs from Git Bash; from PowerShell or `cmd`,
`cargo run --locked -p ort-dylib -- debug release` does the same.

The first build takes ten to twenty minutes and is mostly quiet.
Afterwards a rebuild of your own change is seconds.

The binaries do not link ONNX Runtime, the library the learned
masks, the denoiser and the fill run on: they load it the first time
a model is wanted, so a processor it cannot run on still starts the
editor. `scripts/ort-dylib.sh` makes that library, once, out of the
static build `ort`'s authors publish: it fetches the archive into
`target/ort-dylib/`, checks its hash, links `libonnxruntime.so`
(`.dylib` on a Mac, `onnxruntime.dll` on Windows) with your C++
toolchain, and puts it and Dawn, the GPU backend the models run on,
into `target/debug/` and `target/release/` (name a profile to do just
that one). Run it again after `cargo clean`; with nothing changed it
does nothing. A build without it runs: the editor, the command line
and the tests say "the AI features are off: no libonnxruntime.so
beside greycard …" and go on without them. The x86-64 library needs
AVX2, BMI2 and the rest of x86-64-v3 (Intel from Haswell, AMD from
Zen); on an older processor the AI features are off the same way.
`GREYCARD_ORT_DYLIB` names another ONNX Runtime library to load in its
place, taken without that check.

On Windows on ARM the script refuses: the one prebuilt archive for
that target carries DirectML, which greycard does not use, and none
carries WebGPU. The editor offers to download Microsoft's CPU-only
runtime instead, and `greycard models --fetch onnxruntime` fetches
it. A tester's Windows on ARM package can come from any branch before
a release carries it: run the "Windows on ARM package" workflow by
hand from the repository's Actions tab (or `gh workflow run
arm-package.yml --ref <branch>`; GitHub lists a workflow run by hand
once its file is on master), and the zip is that run's
`package-aarch64-windows` artifact for seven days, inside the zip
GitHub wraps every artifact in.

Nikon's High Efficiency raws (HE and HE*, written by newer Z bodies
such as the Z9, Z8, Z6 III, Zf and Z50 II) are JPEG XS inside, and
their decoder is a build feature:
`cargo build --release -p greycard-ui -p greycard-cli --features
nikon-he`. It is off in the published builds because JPEG XS is
licensed by its patent holders per copy of a decoder.

Build the debug profile instead if you are going to attach a
debugger, but do not develop photographs with it: the pipeline is
tens of times slower unoptimized.

That leaves four files in `target/release/`:

```
greycard-ui             the editor
greycard                the command line
libonnxruntime.so       the runtime the models run on
libwebgpu_dawn.so       the GPU backend it runs them on
```

The two libraries (`.dylib` on a Mac) are symlinks into
`target/ort-dylib/`. The binaries look for ONNX Runtime beside
themselves, and it finds Dawn beside itself, so if you copy them
anywhere, copy both libraries too — dereferenced.

## 5. Run it

```sh
./target/release/greycard-ui ~/Pictures/shoot
./target/release/greycard develop photo.CR3 --dng photo.dng
```

Or from the workspace, which is what you want while changing things:

```sh
cargo run -p greycard-ui --release -- ~/Pictures/shoot
cargo run -p greycard-cli --release -- develop photo.CR3 --preview out.png
```

The first time you use the learned denoiser, a Subject or Object
mask, or the fill, it downloads the model after showing its license,
into `~/.cache/greycard/models` (`~/Library/Caches/greycard/models`
on a Mac). `greycard models --fetch all` does it from a terminal.
Lens corrections use your distribution's lensfun database if it is
installed, and `greycard lenses --fetch` gets one if it is not.

## Installing it on the machine

`scripts/package.sh` rolls what you just built into the same tarball
a release ships, for the platform you are on:

```sh
cargo build --release -p greycard-ui -p greycard-cli
scripts/ort-dylib.sh release
scripts/package.sh
```

It writes `target/dist/greycard-<version>-<arch>-<os>.tar.gz` and
leaves the unpacked staging directory beside it. Running `sh
install.sh` in that directory puts the binaries in `~/.local/bin`,
the desktop entry and icon under `~/.local/share` on Linux, or
`greycard.app` in `~/Applications` on a Mac. `sh uninstall.sh` takes
it back out.

On x86_64 Linux, `scripts/appimage.sh` then turns that staging
directory into `target/dist/greycard-<version>-linux-x86_64.AppImage`,
the release's fourth file. It fetches appimagetool and the AppImage
runtime, pinned and checked, into `target/appimagetool/`, and needs
`curl` and `readelf`. Build with `LCMS2_STATIC=1` as the release does,
or it stops on the `liblcms2.so.2` the binaries would ask the host for;
lcms2-sys does not rebuild when that changes, so on a target that has
built it already, run `cargo clean --release -p lcms2-sys` first. The
AppImage runs the editor, and `--cli` as its first argument runs the
command line instead.

## Before you send a change

The same three the CI runs, all clean:

```sh
cargo test
cargo clippy --all-targets
cargo fmt
```

Run `scripts/ort-dylib.sh debug` before the tests: without it the
tests that open ONNX Runtime return early and pass, with the reason
kept where `cargo test` keeps a passing test's output (`-- --nocapture`
shows it). `GREYCARD_REQUIRE_ORT=1`, which the CI sets, makes each of
them fail instead.

`hooks/pre-push` runs the first two for you on a push that touches
code, and lets a push of docs alone through at once. The tests it
leaves to you and to the CI, which runs them on three platforms.
Turn it on once:

```sh
git config core.hooksPath hooks
```

Some tests are `#[ignore]`d because they want files this repo does
not carry. `cargo test -- --ignored` runs them, but point
`GREYCARD_MODELS` at a model store and `GREYCARD_SAMPLES` at a
directory of raws first — without those variables they pass
instantly and prove nothing.

The demosaic and denoiser benchmarks want the Kodak and McMaster
reference sets. `scripts/fetch-bench-data.sh` gets them once (it
needs `curl` and `unzip`), then:

```sh
cargo run -p greycard-bench --release -- data/bench/kodak data/bench/mcm
```

A change to a reader of outside files (the sidecar, a preset, the
Lightroom and RawTherapee imports, XMP, or the filter language) can
be fuzzed before it goes out. The targets are in `fuzz/`, a workspace
of their own on a nightly toolchain, and the CI runs them weekly;
[fuzz/README.md](../fuzz/README.md) has how to run one and how to
turn what it finds into a test.

## When it goes wrong

**`linker 'cc' not found`** — the compiler from step 1 is missing.
On a Mac, `xcode-select --install`.

**`Could not find system library 'lcms2'`, or pkg-config errors** —
the `-dev`/`-devel` packages from step 1, and `pkg-config` itself.

**`package requires rustc 1.98`** — `rustup update`, or your
distribution's Rust is in front of rustup's on `PATH`.

**The editor starts and exits, or says no adapter** — no usable
Vulkan driver. `vulkaninfo --summary` from `vulkan-tools` says
whether the system has one at all. Inside a VM or over plain SSH
there usually is not.

**"the AI features are off: no libonnxruntime.so beside greycard"**
— `scripts/ort-dylib.sh` has not run for this profile, or
`target/ort-dylib/` was cleared under its links. Run it again.

**"ONNX Runtime at … would not load: …"** — the reason follows. A
missing `libwebgpu_dawn.so` is the same cure as above; a `GLIBC_2.xx`
not found means the library was linked on a newer system than the one
running it, which a library made on the same machine never is.

**The build is killed partway through** — it ran out of memory.
`cargo build --release -j 2` builds fewer crates at once.

Anything else: the editor writes a log, `greycard-ui.log`, under
`~/.local/state/greycard` on Linux or `~/Library/Logs/greycard` on a
Mac. It says which build and machine it is, which graphics card, and
what went wrong. Attach it to an
[issue](https://github.com/jessolmstead/greycard/issues/new/choose).

## Keeping up

```sh
git pull
cargo build --release
```

Dependency versions are pinned in the committed `Cargo.lock`. Add
`--locked` to build exactly what a release builds; leave it off and
cargo may update a dependency within its semver range.
