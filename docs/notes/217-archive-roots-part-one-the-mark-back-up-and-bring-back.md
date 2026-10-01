# 217. Archive roots, part one: the mark, Back up and Bring back (2026-10-01)

The first of §216's two landings: the archive mark, the destination
pairing, Back up and Bring back with the sidecar rule both ways, the
counts in the grid's header and on the Delete sheet, the whole-file
hash column, and the archive's rows in All roots with the local copies
hidden. Remove rejects and its queue are part two.

**What it was.** A NAS or a mounted cloud folder was a plain root. A
shoot reached it by rsync or a file manager, its sidecars only if the
hidden folder was remembered (§153), and nothing in the editor knew the
two copies were one frame. All roots listed both.

**The mark and the pairing.** `roots.json` gains two keys beside the
names (§185), each written only when it has something in it, so a
library with no archive keeps its file byte for byte and the version
stays 1:

    {"archives": ["/mnt/nas/Photos"],
     "backups": {"/home/x/Pictures": ["/mnt/nas/Photos/Clients"]},
     "names": {...}, "roots": [...], "version": 1}

`archives` is a list of roots; a path that is not a root, or an item
that is not text, is dropped on reading and the rest kept, as a name
is. `backups` is, by source (a root, or a folder backed up from outside
any root), the folders under the archives it went to, one an archive at
most; choosing another folder under the same archive replaces it. The
pairings are kept whatever the marks are now, so turning the mark off
and on again finds the folder chosen before. Taking an archive out of
the library drops its mark and every folder under it; taking a source
out drops its pairings. An older build reads `roots` and ignores both
keys, as it ignores the names, and loses them only if it saves.

The mark is set from the chip's menu, "Use as an archive", a checked
item under Rename..., and the chip carries a small cloud before its
count (and "archive" in its accessible value). Turning it off turns off
nothing but the actions; the files are where they were.

**Back up.** From the grid header's button, which is there whenever the
view is a local root's (or a folder of no root's) and an archive
exists, and from the frame menu ("Back up to Archive...", a submenu
with more than one). The header's acts on the open folder, or the
selection when more than one frame is chosen; the menu's on the
selection. With more than one archive the header's button opens a menu
of them. The view's folder is walked every folder down (§216: an export
folder inside the shoot is the user's and goes), the hidden folders
left out, and every folder named `rejects` left out unless the sheet's
checkbox says otherwise.

Before anything is copied a look goes off the window's thread: the
archive and the source's root are each looked at with the roots' 3 s
look, then each frame is looked for on the archive. Hash first: the
frame's content key (§72, from its row when the row's size and mtime
are the file's, else read from its head) is looked up in the index
under the archive's root (`Library::by_hash_under`, any path, the
missing left out), and each find is confirmed on the disk by existence
and size; a find whose whole-file hash the index keeps and which
differs from the frame's is another file. The destination itself is
looked at too, by size and key, since the index may not have passed
over a copy made since. A row with no file behind it is a frame not
there. The sheet then says:
- how many frames to copy and their size, with their sidecars;
- how many are there already with newer sidecars here, which go alone;
- how many are there and the same, and how many of those under a
  different folder;
- named, the frames whose archive sidecar is the newer (skipped), the
  frames whose destination name another file has (left; nothing on the
  archive is written over), and any that could not be read;
- the destination folder, in a field: the pairing when there is one,
  else the mirror, `<archive>/<root's label>`; the frame's path under
  its root goes under it. A folder typed outside the archive is refused
  in place; a new one is looked at again on Enter, as the checkbox is;
- the rejects' checkbox with its count, when a folder is walked.

The confirm keeps the folder for the source and starts the copy.

**The copy.** One file at a time on a thread of its own: the frame,
then its sidecars (the `.gcd` `Sidecar::find` names, in its placement,
and the XMPs `xmp::paths_of` gives, each named after the frame it goes
with). Each is written to a temporary name beside the destination,
`.NAME.PID-N.greycard-backup`, BLAKE3 of the whole file taken as it
streams, synced, dropped from the page cache (Linux) or opened uncached
(macOS) and read back, and renamed to its name only when the two hashes
agree, without replacing (the import's `rename_noreplace`). A mismatch
removes the temporary and names the frame in the report. The file keeps
the source's mtime, so the archive's row agrees with the source's on
size and time. A sidecar going alone replaces the older copy (the one
thing written over, and only when it is older); a `.gcd` goes over the
one the copy has wherever it is, so the copy never ends up with two.
Before writing to a folder the run removes the temporaries an earlier
run left there, an hour since their last write and carrying no process
id running here, and says so. Nothing on the archive is ever deleted.

The run beats at every 1 MB chunk read or written and between files;
a thread of its own watches the beats (`archive::supervise`). Quiet for
3 s (`ROOT_WAIT`, as §215's `PASS_WAIT`) and the copy is set aside: the
window drops its card, sets the copy's cancel, and says "Archive has
said nothing for 3 s: the copy is set aside, and stops after the frame
it is on"; if the share comes back, what it did is said then, late. The
card at the window's foot fills by bytes, with a Cancel that takes
effect between frames. When it is done the copies' rows are written on
the copy's own connection (`index_file`, then the whole hash on both
rows), the folders written to are handed to the indexer as folder
passes, and the status line has the report.

**Which sidecar is newer.** `Sidecar::compare_copies` applies §161's
rule between two places altogether: the save counter when both carry
one and they differ, the mtime otherwise. The same bytes on both sides
(each sidecar against the one its name and place give it there) is the
same. The `.gcd` decides when either side has one; the XMPs by their
mtimes when neither has. A pair that differs and cannot be told apart
is taken for the archive's: nothing is written over on a guess.

**Bring back.** The header's button in an archive's chip view (or a
branch of it), and the frame menu over a selection there, with a pick
of the local roots. The pairing is unwound where it can be: a source
under the chosen root that backed up to a folder these frames are under
gives the destination, the frames' paths under that folder kept. When
none does, the field holds `<root>/<folder's name>` and the sheet says
the archive's folder was not backed up from that root and asks where
under it they go. "Already there" looks under every local root, any
path. A frame there with the archive's sidecar the newer takes the
sidecar alone, home; one whose local sidecar is the newer is skipped
and named. Same verification, card and Cancel.

**The header's count.** A source with a pairing to an archive (it has
been backed up once) shows "3 frames not on Archive" on the header's
button, or "All on Archive", from the same lookup over the frames a
Back up of the view would walk (its folder every folder down, the
rejects left out), taken off the window's thread when a list lands and
after a copy. The pairing is the mark, so every folder of a paired root
shows its count. The count is per archive: with two paired, the first's
names the button, and each choice in its menu carries its own ("Back up
to Cloud (4 frames not on Cloud)..."). The count is the button.

**The Delete sheet's line.** With any archive, the sheet says "Looking
for these on the archives..." and then, from the same lookup over every
archive that answers its look: "All 340 are on Archive", "All 2 are on
an archive" (spread over two), "3 of these are on no archive", "It is on
no archive", and names an archive that did not answer. A line, not a
refusal: the buttons are as they were.

**All roots.** A view of every root keeps the archive's rows and leaves
out those whose content key is also under a local root (on the rows
the read brings, so it costs nothing on the disk); the archive's own
view lists all of it. Turning the mark off brings the copies back into
All roots.

**The index.** Schema 5: `whole_hash TEXT`, nullable, filled only by a
backup or a bring-back (the copy's row and the source's), at the size it
was taken; a pass that finds the file changed clears it. A schema 4
library is brought up in place and reads nothing again.

**The rule.** Nothing in the archive's chip view is refused: flag, Move
rejects, save, Delete selection and Delete rejects folder work there
behind their own sheets, as they did. Back up and Bring back never
delete. A capture, export or timing run never copies (the sheet opens
for `--sheet archive`, and its confirm is refused as a delete's is).

**Tests.** The library: the marks and pairings round-trip, the version
stays 1, the list reads as plain strings, an older shape reads as no
archives, a bad item is dropped, the mark off keeps the pairing and the
archive removed takes both; `by_hash_under` finds a copy at another
path under one root and not a missing one; the whole hash is kept at
its size and cleared by a change; a schema 4 library gains the column.
The edit crate: two copies compare by count then mtime. The copy, on
temporary folders: a folder backed up whole, placements kept, mtimes
kept, the rejects left and then included, the rows and whole hashes
written, and a second look with nothing to do; a frame on the archive
under another folder not copied again, and copied when its row has no
file; a forced mismatch on the read-back removed and named; a stale
temporary removed and said; a newer sidecar each way (sidecar only here
to there, skipped and named there to here, and home again by Bring
back into the hidden folder where the frame keeps its own); Bring back
skipping a frame already local and naming it; a different file at the
name left alone; Cancel between frames; the share-hang path with a slow
fake (a job that goes quiet set aside and landing late, one that beats
slowly never set aside, and a copy held mid-run set aside and stopping
after its frame); the All roots filter; the counts over two archives;
sidecar names after a renamed frame. The window: the mark from the
chip's menu kept and shown; a backup from the header through the sheet
(the folder refused outside the archive, another folder and the
rejects looked at again, the copy, the pairing kept, "All on nas", then
"1 frame not on nas" for a new frame, and Bring back from a branch of
the archive unwinding the pairing to the root with every frame home);
the Delete sheet's line over two frames and one; All roots with and
without the mark.

**Checked in the editor.** A scratch shoot of two raws and an empty
archive named "Archive": the grid's header shows "Back up...", the chip
its mark, and `--sheet archive` the sheet ("2 frames to copy, 93.3 MB,
with their sidecars", the field at `<archive>/local`). The copy itself
was not run through a click; the tests run it.

**What the review changed.** Four rounds, until the reviewer had
nothing left; the holes, each a rule broken one layer from where it was
stated:
- An older XMP could go over a newer one: the first cut decided once a
  frame, from the `.gcd`, and copied every sidecar with it. Now each
  file is judged on its own (`archive::compare_sidecars`): the `.gcd`
  by §161's rule, going over the one the copy has wherever it is, each
  XMP by its mtime against its counterpart; a file over there that is
  the newer, cannot be told apart, or has none here is left and names
  the frame. A test: a `.gcd` newer here by its counter, the archive's
  XMP written later; the `.gcd` goes, the XMP stays.
- A sidecar left at the destination with no frame had the frame land
  beside it. Now anything belonging to the destination frame's name
  (its `.gcd` beside or under the hidden folder, or an XMP of its name)
  keeps the frame out, named, in the plan and again just before the
  frame is copied, whatever the frame has of its own and with sidecars
  off too; the second round found the first fix looked only at the
  names of the frame's own sidecars, so a never-rated frame still
  landed beside another's rating.
- The copy's row writes waited on the library's lock in silence, so an
  indexer pass holding it past 3 s set a finished copy aside. Every
  connection a job opens now has a busy handler from its first
  statement (`Library::open_with_busy`) that beats while it waits, 10
  ms a try up to rusqlite's five seconds. A test holds the write lock
  0.8 s against a 150 ms wait, and the copy lands on time.
- A copy set aside let the next start, whose sweep could take the first
  one's temporary while it still wrote. Now a copy set aside is a mark
  until it lands, no other starts meanwhile, and the sweep keeps any
  temporary written in the last hour or carrying a live id of this
  machine's.
- The header counted the listed frames, rejects in, subfolders out; now
  the walk a Back up would take, per archive, and a count overtaken by
  a newer stops between frames.
- One dead archive on the Delete sheet set the whole look aside and
  lost the live one's line; the roots a job looks at are now looked at
  together, the wait beating, each "did not answer" for itself.
- `--no-sidecars` copied sidecars; now the frames go alone, and the
  sheet says so.
- A late copy was said as a backup with no name; the job carries which
  way it went and where.
- A folder of no root's with the archive inside it would back the
  archive up into itself; the walk leaves the other side's roots out.
- The copy duplicated the import's; it is now the import's own,
  `import::land_with`, which takes a beat, a byte counter, an ending for
  its temporary and whether it may go over an older file.
- §197's open case, tested: a file at the destination with the frame's
  head and size and other bytes is not the frame when both whole hashes
  are known (its name is then taken, and the frame left and named). The
  first fix left a frame with no whole hashes and another time "in
  doubt", never copied and counted as missing for good; the second round
  settles it by reading both files whole and keeping the hashes.
- The window's job sender has a test fake of the watch's shape, so a
  copy set aside and its late landing are tested through the window.

**Decisions the design left open.**
- The pairing is per source and per archive: a list of folders under
  `backups`, one an archive. Two archives keep a folder each.
- "Already there" is existence and size plus the whole hash when both
  rows carry one. A re-exported JPEG with the same head and size is the
  same frame to the key; with both whole hashes kept it is not, and
  then its mirror name is taken and the frame is named, never written
  over.
- A sidecar file that differs from its counterpart and that neither
  the counter (a `.gcd`) nor the mtime can order is the other side's:
  left, and the frame named.
- A copy with the frame's key and size but another time, and no whole
  hash on both sides to tell (§197's open case: a picture written again
  under the same head and size, or an archive filled by a copy that did
  not keep times), is settled the hash-first way: both files read
  whole, a beat at every chunk, and both hashes kept on their rows, so
  the next look reads neither again. The same hash is the same frame;
  another is not a copy, so the frame is copied, or left and named when
  its name is taken there. An archive filled with a plain `cp` costs one
  read of each such pair, once. The read happens in the Back up and
  Bring back sheets' look alone, which the user asked for. The header's
  count and the Delete sheet's line run by themselves (the count at
  every list opened), and §216 calls the lookup they share cheap; §72
  rules out heavy work that starts on its own, and reading an archive
  whole from opening a folder would be that, restarted by every
  overtaken count. So they read nothing whole: a pair only a whole read
  would settle is unknown, never counted as on the archive, and said
  ("3 frames not on Archive, 2 not known"; "2 could not be told without
  reading them whole"), and the header never says "All on" while any
  is. The third review round moved it there.
- A whole hash kept on a row is trusted only while the row's size and
  time are the file's now, as the content key is: a frame written again
  in place under the same start and size after its backup, before any
  pass, is read again rather than taken for its old self. A hash read is
  kept only when the bytes read are the size the file had before the
  read.
- A copy set aside is told to stop after the frame it is on, unlike
  §215's passes, which carry on alone: a pass only reads, while a copy
  writes to the archive, and nothing more is written there while nobody
  is watching it; Back up again skips by hash whatever landed.
- A copy set aside holds only its own archive (§215 scopes a pass set
  aside to its root): a backup to another archive goes on. A copy whose
  thread ends without a report is heard as lost and lets go of the
  window, card and mark both.
- A header Back up over a root's view walks the whole root; over a
  folder or a branch, that folder; with more than one frame chosen, the
  selection. All roots has no header button (no one root to pair); the
  frame menu there offers it when the selection is under one root.
- Bring back keeps no pairing of its own; the folder chosen is for that
  run.
- A temporary is stale at an hour without a write and no live process
  of this machine's with its id; a share whose clock is far off this
  machine's can still keep one longer, or let one go sooner.
- The menus' ellipses are "...", as every other in the editor.
- The All roots count beside the chip is still the sum of the roots'
  counts, the hidden copies among them.

**Not done.**
- Remove rejects, its two buttons and its queue: part two.
- One frame in two places; the merged row.
- Bring back's "the folder is offered to open": the status line names
  the result, nothing opens it.
- A pairing unwinds only when the frames are under one paired folder; a
  view of the whole archive with several sources' folders in it goes to
  the field.
- An editor with a frame open while Bring back writes that frame's
  newer sidecar from the archive keeps its own in memory, and its next
  save goes over what came home: the sidecar-sync line's.
- No real NAS: the hang is the tests' slow fake, and nothing here was
  measured against a share. The first Back up over one is the user's,
  and what it measures goes here.
