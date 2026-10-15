# §316. Splitting render.rs (2026-10-14)

`crates/greycard-ui/src/render.rs` was 5,635 lines: some 2,470 of
viewport renderer and 3,160 of tests. We cut it into
`render/`, the first of the three files in the module split plan, as
a pure move in two commits, each green on `cargo fmt --check`,
`cargo clippy --all-targets -D warnings` and `cargo test`:

1. a bare `git mv render.rs render/mod.rs`, so `git log --follow`
   keeps the history, plus the four `include_str!` paths, which are
   file-relative and gain a `../` (the `.wgsl` files stay where they
   are, since `agx.rs` and `finish.rs` include `viewport.wgsl` too);
2. the cut.

What moved where:

- `mod.rs` (555 lines): the header and imports, the canvas levels,
  `halves`, and the types whose fields are read across the new seams,
  with their impls: `Warn`, `View`, `LookTable`, `Tile`, `Renderer`
  and `Renderer::new`, `Scopes`, and `TARGET_FORMAT`. Private items
  and fields of `mod.rs` are visible in every child, so none of these
  changed visibility.
- `locals.rs` (476): `range_reads`, `LocalGpu`, `ShapeGpu`,
  `LocalsGpu`, `locals_gpu`, `Brushes` and the raster-width assert,
  with the three CPU tests of `locals_gpu` in its own `tests` module.
- `scopes.rs` (370): `EditKey`, `ANALYSIS_WIDTH`, `impl Scopes`, and
  `analyze` and `take_navigator` as an `impl Renderer`.
- `inputs.rs` (351): the display, encoded, output, look, guide and
  detail setters and their texture makers, and `TYPICAL_RANGE`.
- `draw.rs` (711): `Params`, built only in `draw_source`;
  `make_target`, `draw`, `draw_source`, `set_source`,
  `encoded_texture`, `render_encoded`, `upload`, `render`,
  `read_back`.
- `sample.rs` (176): `has_image`, `sample`, `sample_grid`.
- `tests.rs` (3,032): the rest of `mod tests`, still named `tests`,
  one file this round since its helpers interleave.

Each new file starts with `use super::*;` and none says `use
crate::*`. `mod.rs` keeps its import block as it was and adds the
module list and one hub import, `use self::{locals::{Brushes,
LocalsGpu, locals_gpu, range_reads}, scopes::EditKey};`, the moved
items that `mod.rs` or a sibling names.

Lines touched beyond pure moves, against master: 23 `pub(super)`
(`range_reads`, `locals_gpu`, `LocalsGpu` and its four fields,
`LocalGpu`, `ShapeGpu`, `Brushes` and its `empty`, `view`, `sync`,
`EditKey`, `ANALYSIS_WIDTH`, `Scopes::new`, the four texture makers,
`TYPICAL_RANGE`, `make_target`, `draw`); 4 `include_str!` paths; 12
of module list and hub in `mod.rs`; 6 `use super::*;` and 1 import of
`ANALYSIS_WIDTH` in `tests.rs`; 3 imports and 3 lines of wrapper for
the locals tests; 4 `impl Renderer {` and 5 closing braces against 2
dropped; and rustfmt's reflow of what those touched, 16 lines in and
13 out (two signatures that `pub(super)` pushed past 100 columns,
five statements in the tests that the 4-column dedent let fit on
fewer lines). About 70 lines, against some 5,600 moved. Three
visibilities the plan did not list were needed: `LocalGpu` and
`ShapeGpu`, since `draw_source` names them through `LocalsGpu`'s
fields and a `pub(super)` field of a private type is an error, and
`TYPICAL_RANGE`, which is new since the plan and is read by `new`.

What the verification showed:

- `git diff --stat master -- crates/` outside `render*` is empty, so
  every `crate::render::X` and the `render::{Renderer, View}` line in
  `main.rs` compile untouched.
- `git diff master --color-moved=dimmed-zebra
  --color-moved-ws=allow-indentation-change` shows 108 bright lines,
  all of them module, use, `pub(super)`, include path, brace, `impl
  Renderer {` or rustfmt reflow lines.
- The normalized line multiset of master's `render.rs` against the
  seven files differs only in the lines above, and both have 95
  function definitions.
- `cargo test -p greycard-ui -- --list` gives the same 1,176 test
  names as master; the three locals tests now sit under
  `render::locals::tests`, the other 22 under `render::tests`.
- `cargo doc -p greycard-ui --no-deps --document-private-items` with
  `-D warnings` reports the same 15 broken intra-doc links as master,
  none of them in `render`.
- The workspace `cargo test` passes.

The one log line in moved code, "viewport drawn" in `render`, now has
the target `greycard_ui::render::draw` rather than
`greycard_ui::render`. `log.rs` filters on the crate prefix, so
nothing is filtered differently; only the log file's text changes.
We added no `target:` overrides.
