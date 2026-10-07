# 258. A bar on the camera match run (2026-10-07)

The camera match's sheet said only "starting...", then the group in
hand, so a run of a few minutes gave no sense of how far along it was.
It now fills a bar, §192's, with the count under it.

**One bar for the whole run.** The bar is frames developed over frames
to develop, across every group the run takes on. It only moves
forward:

- A frame is counted once it has been measured, so the bar reaches
  full.
- When a group is passed over, the total drops at once and the bar is
  told. A group is passed over when it borrows a look whose donor wrote
  nothing, for example because too few of its frames registered.
  Without that, a run whose last group was passed over ended at 83%.
- During a group's fit, the held-out frames are counted in the words
  ("11 of 25 held out · Canon EOS R5 Mark II Faithful"), and the bar
  holds where the frames left it.

The count comes first in the words ("12 of 25 frames · <group>"),
because a long group name is cut at the end.

**In order.** Each count is taken under the same lock that posts it, so
the window hears them in order. Before, held-out counts from the fit's
threads could arrive out of order and the words stepped backwards. A
run that has finished drops anything still on its way, so a late
message never brings the bar back over the end text.

**The sheet stays still.** The loading card, made for the grid and the
loupe, spans the sheet here with no plate, in the sheet's own text
size and color. Its room, two lines of words, is reserved whenever the
sheet is up. So the buttons don't move when a run starts, at Stop, when
the last group is unchecked, or for a two-line end text. A longer one
is cut at the end of line two. The result lines under it still grow
the sheet as they did before.

**Stop.** The bar holds and the words say what is left:

- "stopping after this frame..." during the frames;
- "stopping after this group..." during a fit, because a fit isn't cut
  short and its look is still written.

**Lag.** The author saw the sheet a frame or two behind the log. The
reviewer couldn't reproduce it over 28 frames at a load of 25: each
capture matched the log's count. The sheet counts frames finished,
while the log names the next frame as it starts, so beside the develop
lines it reads one behind.

Left as is: the two lines' height is reckoned from the font size
(2 × 1.35 × the small size), not measured. With Noto Sans it fits
with 0.4 px to spare; a fallback font with taller lines would elide
line two.
