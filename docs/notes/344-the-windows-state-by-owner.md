# §344. The window's State, by owner (2026-10-17)

The second pass on the window's structure after §341. `State` in
the window's `main.rs` was 223 fields in a 673-line struct, and every
panel reached into it by name. It is now 17 fields: five every
owner reads, the five sheets that already had a struct, and seven
owners, each a struct in `owners/` reached as `st.<owner>.<field>`
as culling's was as `st.cull`. Nothing a user can see changed.

## The owners

- `views` (44): the library views. The browser's rows and their order,
  the filter and the index, the roots, collections, stacks and folder
  tree, the selection, the thumbnails and the clocks of a folder's
  fill and a select, the rows' sidecar reads. Not `library`: the
  roots had that name, now `st.views.library`.
- `develop` (78): the open frame and its edit, the develop's
  generation, the picture on the GPU, viewport and zoom, the crop,
  mask, parts and retouch tools in hand, the panel's lists (looks,
  profiles, presets, the model store) and the display tables.
- `culling` (10): the mode, the lightbox, the camera's pictures held,
  awaited and prefetched, culling's undo.
- `sheets` (27): the sheets with no struct of their own (delete,
  remove and rename a look, the download and lens offers, save a
  preset, sync and paste with the clipboard). The five with one,
  import, Lightroom, RawTherapee, camera match and archive, stay on
  `State` as they were.
- `work` (13): the exports, their queue and kept settings, and the
  writes to the archives' copies.
- `window` (7): the app handle, the settings file, and whether this
  session writes sidecars, where, may delete, and is a batch.
- `launch` (34): what the command line asked of the run: captures,
  timings, the actions once the first picture is up. Not one of the
  roadmap's five: they belong to neither side.

Staying at the top: `files`, `cells`, `current`, `sidecars` and
`from_row`, how any owner names a frame. `files` and `current` are
read in 42 modules each, `sidecars` in 29 and written by every side,
`from_row` runs beside it by file number, and `cells` is what
`current` and the selection number into. The selection (`picked`)
went to the views; `current` is the frame develop opens too.

## How, and the proof

A probe first: every field taken out of `State`, and the compiler's
refusals (`cargo check`, test and non-test) listed all 6,038 accesses
and 269 initializers by module; the inventory is that list with an
owner a field. Then one owner a commit, launch first and develop last,
each building and passing the window's 1,322 tests (20 ignored) on its
own: a script moved the fields, doc comments and initializers verbatim
into the owner's struct, and the compiler's refusals again said where
to put the owner's name, 4,258 accesses in all, each count equal to
the probe's; a last commit moved three fields the review found read by
another owner (33 accesses). A first commit binds the launch's values
in its literal's order, so `State::empty` still runs after every value
the launch reads.

A diff of master and the branch with `.<owner>.` taken out of both
sides, read as tokens, is empty outside `main.rs` and `startup.rs`
but for braces rustfmt moved on five rewrapped closures and the doc
comments that named moved fields as State's; every line that left
`main.rs` arrived in `owners/`. Clippy and fmt are clean. A
`--snapshot` of a raw and one with `--sheet export`, master's binary
against the branch's, differ only in the status line's develop time,
as two runs of master do.

Not a move: `State::empty` builds launch, window, sheets and work,
then culling (its prefetcher starts its threads), then views and
develop. Every hand-written `Default` in the crate is plain data, so
the order is free. Fields drop by owner.

## What the third pass will want

The views' fields are reached 302 times from 21 modules outside the
library side (the archive copies' sync, the archive, the delete, the
Lightroom and RawTherapee sheets, the launch), culling's 52 times
from 13, and the library side reaches develop's 230 times, nearly all
from `panel::cull` and `panel::browser` opening a frame. Those are
the borrows to narrow first; every function still takes `&mut State`.
