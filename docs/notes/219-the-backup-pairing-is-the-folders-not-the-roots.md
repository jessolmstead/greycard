# 219. The backup pairing is the folder's, not the root's (2026-10-02)

The first Back up over a real archive, the evening §217 landed, and the
first Bring back after it.

**What went wrong.** §216 remembered the destination a Back up was given
per source root: the next Back up from that root opened at the folder
chosen before, with the shoot's path under the root appended, and Bring
back unwound the same pairing, offering as home the root whose pairing
covers the archive folder the frames are in. The user backed one shoot
up from a root and changed the field to the archive's year folder, which
is the archive's layout. That one pairing then said the whole year was
that root's: a Bring back of another shoot under the year, one that had
never been on the laptop, offered the root as its home, and the next
Back up from the root would have put the next shoot under the year
folder with its path under the root appended. A pairing per root is the
mirror's shape (§197) kept after the mirror was given up (§216): on an
archive laid out by year or by client, a root has no one folder.

**Now.** A pairing is a folder's: the folder backed up (the view's
folder, or the chosen frames' common folder) to the folder it went to.
`backups` in `roots.json` keeps the same key and shape, keyed by that
folder; the file has only ever been written by the unreleased build, so
nothing is migrated, and an older build still reads past it.

Back up's default, in order: the folder's own pairing to this archive;
else the parent of the most recent destination chosen from the same
root to this archive, with the folder's own name appended, so after one
Back up to the year folder the next shoot opens at `<year>/<its name>`;
else the mirror, `<archive>/<root's label>/<path under the root>`, as
before. The field stays editable and the choice is kept as the folder's
pairing on confirm.

Bring back unwinds only an exact pairing: the archive folder the frames
are in, or one above it, is the destination some local folder was backed
up to, and that folder is home, the frames' path under the destination
kept. A folder under a paired destination that was never itself backed
up is not that pairing's. With no pairing the field holds the chosen
root joined with the archive folder's name and the sheet says it asks,
as before.

A pairing whose destination folder is no longer on the archive, or whose
source folder is no longer here, is dropped when a sheet's look finds it
so, and said in the status line once. Removing a root or an archive
drops the pairings under it, as §217 had it.

**Not changed.** The hash is still the identity and the path a hint
(§72, §216): a frame found anywhere under the archive counts as backed
up whatever the pairing says.
