# 108. The first macOS build (2026-09-20)

A checkout on a Mac, and `cargo build` stopped inside `zbus` before it
reached anything of ours. Two things §106 had reasoned about with no
machine to try them on.

**zbus was borrowing a feature.** greycard-ui asks for zbus with
`default-features = false, features = ["blocking-api"]`, and zbus
refuses to compile unless `async-io` or `tokio` is behind it. It built
on Linux because Slint's tray support pulls `ksni`, which pulls zbus
with its defaults, and Cargo unifies one crate's features across the
graph: our dependency was being completed by somebody else's. macOS
has no tray crate, nothing supplies `async-io`, and zbus stops on its
own `compile_error!`. §106's "zbus compiles on both platforms" was
read off that unified build. The feature list now names `async-io`
itself, which changes nothing on Linux and is what the dependency
needed all along. Gating zbus and the portal and colord calls behind
`cfg(target_os = "linux")` instead is more code for the same result
while the calls degrade as they were meant to; it waits for `rfd`,
which wants that split anyway.

**Two of four build scripts.** §106 said both build scripts passed
`-Wl,-rpath,$ORIGIN`; there are four. greycard-ui's and greycard-ai's
learned `@executable_path`, greycard-cli's and greycard-bench's did
not, so `greycard info` died under dyld at startup: "Library not
loaded: @rpath/libwebgpu_dawn.dylib ... tried '$ORIGIN/...'". All four
now read `CARGO_CFG_TARGET_OS` the same way.

**What the machine then said.** Dawn's dylib does carry an `@rpath/`
install name, so the rpath is the whole fix and `install_name_tool` at
package time is not needed — §106's first thing to check on the Mac,
answered. colord fails on the missing session bus, says so, and the
output falls back to sRGB, which is the degrading that was planned.
The window comes up on Metal, and a 102 MP GFX 100S II RAF decodes,
develops and snapshots: 8.3 s of base develop on an M5, 10.5 s for the
whole run, launch and decode and PNG in. One frame at one size is not
the §90 table, which wants 24 MP, but it is the first number off an
Apple machine and it is in the region the table estimates. The file
dialogs are still the portal's, so Open Folder refuses there.
