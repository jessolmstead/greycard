# §444. A tool's hint, remembered

*2026-10-28*

A tester deselected Fill while the fill model was at work and the
status line's "developing... the fill model is at work on Fill 3"
vanished, the busy bar still going. Every exit from a tool blanked the
line to take the tool's own hint off it, with no look at what the line
held.

The panel now remembers the hint a tool put up. `show_hint` in
`panel/deliver.rs` writes a hint and keeps it on the develop state;
`drop_hint` clears the line only while it still says that hint, then
forgets it. The repair tool's hint, a placing's, the Color mask's, the
droppers', and the Part picker's "which person" all go through it, and
every exit, Esc, a tab change, a mask switch or removal, the repair
toggle, and entering culling, calls `drop_hint`. A develop's word, a
worker's line ("exporting 3 frames..."), or a dropper's miss written
over the hint is then never touched by a tool going down.

A first draft matched the one hint at the three repair exits; a second
spared any line starting "developing..." or "failed: ". Both were
lists, the second a denylist that would have dropped a worker's line
and lost its guard on any develop word phrased otherwise. Remembering
the hint needs no list.
