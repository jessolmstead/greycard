# §305. A snapshot's row says what a click does; a key that hides a frame says where it went (2026-10-11)

Two places where a click or a key did something the user could not
see coming.

## The snapshot row

A snapshot's row is the only way back to it, and until now only the
hover's words named what a click on it did: a click restored, a
double-click renamed. Worse, the double-click restored on its first
click, since a TouchArea reports `clicked` for each click of a
double-click and `double-clicked` only after the second; renaming a
snapshot moved the edit to it on the way.

The row now carries its actions as icons beside the bin, always shown
as the bin is: the history's arrow (undo-2) to restore, a pencil to
rename, the bin to remove. The restore icon restores at once; the
pencil opens the name and restores nothing. The hover's words on the
status line name each one ("Base, <date>; click to restore it", "...
click to rename it", "... click to remove it"), and over the row itself
they still say "click to restore, double-click to rename".

A click on the row still restores and a double-click still renames,
but the click's restore now waits out the double-click's window: the
row's click starts a 500 ms timer holding the row's number, the
double-click stops it and opens the name, and the timer's end
restores. We chose this over making a row click only select, leaving
the restore icon the only restore, because it can be made reliable.
Slint counts a second click by its press, within the platform's click
interval of the first press (500 ms, `Platform::click_interval`'s
default, which none of the backends we run on overrides), and reports
the double-click only at the second release. A timer started at the
first release could still run out between a late second press and a
release held after it, so every press on the row stops the wait and
only a release starts it: whatever the timing of the second click,
its press finds the restore not yet made, and its release is a
double-click. A row click that only selected would have changed what
the row does for everyone who has learned it; the half second it now
waits is the price of a double-click that does not also restore, and
the restore icon is there for whoever wants it at once.

A restore, a rename or a remove from an icon cancels a row click still
waiting, so the two cannot both land. A waiting restore belongs to the
frame its row was clicked on: choosing another frame in the meantime
(an arrow, a click in the strip) drops it, since the timer outlives
the frame change and would otherwise restore the new frame's snapshot
of that number. Entering culling or the grid within the half second
also drops it, since the section goes away; we accept that.

While a snapshot's new name is being typed, the pencil and the
double-click on another row do nothing: opening the other name would
take the text box away without its focus-loss ending the naming, and
the typed name would be lost. Enter, Escape or a click away ends the
naming first.

## A key that hides a frame

A rating, a flag or a label key on a frame the filter then no longer
shows took it out of the strip and the grid with no word: a tester
rated a frame 0 under a one-star filter and lost it. The selection
already went on to the nearest frame still shown (the rule a reject
under "No rejects" set); nothing said why the frame was gone.

Now the status line says it: "IMG_0001.CR3 rated 0 and hidden by the
filter (1 star or more); Clear the filter to see it", and for a set
"3 frames picked and hidden by the filter (...); Clear the filter to
see them". The verb is what the frame now carries (rated n, picked,
rejected, unflagged, labeled red, unlabeled), the filter is in the
words the log already uses for it, and Clear is the button that
brings it back. It is said after the selection has moved on, so the
open's "decoding..." does not cover it, and when that open asks for a
develop it is carried ahead of the develop's own line when that lands
(`status_after_develop`, as a preset's words are). In culling the word
over the picture says it too ("No stars; hidden by the filter"), since
culling's own status line is rewritten as the next frame shows. The
filter's words now say "1 star or more" and "exactly 1 star" rather
than "1 stars".

## Tests

- A snapshot named Base, the edit moved on: the pencil's hover says
  "click to rename it", its click opens the name and, past the
  double-click's window, nothing is restored; a double-click on the row
  opens the name and its first click restores nothing; the restore
  icon restores at once and says so.
- A click on the row restores once the double-click's window has
  passed (the §290 test, now waiting it out).
- A row click alone restores once; a row click followed by another
  frame chosen within the window restores nothing on either frame.
- With a second snapshot's name "Two" being typed, the first row's
  pencil does not open its name, and Enter keeps "Two".
- Three frames at two stars under "1 star or more": 0 on the middle one
  hides it, the selection goes to the next, and the status says the
  sentence above; an unflag under a Pick filter says "unflagged and
  hidden by the filter"; with the filter cleared the same 0 says
  nothing of the filter.
