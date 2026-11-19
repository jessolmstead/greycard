# §713. The GPU marker gives up, and is written whole (2026-11-18)

**What was wrong.** §704 left two things. A marker never gave up on a
backend that kept dying: a machine pinned to DX12 whose DX12 later died at
every start stayed there, and one with both backends counted dead ran the
default for good, until the file was deleted. And the marker was written in
place, so a kill inside the write could leave a torn file that read as a
different state and stranded a machine.

**Decided: three deaths in a row, then one try.** A death that sends the
next start back to the backend it died on (a pinned DX12, or the default
with both counted dead) is counted in the marker as `kept: N`. Any other
start, and a renderer that sets up, puts the count back to nought. At three
in a row the next start tries the other backend once.

- Three, because a pin is meant to outlast false deaths (a kill, a logoff,
  a shutdown, out of memory before the first frame): one is common, three in
  a row are not. A backend that has really died costs three starts.
- A try that draws settles it: from a pin the marker is removed, from both
  dead DX12 becomes the pin.
- A try is one try whatever ends it. A death, a panic, a returned error, a
  loop error, or a clean end without a frame (a lost device survived, then
  the relaunch) all send the next start back, with the count at nought.
  Putting a cleanly ended try back as found would have tried again at every
  start.
- A default whose select fails on a try ends the try and keeps the pin, so
  a software adapter drawing afterwards does not forget a DX12 that works.
- Off Windows there is no other backend: nothing is counted, nothing tried.
- A marker without the `kept:` line reads as nought; a count that is not a
  number reads as nought.

**Decided: write, sync, rename.** The marker goes to a part file beside it
(`gpu-setup.<pid>-<n>.part`), is synced, and is renamed over, with one more
try after 50 ms for a rename something else is holding. A failed write
leaves the old marker whole. Every marker we have written ends in a line
end, so a file that does not is one cut short, and reads as no marker, as
an unreadable one does: the start goes on with the default. Leftover part
files are swept by the start that holds the lock, and only by it.

**Known limits.**
- A 0.12 file torn exactly at a line end before the upgrade still parses as
  another state; this build never writes one.
- Nothing here was run on Windows: DX12's real behavior, a rename over a
  marker another process holds open, and the lock between two starts are
  read from the code and the documentation.
