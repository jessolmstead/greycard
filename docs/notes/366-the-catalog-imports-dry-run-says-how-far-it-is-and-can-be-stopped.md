# §366. The catalog import's dry run says how far it is, and can be stopped (2026-10-18)

The Lightroom sheet opens on a dry run of the catalog: the catalog
read, each of its roots looked for beside the catalog and under the
library's roots three levels deep, then every file looked for and its
sidecar read. On a library kept on a NAS each of those is a round
trip over the network, and a catalog of some thousands of frames
took minutes behind the one line "reading the catalog...", with
Cancel, Esc and the click outside the sheet all off until it came
back. And a panic on the thread would have left the sheet that way
for good, since nothing caught one and the window never heard.

The dry run now says what it is doing under the buttons, from the
same throttled progress the write already had: the root search got a
step of its own, "looking for the catalog's folders, 3 of 12", since
over a NAS it is the slow part and came before the first word of
progress; then the files, "looking for the catalog's files, 1,200 of 23,000".
A folder step always gets through the throttle, so the sheet never
shows a stale count through a slow walk. Only the SQLite read itself
still says just "reading the catalog...": it cannot be stopped
partway, and a stop set during it takes effect right after.

Cancel, Esc and the scrim stay live through a dry run and close the
sheet through the window, so the sheet's generation moves on in the
same step and a result landing late is dropped, a pending write with
it. The thread is told to stop as well, through a flag it checks at
every folder read and every file, so a canceled run is not still
working the NAS under the next one. The flag rides beside the
options rather than in them, since it answers to no command flag; the
command's import runs with one that is never set, and behaves as it
did. A write is checked once, before its first sidecar, and after
that goes to its end, so a stop never leaves an import half written;
the write stays uncancelable from the sheet, as before.

Both threads run under catch_unwind now. A panic lands as the note
"the import failed: ..." with the buttons back, the way a bad catalog
does.
