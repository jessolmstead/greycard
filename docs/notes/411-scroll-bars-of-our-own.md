# §411. Scroll bars of our own

*2026-10-23*

Two testers missed a scroll bar: one did not see that a panel held
more below (issue 30), one asked for bars that stay and a size for
them (pixls, 2026-10-22). The panel and every list drew Slint's
fluent ScrollView, whose bar is a 2 px thumb at 8 percent white
that no theme token reaches.

So the bar is ours now. A ScrollPane in the controls, a Flickable
with the grid's own band down the right and along the foot, takes
every ScrollView's place in the panel, the left pane, the lists,
and the sheets, with the same API, the same wheel, no focus and no
keys as fluent had none, and a press in the track pages, which
fluent did not. The reviewer read fluent's source against it. A
ScrollBars global holds two settings in the Interface category:
Fade or Stay, and Thin, Medium, or Wide (2, 4, 6 px at rest, 6,
10, 14 lit, the band grown with it), read leniently like the theme;
every bar follows at once. Under Fade a pane's thumb stays drawn at
rest as fluent's did, since a bar that hid itself is what issue 30
complains of; the grid's and the strip's fade fully. The resting
thumb is a step brighter on every theme (Charcoal 4.8:1 to 6.4:1 on
the panel), and the pane's padding and the strip's foot grow with
the size so a wide track covers nothing. The fluent ComboBox's
popup list and TextEdit keep std's own bar, out of reach without
the fork.

## Not done

- The history rows clear the band at every size now, which costs
  10 px of elision room at Thin where the thumb already cleared
  them.
