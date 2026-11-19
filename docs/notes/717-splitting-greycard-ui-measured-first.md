# §717. Splitting greycard-ui, measured first (2026-11-18)

The fourth pass on the window's structure, after §344's owners, §351's Slint crate, and §467's library target. Four cuts were proposed:
- the windowless modules into a crate or into greycard-library;
- each large file's tests into a sibling file;
- `panel/` split into a library crate and a develop crate;
- the develop panel out of `app.slint`.

We mapped the crate and timed it before cutting. The measurement keeps two of the four cuts, shrinks one, and turns one into design work.

## What the crate is

`crates/greycard-ui/src` is 156 files and 198,737 lines:
- 95,193 lines of code and 103,511 of `#[cfg(test)]` items;
- 1,868 tests in the binary and 211 in the library target;
- 108 files keep their tests inline, 89,942 lines in all;
- the 30 largest inline test modules hold 58,960 lines, including 5,643 of `panel/cull.rs`'s 9,014.

Most of the windowless code is already in the library target: archive, carried, collections, delete, filter, gpu_marker, import, instance, mounts, tags, and eleven smaller modules, 12,867 lines of code.

Timings: `-j 4`, debug, worktree target, on a machine under other builds, so indicative.

| | wall |
|---|---|
| clean build | 209 s; greycard-slint 106 s of it, then the binary 33 s |
| one `.rs` touched, build | 4 s |
| the same with the window's incremental cache removed | 36 s |
| one `.slint` touched, build | 39 s |
| test build, first; one `.rs` touched | 60 s; 6 s |
| the binary's tests | 200 s |
| the library target's tests | 3 s |

A Rust edit costs 4 to 6 s, and only one that defeats incremental reuse comes near the binary's 33 s. The long pole is the test run: 1,860 tests in one process, about 1,565 of them on a window. No split of the crate shortens that, and a module filter already runs one panel's tests. A `.slint` edit pays greycard-slint's generated code wherever the markup sits.

## The goal

The goal is not speed; no step below makes the build or the tests measurably faster. The aims are:
- files a reader and an agent can open whole;
- more windowless code in the library target, where its tests run in seconds without a window;
- §467's `cfg(test)` hazard closed for each module that moves.

## Why panel/ does not split by side

On the graph of non-test `use` and path edges, with a file and its children as one unit, the binary is one cycle of 81 units and 78,713 lines. Taking out `State`, its owners, and the `Shared` cell still leaves 74. Taking out the shell, the shared panels (deliver, sync, export queue, edit-in, saves), and the frame model (rows, edited, versions, cells, queue view) still leaves 46 units in one cycle: 25 library and 21 develop, 49,623 lines.

The library side's 29 edges into develop come from six units, mostly the browser and culling. The develop side's 31 edges back come from fourteen. They follow the seams the roadmap named in October:
- opening a frame (`panel::edit::develop_of` and seven more, the mask's and the painted fills' openers);
- the sidecar writes, which live in `panel::edit` and which culling calls;
- the loupe, which culling draws through the viewport and the viewport calls back into;
- the badges and tags, which history sets through the browser.

Every module takes `&State` (911 signatures), `State` holds both sides' types, and 51 files reach it through the `STATE` and `WORKER` thread-locals.

Two crates cannot hold a cycle. The split waits on four changes, each a section of its own:
- `State` and the types its owners hold, in a crate below the panels;
- the open-frame seam as a hook the shell installs;
- the sidecar writes moved out of `panel::edit`;
- the loupe's drawing apart from the viewport.

After each lands we measure the cycle again (the study's script takes seconds). The crate split is a move only once library and develop are no longer one component.

## The steps

Each step is one squashed commit by one agent, and each starts from master with the one before it merged. None changes behavior, UI text, or the binary's name, flags, or output. Each is verified as follows:
- `cargo test`, `cargo clippy --all-targets -D warnings`, and `cargo fmt --check` all clean;
- `cargo test -p greycard-ui -- --list`, binary and library together, gives the same names before and after (2,079 today), each test where the step says;
- `git diff master --color-moved=dimmed-zebra --color-moved-ws=allow-indentation-change` shows only the lines the step names;
- `cargo doc -p greycard-ui --no-deps --document-private-items` reports the same broken links as master.

**1. Tests beside their files.** In the 30 files with 1,000 or more test lines, `mod tests { … }` becomes `#[cfg(test)] mod tests;` (keeping a `pub(crate)` where it had one), and its body moves whole, dedented, into `<stem>/tests.rs`, as §318 and §321 did.
- The files under `panel/`: cull, viewport, sync, mask, browser, deliver, history, assets, crop, color, versions, prefs, sizes, collections, edit_in, section_reset, look_rename, export_queue, edit, and merge.
- The files outside it: keys, library, sync, tree, cull, archive, import, entry, filter, and collections.
- Touched beyond the move:
  - the `mod` lines;
  - the file-relative `include_bytes!` and `include_str!` paths, which gain a `../`;
  - rustfmt's reflow of what the dedent lets fit.
- Not touched: no visibility, since a child sees its parent's private items, and no test name.
- No test module holds a `tr!`, so the translations do not move.
- 58,960 lines; `panel/cull.rs` comes out at 3,371.

**2. The windowless leaves into the library target.** grid (684 code / 788 test lines), strip (206/180), and focus_points (165/64) move from `main.rs`'s module list to `lib.rs`. The binary names them through its `pub(crate) use greycard_ui::{…}`, so every `crate::grid::X` compiles untouched. focus_points' `tell`, now `#[cfg(test)]` and called by culling's tests, becomes unconditional and `#[doc(hidden)]`; otherwise the binary's tests lose it. `HEAD_READS` stays `#[cfg(test)]`, since only the module's own tests read it. Log targets keep their text, since both targets are the crate `greycard_ui`.

**3. Four modules, one seam each.**
- **cull** (1,199/1,518): `lightbox::fetch` moves beside the `Want`, `Source`, and `Loaded` it takes, into a child module `cull::lightbox` since cull has a `fetch` of its own, with the `pick`, `Pick`, and `kept_order` it needs. `versions::Key` is named as the engine's `version_key::Key`, which it is. `move_rejects` becomes unconditional. `asked`, `pushed`, and the develops list become unconditional but record only once a test arms them (`Prefetcher::record`, `cull::record_develops`), so a shipping build keeps nothing and pays one atomic load; a reader panics unarmed. `threads_gone` stays `#[cfg(test)]`, read only by cull's own tests.
- **placeholder** (253/159): `panel::cull::{picture_words, zoom_words}` move into it, and `panel::cull` re-exports them.
- **sort** (249/157): `roots::View` moves into it, and `roots` re-exports it. `Kept::by_name` becomes unconditional.
- **export_queue** (241/107): `report::{path_shown, abbreviate}` move into `files.rs`.
- **web** stays in the binary: `adapter::scrub` also calls `window_fit::scale::scrub`, which has a flag of its own that the window's scale code sets, so the seam is two functions and two flags, not one.

The moved functions keep their names and bodies; their old homes re-export them. One sort test that reads `settings::Settings` moved to settings' tests.

## Left out

- **settings** needs six types out of Slint files and the viewport crate, and it swaps its save under `cfg(not(test))`, §467's hazard exactly. Its own section, if ever.
- **stacks, clipboard, open_note, headless, and runners** reach `State` or `Cli`.
- **The library target as a crate of its own.**
  - It would buy a `testing` feature in place of unconditional hooks, Slint off its manifest, and a crate the CLI could share (`root_mapping` is written in both binaries).
  - It would cost moving the catalogs' half of `build.rs` with `i18n`, a line in `translations.sh` and `translations-check.py`, a row and a scan path in the layering test, the scratch-removal test's scope, and a prefix in `log.rs`'s `OURS`.
  - It saves no build time: the library target already compiles apart, and as a crate it would only start sooner, off the critical path.
  - A decision for when the CLI wants it.
- **collections and carried into greycard-library**: their weight is the library's types, but they carry `tr!`. With the decision above.
- **The develop panel out of `app.slint`**: about 1,100 lines of the right pane into a component, as §154 cut the sections. The interface stays on `App`, so Rust changes only the two element ids the tests look up (`App::panel-view`, `App::panel-sections`). It is readable and no faster. A step when the file's size is the pain, not before.
- **Memory**: not measured this time.
