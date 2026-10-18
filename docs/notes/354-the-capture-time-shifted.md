# §354. The capture time shifted (2026-10-18)

Two testers who shoot weddings asked for it: a second body whose
clock was five minutes slow, and a strip sorted by capture time
that interleaves the two bodies wrong. Lightroom's Edit Capture
Time is the reference: shift by a typed difference, or set one
frame's time and move the rest of the selection by the same amount,
the original kept so the shift can be undone to it.

## Where the shift lives

A shift in seconds in the sidecar's meta beside the rating, left
out when zero, held to two hundred years either way. The raw is
never rewritten. It is a field of its own for the join of two
copies of a sidecar, as the rating is, and presets and paste carry
no meta, so nothing copies a shift from one frame to another. Two
changes go down the rating's path over the whole selection: a
delta added to each frame's own shift, and Clear, which sets each
to zero; the second exists because one change goes over frames that
can hold different shifts, and a delta cannot clear them all. Move
on ignores both, and culling's undo takes them back, each frame to
its own shift.

## The index

The index keeps `taken` as the camera wrote it and adds a `shift`
column, written with the rest of the meta. A virtual generated
column, `taken_at`, is the one or the other moved by the shift, in
SQL, with an index of its own, so the sort by capture time, the Day
chips and the `date:` term read the corrected time at a hundred
thousand frames without a Rust pass. The schema goes from 11 to 12;
the migration adds the columns and clears the sidecar hashes, so a
shift already sitting in a sidecar is read on the next pass. A time
the camera wrote malformed, a month of zero or an hour of
twenty-five, is no date in both paths, since SQLite's arithmetic
would return null for it once shifted and the frame would change
place on being shifted.

## XMP and the export

The XMP carries the shift as a field of ours, written and read as
the shift itself, never added to. While a frame is shifted, the
moved time goes into the three standard dates, exif, xmp and
photoshop's, in XMP's ISO form, keeping a fraction or zone another
tool wrote; with no shift another tool's dates are left alone, and
clearing ours writes the camera's time back. The dates are never
read, so a corrected time cannot turn into a second shift. The
export's EXIF carries the moved DateTimeOriginal and CreateDate,
from the window and from `--export` alike.

The camera's zone offset, when it wrote one, stays as it was on a
shifted time. That is right for clock drift, the case this is for,
and wrong by the shift for a camera left on home time and moved
hours to local, where readers that combine the time and the offset
will show the wrong moment. A zone shift is a different operation,
moving the local time and the offset together so the moment stays
the same, and it is on the roadmap as a choice of its own in the
sheet; we kept this one simple.

## The sheet

Shift capture time... in the frame menu, over the selection. It
names the frame on screen if it is chosen, with the time its
camera wrote and the time it has now; a Shift by or Set this frame
to switch; one field that takes `+H:MM:SS` and also `5m`,
`-1h 30m`, `90s` or `2d`, colon forms read from the right so
`5:00` is five minutes, one field rather than three so the sign is
typed once and a paste comes in whole; a line saying where the
named frame moves to; Clear shift, Cancel and Apply, Esc and Enter.
Set this frame to works out the difference for the named frame and
adds it to every frame chosen, so the others keep their relative
shifts. Once the index has written the shifted rows, a list sorted
by capture time is read again and the Day chips recount; outside
the index a list cannot sort by capture time, and the guide says
so. The panel's head gained a line with the capture time, "(shifted
+5:00)" after it, from the index, the cache or the decode's own
EXIF, never a raw read.

## Left for later

A Lightroom catalog's own corrected times: the import does not read
`Adobe_images.captureTime`, so a time fixed in Lightroom comes in
at the raw's EXIF time; comparing the two and writing the
difference as the shift is on the roadmap. No date goes into the
export's own XMP packet, which never had one. A frame whose sidecar
goes to an archive copy has its index row written by that copy's
pass, and the re-sort stops waiting for it after a minute.

## Tests

Core: capture times read in every spelling, bad dates refused (a
thirtieth of February, a thirty-first of April, a sixtieth second,
mixed separators), the leap rule, a shifted export carrying the
corrected EXIF. Meta: kept beside the rating and left out at zero;
a delta adds and a clear takes off; two copies join it as a field.
XMP: a shift writes the corrected dates and reads back as itself;
another tool's dates stand until the frame is shifted. Library: a
shift moves the frame for the sort and the days; a schema-11
library is brought up and reads the shifts already there; the
hundred-thousand-row order test with one row in ten shifted; a
forced catalog import keeps the shift. Window: the difference reads
as it is said, with overflow refused; the sheet shifts the
selection and culling's undo takes it back; a list by capture time
reorders once the index has it, across midnight for the Day chips;
the info line takes the time the open read with no index; a
shifted frame exports its corrected time headless.
