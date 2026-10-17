# §327. A look previewed under the pointer (2026-10-16)

Resting on a row of the Look list shows the picture with that look, at
the Strength set, until the pointer leaves; a click keeps it. We built
it as the roadmap put it: the viewport applies the look at draw time
from a table it hands the renderer, so a preview is a swap of that
table and nothing else. No develop, no history step, no sidecar
written, and the edit is never touched. It lives in
`crates/greycard-ui/src/panel/look_preview.rs`.

**The swap.** The draw resolved the kept look into `State::look` once
per choice and handed it to `Renderer::set_look`. It still does; the
handing is now `look_preview::drawn`, which gives the previewed look
while one has landed and still fits, and the kept one otherwise.
`set_look` already told one table from another by its `Arc` and
re-uploaded only on a change, so a preview costs one upload going in
and one coming back, and a Strength is four floats as before. The kept
look is never re-resolved by a preview: `State::look` and `look_for`
are untouched, so leaving a row puts the kept `Arc` back on the binding
without a read, even when the hovered tables have pushed it out of the
look cache's four.

**The read.** The click path reads its table on the window's thread, in
the draw, through `LookLut::look_under` and the look cache. A hover
cannot: a sweep down a long list would parse a table a row, and a
64-node `.cube` is a quarter of a million lines. So the preview calls
the same resolution (`look_under_in`, with the variant for the
picture's display curve and the gate) on a thread of its own, the
pattern the look sheets' counts use, and lands it through
`invoke_from_event_loop`. Because it goes through the same cache, a
click after a rest finds the table read already and keeps it without a
stall. None's row has nothing to read and lands at once as no look; a
table fitted under another curve lands as no look too, which is what
the click would show. The status line says nothing.

The cache holds its lock through a read (about 20 ms for a 64-node
table in a release build, ten times that in a debug one), so a sweep
would queue reads behind each other for rows long left. Each ask
stores its number in an atomic the thread checks before it starts its
read, so an ask replaced before its thread runs reads nothing; a read
already waiting on the cache's lock still completes and is dropped.

A table that lands is shown only if it is still the newest ask and the
pointer is still on its row. One that lands with the pointer off the
row is dropped, and the preview remembers that it was: the first cut
kept the row's name on the preview and took it as "asked already", so
leaving a row for a moment and coming back within the rest never
showed its table at all. A return to a row whose table was dropped asks
again; a return while the read is still out waits for it.

**The rest, and the leave.** No hover delay existed to reuse, so the
module has its own, `REST`, 150 ms. The pointer coming onto a row arms
it for that row; leaving the list arms it for the kept look. We made
the leave wait as well as the arrival because rows sit 2 px apart: a
leave that restored at once would show the kept look for a rest
between every two rows of a sweep, a strobe. With both waiting, a sweep
goes from one look to the next, and the last row's table stays on show
until the next one's lands. The row reports its name with its leave,
and a leave for a row the pointer has already moved off to another is
ignored, so the order in which Slint reports two rows' hovers does not
matter. The kept look's own row asks for nothing.

**Rows that go without a leave.** Slint reports no leave for an item it
destroys (`send_exit_events` walks only the items still there), and two
things destroy the row under a resting pointer. The panel put away
(Tab, F8, the lightbox) takes the whole subtree with it, so the first
cut left the preview standing over a picture with no panel, and still
after Tab brought the panel back; another tab and the section folded do
the same. The window now raises `look-list-gone` on a change of any of
the three, which ends the preview and takes the pointer to be off the
list; the next move over a row reports it afresh. And `show_looks`
always puts up a new model, so every row is rebuilt: on a click, but
also from `show_edit` on an undo, a history pick or a sync landing in
the background, after which a flick off the list left the preview
behind. It now ends the preview and forgets the row wherever it
replaces the rows. The hover state sits in a `RefCell` on the state for
that, since `show_looks` and `show_edit` only read the state, as
`look_seen` does for the same reason.

**What ends it at once.** Besides those: a click (the list's own path,
unchanged, which records the one history step), a frame opened or
picked, a new list, culling entered, the Strength or the display curve
changed, the look folder listed again. And the draw ends any preview
that no longer fits what it is drawing: another frame than the one it
was asked on, a held picture or a history row's peek over it, culling
or a pick still reading its sidecar, a Strength or curve other than its
own, the Develop tab not on show, the section folded or the panel put
away. That check is the guard against a stale table after a frame
change that some path we did not wire reaches: the preview carries the
frame, the Strength and the curve it was asked under, and none of it is
trusted past them.

**Tests.** Eleven in `panel/look_preview.rs`, on the headless window
with real pointer moves over the rows the screen reader knows as "Look
Neon" and "Look None", a scratch look folder of two tables told apart
by size, and the reads landed when the test says (the test build reads
at once and holds the result). A rest shows the look at the Strength
set and leaving brings the kept one back, with no step and the edit as
it was past the save's wait; the kept row asks for nothing and None
shows no look; a table landing after its row was left, or after the
pointer moved to another row, is dropped; off the row and back within a
rest shows its table, asked again if it was dropped and read once if it
was still out; a sweep never shows the kept look between two; a frame
change ends it, and a table for the last frame landing after it is
nothing; a Strength or curve change ends it, reported or seen only by
the draw; the pointer leaving the window ends it; F8 and Tab end it
and the panel back shows the kept look; the rows put up again by
`show_edit` end it; a click keeps the look as exactly one history step.
Undoing each of the three fixes above fails its test.

**Run live.** The review drove the hover in a headless editor with
`--keys move:X,Y`: a rest on a row greys the viewport, histogram and
navigator while the kept look stays chosen and the strip's thumbnail
keeps its color; moving onto the picture brings the kept look back;
Tab with a preview up shows the kept look; a sweep from one row to the
next shows the next. That is the real thread through
`invoke_from_event_loop`, which no test runs (the test build stands in
for it). Scrolling the panel
under a still pointer reports the new row's hover a move late, one
tick of Slint's; we left it. No other list previews on hover, and the
list has no keyboard navigation to leave alone.
