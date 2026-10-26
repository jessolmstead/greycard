# §430. The export queue's view, polished

*2026-10-25*

A tester on a Mac found four rough edges in the queue's view (§416).

**The tab bar.** Over a frozen frame the panel hid the four develop
tabs and kept Cull's, which then stood alone where the bar was and
read as a stray button. We show no tab bar at all over a frozen frame.
Rating, flags, and labels already work there from their keys and the
strip, since they are the sidecar's and not the edit's, and C still
enters Cull. We did not move the CULLING section into the panel as its
content: its compare view, rejects folder, and way out are a mode's
tools, and showing them unasked would make the queue's Loupe a cull
mode in all but name.

**Running the queue.** Export the queue lived only at the export
sheet's foot, so the view that shows what waits could not send it. The
grid's header in the queue's view now carries the sheet's count line
("2 sets, 5 frames queued", ", running" while it runs) and an Export
the queue button on the same callback as the sheet's. The button is
off while any export runs and while the queue's run is under way,
including its look for files, which the sheet's running flag does not
cover; the run's own "exporting 3 of 12" already reads on the header's
status line. The sheet keeps its button.

**The row.** The Export queue row moved from among the roots to its
own place below COLLECTIONS, in no section: it is a listing, not a
folder, and it is the one way into the queue's view, so folding a
section (a fold the settings keep) must never hide it.

**The scopes.** The tester saw blank scopes over a queued frame. We
could not reproduce it on Linux (lavapipe): over a queued JPEG and a
queued CR3, opened by the Loupe button, Enter, and from a second tile,
the same file as the folder's frame or another, before and after a
Cull round trip, the histogram filled every time. The scopes are fed
from the render loop whatever the view, and nothing in the frozen path
gates them. The review's lead: when the camera's picture stands in
for the develop, the stand-in clears the scopes (`show_overlays` in
`panel/cull.rs`), and if no analysis follows when it comes down they
stay blank. A Mac run decides.

## Review

A fresh review found the row's first place, at the foot of
COLLECTIONS, folded away with the section, which the settings keep
folded; it is the one way into the view, so it moved out of every
section. The busy flag was walked through every start and stop of
the run, the look for files, a cancel, and an error. The scopes'
lead above is the review's.
