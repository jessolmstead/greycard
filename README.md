# greycard

Every raw editor answers two questions. What do the numbers mean:
light in the scene, or brightness on a screen? And where does the
truth about your photographs live: in your folders, or in the
program's database?

greycard answers: light, and your folders. It was built on those two
answers from the first commit, and the rest follows from them. It is
accurate. Its controls feel the way you expect. It is fast. And it
has the tools a working photographer needs.

## What that buys you

**Accurate.** White balance is applied as gains in camera space, and
the camera matrix is interpolated for the light you shot in. From
the decoder to the display transform the image is linear Rec.2020,
nothing clipped and nothing bent to fit a screen until the last
step. A stop of exposure is a stop. Highlights above white are still
there when you go looking for them. The monitor profile comes from
colord and soft proofing is one click. Every operation has a CPU
reference and a test, the GPU is checked against it, and the
viewport and the export agree to a fraction of a percent.

**Built on the best published work.** AMaZE and RCD demosaicing,
deconvolution sharpening and defringe from RawTherapee. Highlight
reconstruction and profiled denoising from darktable. greycard is
GPL-3.0-or-later so that it can use these directly, with the source
and the authors named in each file, instead of reinventing them
worse.

**The controls you know.** Exposure, tone, curves, a color mixer,
grading, masks and presets, in the places you expect them, and
Lightroom's presets load. The difference is underneath: each slider
acts on the light in the scene rather than on a display curve, and
that is what makes an edit predictable. Culling is a keyboard job on
the camera's own JPEG, and ratings, flags and labels travel to
Lightroom and darktable through XMP.

**Fast.** A slider shows in the viewport as it moves, drawn by the
shader on the picture already on the GPU. The full develop runs on a
worker thread once the sliders rest, and the window never waits for
it. Switching frames puts the camera's picture up in about a tenth
of a second on a 45 MP raw, and the develop replaces it when it
lands. Sharpening, chromatic aberration correction and the display
path all run on the GPU.

**What is in it.** Gradient, radial, brush, subject and object
masks, each with its own light, color and tint. Heal, clone and a
learned fill. A learned denoiser trained on a measured noise model,
mosaic in and color out, running on your machine with published
weights, and a profiled denoiser beside it. Lens corrections from
lensfun. Parametric and point curves, 3D LUT looks, a guided
perspective tool, scopes. Focus stacks merged to linear DNG, and a
command line that develops any raw to DNG or TIFF on a machine with
no window.

**Your folders are the truth.** Edits and their history live in a
sidecar beside the file. The library, when it lands, will be an
index that can be thrown away and rebuilt. Move a shoot to another
drive and nothing breaks.

## What it does not do yet

- There is no library. It opens a folder, and G lays that folder out
  whole as a zoomable contact sheet. An index across folders,
  collections and a Lightroom catalog import are designed and next.
- The tone controls are still being tuned: their ranges and response
  curves are being fitted so that a small move does what your hands
  expect. That work gets the same care as the color science under
  it.
- It runs on Linux, on Apple silicon Macs and on Windows. There is
  no Intel Mac build and no Windows-on-ARM one: ort ships no WebGPU
  ONNX Runtime for either.
- Camera support is [rawler](https://github.com/dnglab/dnglab)'s.
  Fixes go upstream, not into a fork.

## Why this could be built now

GPUs do float arithmetic everywhere. A 45 megapixel float image fits
in memory. The scene-referred approach was worked out in public over
ten years by the ACES community and darktable's developers. There is
no installed base of edits that must keep rendering the same. And
the GPL let years of other people's work arrive in days.

What nobody has shipped is both at once: a pipeline that works in
scene light and controls that feel the way a photographer expects.
The open source editors got the first and stopped. The commercial
ones got the second and cannot change now. greycard's bet is that
the two were never in tension, and that a slider on real light feels
better than one on a display curve because the picture responds the
way the scene would. That is what this project is for.

## Where it fits

[docs/where-greycard-fits.md](docs/where-greycard-fits.md) sets it
beside Lightroom, Capture One, DxO, darktable, RawTherapee and
RapidRAW, and says what each got right.
[docs/notes.md](docs/notes.md) is the design record, every decision
numbered. [docs/roadmap.md](docs/roadmap.md) is the plan, by
release, and [docs/changelog.md](docs/changelog.md) is what shipped.

## Installing and running it

**From a release, on a Mac.** The step by step for someone who has
not done this before, security dialog included, is
[docs/testing-on-a-mac.md](docs/testing-on-a-mac.md). In short: download
`greycard-<version>-aarch64-macos.tar.gz` from [the releases
page](https://github.com/jessolmstead/greycard/releases), open
it, and in a terminal run `sh install.sh` inside the folder it
unpacked. That puts `greycard.app` in `~/Applications`, where
Launchpad and Spotlight find it, and links `greycard` and
`greycard-ui` into `~/.local/bin` for the terminal. The app is not
notarized by Apple, so if you drag it to Applications yourself
instead, the first open is refused and you allow it under System
Settings, Privacy & Security, Open Anyway; `install.sh` spares you
that by clearing the download's quarantine mark. It needs macOS 13.4
or newer and an Apple silicon Mac; there is no Intel build.
`uninstall.sh`, in the same folder, takes it all back out.

**From a release, on Linux.** Download
`greycard-<version>-x86_64-linux.tar.gz` from the same page, extract
it, and run `sh install.sh`. It copies the binaries to `~/.local/bin`,
the desktop entry and icon under `~/.local/share`, and refreshes the
desktop database and icon cache. greycard then shows up in your
application launcher. `uninstall.sh`, in the same tarball, takes it
back out.

**From a release, on Windows.** Download
`greycard-<version>-x86_64-windows.zip` from the same page, right-click
it, Extract All, and double-click `install.cmd` inside the folder it
unpacked. That copies greycard to
`%LOCALAPPDATA%\Programs\greycard`, adds a Start menu entry, puts the
command line on your Path, gives `.gcd` sidecars their own icon and
offers greycard in Explorer's Open with for raw files without taking
any default away. Nothing needs administrator rights and nothing is
written outside your own profile. The executable is not signed, so
the first run may show a SmartScreen box: More info, then Run anyway.
`uninstall.cmd`, in the same folder, takes it back out and leaves your
settings and presets alone. If you would rather not install anything,
`bin\greycard-ui.exe` runs where it is.

**From source.** Step by step from a machine with nothing on it,
Rust and the compiler included, is
[docs/building.md](docs/building.md). In short: needs Rust 1.98 or
newer. On Debian or Ubuntu:
`liblcms2-dev libfontconfig1-dev libwayland-dev libxkbcommon-dev
pkg-config` (lcms2 for the color profiles, fontconfig and wayland
for Slint's backend, xkbcommon for its keyboard). On Fedora:
`lcms2-devel fontconfig-devel wayland-devel libxkbcommon-devel
pkgconf-pkg-config`. On Arch: `lcms2 fontconfig wayland libxkbcommon
pkgconf`. On a Mac, Xcode's command line tools (`xcode-select
--install`) and nothing else: lcms2 builds from the source its crate
carries. On Windows, the MSVC build tools, and the toolset has to be
at least 14.51.36231 or the link fails on `__std_*` symbols —
docs/building.md says why and how to check.

```sh
cargo build --release
```

The first build fetches a prebuilt ONNX Runtime, so it needs a
network connection once. It leaves `target/release/greycard-ui` and
`target/release/greycard`, with `target/release/libwebgpu_dawn.so`
(`.dylib` on a Mac, `webgpu_dawn.dll` plus `dxcompiler.dll` and
`dxil.dll` on Windows) beside them; the binaries find those through
an rpath, or beside themselves on Windows, so keep them together if
you move them. `scripts/package.sh` rolls them into the release
package for the machine it runs on: a tarball on Linux, an app
bundle in one on a Mac, a zip with install.cmd in it on Windows.

Or run either straight from the workspace, for development:

```sh
cargo run -p greycard-ui --release -- ~/Pictures/shoot
cargo run -p greycard-cli --release -- develop photo.CR3 --dng photo.dng --preview photo.png
```

The editor is a Slint window with a wgpu viewport. The command line
develops to linear DNG for other editors, 16-bit TIFF, or a preview.
`greycard-core` is the engine as a library, with no interface
toolkit, tone-mapping choice or edit schema in it. `greycard-bench`
scores every demosaic and denoiser against the Kodak and McMaster
sets; fetch them once with `scripts/fetch-bench-data.sh`.

**What it needs.** A GPU with Vulkan on Linux, or Metal on a Mac:
the viewport is wgpu through Slint, and there is no software renderer
for it. The learned models (the denoiser, the subject and object
masks, the fill) run on the same GPU through ONNX Runtime and Dawn,
and fall back to the CPU, slower, if it is not there. Wayland or X11
on Linux.

Lens corrections want the lensfun database; if your distribution
already has it installed, that copy is used. Otherwise the LENS panel
offers to fetch it (under half a megabyte, under its own license) into
`~/.cache/greycard/lensfun`, and `greycard lenses --fetch` does the
same from a terminal. First use of a model, Subject or Objects for
masking, the fill, or the learned denoiser, downloads it after showing
its license, into `~/.cache/greycard/models`: the denoiser is 5 to
20 MB depending on its tier, the subject mask about 115 MB, the
object mask about 184 MB, and the fill about 208 MB. `greycard models`
lists them and `greycard models --fetch <tier|id|all>` gets them
without the window, for a headless machine. The browser's thumbnails
are kept in `~/.cache/greycard/thumbs`, by what is in each file rather
than where it is, up to 300 MB by default (Settings sets the cap and
clears it). On a Mac these caches are under
`~/Library/Caches/greycard` instead.

Settings live in `~/.config/greycard/settings.json`, or
`~/Library/Application Support/greycard/settings.json` on a Mac.
Edits live in a `.gcd` sidecar beside each raw. The monitor profile
comes from colord on Linux, or a file chosen in the panel's MONITOR
section; on a Mac it is the file, and the chooser starts in
`~/Library/ColorSync/Profiles`.

## Reporting a bug

In the editor, *Report a problem...* at the foot of the left panel
opens the bug report form in your browser with the version, the
system and the graphics card filled in, and opens the log's folder
beside it so the file can be dragged into the form.

The editor keeps a log, `greycard-ui.log`: under
`~/Library/Logs/greycard` on a Mac, `~/.local/state/greycard` on
Linux, `%LOCALAPPDATA%\greycard\logs` on Windows, with the previous
run beside it as `greycard-ui.log.1`. It says which build and machine,
which graphics card, each file opened and what it was, how long each
step took, and whatever went wrong, a crash's backtrace included.
Attach it. The terminal shows warnings only; `-v` shows what the log
keeps, `-vv` more, and `RUST_LOG` gives exact control. The command
line, `greycard`, has the same `-v` and no file: paste what it said.
Say which build you are on: `greycard --version` for the command
line, `greycard-ui --version` for the editor; the log's first line
says too. The most useful thing you can send is a raw file that
renders wrong, along with the camera model; it is developed against
Canon and Fujifilm GFX bodies. [Open an
issue](https://github.com/jessolmstead/greycard/issues/new/choose)
and pick the bug report form.

GPL-3.0-or-later. Ported code says where it came from and who wrote
it, in the file header.
