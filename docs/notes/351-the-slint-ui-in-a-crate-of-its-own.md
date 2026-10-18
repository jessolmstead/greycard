# §351. The Slint UI in a crate of its own (2026-10-18)

The third pass on the window's structure, after the CLI split
(§341) and the State grouping (§344). The roadmap's plan was to
carve the library views into a crate of their own and halve the
window crate's compile. We measured before carving, and the
measurement changed the cut.

## What the measurement said

greycard-ui is 163,000 hand-written lines and 1,392 tests. Its one
rustc also compiles 392,000 lines of Rust generated from the 17,400
lines of `.slint` under its `ui/`, six times the hand-written code
by bytes. From clean at `-j 4` that was 96 to 115 s and 9 to
11 GB for rustc, 146 s for the test build with the binary and the
test harness each compiling the generated code at once, and 25 s
for an incremental rebuild after one touched `.rs` file.

The library side as the roadmap drew it is 44,000 lines, a quarter
of the crate and about seven seconds of that build. Of its 419
tests, 257 build the whole window and cannot leave it. And a views
crate cannot compile its own `.slint`: Slint generates one component
tree a crate, and the grid and strip are children of the window.
The weight is the generated code, and the cut that moves it is the
Slint UI itself.

## The crate

`crates/greycard-slint` holds `ui/`, moved whole, the build script
that compiles it (the `fluent-dark` style, and element names kept in
debug builds so the testing backend finds controls), and a library
of `slint::include_modules!()` plus three constants the window's
tests read: the text of `app.slint` for the key-table test, the
text of `panel/mask.slint` for the menu tables, and the panels'
folder. The window does `pub(crate) use greycard_slint::*;` where
`include_modules!()` was, and its build script keeps only the
Windows resource. No `impl` in the window was on a generated type,
so nothing else changed; every `app.set_*` call stands as written.

The crate has `test = false`: a test build of it would compile the
generated code a second time for an empty binary in every workspace
test run, and we saw it do so before the flag went in. Its `slint`
dependency carries greycard-ui's whole feature list, not only what
the generated code needs, so Slint resolves the same whichever crate
is named; the comment on each says to keep the two lists equal.

## Measured

Worktree target, `-j 4`, peak is every rustc at once.

| | before | after |
|---|---|---|
| build, clean of both | 115 s, 9.0 GB | 113 s, 8.1 GB |
| build, window only | | 24 s, 2.3 GB |
| test build, clean of both | 146 s, 13.6 GB | 142 s, 8.6 GB |
| test build, window only | | 45 s, 4.5 GB |
| one .rs touched, build | 25 s, 6.6 GB | 8 s, 1.6 GB |
| one .rs touched, test build | 31 s, 12.6 GB | 20 s, 3.1 GB |
| one .slint touched, build | 28 s, 6.2 GB | 42 s, 5.2 GB |
| one .slint touched, test build | 42 s, 12.3 GB | 15 s, 2.6 GB |

The window's own build, the one every Rust edit pays, is a quarter
of what it was in time and memory. A `.slint` change costs more for
the build alone, since the UI crate and then the window compile one
after the other, and less for the build and the tests together,
since both share one compile of it. The Slint crate's own compile,
76 s and 8 to 11 GB, is paid only when a `.slint` file changes.

## The trap left open

Built through the window, its tests or the workspace, the Slint
stack resolves the same features. Built alone with `cargo build -p
greycard-slint`, a dozen packages under it resolve with fewer
features, the ones the editor's other dependencies turn on, so Cargo
compiles a second copy of the Slint stack and of this crate, 100 s
and 7 GB, which the editor's build then does not use. The two copies
coexist, so nothing is recompiled later; it is waste, not a
rebuild. Closing it would take a workspace-hack list of a dozen
dependencies on the crate. We left it with a comment: build it
through greycard-ui.

## What follows

The desktop file chooser out of `export.rs` into the window, then a
render crate between edit and the window holding the finish, AgX,
detail, export, border, watermark and geometry, which import nothing
of State or the App type; then a thin views crate of shared types
and the model modules with no outward edge. The bindings wait for
0.9.0 behind three seams: opening a frame, culling drawing through
the develop's viewport, and the thumbnails on the worker handle.
