# 106. The tree stops assuming Linux (2026-09-19)

Asked what stood between the tree and a macOS or Windows build to hand
a tester, the answer was three Linux assumptions and no attempt yet.
Fixed here, ahead of the first native build on either machine; the
roadmap's line for the builds stays open until one has run.

**The linker flags.** Both build scripts passed `-Wl,-rpath,$ORIGIN`
unconditionally so the binaries find Dawn beside themselves. MSVC's
`link.exe` has no `-Wl`, and `$ORIGIN` means nothing to macOS's
loader. Each now reads `CARGO_CFG_TARGET_OS`: Windows gets no flag,
since a DLL beside the executable is found first anyway; macOS gets
`@executable_path`; everything else keeps `$ORIGIN`. Whether Dawn's
dylib carries an `@rpath/` install name is the thing to check first
on the Mac (`otool -L target/release/greycard-ui`); if it does not,
`install_name_tool` at package time, not a code change.

**The directories.** Settings, presets, the model store and the
lensfun store each fell back from `XDG_CONFIG_HOME` or
`XDG_CACHE_HOME` to `$HOME/.config` or `$HOME/.cache`. Windows sets
no `HOME`, so settings would have quietly not saved and the two stores
would have refused to fetch. The `dirs` crate (a workspace dependency,
7.0) now supplies the fallback: `~/.config` and `~/.cache` on Linux
as before, `~/Library/Application Support` and `~/Library/Caches` on
macOS, `%APPDATA%` and `%LOCALAPPDATA%` on Windows. The XDG variables
still win when set, on every platform, because the per-agent editor
runs and the tests rely on that. The dialogs' start folders use
`dirs::home_dir` for the same reason, and the ICC chooser starts in
each platform's profile folder (`~/Library/ColorSync/Profiles`,
`System32\spool\drivers\color`).

**The package script.** It named `libwebgpu_dawn.so` and
`x86_64-linux` outright. It now reads the host triple from `rustc
-vV`: the library is `.so`, `.dylib` or `webgpu_dawn.dll`, the
binaries get `.exe` on Windows, and the tarball is
`greycard-<version>-<arch>-<os>`. The Linux tarball is byte for byte
what it was. `install.sh` treats a dylib or dll like a `.so`. An app
bundle and a Windows installer are packaging still to do, on top of
the tarball; the script only makes sure the same tarball can be rolled
on any host to hand a tester.

**What was found to need nothing.** ort's prebuilt table has
`x86_64-pc-windows-msvc+webgpu` and `aarch64-apple-darwin+coreml,webgpu`;
its resolver takes the closest superset, so the default `webgpu`
feature lands on the Apple bundle that carries CoreML too, and the
roadmap's `Provider::CoreMl` becomes a feature flag. Intel Macs and
Windows on ARM have no WebGPU build. lcms2-sys builds its vendored C
when pkg-config finds nothing, so only a C compiler is needed. zbus
compiles on both platforms; with no session bus, colord's failure
already yields no display profile, an export falls back to the path
beside the raw, and Open Folder says "the desktop offered no file
chooser". Native dialogs (the `rfd` crate) are the fix for that, and
wait on seeing the window come up at all.
