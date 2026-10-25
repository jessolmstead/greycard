# §425. Fills kept on disk

*2026-10-25*

A frame with two remove fills ran both removals at every open: 172 s
of fills before a 3 s develop. The fill cache was memory only, and
the open clears it.

Each made fill is now written beside the learned-mask rasters, in
the `masks` folder, behind the same `disk` switch, and read back
before any model loads; `fill_kept` reads it too, so a kept fill is
not announced as "Filling". Memory stays the fast path; a hit is
logged at info. Failures stay in memory.

The slot hashes the file, the patch as memory compares it, its id
and seed, the picture's size, the region, the window, both engines'
model ids, and a print of the picture under the window: 16-pixel
block means of the pixels outside the hole, at f16. Memory could
leave the picture out, living only until the next open; on disk a
fill outlives a white balance change and is shared by virtual
copies, which share the file and copy patch ids. The print covers
white balance, profile, lens, noise, a turn, and every copy at once;
a false miss only remakes the fill. The model ids make a distilled
remove or a new LaMa fill afresh. A 40-byte header (version byte,
engine, size, window, region) is read first, so a stale slot is not
read through on every develop; bumping the version remakes every
fill without a sweep.

Values are little-endian f16: a fill is linear and passes 1, which
the masks' 8-bit PNG cannot hold, and f16 halves a fill of tens of
megabytes with a step below the grain put back over it. The made
fill is rounded to f16 in memory too, so a read-back is bit for bit
the one first shown.

A disk hit is served before the engine is asked, so a machine that
has since lost the model or its GPU shows the kept fill rather than
leaving the area and saying why. We keep that: it is the picture the
screen showed.

Nothing prunes the `masks` folder; a patch redrawn many times leaves
old slots behind. A sweep, when one comes, takes rasters and fills.

## Review

A fresh review found the first cut's key too narrow for disk: memory
left the picture out and lived only until the next open, where a disk
slot would have outlived a white balance change and been shared by
virtual copies, and the model ids were missing, so a distilled remove
would have been served the old fills. Both are in the key now. One
property worth knowing: a patch's print includes the earlier patches'
output, so a change to one remakes the later fills at the next open,
while memory serves the old ones until then. That is right, and it
is the order the fills are blended in.
