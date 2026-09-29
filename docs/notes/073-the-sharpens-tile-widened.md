# 73. The sharpen's tile, widened (2026-09-18)

§56 left the capture sharpen's tiling as the last of its structure:
32-pixel tiles with a 5-pixel border of context computed and thrown
away, 42 by 42 for 32 by 32 kept, 1.72 pixels for every one used. The
border is what lets a tile deconvolve without its neighbors, and it
cannot be made much thinner — RawTherapee's 5 is already short of the
distance twenty Richardson-Lucy iterations propagate — so the waste
comes down only by keeping more of what each border pays for.

`TILE` is now 128, which computes 138 by 138 for 128 by 128: 1.16
against 1.72, and over a whole 45 MP frame with its part-tiles at the
right and bottom edges, 1.17 against 1.73. A wider point spread takes
a border of 8 and the same move takes 2.25 to 1.27.

The early stop had to stop being the tile's. It ends a patch's
iterations when any pixel in it falls under half its blended start,
which is the guard against a halo going dark, and one such pixel in a
128-pixel tile would have held back sixteen times the area the old
tile did. So the tile now carries a grid of 32-pixel blocks, `BLOCK`,
each with its own verdict: the iterations run over the whole tile, and
a block that trips the floor is blended into the answer there and
then and marked settled; the tile stops when the last of its blocks
has. A block with nothing to sharpen in it — the old whole-tile skip —
is settled before the first iteration. Since `TILE` is a multiple of
`BLOCK` and tiles start on their own multiples, the blocks sit on
exactly the old 32-pixel grid, so the stopping is the decision it was
and only the context behind it is wider. That guard is not a corner
case: §43 found it firing in tile after tile on a downsized export at
radius 0.8, and it is what keeps the halo out. The check itself is a
minimum over the row rather than a search for the first pixel under
the floor, which vectorizes; branchy, it cost 38 ms of the frame,
reduced to 18.

Rows of tiles are the unit the work is handed round in, so `tile_for`
halves the tile until the picture is at least 32 of them tall: a 45 MP
frame and a 24 MP frame both keep 128; a landscape export downsized
to 2048 on the long edge takes 32, as it had before, and a portrait
one 64, rather than leaving most of a machine idle. It reads the picture's height and not the machine's
thread count, so the answer does not depend on where it ran.

**Checked.** Release build on the 16-core desktop, otherwise idle,
each figure the best of four runs on the same developed picture, the
old code and the new built from the same bench and run alternately.
On the 45 MP R5 Mark II frame of §56 (5464x8192, two thirds of it
sharpened) the sharpen went from 590 to 435 ms; on a second 45 MP
frame with a quarter of it sharpened, 525 to 439; on a 24 MP R6
Mark II frame, 283 to 243. A sharpen slider change, which redoes the
base copy, the sharpen and the half floats for the GPU and nothing
else, is 660 to 503 ms on the first frame, 594 to 511 on the second
and 320 to 284 on the 24 MP one — the 0.65 s of §56 is now 0.50.
Tile 64 and 256 were measured too, interleaved in one process: 599,
519, 467 and 459 ms for 32, 64, 128 and 256 on the first frame, and
282, 253, 236, 245 on the 24 MP one, so 64 leaves a third of the gain
and 256 is a wash on a big frame and worse on a small one; past that
the four tile buffers leave the private cache and it gets worse
still. Row bands the width of the frame, the other shape on the list,
lose on both counts: a 32-row band wastes more (1.31, the border only
above and below, against 1.16) and its buffers are 5.5 MB a thread
rather than 300 KB; 538 ms against 467 for the square tile in the
same run, and 640 and 998 for bands of 64 and 128 rows.

The answer is not bit for bit the old one — a different tile is a
different sum and a different amount of context — and the difference
is the old tiling's seam error going away. On the 45 MP frame the two
agree to 3.7e-7 in the median and 5.5e-4 at the 99th percentile, and
the worst 0.01 percent of pixels sit, 56 percent of them, in the four
columns either side of an old 32-pixel seam that hold 12 percent of
the area. The test puts it directly: on a synthetic bar pattern
harsher than any photograph, deconvolved in one tile wider than the
picture — no seams at all, which is the answer a tiling is trying to
be — the 128-pixel tiling is eight times nearer that answer than the
32-pixel one was in the mean, and nearer at its worst as well, while
the two tilings are nowhere a part in a hundred apart. Column by
column across an old tile, the mean departure from the untiled answer
was 737 parts per million at the seam and 70 to 110 in the middle;
it is now 87 and 6 to 20.

What was tried and did not pay. Turning the early stop off entirely
saves 18 ms of the 435, so the guard is not what the time goes on and
there was nothing to win by loosening it. And a tile that falls back
to 32-pixel work when few of its blocks have anything to sharpen would
buy something on a frame that is mostly sky — the flat blocks inside a
wide tile are iterated as context now, where before they were skipped
— but on the frames here it is worth a few percent for a second path
through the loop.
