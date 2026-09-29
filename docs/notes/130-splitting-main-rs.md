# 130. Splitting main.rs (2026-09-21)

`crates/greycard-ui/src/main.rs` is 10,428 lines and it doubled in
three days: 5,381 on 09-17, 7,486 on 09-19, 10,295 on 09-20, touched
54 separate times over the two wave days. Nothing in it is wrong. The
crate's own rule, written into `cull.rs` and `grid.rs`, is that
everything which decides something lives in a pure module and can be
tested without a window; that rule worked, and what is left in
`main.rs` is exactly what it pushes there, the Slint wiring. The rule
was never applied a second time, to the wiring itself, so every
feature since has added its state fields, its callback registration
and its `read_`/`show_` pair to the one file, and five agents in
worktrees all land in it at once.

Where the lines are: `install_callbacks` is 2,997 of them and holds
128 callbacks; `main` is 880; the four `#[cfg(test)]` modules at the
bottom are 1,124; `deliver` is 505; the rest is some 180 free helper
functions, the `read_X`/`show_X` bridges. The 128 callbacks are
already clustered by concern in source order rather than interleaved
— the curve's eleven run 3876 to 4048, the mixer and the
black-and-white 4270 to 4376, the presets 6083 to 6266 — so the split
is a set of cuts and not an untangling.

One module a panel section, each holding its state-to-UI bridge, its
callbacks and its tests, and all of them under a `panel/` directory
so the wiring sits apart from the pure modules it drives: `panel/cull.rs`
beside `cull.rs`, and the rule the crate runs on is visible in the
tree rather than in a suffix on some names and not others. The
sections: `startup` (main's body, the monitors, the opening scope),
`cull`, `mask`, `browser`, `color` (the wheels, the tint, the mixer,
the black-and-white, the white balance), `viewport`, `deliver`,
`edit` (`read_edit`, `show_edit`, the folds), `assets` (presets,
looks, camera profiles, the lens fetch), `crop`, `curve`, `history`,
`retouch`. `install_callbacks` becomes a dispatcher of thirteen
`install(app, &state, &worker)` calls, and each closure keeps the
`state.clone()` and `app.as_weak()` capture it has now, so no
callback body changes. `State`'s fields go
`pub(crate)` rather than gaining forty accessors the compiler would
only have to check anyway; `App` comes from `slint::include_modules!()`
in the crate root, so a submodule says `use crate::App;`. `main.rs`
lands at about 400 lines: `Cli`, `State`, the module list.

The payoff beyond the size is the tests. §129 built `retouch_state`,
which wires `install_callbacks` to a file-less `State` and a no-op
`Worker` on the headless backend, and `key_tests` has its `window`
helper beside it; both are private to `main.rs`. Promoted to a shared
`#[cfg(test)] pub(crate) mod testing`, every panel module can drive
its own callbacks headlessly. That is where the open
interaction bugs sit — the mask lines over the left bar, the crop's
'original' losing a portrait orientation — neither of which a CPU
reference test will ever catch.

Landing it: first a `pub(crate)` pass on `State`'s fields and the
shared helpers, moving nothing; then `startup`, then `deliver`, both
self-contained; then one commit a module, each taking its helpers,
its callback block and its tests together; last, `install_callbacks`
collapses to the dispatcher. Sixteen or so commits, each small enough
to review and each green on `cargo test`, `clippy --all-targets` and
`fmt`. Every one of them moves code and changes none, and the review
holds it to that with `git diff --color-moved=dimmed-zebra`, which
greys out a line that only relocated and leaves a reviewer reading
the few that did not, which should be the `pub(crate)`s and the
paths. One agent and not a wave: five of them pulling functions out
of one file is the worst merge available, and the compiler checks
every step of this one, which makes it the safest seat to spend. It
goes first, after §131's turn lands and before anything else starts:
a branch rebased across a file move is not a rebase but a re-apply
by hand, so the turn is the last feature to land on the old layout.

Two things the split does not do. `State` is not broken up: `empty`
is documented as the one place every field is named, so that adding a
field once keeps `run` and the tests whole, and that invariant is
worth more than the tidiness of four smaller structs. And nothing
that decides anything moves into the new modules — where a helper
being moved holds a decision, the decision goes to the pure module
and the glue stays behind. That is the only place this refactor
changes code rather than relocating it, and it is the only place
the line count actually comes down instead of moving around, so it
is a second pass after the move is on master, one decision a commit
with its test, and never mixed into the sixteen: the first series
stays a pure move that the dimmed diff can vouch for. Those two
bugs are the second pass's first two commits.

`app.slint` has the same history at 4,436 lines. Slint imports a
component from another file, so the panel sections can move out one
a file the same way, on the same rule and reviewed the same way. It
is a separate job for a separate agent, since the two files do not
collide, and it can wait: the Rust side is where the tests are.

**Landed the same night.** Fifteen commits on the branch, one squash
commit on master, since the fifteen were for the review and a change
that alters no behavior has nothing inside it to bisect. `main.rs`
went from 11,008 lines (the turn had added six hundred since the
count above) to 627, of which `State` and `State::empty` are 360.
The dispatcher has twelve calls, not thirteen: `startup` holds
`main` and registers nothing. The registrations number 125 by name
set, not 128; the line count above included a few `on_` lines that
were not registrations. The reviewer compared every item's body
against master with visibility and whitespace stripped and found
eight that differed: the dispatcher and seven signatures rustfmt
re-wrapped once `pub(crate)` pushed them past the column. One thing
for the second pass to take first: `finish.rs` now reaches up into
`panel::color` for a 3×3 inverse, the one pure module importing from
the wiring, which is the dependency the directory exists to forbid;
the inverse belongs in core's color module.
