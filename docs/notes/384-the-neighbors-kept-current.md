# §384. The neighbors kept current (2026-10-21)

Two gaps the prefetch of the neighbors left open: a filter or sort
change while a frame was open left the worker making the old
neighbors, and a neighbor whose edit changed after its prefetch (a
sync, a paste, a catalog import, a sidecar rewritten elsewhere) left a
base on the shelf that no open would ever take.

## Sent again when they move

The window now keeps what it last told the worker of the open frame's
neighbors (`NeighborsSent`: the open's generation and file, the
neighbors, and the filter and the order the strip had). Every rebuild
of the strip's rows (`rebuild_browser`, which the filter bar, the kind
filter, the rejects, a smart collection, and a sort's merge read all
end in) and every save or row change of a frame not open
(`edited::saved`, `edited::changed`) asks `neighbors_moved`: while a
frame is open in the editor and still shown, the neighbors are worked
out again and compared, and only when the frames, their sides, turns,
seeds, or their edits' base differ are they sent again. So a redraw, a
rating, or the same filter applied twice sends nothing. The return
from culling or the grid sends them as the strip has them then, as
before.

`Worker::neighbors_again` now takes the open's generation and
replaces the list that open sent, whether it waits or is done, or
takes the place of none once that open has landed on that file; a list
of a later open is never touched. The window records what it sent only
when the worker took it, so a refused list is offered again at the
next change. The log says "neighbors resent after
a filter change" (a sort change, a neighbor's edit changed, a change
to the list, the return to the editor) and names them.

## A base whose edit changed

`edited::saved` and `edited::changed` now tell the worker
(`Worker::edit_changed`) the file and every edit its sidecar holds,
the versions' included, or none while the sidecar stands in from its
row. On its thread, before taking any job, the worker lets go of the
file's shelved bases whose base none of those edits share, all of them
when none came, and logs "shelved base of NAME dropped: edit changed".
The message carries the file's turn too, so a frame turned by a paste
drops its unturned base. A change after the base keeps it, and so does
a learned strength alone when no learned tier is asked for, since a
frame never developed seeds that at its open and not in its sidecar; a
base made under a tier keeps only the sidecar's strength, so one
shelved from a seeded frame never saved goes on the next save of the
file, a base made again and never a wrong one. A prefetch in hand of
that very file gives way only when the change reaches its base, and is
then let go rather than put back on the front of its list, which the
window has just sent again under the new edit: put back, the old base
was made in full and shelved after the drop, where nothing would ever
drop it. The neighbors are then sent again under the new
edit (above), and the next idle makes the base afresh: dropping it
alone would not do, since a list already made has nothing left in it,
and one still waiting carries the old edit.

## Verified

- Unit: the shelf drops a file's bases whose base no edit of it
  shares, keeps one whose change is after the base or in an unused
  learned strength, keeps a version's beside it, and drops all with no
  edits at hand. On the worker's thread: a change after the base keeps
  it and the resent neighbor is skipped as on the shelf; a change to
  the white balance drops it, said, the resent neighbor is made again,
  and its open takes it from the shelf. A prefetch held between stages
  goes on through an exposure synced onto it, gives way to a white
  balance pasted onto it and is let go, and only the newest edit's base
  is made and taken by its open. A turn change drops a base.
  `neighbors_again` replaces its own open's list and never another's.
- UI: with a frame open, a filter that rejects both neighbors sends
  the new next and previous in place of the old; the same filter again
  sends nothing.
- Release build, headless weston, five 45 MP raws (two bodies) named
  so their name order and capture order differ, the lens database
  present, `--time-select 3` with 7 s between steps, and a new measuring
  knob, `GREYCARD_TIME_SELECT_SORT_AT=2:0`, which picks the Sort menu's
  first entry (capture time) once the second develop has landed and
  waits the gap again before the step. On the second frame the
  prefetch had made the next frame by name and skipped the previous as
  shelved; the sort moved a third file beside it, the log read
  "neighbors resent after a sort change: next f4.CR3", that file was
  prefetched (4.96 s), and the step onto it read "base from the shelf,
  upload 0.18 s, total 0.36 s". Two runs: every step from the shelf,
  select to develop 578 and 593 ms mean over three steps, the first
  file 5.1 to 5.5 s. Without the resend that step would have been a
  fresh base, about 5 s here; we did not run that build.

## Not verified

- A sort over an indexed view, whose merge read lands later, reaches
  the same rebuild but was not run; the headless runs were a plain
  folder, sorted in place. A sort has no UI test of its own.
- The drop on a real sync, paste, or catalog import in the window; the
  hooks are `edited::saved` and `edited::changed`, tested only through
  the worker.
- A change to a neighbor made while the open's own develop is still
  running, then a trip to the grid: the list is lost as before.
- Leaving culling onto a frame sends an open with a new generation but
  no neighbors, so the return's `neighbors_back` is refused there, as
  before: that frame's first step is not prefetched.
- The sidecar first read for a stand-in neighbor guessed at its
  default edit is not dropped; its base misses the shelf as §379 says.
- Windows and macOS.
