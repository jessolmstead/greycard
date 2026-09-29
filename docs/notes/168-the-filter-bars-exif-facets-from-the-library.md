# 168. The filter bar's EXIF facets, from the library index (2026-09-24)

Roadmap v0.7.0's filter-bar line, scoped to the folder the browser has
open, since roots do not exist yet. §160 built the index and left the
facet counts to the bar. This is the bar.

**What the editor does with the index.** It keeps the index up to
date for the open folder, on a thread of its own (`greycard-ui`'s
`library.rs`, the `Indexer`). When the editor starts, and whenever
`open_files` puts a new list in the browser, the indexer is asked for
`index_folder` over the folders the files are in. After every sidecar
write it is asked for `index_file` on that frame. That covers the
culling keys' `write_sidecar`, the edit's `save_edit` and the
history's save, so a rating, a flag, a label, a keyword or an edit
all bring the row up to date. The thread opens the one connection
that writes, `Library::open` on `Library::user_path()`, or on
`--library PATH` when that flag is given; nothing else chooses
another database. A test never starts an indexer on the user's
library. The tests that need an index make one in their own
temporary directory, and `State::empty` has none.

Between asks, the indexer takes everything already queued in one go:
duplicate files are dropped, and of the queued folder asks only the
newest is kept. A pass that has started is stopped too, between
batches. The library grew `index_folder_until(dir, progress, stop)`.
It asks `stop` after each batch is committed, and when `stop` says
so it ends there with `Report::stopped` set. The rows it wrote
stand. Nothing is marked missing, because the files it never reached
were never looked for. The next pass takes up from there, since the
files already written count as unchanged. The editor's `stop` is
true when either of two things is shared with the window:
- the generation of the folder the window wants has moved on;
- a save's `index_file` is waiting.

In the first case the old pass is dropped and the new folder's starts
at once. In the second the file is written and the same pass carries
on. So a save waits for at most one batch (50 files or a second)
rather than a whole pass, and opening folder B in the middle of a
2,000-file pass on folder A stops A at its next batch. A panic on the
thread, outside the per-file guard the probe already has, is caught
and sent to the window as `Failed`. The window then clears its
progress and lets a waiting capture go, rather than showing
"Indexing…" forever. On the way out the editor stops the pass and
closes both connections, waiting up to five seconds, so the
write-ahead log and its shared-memory file are folded in and
removed. Before this, every run left the two files behind the
library, 4.1 MB of them after 2,013 files.

The window holds a second connection, opened read-only once the
indexer says the file exists. It tries again at each word from the
indexer while the open fails, and the facets' row says the index
could not be read. The filter's questions go to that connection, on
the UI thread. Its busy timeout is 20 ms rather than rusqlite's five
seconds. Under write-ahead logging a reader does not wait on a writer
that is mid-transaction. It waits only while the log is recovered
after a crash or reset by a checkpoint, which is rare and short.
When a read does come back busy, the window keeps its last answer
rather than hold the frame.

The pass itself never runs on the UI thread, which is what keeps the
first frame from waiting. During a pass the indexer reports its
progress every 250 ms. At each report, and at the end, the window
reads the folder's row ids again. If the filter asks the index
anything and the answer changed, the browser's list is rebuilt from
that answer without asking again; otherwise only the chips are
redrawn. So on a cold folder the facets fill in while the pass runs,
and a frame the filter does not want disappears once its row says so.

**When a pass fails.** A pass can fail for more than one reason, the
likeliest being another process holding the library's write lock
past the writer's five-second timeout. `Told::Indexed` now carries
the error. The facets' row then says "Library index busy; trying
again", and the window asks for the folder again two seconds later.
It does this up to five times, keeping the `--filter` chips and any
capture waiting on them. After that it says the index is unavailable
and lets the capture go. The first cut reported a failed pass as a
pass that found nothing: the window showed "No camera, lens or date
in these files' EXIF" over a folder of raws, dropped the `--filter`
chips without a word, and never tried again. Measured with a
`BEGIN IMMEDIATE` held for 12 s by `sqlite3` from another process,
over an 8-frame folder:
- The first pass failed at 5.1 s ("database is locked").
- The second, asked two seconds later, waited on the lock and
  finished at 11.0 s with 8 added.
- The `--filter camera:R6` chip came on, and the snapshot was taken.

**Rows are looked up by path, one query a folder.**
`Library::ids_of(paths)` canonicalizes each folder once, which is
where macOS's `/var` link and Windows' `\\?\` prefix are dealt with,
reads `path, id` for that folder, and maps the window's paths to row
ids. A frame with no row gets `None`. The rule the item states, never
hide a frame for want of an index row, is exactly this: the filter
gets one bit a frame (`filter::Frame::index`), which is true when
nothing is asked of the index, true when the frame has no row yet,
and otherwise the row's answer.

**What is answered where.** The EXIF is not in a sidecar, so camera,
lens, ISO, focal length and the day are the index's. So is the
folder, which the index holds canonical and a path in hand may not
be: macOS's `/var` is `/private/var` in the row. The meta is in the
editor's sidecars already, fresher than a row that an `index_file`
is still writing, so a meta test is answered from the sidecar even
when it is typed as a term. The library grew
`Term::on_meta(path, meta)`, which answers a word, `name`, `keyword`,
`rating`, `flag`, `label` and `missing` the way the SQL answers them
from the row, and returns `None` for an EXIF or folder term. A
library test holds it to that: 23 terms, each checked on the three
seeded frames, and every one lists the same files from the sidecar as
from the index.

The keyword chips are the sidecar's in both directions. They filter
from the sidecars in hand, and they are also counted from them. Each
keyword is folded to lower case, and a chip counts the frames that
hold it among those the other groups leave, the index's bit
included. The chip shows the first spelling met in file order. The
first cut counted the keyword with the index's `GROUP BY` over the
keyword table while filtering from the sidecars. Under
`--no-sidecars` the two disagree: the rows read the sidecars on disk
and the window holds defaults, so `--no-sidecars --filter kw:harbor`
showed "Harbor 2" on and 0 frames. Even with sidecars on, a keyword
saved lagged its count by an `index_file`, and a whole pass when one
was running. Counted in memory, the count is right at once and
needs no index at all. The library's keyword `GROUP BY` stays, for
the CLI and for the roots, where the frames are not all in hand.

**The facets.** `Facet` is camera, lens, iso, focal, date and keyword.
`Library::facet_counts(facet, within, filter)` is one `GROUP BY`. The
key is `files.camera`, `files.lens`, `files.iso`,
`round(files.focal, 1)`, `substr(files.taken, 1, 10)`, or
`ulower(k.word)` over the keyword table with `count(DISTINCT id)`.
The rows counted are those that pass the filter, narrowed to the ids
in `within` (`json_each` over a JSON array bound as one parameter, so
a read-only connection needs nothing written). A chip hands its value
back as `Term::Facet { facet, values }`. That term has no text
syntax. It is any-of within the facet, like the flag and label chips,
and it matches through the same expression the value was grouped by.
That makes it impossible for the count on a chip and the files
pressing it lists to disagree: a focal length is grouped and matched
to a tenth of a millimetre, a day by its date. A library test presses
every chip of every facet on its own and checks that it lists exactly
the count. A value comes back in its shortest spelling, 35 and not
35.0, and parses back to the same double. Cameras, lenses and
keywords are listed with the most-held first. ISOs, focal lengths and
days are in their own order, so 100 comes before 3200.

**The counting rule is §133's.** A chip's count is how many frames
hold its value among the frames the other groups leave, with its own
group set aside. For an EXIF facet, the groups the sidecars answer
(stars, flags, labels, the words and meta terms typed, and the
keyword chips) choose the ids passed as `within`. The index's groups
(the typed EXIF terms and the other facets' chips) are the query's
filter, less the facet's own. So the camera row does not move when a
camera is pressed, and the lens row narrows to what that camera
shot. The meta rows' `Counts::of` treats the index's bit as one more
group that the other rows apply. The UI tests check the rule chip by
chip against what the filter lists.

**The text field takes the §160 grammar.** The library exposes its
`tokens` and an `is_term` (a field it has, an operator, a value). A
token shaped like a term is parsed as one. Anything else is split on
whitespace and looked for as a word, which is exactly what the search
box did before, quotes kept as characters. A field with no term in it
therefore means what it meant, and `foo:bar` or `12:30` is still a
word. A term that does not parse, such as `rating>=9`, is kept as the
word it would have been. Its error goes on the status line when the
filter is changed, and not again each time the index moves under it.

**The bar.** The grid's header has a row of facets under the meta
chips: a caption, then that facet's chips with their counts, then the
next facet. The header's height is still written out, and it is now
one chip row taller whether or not the index has anything yet, so
that the first pass does not re-flow the sheet. The grid's layout
test moved from 837 to 805 px of sheet, and from 40 to 32 frames
asked for first. With no chips yet, the row says what is happening:
"Indexing the folder: N of M", "Opening the library index", that the
index is busy or unavailable, or that the files' EXIF has no camera,
lens or date. In the CULLING section each facet gets a row of its
own. Clear turns the facets off with the rest.

The facet row scrolls sideways rather than wrapping, and a plain
wheel scrolls it. Slint 1.18's `Flickable` moves sideways only for a
drag or for Shift and the wheel, so with the sample folder's ten
cameras everything from Lens on sat past the right edge. Most mouse
users would never have found it, and nothing hinted that it scrolls.
A `TouchArea` inside the `Flickable` takes an up-and-down turn over
the row, or a sideways one, and moves the row by it, clamped to its
ends. It lets the event go when the row fits, so the grid under it
still gets the wheel. A UI test puts forty chips in the header,
turns the wheel over them, and checks that a click lands on a later
chip. The other options were one facet a row, which would cost five
more rows of header over every grid, and a fade at the edge. The
wheel settles the reach. A fade would need the panel's and the
header's different grounds, and is left.

At most twelve chips a facet are offered that are not on. The first
cut kept "count at least the twelfth's" in the facet's own order
until the room ran out. So when the twelfth count was a tie, the
first twelve in numeric or date order won, and a value held more
often could be pushed out. On the sample folder that dropped 200 mm
(3 frames) from the focal row and 2026-05-20 (6 frames) from the day
row, while it kept the twelve oldest days. Now every value held more
often than the twelfth is kept first. The ties at the twelfth fill
what room is left, in the facet's order. A test puts sixteen
one-frame focal lengths ahead of three held more often. Every chip
that is on is kept, whatever its count. One on that no frame holds
any more still shows, at zero, so it can be turned off. A keyword
shown that way is named as some sidecar in the folder spells it,
not in its folded form.

**`--filter`** still takes the three old names. One of them in the
wrong case (`picks`) is warned about and shows all, as before, rather
than becoming a word search. Anything else is the filter language. A
camera, lens, iso, focal, date or keyword term with `:` or `=`
becomes a chip once the pass has finished, read as the language
reads a text field: `:` is every value containing the text, so
`camera:R6` turns on both "Canon EOS R6" and "Canon EOS R6 Mark II",
and `=` is the value that is it, case aside, so
`camera="Canon EOS R6"` turns on the one. A number is equal either
way. Every other term goes in the text field. A snapshot waits for
those chips (`awaiting_index`, which sits beside `awaiting_turn`).

**Moving rejects** out renumbers the files. The index's answers are
carried over to the new numbering, and a pass over the folder marks
the moved frames' rows missing.

**Measured**, release build, on this machine while other worktrees
were building (load average 10 to 11). So these are ranges, and the
reviewer's runs on the same data came out slower (below).

On the 33 sample raws copied into a scratch folder, on a fresh
library under a scratch `XDG_DATA_HOME`:
- The first pass took 0.07 to 0.29 s (33 added); the second 0.00 s.
- The `--filter` chips were on within 2 ms of the pass finishing.

On 2,013 hard links to the same 33 raws, files page-cached and the
thumbnail cache warm:
- Three cold passes took 1.19, 1.46 and 1.53 s, and an earlier run
  1.41 s. The reviewer measured 4.63 s cold with a cold thumbnail
  cache making thumbnails beside it, and 2.65 and 1.99 s warm.
- Reading the folder's row ids took 0.4 to 1.9 ms.
- Deciding which rows pass took 0.3 to 0.4 ms (732 of 2,013 rows
  passing `camera:R6`, which is 12 × 61).
- Counting the facets took at most 3.2 to 3.9 ms a run with the
  keyword counted in memory. The first cut, with the keyword's
  `GROUP BY` among them, took up to 5.9 ms here and 8.4 ms in the
  reviewer's runs.
- A warm pass took 0.01 s.
- The thumbnails, all from the cache, came in 0.51 s with a cold
  pass running beside them and 0.24 s with it warm. The pass shares
  the disk and a core; nothing waits on it.

The sample folder's camera row sums to its 33 frames (11 + 6 + 4 +
3 + 3 + 2 + 1 + 1 + 1 + 1).

**Three questions, and what the bar does now.**
- *A frame with no value for a facet can never be picked.* The
  Panasonic's lens tag is empty, so it is on no lens chip, and there
  is no "none" chip to ask for the frames without a lens. It could
  be added as a chip whose term is `key IS NULL OR key = ''`,
  counted by the same `GROUP BY` with the NULLs let in. It is left
  out because a "no lens" chip on a shoot of adapted glass would be
  most of the row's count and read like a lens. The typed language
  cannot ask it either: `lens:` wants a value. Worth deciding with
  the roots, where a whole library of phone JPEGs has no lens.
- *A frame whose hash fails never gets a row*, so under any EXIF
  chip it shows, by the rule that a frame without a row shows until
  the pass says otherwise. For a file that cannot be read at all
  that is arguably right: the index cannot say it is not an R6. But
  the pass never will say, so it shows for good. The fix is for the
  index to keep an error row (path, size, mtime, no hash). The pass
  would then have said something, and the frame could fail every
  EXIF test the way a picture with no EXIF does. That is a schema
  change in the library, and left for it.
- *In culling, a pass with an EXIF filter on can move the current
  frame.* When a report brings the row that says the frame on
  screen fails the filter, the list is rebuilt and the selection
  moves to the nearest frame still shown, which in culling is a
  switch of the picture under the user. This only happens while a
  pass runs over frames the index had no rows for, with an EXIF chip
  on, so in practice on the first open of a new card with a chip
  chosen before the pass has reached the frame. The alternative,
  keeping a frame on screen that the filter hides until the user
  moves, is what the meta keys already do not do (§117's rule: a
  frame just rejected under "Picks" gives way at once). It is left
  as it is, and noted here.

**Left.** Roots, and the all-roots view, which is the next line: the
indexer takes a list of folders already, and the questions take ids
rather than a folder. The twelve-chip cap is a guess; past it, a
value is one typed term away (`focal:70`). The filter is not
remembered between sessions. The chips for `--filter` wait for the
end of the pass, so on a large cold folder they come on only when it
finishes. Nothing checks the index against a folder changed on disk
while it is open: the next open does, and so does the watcher when
the roots bring one. Under the testing backend, a click on the facet
row right after the wheel, with no pointer move between them, is
still aimed at the chip that was under the pointer before the row
moved. The test moves the pointer, as a hand does, and the effect
has not been looked for by hand.

**The review.** The reviewer read the four commits, ran the pass
over 2,013 links with the thumbnail cache cold and warm, held the
library's write lock from another process for 20 s, fed it a corrupt
file and a read-only home, and read Slint's `Flickable` source to
settle the wheel. The design held: writes only on the indexer's
thread, the window's connection read-only, no test near the user's
database, the counting rule the meta rows' own. What it found was
the chip cap keeping the first twelve values in numeric order rather
than the most held (200 mm with three frames dropped for seven held
once), a pass that could not be stopped when another folder was
opened, a panic on the thread leaving "Indexing…" up for good, a
locked database reported as "No camera, lens or date in these
files' EXIF", keyword chips counting from the index and filtering
from memory, and a facet row nobody with a plain mouse could reach.
One round of fixes, two more commits, and the draft's cold-pass
number replaced by a range beside the reviewer's.
