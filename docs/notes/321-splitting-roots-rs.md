# §321. Splitting roots.rs (2026-10-15)

`crates/greycard-ui/src/roots.rs` was 8,789 lines: some 3,545 of the
library's roots in the editor (the watcher, the pane, the views, the
reads off the window's thread and their landing, and an open list kept
up with the disk) and 5,240 of tests. We cut it into `roots/`, the
last of the three files in the module split plan, as a pure move in
three commits, each green on `cargo fmt --check`, `cargo clippy
--all-targets -D warnings` and `cargo test -p greycard-ui`:

1. a bare `git mv roots.rs roots/mod.rs`, so `git log --follow` keeps
   the history. Nothing in it is file-relative: no `include_str!`,
   `file!` or `#[path]`;
2. the production cut, with `mod tests` moved whole into `tests.rs`;
3. `tests.rs` into `tests/mod.rs` and six children.

What moved where:

- `mod.rs` (785 lines): the header and the import block as they were,
  and the types whose fields are read across the new seams, with their
  impls: `View`, `Library`, the read's constants (`ROOT_WAIT` through
  `PASSES_KEPT`), `Progress`, `Purpose`, `Select`, `Listing`,
  `Source`, `Look`, `OnScreen`, `Brought`, `Found`, and `Answer` and
  `Checks`; `rows_listed` and `count_listed`, which the look and the
  view both call; `install`; and the test landings `land_sent`,
  `land_sent_last_first` and `land_sent_in`, which the crate's other
  tests call. Private items and fields of `mod.rs` are visible in
  every child, so none of these changed visibility.
- `watcher.rs` (577): `online`, `start`, `watch`, `Plan`, `Launch`,
  `Built`, `WATCH_REMOTE`, the network mounts (`remote_fs`, `Mount`,
  `mount_on_purpose`, `remote_under_fs`), the build landed
  (`build_in_place` through `send_build`), `Poll` and `poll`.
- `pane.rs` (477): `recount`, `show`, the root's names, the folder to
  add, `edit_roots`, `add`, `Check`, `checked_within`, `send_check`,
  `add_checked`, `remove`, `remove_listed`, `rename`, `open_sheet`.
- `view.rs` (554): `roots_of`, `hidden_under`, `every_root`, and
  `open_view` through `refresh_view`: the view's read sent, the folder
  open, the bar and its words, `ask`.
- `checks.rs` (80): `CHECKS`, `offline_of`, `answer_of`, `answers_of`,
  `LOOKS_AT_ONCE`, `answers`.
- `look.rs` (399): `impl Look { run }`, `list_folder`, `send_off`.
- `landing.rs` (283): `land`, `take_empty`.
- `kept_up.rs` (451): `folders_passed`, `follow_current`,
  `owed_thumbnails`, `ask_owed`, `on_screen`, `merge`,
  `background_done`, `changed_rows`, `keyed`, `empty_view`.
- `tests/mod.rs` (337): still `mod tests`, so `sync.rs`'s sixteen
  `crate::roots::tests::NOT_ANSWERING` resolve as before. It keeps the
  thread-locals and hooks the production code reaches, the helpers
  every file of tests uses (`run_checks` through `frames`), and those
  hoisted because more than one child uses them: `three_under_a_root`,
  `read_so_far`, `tick_on`, `disk_reads_under`, `open_three`,
  `rated_under_a_root`, and `Shoot` with its impl.
- `tests/lists.rs` (1,119, 16 tests): merges, the frame on screen
  followed, passes and offline roots.
- `tests/pane.rs` (594, 8): the pane, its menus, the keys over it.
- `tests/loading.rs` (284, 7): the bar.
- `tests/reads.rs` (455, 9): folders locked or through links, views
  read from the rows, folder opens.
- `tests/picks.rs` (1,816, 33): picks and culling while reads are out,
  and the keys pressed meanwhile.
- `tests/watcher.rs` (644, 15): the watcher, the timer, network mounts.

Each new file starts with a `//!` line or two and `use super::*;`, and
none says `use crate::*`, which would make roots' own `View` and
`Look` ambiguous with `crate::View` and `greycard_edit::Look`.
`mod.rs` keeps its import block as it was and adds, after it, the
module list, one hub import of the moved items that `mod.rs` or a
sibling names (`offline_of`, `empty_view`, `merge`, `land`,
`send_off`, `add`, `open_folder_to_add`, `open_sheet`, `remove`,
`rename`, `loading_done`, `Poll`, `online`, `watch`), one
`#[cfg(test)]` hub import of the eleven only the tests name
(`changed_rows`, `follow_current`, `Check`, `checked_within`, `ask`,
`grouped`, `loading_tick`, `Plan`, `WATCH_REMOTE`, `build_in_place`,
`poll`), and the `pub(crate) use` re-exports that keep today's
`crate::roots::X` paths: `answer_of`, `answers`, `answers_of`,
`ask_owed`, `background_done`, `folders_passed`, `edit_roots`,
`one_root_named`, `recount`, `root_name`, `said`, `show`,
`loading_words`, `open_listing`, `open_view`, `refresh_view`,
`poll_every`, `restart_poll`, `start`. Under cfg(test) the production
code's `tests::SENT`, `tests::REMOTE` and the other hooks resolve
through each child's `use super::*` with no edit, as the plan
expected; the fallback `use super::tests;` lines were not needed.

Lines touched beyond pure moves, against master, 93 in and 44 out
(blank lines aside): 14 `pub(super)` (`open_folder_to_add`,
`open_sheet`, `offline_of`, both cfg variants of `send_off`, `land`,
`build_in_place`, `poll`, `WATCH_REMOTE`, `Check` and its method
`land`, `checked_within`, `grouped`, `ask`); 15 `mod` lines (9 in
`roots/mod.rs` with `#[cfg(test)]`, 6 in `tests/mod.rs`); 23 of hub
imports and re-exports (9 hub, 7 test hub, 7 re-export); 23 `//!`
header lines and 13 `use super::*;`; the one doc link given its
target, the header's `[`Plan::build`](watcher::Plan::build)`, since
`Plan` is named in `mod.rs` only under cfg(test); `pub(crate) mod
tests {` become `pub(crate) mod tests;` and its closing brace dropped;
and rustfmt's reflow of four statements in the tests that the
4-column dedent let fit on fewer lines (26 lines out, 4 in). No field
changed visibility and no signature reflowed.

What the verification showed:

- `git diff --stat master -- crates/` outside `roots*` is empty, and
  so is the whole tree outside it, so every `crate::roots::X`,
  `crate::roots::tests::NOT_ANSWERING` and the
  `crate::roots::{land_sent, open_view}` imports in `tree.rs` and
  `panel/collections.rs` compile untouched.
- `git diff master --color-moved=dimmed-zebra
  --color-moved-ws=allow-indentation-change` shows 154 lines in plain
  color (45 out, 109 in, 17 of them blank), all of them module, use,
  `pub(super)`, header, brace, the doc link or rustfmt reflow lines;
  the rest is dimmed or a moved block's zebra border.
- The normalized line multiset of master's `roots.rs` against the
  fifteen files differs only in the lines above (29 lines only in
  master, 78 only in the branch, the `pub(super)` lines matching after
  the normalization), and both have 209 function definitions.
- `cargo test -p greycard-ui -- --list` gives the same 1,240 tests as
  master (1,239 names; `forget_missing_drops_the_gone_and_keeps_the_offline`
  is in two modules on master already). The 88 roots tests now sit
  under `roots::tests::{lists, pane, loading, reads, picks, watcher}`;
  none is left in `tests/mod.rs`. After the production cut and before
  the subdivision the list was identical, paths and all.
- `cargo doc -p greycard-ui --no-deps --document-private-items` with
  `-D warnings` reports the same 15 broken intra-doc links as master,
  at the same places, none of them in `roots`.
- The workspace `cargo test` passes.

The log lines in moved functions now have the targets
`greycard_ui::roots::watcher`, `::pane`, `::view`, `::look`,
`::landing` and `::kept_up` rather than `greycard_ui::roots`. `log.rs`
filters on the crate prefix, so nothing is filtered differently; only
the log file's text changes. We added no `target:` overrides.

What the plan got wrong, or what changed under it since 854c78e:

- `Checks` and `Answer` stay in `mod.rs`, not `checks.rs`: the test of
  a root that does not answer builds its own `Checks` and reads its
  `started` field. The plan's rule puts such a type in `mod.rs`;
  moving it would have cost a field visibility. `checks.rs` keeps the
  free functions and `CHECKS`, and is 80 lines rather than about 180.
  `mod.rs` comes out at 785 lines against the plan's 650, for that
  and for what §310, §317, §319 and §320 added to the types it holds.
- The private items only the tests reach arrive through a
  `#[cfg(test)]` hub import in `mod.rs`, as §318 did for `open` and
  `blend`, rather than a glob line in `tests.rs`: it names the eleven
  and no more, and the tests' children get them through their own
  `use super::*`. The plan listed seven
  of them for `pub(super)`; `loading_tick`, `changed_rows`,
  `follow_current` and `Plan` were already `pub(crate)` and needed
  only the import.
- Every `pub(super)` the plan listed was needed and no other; no hub
  import went unused; and its hoisting list for the tests was
  complete: no helper crossed between the children beyond it.
- The plan's line numbers were off by §310, §317, §319 and §320, but
  every item it named was still there and in the same order.
  `rows_listed` and `count_listed`, flagged for a re-map, still serve
  the look and the view and stay in `mod.rs`; §319's `Found::frames`
  and `collection_frames` live in `Found` and `State`, and §320's
  compare refit sits inside `merge`, none of which changed home, so
  nothing new crossed a seam.
