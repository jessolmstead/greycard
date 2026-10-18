# §361. The render crate (2026-10-18)

The step §351 and §356 led to: what makes a developed frame into a
picture or a file, the finish on the CPU (the viewport's transform
and the reference its shader is held to), AgX, the masks' detail
bands, the straighten and crop, the border, the watermark and the
export, is `crates/greycard-render`, between the edit and the
window. Nothing in it knows of the window's state, its Slint types,
its panels or its worker, so the command line and a headless run can
render through it without the window; nothing does yet, since the
CLI never reached these modules and the headless export still comes
in through the window's startup.

## What moved

The seven modules the roadmap named, about 9,960 lines, unchanged
in their text: the only edits are visibility widened from crate to
public where the window reads an item, two test-only cfgs widened to
a `testing` feature, and the shader's path in two tests, which read
the window's `viewport.wgsl` across crates since the GPU side stays
in the window. Following the imports took two more pieces with
them: the box reduction out of `edited.rs`, which detail's test
holds its reduction to, now `greycard_render::reduce`; and the test
helpers the moved tests shared with the window's, a scratch folder's
removal, the masks' camera profile, a camera body by name and the
watermark's bar picture, now `greycard_render::testing`, built for
the crate's own tests and behind the `testing` feature for the
window's, which its dev-dependency turns on. The AgX oracle fixture
moved with its test. The window re-exports the seven module names
and the helpers where they were, so its call sites stand as written;
one test path in the window changed, to the helper's new home. No
test had to stay behind: 104 moved, the window's count fell by
exactly that, and the workspace runs the same 2,732 tests as
before.

The crate depends on core and edit and on what the moved code
already used; the watermark's font crates left the window's manifest
with it. Built alone it compiles only itself, so the second-copy
trap §351 left open for the Slint crate does not arise here.

## Measured

The window's build is within noise of before, clean and
incremental, and its rustc uses five to seven percent less memory,
in proportion to the lines that left. That is what §351's
measurement predicted: the window's weight was the generated Slint
code, now in its own crate, and its remaining cost is its own rustc
plus the linker, which this move does not touch. A change to a
render file costs the render crate's rebuild and the window's
incremental build, about nine seconds against five for a window
file. The gain of this pass is the boundary, not the build.

## What follows

The CLI and the headless export can depend on the crate directly.
The thin views crate and the model modules are the next passes, on
the order §351 set out, and the bindings wait for 0.9.0.
