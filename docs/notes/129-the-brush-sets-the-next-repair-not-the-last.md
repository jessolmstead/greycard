# 129. The brush sets the next repair, not the last (2026-09-21)

Retouch Size/Feather/Opacity resizing the wrong stroke (roadmap Bugs): fixed by
clearing the RETOUCH panel's selection (`patch = -1`) once a spot or stroke
finishes in `on_place_released` (main.rs), instead of leaving the just-drawn
patch selected. `on_patch_edited` already no-ops when nothing is selected, so
the sliders and the viewport wheel now set the brush for the next repair
rather than resizing the last one; choosing a repair from the RETOUCH list
still selects and edits it as before, and its history still coalesces to one
state per drag through the existing debounce in `develop_soon`/`schedule_save`.

Esc and a click on the empty picture were wired in app.slint to the same
`patch-changed(-1)` a list click already uses, so they let go of a
deliberately-chosen repair too, without needing a new callback.

Two tests added to `crates/greycard-ui/src/main.rs`'s `key_tests` module,
built on a new `retouch_state` helper that wires `install_callbacks` to a
file-less `State` and a no-op `Worker` (nothing like this existed before;
`key_tests` only exercised app.slint's key handling directly). Not covered by
an automated test: the click-on-empty-picture branch, since it needs a sized,
hit-testable viewport under `dispatch_event` pointer events, which felt too
fragile for the payoff given Esc exercises the identical Rust-side code path.

The test that pins it drives the real callbacks on a headless
window, and for that the whole `State` had to be built outside
`run()`; rather than a second copy of a forty-field literal, `State`
gained one constructor, `empty`, that names every field once, and
`run()` overlays only what the command line and the settings choose.
