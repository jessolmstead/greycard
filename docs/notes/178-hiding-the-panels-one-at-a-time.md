# 178. Hiding the panels one at a time (2026-09-26)

Tester report #3 asked for the panels to put away; the roadmap had
the shape of it already: F7 the left pane, F8 the develop panel, F6
the strip, Tab all three, remembered, and an arrow on each edge.

`panels-hidden` on `App` became three, `left-hidden`, `right-hidden`
and `strip-hidden`, and each place that read the one reads its own.
The side panes still leave the layout by `if` and the strip still goes
to zero height, for the reasons in the Tab note (their wrapping texts
at zero width; `App` reaching `strip.content-x`). The keys are
Lightroom's: F7 and F8 are its side panels, F6 its filmstrip. They
take the same guards as Tab: nothing under a sheet or over the grid,
nothing with a modifier held.

Tab keeps its old meaning where there was one to keep: from everything
shown it puts everything away, and from everything away it brings
everything back. From a mix it puts the rest away, so a second Tab
always shows everything; Lightroom's Tab does the same with its two
side panels. The status plate goes only when all three are away, which
is exactly the old Tab state, so `--hide-panels` captures as before.

The three are kept in `settings.json` as `hide_left`, `hide_right` and
`hide_strip`, gathered with the rest when the window closes. Tab
reversed its earlier "not remembered": all three away is now a state
the next launch opens in, but Tab brings them back from there, and the
arrows (below) stay findable. A run opened with `--hide-panels` writes
back what the file already held, since the flag is a look at the
picture alone and not a choice about the next session.

The arrows are drawn over the viewport's edges rather than in a gutter
beside it, so they take no width from the picture: 16 by 40 px at the
middle of the left and right edges, 40 by 16 at the middle of the
bottom, on a translucent plate, the chevron pointing the way the edge
will move. They sit before the grid and the sheets in `app.slint`, so
both cover them. With all three away they are quiet, drawn only under
the pointer, so that state is still the picture alone (in a
`--snapshot` too) and a hand that folded all three by the arrows can
still find them. `--screenshot` reads the viewport's texture and never
had them. A click hands the keys back to the window, as the viewport
does, so the next Tab or arrow key is not lost.

Resizing, the other half of the roadmap line, is still to come: the
widths are now `left-w`, `right-w` and `strip-h` on `App`, one place
for a drag to set.
