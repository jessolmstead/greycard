# 137. The Windows package (2026-09-22)

§106 took the Linux assumptions out of the tree ahead of a Windows
build nobody had run. One has now run, and the roadmap's line for it
closes: `cargo fmt --check` clean, `cargo clippy --all-targets` with
no warnings, and all 697 tests pass with 14 ignored, on
x86_64-pc-windows-msvc with rustc 1.98.1. Everything §106 guessed at
held. The rest of this is what it took to turn that build into
something a tester can download.

**What the first run said.** The log §112 built is there on Windows
too, at `%LOCALAPPDATA%\greycard\logs`, and the first run's file is
the best record of the port: the viewport came up on an RTX 5070 Ti
through Vulkan, greycard-gpu took the same adapter, and a 45 MP Canon
R5 II developed in 1.66 s with the CA correction and the sharpen both
on the GPU — the same shape as the numbers on the other two
platforms, so nothing fell back to a CPU path unnoticed. Two lines
in the whole run were above info, and they are one known thing:
BiRefNet's decoder asking for seventeen storage buffers where Dawn
allows sixteen, which is already the v0.3.0 item, reproduced here on
NVIDIA exactly as it reproduces elsewhere. A port whose only
complaint is a bug you had already written down is a port that
worked.

**A zip, not a tarball.** The other two platforms get `.tar.gz`.
Windows 11's Explorer can open one and Windows 10's cannot, and
`tar` on the PATH is no help to someone who has never opened a
terminal — which is the whole population this package is for. So the
Windows archive is a `.zip`, written by `scripts/zip.ps1`, and it
takes the same care the tarball takes and for the same reason:
entries in sorted order, forward slashes, and the commit's date on
every one, so two builds of a commit are byte for byte the same.
`Compress-Archive` is not used — it writes in enumeration order and
stamps each entry with the file's own mtime. Two traps were worth
the finding. A `ZipArchiveEntry` in Create mode refuses a timestamp
once its stream has been written to, so `CreateEntryFromFile`, which
does both in one call, cannot be used either; the entry is created,
stamped, and only then opened. And `Sort-Object -CaseSensitive` is
still culture-aware, which would order `greycard-ui.exe` against
`greycard.exe` differently on a runner with another locale; the sort
is `[StringComparer]::Ordinal`, which is what `LC_ALL=C sort` means
on the other side.

**Three libraries, not one.** `package.sh` shipped Dawn alone,
because on Linux and macOS Dawn is all there is. On Windows
`webgpu_dawn.dll` names `dxcompiler.dll` and `dxil.dll` in its own
strings and loads them by name to compile shaders for the D3D12
backend, and ort's prebuilt bundle puts all three in
`target/release`. Shipping one of three would have made a package
whose ONNX Runtime fails on the machine it was unpacked on and
nowhere earlier. The script now carries a list rather than a name.

**The Visual C++ runtime, and why the build machine could never have
shown it.** `ldd` over the staged binaries — the check `package.sh`
has always printed into the build log for exactly this, never before
with anything to say — named `msvcp140.dll`, `msvcp140_1.dll`,
`msvcp140_atomic_wait.dll`, `vcruntime140.dll` and
`vcruntime140_1.dll`. Windows ships the UCRT and does not ship these:
they come with a Visual C++ redistributable, which every machine that
has ever built anything already has, and which a photographer's
laptop may well not. The failure is a "VCRUNTIME140.dll was not
found" box with no other clue, and no machine capable of producing
the package can reproduce it. The five are what the two binaries and
Dawn import between them and they are closed under their own imports;
they now go into `bin/` beside the binaries, app-local, which is a
deployment Microsoft's redistributable license allows. They are
copied out of the toolset's own `Redist` directory rather than out of
`System32`, which holds the installed copy and is not ours to hand
on, and the directory is found through `vswhere`. A package that
cannot find it fails rather than quietly going out short.

**The icon has to exist before the link.** Linux reads the SVG
directly and the Mac tarball folds it into an `.icns` at package time
with `sips` and `iconutil`. Windows cannot do either: there is no
`.desktop` entry to carry an icon, so Explorer, the taskbar, the
Start menu and the properties sheet all read it out of the
executable's own resource table, which means it must be an `.ico` at
link time and not at package time. Rasterizing in `build.rs` would
put resvg and its tree into every platform's build of greycard-ui for
something only one platform uses. So `assets/icon/greycard.ico` and
`assets/icon/application-x-greycard-edit.ico` are committed, and
`tools/icon` — a crate detached from the workspace, so `cargo test`
at the root never builds it — regenerates them from the SVGs when
those change. The SVG stays the one source on all three platforms;
only the step differs. `winresource` compiles the resource, and is a
build dependency under `cfg(windows)` only, since it drives the SDK's
`rc.exe`; the version fields go in with it, so the properties sheet
says greycard 0.1.0 and GPL-3.0-or-later. Both binaries get it, so
the two sitting beside each other in `bin\` do not show one icon and
one blank page.

**install.cmd, and a Path read through the registry.** `install.sh`
is no use to Windows, so the archive carries `install.cmd` and
`uninstall.cmd`, each a few lines that run `install.ps1` with
`-ExecutionPolicy Bypass` — a `.cmd` is what Explorer will run on a
double-click and a `.ps1` is not, and the bypass applies to the one
run and changes no setting. Everything lands in
`%LOCALAPPDATA%\Programs\greycard`, one directory, because the
binaries look for `webgpu_dawn.dll` beside themselves and nowhere
else. There is a Start menu shortcut, and the install directory goes
onto the user's Path. That last one is the part with a trap in it: a
user Path is a `REG_EXPAND_SZ` that may hold
`%USERPROFILE%\.dotnet\tools`, `[Environment]::GetEnvironmentVariable`
expands such a thing before handing it back, and writing the expanded
string back is how a Path loses its variables permanently. So it is
read and written through `Microsoft.Win32.Registry` with
`DoNotExpandEnvironmentNames` and its own value kind kept. Tested on
a Path that had exactly that entry in it: installed, and it survived;
uninstalled, and the Path came back the same but for the trailing
empty entry that splitting drops.

**register.cmd: one type taken, one offered.** §120 paid the desktop
cost of the sidecar on Linux and left the Windows half for whenever
packaging was exercised. Everything goes under
`HKCU\Software\Classes`, so there is no elevation prompt and nothing
is written for another account. Two types: `greycard.gcd` takes the
`.gcd` extension outright, since nothing else claims it, and gets the
sidecar's own icon — the same two-cards-on-a-document mark Linux uses
— so a folder of frames does not read as a folder of application
tiles. `greycard.photo` is offered and not taken: it joins each raw
extension's `OpenWithProgids`, which puts greycard in the Open with
list and leaves whatever already opens a `.cr3` alone. Taking a
default is not a script's to do on Windows 8 and after — the user's
choice is recorded where a script is not allowed to write — and
offering is the honest version of what the roadmap asked for.
`unregister.cmd` removes values rather than keys on the
`OpenWithProgids` side, since another application's offer for the
same extension lives in the same key. The whole round trip was run on
a real machine: installed, every entry checked, uninstalled, and the
`.cr3` key left holding the AppX offer it had before.

**The third runner.** The release workflow's matrix gains
windows-2025, its package a `.zip`, and the artifact and release
globs take both extensions. The one thing that runner has to have is
an MSVC toolset at least as new as the one ort's prebuilt
`onnxruntime.lib` was compiled with — 14.51.36231 for ort
2.0.0-rc.13 — because an older one fails at link with unresolved
`__std_*` symbols out of the MSVC STL and at no earlier point.
windows-2025 took Visual Studio 2026 in June 2026, so it should have
it, and a step prints the installed toolsets and redistributable
directories before the build so a link failure is read off the log
rather than guessed at; `windows-2025-vs2026` is the label to pin if
that image ever goes backwards. This is the one part of the work not
verified by running it, and the first tag is what verifies it.

**No console behind the window.** Launched from the Start menu the
editor opened a black console box beside itself, because a Rust binary
is a console-subsystem one unless it says otherwise and Windows gives
every such binary a console whether it wants one or not. `greycard-ui`
now carries `#![cfg_attr(windows, windows_subsystem = "windows")]`;
checked in the PE header, which reads GUI for the editor and console
for `greycard`, and by the absence of a conhost child at run time. The
log is untouched by this, being a file sink rather than the terminal,
and a run through the installed copy wrote its lines as before. What
is given up is small and worth naming: a terminal that launches the
editor no longer waits for it, so `greycard-ui --version` arrives
after the prompt comes back. The command line is where terminal work
belongs and it stays a console binary.

**What a real shoot said.** The port was exercised against 567 raws in
one folder — 398 Canon CR3, 139 DNG, 30 Fujifilm RAF — every preview
decoded, every orientation read, `Rotate270` included, in 26 s. The
CLI developed a CR3 and a DNG to linear DNG, 16-bit TIFF and a PNG
preview, all correct. The editor opened the folder, developed, wrote
a sidecar and exported a JPEG at 4000x6000 upright from a Rotate270
source with its EXIF carried, and said nothing above info for the
whole session. A `.gcd` double-clicked launched the installed copy and
mapped the sidecar to its raw; the Open with command opened a file
whose path held spaces, parentheses and an ampersand. The learned
subject model ran and masked correctly, on the CPU, falling back from
WebGPU for the storage-buffer limit that is already the v0.3.0 item.

**Two things the platform decides, not us.** Open Folder is
`pick_folder`, which on Windows is `IFileOpenDialog` with
`FOS_PICKFOLDERS`, and that flag hides files: a folder of raws looks
empty while you are standing in it, where the portal on Linux greys
them out instead. It is what every Select Folder dialog on Windows
does and it is left alone. And the monitor's own profile is not read:
`colord_monitors` returns nothing off Linux, so "System" means sRGB
until WCS and ColorSync are read, which a wide-gamut monitor makes
wrong and which is now a v0.6.0 item rather than a comment in
`display.rs`.

**Not done.** An MSI, with Add/Remove Programs and an uninstall entry
the control panel knows about: nicer than a zip with a script in it,
and not different in kind, the same call §110 made about a `.dmg`.
Signing: an unsigned executable gets a SmartScreen "Windows protected
your PC" box on first run, which a certificate answers and which
costs money, and is the same wait as notarization. CI does not build
on Windows — only the release workflow does — so a change that breaks
the Windows build will not be caught until a tag; adding the third
matrix line to `ci.yml` is cheap and was left out of this change
rather than bundled into it. And `assets/greycard.desktop` lists no
`image/x-olympus-orf` although the browser has read `.orf` since it
had a list, which is Linux's half of the same oversight and a
one-line fix.
