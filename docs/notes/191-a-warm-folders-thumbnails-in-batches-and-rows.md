# 191. A warm folder's thumbnails in batches, and rows far from the screen left for a scroll (2026-09-27)


Roadmap line (Editor pool): a folder of 20,000 whose pictures are all
in the cache does not render its window until every hit has been
delivered, 5.7 to 6.3 s (§172, found by §174's review).

**What it was.** Each picture the pool finished was one
`invoke_from_event_loop` call. winit's X11 loop drains its whole
user-event channel (`while let Ok(event) = user_receiver.try_recv()`)
before it redraws, and that includes calls queued while it drains. Eight
threads answering cache hits keep ahead of a window that takes each one
in about 0.1 ms, so the loop stayed in that drain until the last
picture. The window's rendering setup did not run until then, and so
neither did the first frame or the first develop.

**Measured before.** A synthetic folder, `target/scratch/f20k` in the
worktree: 20,000 hard links to 50 generated 600x400 JPEGs (ImageMagick
`plasma:`, no raws). The thumbnail cache was warmed by one run first,
so every run below was 20,000 hits. Release build, headless mutter's
Xwayland at 1920x1200 (`mutter --headless --wayland --virtual-monitor
1920x1200 -- ...` on a private session bus, `SLINT_SCALE_FACTOR=1`),
every run with its own `XDG_*` under the worktree's `target/scratch/xdg`,
the RTX 5070 Ti, 8 pool threads, load average 3 to 5. The folder's log
line now says what the run cost the window's thread (below), and I added
that counting first, on master's delivery, so "before" is master's
behavior with the new line. Eight runs each of the grid (`--grid f20k`)
and the loupe (`f20k`), opened on the first frame:
- 20,000 calls on the event loop, 1.93 to 2.06 s of the window's thread
  in them in total. No frame drawn until the last had been taken in.
- The last picture taken in at 2.77 to 3.19 s from the folder's open.
- The rendering setup (the `gpu:` line) at 3.80 to 3.96 s of the
  process's uptime and the first develop at 3.97 to 4.18 s. That is
  0.8 s after the last picture, the first frame building twenty thousand
  cells each with its picture.

This machine's numbers are about half §174's (5.7 to 6.3 s). Small
JPEGs look up faster than §174's raws, and the mechanism is the same.

**Now.**
- *One call for many.* The pool can deliver through an outbox
  (`thumbpool::Outbox`, `Pool::batched`, `Worker::batched`, which the
  editor uses). A thread puts what it finished there and calls the
  window only when the window has not been told since it last found
  the outbox empty. That call is `Outcome::Thumbnails(Batch)`. The
  window takes pictures from the batch 32 at a time
  (`deliver::take_thumbnails`) for a turn's time. What is left is taken
  on a zero `slint::Timer`, not another `invoke_from_event_loop`: winit
  would take that call in the same drain, while the timer's turn comes
  after the frame and the input. A turn lasts as long as the window
  spent on the frame and the input since the last one, 8 to 100 ms
  (`thumb_turn`), so the pictures get half the window's time however
  much a frame costs. `Worker::new` keeps one call per picture, and is
  now the tests' constructor only (`#[cfg(test)]`). The tests read
  outcomes off a channel, one of them in `roots.rs`, which this item
  was not to touch.
- *The files on screen first.* The outbox keeps what the window shows
  (the range `Pool::want` last named, the strip's or the grid's) in a
  queue of its own, taken before the rest. When the range changes
  while pictures wait, the waiting ones are sorted into the two queues
  again (`Outbox::show`). The first cut kept one list and partitioned
  it on every take, which is O(waiting) per 32 pictures. On the 20,000
  that was 0.40 s of the window's thread against 0.03 s with two
  queues.
- *Rows far from the screen left for a scroll.* Batching alone brought
  the window up at once, but the pictures took 12 to 28 s to fill at a
  fixed 8 ms turn and 5.2 to 5.6 s at the adaptive one. A picture put
  on a row costs Slint more at the next frame than it does going on.
  The strip and the grid are plain `for` repeaters over all 20,000
  rows, and a frame after a turn of 800 rows took 170 to 400 ms to
  render. So `show_thumb` leaves a row that has no picture yet and is
  far from the screen alone. The picture is kept in `thumb_base`, and
  `thumb_shown` stays `None`, which is what marks such a row. "Near"
  (`near_screen`) is:
  - 256 rows either side of the current frame's row, or of row 0;
  - the grid's last range (`grid_shown`) with as much again either side;
  - the strip's last range (`strip_shown`, new in `State`, reset with
    the list like `grid_shown`) with as much again either side.

  `fill_near_screen` puts the kept pictures on the rows that have come
  near. It runs when the grid's or the strip's range changes, and on a
  select (`open_row`, culling included). A row that already has a
  picture gets a new one wherever it is, so a turn or a larger picture
  never leaves an old one standing. `rebuild_browser` goes through
  `show_thumb` as before, so after a filter change or a merge, rows far
  from the screen fill as they are scrolled to rather than all at once.
  That includes a filter being cleared. A picture that comes while the
  filter hides its frame no longer marks `thumb_shown`, so the frame
  waits like any other row far from the screen. The first cut marked it,
  and clearing a filter over thousands then put all their pictures on
  their rows in one call, as on master (found in the review).
- The folder's log line reports the window's side: "taken in by the
  window in N turns, T s, the longest L ms; F frames drawn meanwhile,
  the first at X s, the longest wait between two G ms". The last is the
  longest gap between two frames while the pictures came, with the gap
  still open when the run ended counted in.

**Measured after.** Same setup, runs interleaved with master's, opened
on the first frame, 8 grid and 8 loupe:
- 20 to 31 turns, 0.03 s of the window's thread in all, the longest
  turn 16 to 17 ms.
- The last picture taken in at 1.51 to 1.60 s from the folder's open,
  against 2.77 to 3.19. The pool's own time is unchanged (12 s summed
  over the threads). The window, taking them one call at a time, was
  what set the pace.
- The window drew from the start: the first frame at 1.01 to 1.08 s
  from the folder's open, which is the window's own start-up. Rendering
  setup at 1.20 to 1.31 s of uptime against 3.80 to 3.96, and the first
  develop at 1.36 to 1.52 s against 3.97 to 4.18. 3 to 15 frames were
  drawn while the pictures came, against none.
- The longest wait between two frames was 165 to 257 ms, against the
  whole 2.8 to 3.2 s. That is the cost of one frame over twenty thousand
  cells (the grid more than the loupe's strip), not the pictures.
- Opened on frame 15,007 (the remembered last file), so the 256 either
  side are all rows: 0.08 s of the window's thread and the longest
  turn 43 to 44 ms. The other numbers were as above.

Grid snapshots (`--grid --snapshot`) are pixel for pixel master's
(ImageMagick `compare -metric AE`: 0) on a cold cache (the 50 JPEGs,
cache emptied, 50 made), on the warm 20,000 opened at the top, and on
the 20,000 opened on frame 15,007 with the grid scrolled to it. A cold
folder takes the same path: a picture made rather than found goes
through the same outbox.

The review pointed out that `grid_filled_rows` is true whenever the
grid is closed, so a loupe snapshot might now be taken before the
strip's pictures are in. Checked with a loupe `--snapshot` on the warm
50-JPEG folder, three rounds each against master's binary from before
dfd53c2. The captures differ by 282 pixels, the same in every round,
all in a 274x28 box at (1210, 194). That box is §189's new "Selection"
button beside the scope's tabs, which master's older binary does not
have. With that box painted out, AE is 0, and the strip's cells are
filled in both. The capture waits for the develop, and on a small
folder the strip's pictures are in by then. A capture that waits for
the strip itself was not needed.

**Tests.** In `thumbpool`:
- 2,000 instant hits on eight threads: the window is told once, all
  2,000 come from that one batch, each once, and the next 1,000 bring
  one more.
- 5,000 taken in slices of 32 with 2 ms of "frame" between batches:
  each once, and fewer than 500 batches.
- The files on screen first: when named before the pictures were made
  (20..=24), and when the range changes while they wait (60..=69, then
  the earliest of the rest).
- A folder change empties the outbox.
- After a folder change the window is told again. `forget` leaves the
  window told with nothing waiting. Its take finds the outbox empty and
  clears the flag, and the new folder's pictures then bring a second
  `Thumbnails`.

In the browser, on the headless backend:
- A batch of 100 taken with a budget of nothing: the first turn takes
  32, and the zero timer's turn takes the other 68.
- Pictures that came while "No rejects" hid their frames: when the
  filter is cleared, one near the screen is carried to its row, one far
  off (900) is left off its row, and a scroll to it puts it on.
- Pictures delivered as a batch for rows 5, 300 and 900 with the grid
  at 0..=9: 5 goes on its row, 300 and 900 are kept. The grid's range
  moved to 895..=904 puts 900 on, and the strip's report of 295..=305
  puts 300 on. With the grid back at the top, 900's larger picture
  still replaces the one on its row.
- `thumb_turn`'s bounds.

Re-measured after the rebase onto §188 and §189, two runs each: the
grid's last picture at 1.56 and 1.62 s, 16 and 22 turns, 0.03 s on
the window's thread, the first frame at 1.23 and 1.06 s, the longest
wait between two frames 250 and 236 ms; the loupe 1.59 and 1.58 s,
177 and 170 ms.

**What is left.**
- A frame over twenty thousand cells costs 45 to 400 ms here, because
  the strip and the grid instantiate a cell for every row. That is now
  the longest the window waits while pictures come, and it is what
  scrolling a folder that size costs anyway. A virtualized grid and
  strip (only the rows on screen instantiated) is its own item.
- A scroll to a region far off puts its kept pictures on in that turn:
  about 3 screenfuls of rows at about 0.1 ms each.
- `show_no_thumb` still marks a failed cell on its row at once, wherever
  it is. Failures are few.
- Not measured on raws or on §174's all-roots view. The mechanism does
  not depend on either: the all-roots view delivers through the same
  pool and outbox.
