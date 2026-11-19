# §720. The panels stay one crate (2026-11-19)

§717 held the split of `panel/` into a library crate and a develop crate behind four changes. We sized the four on master after its steps 1 to 3 and drop the split as a goal.

## The cycle, measured again

| Taken out | §717 | Now |
|---|---|---|
| nothing | 81 units, 78,713 lines | 77 units, 77,046 lines |
| main, the owners, shared | 74 | 72 |
| the shell, the shared panels, the frame model | 46 | 43 (22 library, 21 develop) |

The last row hid a precondition. With the shell and the shared units kept, the library side reaches them through about thirteen more seams: `settings::Settings` from 14 units, the hints, the startup's readers, the chooser, the saves, the export queue, sync both ways, and the `STATE` and `WORKER` thread-locals. All four changes together, at their broadest, leave 74 units in one cycle. Two crates want about seventeen seams, not four.

## The four, sized

| Change | Size | The cycle after it alone |
|---|---|---|
| `State` in a crate below | 197 types in 72 files, about 10,000 lines with the impls that must travel, some 320 fields made `pub` | 77 to 72 |
| Opening a frame as a hook | a table of about 35 function pointers, 90 call sites | 77 |
| The sidecar writes out of `panel::edit` | 108 lines, 55 call sites | 78 |
| The loupe apart from the viewport | 727 lines: what culling takes from the viewport is the window-capture driver | 78 |

## Why not

- §717's aims were files a reader can open whole and windowless code tested without a window. Neither needs two crates.
- No step is faster: a Rust edit costs 4 to 6 s, and the long pole is 1,860 tests in one process.
- The hook trades calls a reader can follow for indirection, and a hook left unset in a test skips the panel's update silently.
- Nothing else wants a panel crate.
- What keeps a test on a window is `State` itself: `State::empty` needs an `App`, since the window's owner holds a `Weak<App>`. That is a change of its own, unmeasured.

## What we do instead

Moves inside the crate, each one squashed commit under §717's checks:

1. The sidecar writes into `src/sidecars.rs`: develop's file held a write that 16 files call.
2. The capture driver (`Shown`, the import steps, the snapshot and screenshot code) out of `panel/viewport.rs` into `src/capture.rs`.
3. The browser's opening path into `panel/browser/open.rs`, about 630 lines, measured before it is specified.

A cycle inside one crate is legal, so if the two sides want a visible boundary it is two directories, `panel/library/` and `panel/develop/`.

The move shows one duplication: "write the XMP, then the sidecar" is written three more times beside `write_file` (culling's unreject, the batch save, sync).

The measurement is one pass of §717's script with the edges each change would cut taken out; it has not been reviewed apart from this section.
