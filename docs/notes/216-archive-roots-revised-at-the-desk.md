# 216. Archive roots revised at the desk (2026-09-30)

§197 was written before the build and before §207, §209, §212 and
§215 landed. Walking it through the workflows an archive on a NAS is
actually for, before part one is built, changed seven of its rules and
added seven. This section is the list; §197 stands for everything it
does not touch, and the built sections will follow with what they
measured.

**The destination is a folder, not a mirror.** §197 put a backup at
`<archive>/<root's label>/<relative path>`. An archive that has been
filled for years has a layout of its own (client folders two levels
deep, §212), and a laptop root called Photos would mirror to a
`Photos/` folder the archive never had. The sheet now shows the
destination folder under the archive and lets it be changed; the
mirror is the default the first time, and the folder chosen is
remembered per source root in `roots.json`, under a key of its own, as
the names and the archive mark are (§185, §197), so an older build
reads past it. The shoot's path relative to its root still goes under
that folder. Bring back unwinds the same pairing where it can and
asks otherwise, as before. The hash was already the identity and the
path a hint (§72); this makes the hint one the user wrote.

**Already there is decided by hash first.** §197 decided what to copy
from the disk on both sides, which reads as by path, and a shoot
renamed here after its first backup, or an archive reorganized by hand,
would be copied whole a second time under the new name. A frame is
looked for first by its hash under the archive's root in the index
(`by_hash`, narrowed to the root, any path), the find confirmed on disk
by existence and size, and counted as backed up wherever it sits; the
sheet says "12 already on Archive, under a different folder". Only a
frame found nowhere under the archive is copied. The disk is still
what is trusted: a row with no file behind it is a frame not there.
The same lookup, with the roots swapped, is what Bring back and the
Delete sheet (below) use.

**The rejects are left out, said, and a checkbox brings them.** A
shoot's `rejects` folder sits inside the shoot's folder, not beside
it as §197 loosely had it (`cull::rejects_dir`, §123), so a walk of
the shoot would copy the rejects along. A folder's Back up skips the
rejects folder by default, the sheet says how many frames it skipped,
and a checkbox includes them. The two cases where the rejects should
go, a shoot not yet culled and a keep-everything backup, are the
checkbox's, and the first is empty anyway.

**Remove rejects can delete, behind the same sheet.** The workflow
that shaped this: a shoot backed up the day it was taken, culled at
home over the following days, and the rejects wanted off the archive
so they do not take its space forever. As §197 had it, Remove rejects
moved the archive's copies into a rejects folder there, and the space
came back only with a second trip, to the archive's chip view, for its
Delete rejects folder. The sheet now has two buttons: "Move to rejects
on Archive", the default, which Enter presses, and "Delete from
Archive" beside it, red, click only, no FocusScope, with §190's trash
or permanent wording and offer. Same list, same confirmation, one
trip. The queue is move only: a removal confirmed while the archive is
offline is queued as §197 has it, and a delete confirmed while it is
offline is refused and said, never queued. A delete does not happen
unwatched when a share reappears.

**The rule, reworded.** §197: an archive is never deleted from, and
never written to except through the three actions. Both halves were
too wide. The archive's chip view behaves as any root's: a frame
flagged there, Move rejects there, an edit saved there, all work, since
the rule is about what crosses between roots, not what the user does in
a view. And nothing is deleted under an archive except behind its own
sheet: Remove rejects' second button, Delete rejects folder from its
chip view, and a frame's own Delete selection, which the roadmap had
noted left a frame that exists only on the archive with no route at
all. Back up, Bring back and the queue still never delete. A cull done
on the archive's side leaves the laptop's copy unculled; that is the
sidecar-sync track line's job and is not built here.

**Bring back carries a newer sidecar.** The inverse of Back up's rule
was missing: a shoot edited from the archive's side on another machine
had no way home, since Bring back skipped any frame whose hash was
already under a local root. It now does for such a frame what Back up
does the other way, sidecar only, by §161's rule for which is newer,
and skips and names the frames whose local sidecar is the newer one.

**No write-through; a count instead.** The roadmap asked whether a
mode should keep the archive current on its own, without deleting. No:
a copy that starts by itself when a share appears is what §72 set out
not to build, and edits crossing on save are the sidecar-sync track
line. What there is instead is a count: a folder that has been backed
up once shows "3 frames not on Archive" in the grid's header, from the
hash lookup above, and the count is the button. The drift is seen, and
the verb is still the user's.

**The Delete sheet says what is on an archive.** Back up and then
delete local, to free the laptop, is the other half of the workflow,
and the thing to know at the Delete sheet is whether the frames are
safe somewhere. The sheet adds a line from the same lookup: "all 340
are on Archive", or "3 of these are on no archive". A line, not a
refusal.

**A copy lands under a temporary name.** Cancel takes effect between
files, but a crash or a pulled cable mid-copy leaves a partial file on
the archive under the real name, and a later look by path would take
it for the frame. The copy is written to a temporary name beside the
destination and renamed over once the read-back hash matches; a
temporary file found at the next Back up is removed and said. With the
hash-first check a partial is never counted as backed up either way.

**Back up on §215's lane.** §197 said off the window's thread with a
bar and a Cancel; §215 has since given every pass over a root the 3 s
look before it begins and a beat while it runs. Back up, Bring back and
Remove rejects take the same look and beat: a share that answers the
look and then hangs mid-copy sets the job aside rather than wedging a
thread, and the status line says so as it does for a pass. Cancel
stays between files.

**The whole-file hash is kept.** §197 left open whether BLAKE3 of the
whole file, taken as a backup streams, is worth a column. It is: a
nullable column filled only by a backup or a bring-back, costing
nothing, and the thing a verify-the-archive action would need later,
since §215 put hashing an archive cold at hours.

**The archive stays in All roots, its local copies hidden.** §197 left
an archive out of All roots until one frame in two places lands, so a
shoot on both sides would not be listed twice. That reasoned from the
shoot and forgot the archive: a NAS holds years of shoots with no local
copy at all, and leaving it out takes most of a body of work out of
the view that exists for judging one. All roots now keeps the archive's
rows and hides only those whose hash is also under a local root, which
is the same lookup as everything above and the half of one frame in
two places that costs nothing: no merged row, no choice of which copy
opens, a filter on the archive's rows. A shoot on both sides shows
once, from the local copy; the old shoots stay. One frame in two places
replaces the filter with the merged row when it lands.

**Two archives.** A pick in each sheet, Remove rejects per archive, the
header's count per archive. §197 had this and nothing changes.

**Walked and passing as written.** A second day's frames into the same
folder (new frames copied, the rest skipped by hash); the archive
asleep at cull time (the move queued); an export subfolder inside the
shoot (copied, it is the user's folder); a frame culled on the laptop
after its backup (sidecar only on the next Back up).

**Still two landings.** Part one is the mark, the destination pairing,
Back up and Bring back with the sidecar rule both ways, the counts in
the header and on the Delete sheet, the hash column, and the archive's
rows in All roots with the local copies hidden. Part two is
Remove rejects with both buttons and its queue. One frame in two
places waits on the first.
