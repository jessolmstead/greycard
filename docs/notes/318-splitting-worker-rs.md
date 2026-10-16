# §318. Splitting worker.rs (2026-10-15)

`crates/greycard-ui/src/worker.rs` was 7,815 lines: some 4,440 of
the engine's thread, its handle, the export path, the develop and the
thumbnails, and 3,375 of tests. We cut it into `worker/`, the second
of the three files in the module split plan, as a pure move in three
commits, each green on `cargo fmt --check`, `cargo clippy
--all-targets -D warnings` and `cargo test -p greycard-ui`:

1. a bare `git mv worker.rs worker/mod.rs`, so `git log --follow`
   keeps the history. Nothing in it is file-relative (the two
   `env!("CARGO_MANIFEST_DIR")` in the tests are the crate's, not the
   file's), and `mod reduced_check;` already resolved to
   `worker/reduced_check.rs`;
2. the production cut, with `mod tests` moved whole into `tests.rs`;
3. the four ignored real-frame suites out of `tests.rs` into
   `samples.rs`.

What moved where:

- `mod.rs` (1,125 lines): the header and the two import blocks as
  they were, and the types whose fields are read across the new
  seams, with their impls: `Developed`, `Halves`, `Job`, the reports
  (`LearnedReport`, `FillReport`, `WhiteBase`, `LensReport`,
  `SourceKind`), `Outcome`, `Queue`, `Deliver`, `LEAVING`,
  `ThumbCache`, `ThumbCaps`, `Worker`, `Last`, `Keep`,
  `ShownTexture`, `InHand`, `FrameDevelop`, `Blame`,
  `panic_message`, `quarters`, `Input`, `Base`, `KeptPlanes`,
  `PlanesJob`, `CancelOnDrop`, `PreSharpen`, `KeptHaze`,
  `DehazeRan`, `DetailRan`, `LearnedBase` and `pair_serves`. Private
  items and fields of `mod.rs` are visible in every child, so none of
  these changed visibility.
- `handle.rs` (643): `impl Worker`, `order_thumbnails`, `fetch` and
  `fetch_lenses`.
- `engine.rs` (665): `left_out_of`, `run`, `during_develop`,
  `file_label`, `discard_base` and `has_live_part`.
- `export.rs` (189): `open_picture`, `seeded`, `export_other`,
  `write_export`, `finish_export`.
- `masks.rs` (185): `Rasters`, `learned_masks`, `MasksMade`,
  `masks_for`.
- `keep.rs` (163): `keep_open`.
- `decode.rs` (127): `timed_open`, `open`, `lens_report`,
  `correct_lens`, `correct_lens_of`.
- `develop.rs` (840): `develop_job` and its internals, `BeforeSharpen`
  through `note_developed`.
- `base.rs` (273): `turns_exactly`, `turn_base`, `engine_base`,
  `learned_base`, `run_learned`, `blend`.
- `thumbs.rs` (303): `THUMB_RECIPE` through `srgb_encode`.
- `tests.rs` (2,103): `mod tests`, still named `tests`, so
  `worker::tests::thumb_scratch` keeps resolving for `reduced_check`.
- `samples.rs` (1,258): every ignored real-frame suite,
  `the_edit_s_pictures_of_the_samples`, `the_sky_over_real_frames`,
  `parts_on_real_frames` and
  `the_dehaze_viewport_against_the_export_on_real_frames`, as a
  `#[cfg(test)] mod samples` beside `reduced_check`. It takes
  `read_viewport`, `skipped` and `thumb_scratch` from `tests`.
- `reduced_check.rs` (758): unchanged.

Each new file starts with a `//!` line or two and `use super::*;`, and
none says `use crate::*`. `mod.rs` keeps its import blocks as they were
and adds, after the first, the module list, one hub import of the
moved items that `mod.rs` or a sibling names, one `#[cfg(test)]` hub
import of the two only the tests name (`base::blend`, `decode::open`),
and the re-exports that keep today's paths: `pub use` of
`THUMB_RECIPE`, `THUMB_WIDTH`, `count_thumb_cache`, and `pub(crate)
use` of `order_thumbnails`, `correct_lens_of`, `file_stat`,
`thumb_key`, `thumb_tag`, `thumbnail`.

Lines touched beyond pure moves, against master: 26 `pub(super)` (24
in production: `run`, `left_out_of`, the five export functions, the
three of masks, `keep_open`, `open`, `timed_open`, `lens_report`,
`correct_lens`, `develop_job`, the five of base, `thumb_lookup`,
`cached_thumbnail_noting`, and the method `Worker::stop_within`; and
`read_viewport` and `skipped` in the tests, for `samples.rs`); 11
`mod` lines and 2 more `#[cfg(test)]`; 12 lines of hub imports and 6
of re-exports; 13 `//!` header lines, 10 `use super::*;` and the one
import in `samples.rs`; `mod tests {` and its closing brace dropped;
and rustfmt's reflow of what those touched: three signatures that
`pub(super)` pushed past 100 columns (`blend`, `learned_masks`,
`open`; 3 lines out, 13 in) and twelve statements in the tests that
the 4-column dedent let fit on fewer lines (27 out, 12 in). About 100
lines in and 32 out, against some 7,800 moved. No field changed
visibility.

What the verification showed:

- `git diff --stat master -- crates/` outside `worker*` is empty, so
  every `crate::worker::X` and the `pub(crate) use worker::{…}` line
  in `main.rs` compile untouched.
- `git diff master --color-moved=dimmed-zebra
  --color-moved-ws=allow-indentation-change` shows 167 lines in plain
  color (57 out, 110 in), all of them module, use, `pub(super)`,
  header, brace or rustfmt reflow lines; the rest is dimmed or a
  moved block's zebra border.
- The normalized line multiset of master's `worker.rs` against the
  twelve files differs only in the lines above (32 lines only in
  master, 79 only in the branch), and both have 139 function
  definitions.
- `cargo test -p greycard-ui -- --list` gives the same 1,216 test
  names as master; the four real-frame suites now sit under
  `worker::samples`, the rest where they were.
- `cargo doc -p greycard-ui --no-deps --document-private-items` with
  `-D warnings` reports the same 15 broken intra-doc links as master,
  none of them in `worker`.
- The workspace `cargo test` passes.

The log lines in moved functions now have the targets
`greycard_ui::worker::handle`, `::engine`, `::masks`, `::keep`,
`::decode`, `::develop`, `::base` and `::thumbs` rather than
`greycard_ui::worker`. `log.rs` filters on the crate prefix, so
nothing is filtered differently; only the log file's text changes. We
added no `target:` overrides.

What the plan got wrong, or what changed under it since 854c78e:

- `open.rs` would have been a module named like the function `open`
  it holds, which the tests call by its bare name and found the
  module instead (E0423). We named it `decode.rs`, by the plan's own
  rule that a module's name never clashes with an item.
- `PlanesJob` and `CancelOnDrop` stay in `mod.rs`, not `develop.rs`:
  a test added since the plan (the planes' thread that declines or
  dies) builds a `PlanesJob::Making` with a `CancelOnDrop(…)` and
  calls `land`, so the tuple field and the method cross a seam. The
  plan's rule puts such a type in `mod.rs`; moving it would have cost
  a field visibility and three more `pub(super)`.
- `write_export` needs `pub(super)` and a place in the hub: `run`'s
  export arm calls it. The plan missed it.
- `open` (tests, in `tests.rs` and `samples.rs`) and `blend` reach
  the tests through a `#[cfg(test)]` hub import in `mod.rs` rather
  than a line in `tests.rs`, so `samples.rs` gets them too by its
  `use super::*`.
- The plan's line numbers were all off by §308, §315 and §317, but
  every item it named was still there; nothing new crossed a seam
  besides `PlanesJob`. §317's keying by `versions::Key` lives in
  `Job`, `Outcome`, `Keep` and the thumbnail functions' signatures,
  none of which changed home.
- There are four ignored real-frame suites in the tests, not three:
  the plan's list missed `the_edit_s_pictures_of_the_samples`. All
  four moved, so the rule is that every ignored real-frame suite lives
  in `samples.rs`. The fourth also needs `thumb_scratch`, which was
  already `pub(super)` for `reduced_check`, so it adds a name to
  `samples.rs`'s import and no visibility.
- Every `pub(super)` the plan listed was needed, and beyond it only
  `write_export` and `open`: no hub import went unused, and the plan's
  untested rule that a parent's import used only through a child's
  glob counts as used held (`mod.rs`'s own imports serve the children
  without a warning).
