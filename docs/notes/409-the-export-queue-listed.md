# §409. The export queue listed

*2026-10-23*

The export queue (§213) showed a count at the sheet's foot and
offered Clear for the whole; a tester wanted to see what waits and
take one set out. The foot now lists the sets, one row each with
the preset's name or the format, the frame count, and the folder
shortened as the sidecar rows shorten theirs, with the presets
list's bin at the row's end: a removal takes the set out of the
queue and the file at once, no confirm, since the whole queue's
Clear keeps its own. Past five rows the list scrolls inside its box
so the sheet keeps its height.

The run holds its place as an index into the queue, so a removal
before the running set moves the index up by one, a removal after
it is never started, and the running set's bin is off. All of it
runs on the window's thread, where the worker's results and the
files look arrive through the event loop, so nothing races the
run's advance; the reviewer checked each reader of the index.

## Not done

- While the run is still looking for a set's files, the row says
  running and the count line does not; harmless and inconsistent.
