# 177. The first outside tester's reports (2026-09-25)

Eight issues came in on the morning of 2026-09-25 from one tester on
0.1.1, Windows 11 and an RTX 5080: #2 to #10, #8 closed by the tester.
Each was traced to its code first, then the bugs went out to agents in
worktrees, one each, with a fresh reviewer per branch and one round of
fixes. Two of the reports were requests (#3, hideable panels, already
a backlog line; #10, below) and one (#9) was a switch that did what it
was built to do and should not have existed. The reviewers found a
real defect on four of the six branches: a dropper that read the wrong
pixel for the second a turn takes, an Alt key that stuck on Wayland, a
preset restore that could say every default was back while one was
not, and a source that could land on the wrong clone spot.

**Undo on a clone (#7).** A new clone or heal spot is placed with no
source; the develop chooses one and hands it back. The panel wrote it
into the edit and scheduled a save, which recorded a state. The save
rests 800 ms after the release and the develop starts after a 300 ms
debounce, so on a large raw, where the develop takes more than about
half a second, the save came first and recorded the spot without its
source, and the source landed as a second state. Undo stepped back to
the sourceless one, which developed again, chose the same source (the
search is deterministic), and recorded it again, which cleared the
redo: the spot came back, and it took a second undo to get past. A
chosen source finishes the step that placed its patch; it is not a
step. The develop now hands back the patches it chose for, whole, and
`Sidecar::take_sources` puts each source into every state still
waiting on that patch (the history, the redo, the snapshots) in place,
recording nothing. A patch takes a source only from one equal to it in
every other field, not just by id, because ids are reused (`next_id`
is the highest plus one): a spot deleted and replaced under the same
id while its develop is in flight, or moved or resized in flight,
takes nothing. A sidecar 0.1.1 already wrote with the spot twice heals
the first time the sourceless state develops: it becomes equal to its
neighbor and the two merge, keeping the earlier step's words. Not
recording a sourceless state at all would not have closed it, since
undo records whatever the panel holds before it steps.

**The Tone curve switch (#9).** The switch under LIGHT was
`light.tone.enabled`, and off it took away the base display curve, the
ACES fit and its shoulder, for a clip at 1.0, while the 0.8-stop
baseline still went on. A raw with nothing done to it went darker the
moment it was flipped (a mean of 0.510 to 0.459 on the test export),
and Contrast, Highlights, Shadows, Whites and Blacks greyed out with
it. The curve is not an edit: it is how a scene-referred raw becomes a
picture at all, and the baseline and every Light slider are tuned on
top of it. So the switch is gone and the base curve always applies to
a raw; a picture already rendered for a display takes its clip at
white as before. The Light section's own switch had the same fault by
another road, since `Light::effective()` turned the tone off with the
exposure; off now means exposure zero and the sliders neutral over the
curve, so switching it off with nothing set changes nothing, measured
on an export to zero differing pixels. The viewport's `curve` uniform
keeps only the raw-versus-display split, and a GPU test holds each
source kind to the CPU's `finish_pixel` (0.0024 at worst).

Two things render differently for files that exist. With the picture's
Light off, the shape was skipped whole, so every mask's tone sliders
did nothing either; now they act whatever the picture's switch says,
as the masks' exposure already did. And a mask whose own Light was off
kept adding its tone sliders, since only the curve's switch was
cleared; now they are neutral. `Tone` loses its `enabled` field; the
edit structs are `serde(default)` without `deny_unknown_fields`, so an
old sidecar or preset still reads, the field ignored.

**Removing a preset (#4).** One click on the bin deleted the file, and
the seed's rule that one binned stays binned left a shipped preset
removed by accident no way back. Now the bin arms the row: a muted
line asks "Remove <name> from the list?" and the Import and Save row
turns into Cancel and Remove, the same swap the settings sheet uses
for its sidecar move. The name is on its own line because the pane is
too narrow for "Remove Red-Filter Mono" on a button, which the first
cut tried. Escape, applying a preset and every re-show of the list
disarm it. The way back is `Store::restore_shipped`, behind a Restore
button in Settings beside the thumbnail cache, shown while a shipped
preset is missing. Missing means no preset in the list goes by its
name, case aside; a file under the shipped stem does not count, since
it may hold a preset the user renamed or not read at all, and the
first cut, which counted it, could say every default was back while
one was not. The restore overwrites nothing: the shipped stem when it
is free, else the first free `stem-2`. The confirm is tested as a
pointer uses it, `testing::click` sweeping the column of bins.

**The linear gradient's pivot (#6).** Dragging an end line moved only
that end, so the gradient turned about the other line, its middle
drifted and its width changed with its angle. Now the dragged end goes
to the pointer and the other is its mirror through the middle of the
shape as it was pressed, so the middle is the pivot and the width
changes on both sides at once. Near the middle the drag still follows
the pointer's direction with the half-length held to the minimum
radius; only the exact middle falls back to the press-time direction.
The old drag is kept on Alt, for pinning one edge on a horizon and
lengthening only the fade, with the same minimum length. Alt is read
from each handle's own pointer event on every move. The first version
kept it from the window's key events and a reviewer caught two ways it
stuck: winit on Wayland sends no release for a key held when focus
leaves, so Alt+Tab away left it held, and the root FocusScope never
sees a key while a slider or a text field has focus. Whether Alt
reaches the app depends on the desktop: GNOME moves windows on Super,
so it does; Xfce, Cinnamon, MATE and Plasma 5 take Alt+drag to move
the window; Windows binds nothing to it.

**The frame turn (#5).** Turn left and Turn right, and `[` and `]`,
turn the sidecar's orientation, which the engine applies in its own
orientation step, so the turn waited for a develop at the new turn.
The crop and the masks turned at once and the picture 1.3 s later on a
45 MP CR3, which read as the key not having taken. Now the viewport
turns the develop it has: the render keeps the turn the develop on the
GPU was made at, and the difference goes into the shader, which maps
each source position to the texture position holding it (the inverse
of `orient`'s quarter turns, pixel centers on pixel centers) before
every read of the picture and the guide. Masks are already where the
turn put them. The droppers read through the same map in whole pixels,
which the first cut missed: for that second a white balance pick read
the pixel turned away and saved it. The size the edit is measured by
is the open frame's own develop turned by the difference, so a turn
from the culling loupe over a selection cannot leave the crop a
quarter off. Several turns stack in the one difference and the
develops asked for on the way are dropped as stale. The picture turns
2 ms after the key; a GPU test holds the read through each quarter to
the texture turned on the CPU, identical to the level, cubic and
bilinear, with a guide.

The develop itself is shorter only when nothing after the engine's
orientation step cares which way up the picture is. Everything before
the step reads the mosaic and the step is a permutation, so the kept
base is turned where it stands and its guide read again, matching a
fresh develop to the bit (tested with a mirrored tag and the learned
denoiser's pair): 0.07 s against 2.1 s for the base. The lens
correction and the defringe run after the step and are symmetric only
in exact arithmetic; on the same CR3 the lens profile moved 77% of
samples by up to 1.0e-4 and the defringe 3% by up to 1.8e-3, a
threshold landing the other way for a few pixels. A frame with either
on still develops afresh at its new turn, so most raws still pay the
full develop; they no longer wait for it to show.

**The Masks tab (#10).** The adjustment's own controls (name, Show
mask, Invert), the chosen shape's (its invert, feather and range
windows) and the adjustment's look sat in one flat style. Now the
shapes are always shown under a plain "Shapes" label, the fold over
them gone (it was never saved), and the chosen shape's controls sit in
a card under the list, headed with its kind and numbered in row order
when the kind repeats, "RADIAL 2"; the chosen row takes the selection
tint the history and the grid use. The shape's switch is "Invert
shape", leaving one plain Invert. Add / Subtract / Intersect stays
outside the card under a "Next shape" label: it is how the next added
shape joins, nothing sets it from the chosen one, and inside the card
it named the wrong mode for an intersected shape, which the review
caught. It dims while a chosen brush, sky or object is being added to.
The add buttons stay in view, since three of them are the chosen
shape's own tools (Paint, Pick); moving those and the brush options
into the card is a possible follow-up.

The look sections carried the adjustment's name after their title,
"LIGHT · Adjustment 1". It now comes first, "ADJUSTMENT 1 · LIGHT", as
a text of its own that is the part cut when the panel is short, so the
section's word stays whole. A blank name reads as the adjustment's
place in the list, so the Masks tab never shows a bare "LIGHT" that
could be taken for the global one.

**The left pane and the window (#2).** The left pane's column (Open
folder, Navigator, Presets, Snapshots, History) had no scroll of its
own and Snapshots no cap, so the column's minimum height was the sum
of its sections, and Slint hands a layout's minimum up to the window.
On Windows a minimum past the screen takes the window out of
fullscreen and grows it: 80 snapshots made a 1500×950 window
1500×3310. Now Snapshots caps its list as Presets and History do, and
the column sits in a ScrollView with Settings and Report fixed below
it, the right panel's pattern; a ScrollView passes no natural size up,
so the same 80 snapshots leave the window at 1500×950 with a minimum
of 473. The lists had also been squeezing to a two-row floor when the
pane ran short; under a scrolling pane that shrank them unevenly
before any scrolling, so `Section.shrinks` is gone and each list sits
at its capped height. The fluent ScrollBar's 14 px touch band lies
over a list's right edge, where the bins are, so a click on a bin's
edge paged the list; the rows now keep clear of it.

That branch also moved the Remove button out from under the #4 test
that clicks its way down the pane, and the sweep went on past the list
and clicked Report a problem... on every step: the test harness wires
the real callbacks, and each click opened the bug form in the browser,
enough tabs to crash it. The browser, the file manager and the file
chooser now do nothing under `cfg(test)`, and the test waits out the
confirm row's 120 ms resize on the mock clock and stays inside the
presets section's own rows.
