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

A pairing whose destination folder is no longer on the archive is
dropped when a sheet's look finds it so, and said in the status line
once. One whose source folder is no longer here is kept: a folder here
deleted after its backup is the usual reason a shoot is brought back,
and the pairing is what tells Bring back where home was; the folder is
made again on the copy. (The first draft of this section dropped both;
the build's review changed it.) Removing a root or an archive drops the
pairings under it, as §217 had it.

**Not changed.** The hash is still the identity and the path a hint
(§72, §216): a frame found anywhere under the archive counts as backed
up whatever the pairing says.

## Built, the same day

**What was built.**
- `roots.json`'s `backups` is keyed by the folder backed up (the view's
  folder, or the chosen frames' common folder), same key and shape, one
  destination per archive per folder. Nothing migrated; a pairing the
  unreleased build wrote under a root reads as that root folder's.
- The order the pairings were chosen in is a new key beside it,
  `backup_order`: an array of `[source, destination]` pairs, oldest
  first. serde_json writes an object's keys sorted, so the order could
  not live in `backups` itself; a time would have changed the shape.
  An older build reads past the key (and loses it only if it saves;
  then the pairings read in the file's order). An order item that does
  not name a kept pairing, or is not a pair of strings, is passed over.
  In memory the pairings are a `Vec<Pairing>`, newest last.
- `Roots::backup_default(folder, base, archive)`, in order:
  1. the folder's own pairing to this archive;
  2. a folder above it paired to this archive (the nearest);
  3. the most recent pairing from a folder under the same root to this
     archive, unless that pairing's source is under the folder being
     backed up (a root's view after one of its shoots, or a selection
     whose common folder is above the last shoot); then step 4;
  4. the mirror, `<archive>/<root's label>/<path under the root>`.

  In 2 and 3 a pairing's destination is read one of two ways. If its
  last component is its source folder's name (or, for a root, the
  root's label), the destination is the folder itself under its own
  name. Then 2 gives that destination with the folder's path under it,
  and 3 gives a sibling, `<its parent>/<this folder's name>`.
  Otherwise the destination was used as a container (a shoot or a root
  backed up to `<year>` itself), and both give
  `<destination>/<this folder's name>`. So after one Back up of a shoot
  to `<year>/<shoot>`, or to `<year>` itself, the next shoot defaults
  to `<year>/<its name>`, as §219's example has it. The field stays
  literal: the frames go into the folder shown, at their path under
  the folder backed up, and the sheet says so.
- `Roots::unwind(from, home, archive)`: the pairings from folders under
  the chosen local root whose destination is the archive folder or one
  above it, deepest first. The sheet's look takes the first that is
  exact (destination is the folder), or, for one above, whose source
  has the same folder under it on the disk. Else the field holds
  `<root>/<archive folder's name>` and the sheet asks, as before.
- `Ask.base` for a Back up is now the source folder, not the root: each
  frame goes at its path under the folder backed up. The mirror default
  keeps the path under the root, so a first Back up lands where it did.
- The sheet's look (already off the window's thread, after both roots
  answer) checks every pairing between the sheet's local side and its
  archive: a destination not a folder on the archive is gone. While the
  field is still the default the look settles it from the pairings less
  those gone. On landing the gone pairings are dropped through
  `roots::edit_roots` and said in the status line ("forgot that X was
  backed up to Y: that folder is no longer on nas", or a count with
  names), once: the next look does not find them.
- A pairing whose source folder is no longer here is kept, not dropped
  (the coordinator's ruling, departing from §219's text): a shoot is
  usually brought back because its folder here was deleted after the
  backup, and the pairing is where home was. Bring back of its exact
  destination unwinds to that source, and the copy makes the folder
  again, as it does any destination.
- A destination typed with a trailing separator is kept without it
  (`roots::tidy`, the path rebuilt from its components), in the field's
  look and in `set_backup_folder`.
- The header count shows when any folder under the view's root has a
  pairing to the archive (`Roots::backed_up_under`).
- Removing a root drops pairings whose source is under it; removing an
  archive drops those whose destination is under it.

**Decisions §219 left open.**
- The order is kept as `backup_order`, pairs of source and destination,
  oldest first (above).
- A fourth default between the folder's own and the sibling (step 2):
  a folder under one that was itself backed up to this archive (a root
  backed up whole, then a shoot of it) opens at that destination with
  its path under it, as Bring back would unwind it. If that destination
  was a container, it opens inside it by its own name instead. Without
  that, the user's own file (a root paired to the year folder) would
  rebuild §219's bug: `<year>/<path under the root>`.
- The sibling-or-container reading and the source-under-the-folder
  skip are the review's rulings (above).
- "Not that pairing's" is told by the disk: a folder strictly under a
  paired destination unwinds to that pairing only when the source has
  the same folder under it (an exports folder of the shoot backed up
  there); a sibling shoot under a year folder does not. The check runs
  in the look, off the window's thread.
- An archive that lists nothing at all (a share's mount point with the
  share not mounted, which still answers the roots' look) says nothing
  about the folders under it: none of its pairings is taken for gone.
- For Back up the default is settled once, by the first look; once the
  user types or the rejects box is changed the field is theirs.
- A folder of no root's is its own base for the sibling and the count,
  as it was for the pairing.

**Tests.** Library: the defaults in order (mirror, sibling after one
pairing, the most recent pairing naming it, own, under, another root's
pairings ignored, the mirror for a root view or a folder above the last
source); a shoot to the year folder itself taking the next inside; the
user's file, a root paired to the year folder, giving `<year>/<name>`
for a shoot at any depth, and the path kept when the root went under
its own name; the trailing separator; unwind's candidates deepest
first and only from the chosen root; the round trip with the order, the
older shape without the order, a bad order item, one pairing dropped,
an archive's removal and a root's. Window: the mirror, one Back up to
the year folder typed with a trailing slash and kept without, the next
shoot opening beside it with the header's count, the first at its own
pairing; §219's story (a shoot backed up to the year folder itself, the
year folder and the shoot's exports there unwinding to the shoot, a
never backed up shoot under the year folder asking); the story as the
user hit it (the root's view backed up with the field changed to the
year folder; Bring back of a shoot under the year folder that was never
here asking with `<root>/<name>` and the "was not backed up from" note;
a second, deeper shoot of the root defaulting to `<year>/<its name>`);
the mirror for the root's view and for a selection above the last
shoot; a pairing dropped
for its destination, said once and the default settled without it; a
pairing whose source folder is gone kept, nothing said, and Bring back
of its destination unwinding to it and making the folder again; an
empty archive dropping nothing. The §217
window test now expects the frames at their path under the shoot's
folder.

**Measured.** Nothing on a share; the stat per pairing in the look is
one `is_dir` call each, on the order of the pairings between one root
and one archive.

**Left.** The pairings checked are those between the sheet's root and
archive only; a pairing from another root is checked when that root's
sheet looks. The editor was not run; the tests drive the window.
