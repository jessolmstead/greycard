# 132. The decisions leave the panel (2026-09-21)

§130 split `main.rs` into one module a panel section and said, in so many
words, that it had left something undone: where a helper being moved held a
decision, the decision belonged in a pure module and only the glue belonged
in `panel/`. The move went first and stayed a pure move, so a dimmed diff
could vouch for it. This is the second pass, one decision a commit with its
test, and the two open interaction bugs at the head of it, because they are
the reason the pass is worth doing at all: both sat in the wiring, and no
CPU reference test was ever going to find them.

### The mask lines over the left bar

The viewport was not clipped. Everything drawn over the picture — a linear
gradient's outline, a radial's ellipse, the shape handles, the brush under
the pointer, the crop's shade — is placed in view pixels off a geometry that
is free to run past the edge of the view. A gradient's outline is
deliberately four thousand pixels long, because it has to reach the corners
at any angle; a handle sits wherever its shape sits, which under a zoom or a
pan is off the picture entirely and at a negative x. The viewport `Rectangle`
in `app.slint` had no `clip`, so all of that was painted over the navigator,
the snapshots and the history, and over the filmstrip below.

It also took their clicks. A `CropHandle` is a `TouchArea`, and an unclipped
one outside the viewport is still pressable, so a mask handle that had
drifted left was sitting on top of the history list. That is the half of the
bug nobody had reported yet, and it is the half a test can hold.

The fix is one line, `clip: true`, and it is entirely in Slint: there is no
Rust decision to move, because the overlay is meant to run past the edge and
the viewport is meant to be the thing that stops it. What is testable is the
consequence. `a_handle_off_the_picture_does_not_take_the_left_bars_clicks`
puts a handle at a negative x through the panel's own `mask-handles`
property, clicks where it would be over the left bar, and asserts
`mask-grabbed` did not fire; then puts the same handle on the picture and
asserts it did, so the test is about the clip and not about the wiring. It
fails on the old `app.slint`.

Why nothing caught it: every test the crate had ran under the picture, on
numbers. `shape_handles` was right, `source_to_view` was right, and the
overlay was right too — the bug was that nothing said where the overlay
stops, and only a window can be asked that. §130's `testing` module is what
makes it askable: a headless window, laid out at 1500 by 950, that answers
pointer events.

### 'Original' on a frame shot on end

`Aspect::Original.ratio` stood the plane up as landscape and then read which
way up the crop should go off the `portrait` flag, which starts false. So a
frame shot on end, developed 5464 by 8192, picked "Original" and got a
landscape crop of itself. The flag was doing two jobs: for a named ratio it
is the only thing that says which way up (a 3:2 is written landscape
whatever the frame is), but `Original` is read against the plane, and the
plane already says.

So `Original` keeps the plane's own ratio now and `portrait` turns that, and
`Geometry::turned` stops flipping the flag for `Original`: the plane turns
under it, and flipping the flag as well would turn it twice. The two wrongs
had been cancelling — a landscape frame turned on its side came out right,
which is why the bug only ever showed on a frame that arrived upright.

Why nothing caught it: `aspects_and_handles` tested `Original` on a 6000 by
4000 source only, landscape both with the flag and without, and the turn
test happened to use `Original` where a named ratio was what it meant. The
one case nobody wrote was the one the camera writes every time somebody
turns it ninety degrees. Two tests now: the ratio on `(H, W)` as well as
`(W, H)`, and, through the headless helpers,
`original_on_an_upright_frame_crops_it_upright`, which sets the developed
size on a file-less `State`, invokes `geometry-changed` as the aspect list
does, and asserts the crop that comes back is the whole upright frame — and
that the Portrait toggle still turns it.

### Which is a change of meaning, so: schema 4

A fix to a rule is a fix; a change to what a stored field means is a
migration, and this is the second. Every sidecar on disk that names
`Original` was written against the old reading, and the old build flipped
`portrait` on every quarter turn, so what it wrote for the ordinary
"pick Original, then Turn left" on a landscape frame is `turns: 1,
portrait: true, crop: the whole plane`. Under the new reading that spells
landscape on an upright plane. It still *renders*, because a stored crop
that fits is handed back untouched — but the next refit, which is the
aspect list, the Portrait toggle, the angle slider or either perspective
slider, would find the shape wrong and quietly take 4000 by 2667 out of a
4000 by 6000 plane. Where no crop is stored at all the change shows on load.
Somebody's picture, re-cropped by a version bump they did not ask for. So
`VERSION` goes to 4.

The conversion is small and the awkwardness is where it has to happen. On a
landscape plane the flag already meant what it means now, so it is left
alone. On an upright plane it did not: ticking it was the only way to ask
for the frame's own shape, and leaving it was the bug. Both now mean the
frame's own shape, so an upright plane ends flag-false either way. That is
`portrait AND NOT plane_is_portrait`, and it preserves what every stored
edit rendered as except the one spelling whose rendering was the bug — which
is the whole point of doing it.

The awkwardness: the plane is the frame's developed picture turned by the
edit's own quarter turns, and a sidecar does not hold the frame's shape.
`migrate` is a function on JSON and has no frame to ask. Rather than guess —
a guess here re-crops a picture, which is the thing being avoided — an edit
that needs the step is left at version 3, says so through
`Edit::needs_frame`, and is finished by whoever has a frame in hand with
`Edit::migrate_with_frame`. Until then it reads, renders and writes back as
the version 3 it still is, so a build that only listed a folder loses
nothing and cannot write a half-migrated sidecar: the version field is
serialized as held, so a save and a reload are idempotent. Everything whose
aspect is not `Original` comes up to 4 in `migrate` by its version number
alone.

Where the frame is in hand: the editor migrates as a frame is selected, as
it is culled, and before a turn maps it, off `frame_aspect` — the same
priority order the masks are measured by, whose first cheap answer is the
cached `stance_path` from §131 at half a millisecond warm. The CLI's
`--apply` asks the file directly. A frame nothing can measure is left for
next time rather than guessed at.

One more thing the bump needed, which the first cut of it did not have:
`needs_frame` states an invariant — this crop must not be refitted — and
stating an invariant is not enforcing one. A frame nothing can measure
still reaches the panel, still carrying the version 3 reading of its flag,
and the angle slider would then have cropped it to that reading: the very
thing the bump exists to prevent, arrived by the back door. So `refit_crop`
takes the answer as an argument rather than checking one inside, and the
compiler asks the question at each of its five callers instead of trusting
each of them to remember. `an_edit_still_waiting_on_its_frame_is_never_refitted`
drives the panel's own callback and pins both halves: the crop is left
alone while the frame is unmeasured, and the same edit once measured takes
the refit as it always did — which is also what says the callback was
reached at all, since a guard and a dead wire look alike from outside.

Five rows are pinned by
`a_version_three_original_keeps_its_shape_once_the_frame_is_known`, one per
spelling the old build could write, each asserting the new build renders
what the old one did — except the upright-and-unticked row, which must now
be the whole frame, because that row *is* the bug.
`a_sidecar_migrates_all_its_states_once_and_only_once` covers the history,
the undone states and the snapshots, since undo, redo and a snapshot each
put one of them back on the panel, and pins that a saved and reloaded
sidecar is not migrated twice. And
`a_turned_original_from_an_older_sidecar_is_not_re_cropped` drives the panel
itself: the old build's own output through `refit_crop`, which is where the
silent re-crop would have happened.

### The rule the pass applied

Everything that decides something lives in a pure module and can be tested
without a window. A pure module may be in `greycard-ui` itself, beside
`cull.rs` and `grid.rs`; in `greycard-edit` when the decision is about the
edit; in `greycard-core` when it is engine arithmetic. `panel/` keeps the
glue: reading the panel's properties, writing them back, borrowing `State`,
sending a job to the worker.

The reviewer's note from the split went first. `finish.rs`, a pure module,
had been reaching up into `panel::color::invert3` — the one pure-module →
`panel/` dependency, and the whole reason the directory exists is to make
that visible. The engine already had its own private copies of `mul3` and
`invert3` beside Oklab's matrices; there is one public pair now, in
`greycard_core::color`, with `Matrix3`, and nothing pure imports from the
wiring any more.

| what | from | to |
| --- | --- | --- |
| `mul3`, `invert3`, `Matrix3` | `panel/color.rs` | `greycard_core::color` |
| `nearer_window` | `panel/viewport.rs` | `greycard_core::develop::defringe` |
| `neutral_gains` | `panel/color.rs` | `greycard_core::color` |
| `shape_of`, `shape_handles`, `shape_dragged`, `MIN_RADIUS` | `panel/mask.rs` | `Shape::of_kind`, `handles`, `dragged` in `greycard_edit::mask` |
| `aspect_of`, `aspect_name`, `parse_ratio` | `panel/crop.rs` | `Aspect::from_name`, `name`, `parse_ratio` |
| `Guiding`, `guide_axis` | `panel/crop.rs` | `greycard_edit::geometry`, with `Guide::from_name` |
| `turn_thumb` | `panel/browser.rs` | `geometry::turn_pixels` |
| `CURVE_HIT`, `CURVE_MIN_GAP`, `nearest_split`, `nearest_point` | `panel/curve.rs` | `curve::HIT`, `MIN_GAP`, `Parametric::nearest_split`, `curve::nearest_point` |
| `overlay_changes` | `panel/cull.rs` | `greycard_edit` |
| `turn_into`, `meta_into` | `panel/browser.rs` | `greycard_edit` |
| `history_row` | `panel/history.rs` | `Sidecar::state_at_row` |
| `flag_code`, `label_code` | `panel/browser.rs` | `Flag::code`, `Label::code` |
| `look_rows`, `look_warning` | `panel/assets.rs` | `greycard_edit::look::rows`, `warning` |
| `profile_warning` | `panel/assets.rs` | `greycard_edit::camera::warning` |
| `not_previewed` | `panel/history.rs` | `finish.rs` |
| `row_of_shown` | `panel/cull.rs` | `cull.rs`, beside `nearest_row` |
| `select_index`, `never_developed`, `sidecar_to_raw`, `list_files` | `panel/browser.rs` | new `files.rs` |
| `wheel_pick`, `wheel_place`, `WHEEL_DEAD` | `panel/color.rs` | new `wheel.rs` |
| `effective_zoom`, `view_cell` | `panel/viewport.rs` | new `zoom.rs` |

Three new pure modules in `greycard-ui`: `files.rs` (what the editor opens
and in what order), `wheel.rs` (the hue circle the three grading wheels and
the tint ring share, where the two directions have to be each other's
inverse), `zoom.rs` (what a fit is, and how much room a compare view leaves).
All three sit beside `cull.rs` and `grid.rs`, which is the shelf the crate
already had for this.

Some of the moves are arithmetic that had simply been written in the wrong
crate — the 3×3 matrices, the defringe's hue window, the neutral dropper's
gains. Some are rules the edit owns and the panel had been keeping a second
copy of the answer to: which handle reshapes a shape, what "65:24" means,
how near a press has to come to take a curve point. And some are about the
stores rather than about the panel that lists them: a look or a camera
profile the edit names and the directory has not got is still listed and
still chosen, because the panel says what the edit says, and that is a rule
about the store.

### What stays in `panel/`

The glue, and only the glue: `read_X` and `show_X`, the callback bodies, and
the thin wrappers that fetch a pure module's arguments out of `State`.
`effective_zoom(st, vw, vh)` is now three lines that find the zoom, the cell
and the image size and hand them to `zoom::effective`; that shape — a named
wrapper over a pure call — is the pattern the rest should follow.

Two things stayed on purpose.

`file_name` is still in `panel/browser.rs`, where the split put it. Seven
modules use it and it decides nothing: it is `Path::file_name` with a
`String` on the end. Moving it would have touched seven files to no
purpose.

`frame_aspect` stayed too, and this one is a judgment. It is a priority
order over four sources — the develop on screen when it is this frame's, the
size the file itself reports, the culling preview, the filmstrip's
thumbnail — and three of them are only consulted if the ones before them had
nothing to say. One of those, the file's own, costs a metadata probe of 3 to
40 ms. Splitting the decision from the fetching would mean either reading
all four eagerly, which puts that probe on every turn of every frame, or a
pure function that takes four closures and is nothing but the four `if let`s
written again with more ceremony. The order is already pinned by
`a_frame_without_its_own_develop_does_not_borrow_the_last_ones_shape`, which
drives it on a file-less `State` and needs no window, so the thing worth
protecting is protected where it is. It is the one entry on the list that
did not move, and the reason is the laziness and not the `&mut`.

The items §130's list marked cosmetic or homeless were left: `sync_rows`,
`size_text`, `date_of`, `listed`, `picking_hint`, `placing_hint`. None of
them decides anything a test would be glad to know.

### A badge on the wrong thumbnail

Found while reading rather than reported, and left in as its own commit
because it is the same shape as the other two: a decision kept in the
wiring, and no test that could have caught it. `show_badges` read the
thumbnail at the file's *row* and wrote it back at the file's own *index*.
Those are the same number only when the browser is showing everything, and
`row_of` exists precisely because they are not. Set a star under
"No rejects" and it landed on whichever frame happened to be that many rows
down — someone else's picture, wearing your rating.
`a_badge_set_under_a_filter_lands_on_the_filtered_row` hides one frame of
five, so every row is the file one along and every wrong answer is a row
that exists: the bug stamps a neighbor rather than running off the end,
which is why it never panicked and never got noticed.

### What the headless helpers bought

`testing.rs` gained one function, `click`, which dispatches a move, a press
and a release at a point of the window. With `window` and `retouch_state`
from §130 beside it, both bugs of this pass are now covered by tests that
drive the real callbacks on a real layout and no visible window: a click
that must not land, and an aspect picked from a list. That is two of the
three kinds of bug this crate has — the arithmetic, the wiring, the
drawing — and the drawing is still only reachable by eye. A handle's
pressability is the nearest a test gets to asking where a thing was drawn,
and it was enough here because the clip governs both.

The workspace is at 660 tests, up fourteen from the split's 646: most of the
pass carried a test across with the decision it belongs to rather than
writing a new one, which is the point — the tests were already pure, they
were just filed under the wiring. `clippy --all-targets` and `fmt --all
--check` are clean at every one of the twenty-six commits.

Three tests were also caught claiming more than they held: a split fixture
already in order, so nothing said the hit is read off `ordered_splits`;
`not_previewed` naming two of its seven; and the hue circle's round trip
never feeding the dead center back, which is the one place the inverse is
deliberately not total — at nothing the marker sits exactly on the dead
edge, so the strength comes back as nothing to within rounding and the hue
may or may not read, and either way the control keeps the hue it had. A
comment that says more than its assertions is worse than no comment, since
it is the comment a later reader trusts.
