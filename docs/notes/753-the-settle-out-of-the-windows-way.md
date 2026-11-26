# §753. The settle, out of the window's way (2026-11-26)

§745 sharpens what the window shows first and the whole picture
after, in a settle. On a 102 MP frame on a laptop's GPU, panning
after a Detail move then lagged for five to ten seconds. Two things
added up, both measured on a slow software GPU at that size, where 37
of 480 pan frames were shown:

- **A pan restarted the work.** A view moved past the first round's
  tiles made a new first round, which could not give way, and a fresh
  settle after it. While the pan went on, the settle never finished.
- **The window waited behind the settle.** The worker draws on the
  window's own device and queue, and the settle's submissions ran to
  250 ms each.

Before §745 that work ran before the first show, while the user was
waiting anyway. This section amends §745's "Giving way".

**A pan asks nothing.** The settle no longer gives way to a view
that moved. Tiles a pan bares stay soft until it lands, which §745
already accepted.

**A zoom in, into the picture, has a round of its own.** A view that
needs more iterations than the round on screen ran, over a part of
the picture, gets one: 1:1 after a move at fit is judged at every
iteration without waiting for the settle. Where the view covers the
whole picture the settle is the answer. Each such round runs more
iterations than the last, so a wheel from fit to 1:1 makes two. It
gives way to a newer develop as the settle does, and it keeps the
earlier round's pixels outside the tiles it sharpened, so zooming
back out shows nothing unsharpened.

**Short submissions.** The settle and the zoom's round run under an
8 ms budget (`BACKGROUND_BUDGET`): the iterations in batches cut to
fit it, and the whole-picture passes in bands of rows, each its own
submission, with a give-way asked between. A band is exactly the
whole pass's rows, and tiles are worked the same in any batch, so the
picture is the same to the bit. No settle makes more than 2,000
submissions, so a device where one tile's iteration outruns the
budget takes longer slices and not 125,000 waits. A background run
stores no rate; a foreground run's batches are as they were.

**The working set.** The sharpen's planes are 1.8 GB at 102 MP and
were kept for the session. On a discrete card they still are. On
anything else, where the card's memory is the machine's, they are
made when an adjustment begins, kept through it (the first round, the
settle, and any develop that cancels it), and let go when the frame
is at rest with nothing queued.

**Measured** at 102 MP. On the software GPU: 478 of 480 pan frames,
the worst 28 ms in the pan and 137 ms over the settle (it was 744),
the first show at 3.9 s as before. On a 5070 Ti: every frame shown
in a pan, a zoom, and a wheel, the worst under 30 ms, and the settle
landing in 0.4 s where it had been restarted for 4.6. Not run on
Metal.

**Left.** One to three frames a settle still pass 100 ms on the
slow device: the threshold search, the delta's read-back, the stages
after the sharpen, and making the working set are not split.
