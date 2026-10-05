# 244. One frame in two places, part 2: the editor's side (2026-10-04)

The second of §233's two parts, on §243's file's side: the sidecar
written to the archive's copy behind every save, the writes that could
not go kept in the library's index, the catch-up when the archive
answers, the status line's word for what waits, Back up and Bring back
comparing and joining the two `.gcd`s rather than reading their save
counts, and the frame on screen following to the archive's copy when
its own root goes. All of it is `crates/greycard-ui/src/sync.rs`, two
tables and their API in `greycard-library`, a property and a callback
in the Slint, `compare_sidecars`/`join_sidecars` in `archive.rs`, and
hooks in the saves that were there.

**The rule the module is built on.** Every write, every comparison and
every read of a sidecar that is not the window's own runs off the
window's thread (`archive::supervise`), beating as it goes; what it
finds lands on the window's thread, where the window's own sidecar and
its save are. A copy that is behind the window's is written; one that
is not is brought back whole, compared again with what the window holds
*now*, and taken or joined from that. A save made while a job was out
is never undone by what the job saw before it. The window's thread
never waits on a share: not for a write, not for a stat, not for a
read.

**The write behind the save.** Every save the window makes of a frame
it holds goes through `panel::edit::save_sidecar`, which saves the
file and then `library::sidecar_written` → `sync::after_save`. The
frame's copy on each archive is found through the index alone, nothing
asked of the disk (`pair`): the row's content hash, `by_hash_under`
over the archive roots, and then, among the hits under one archive,
*the one under the folder this frame's folder was backed up to*
(`Roots::backups`, oldest pairing first); with no pairing, a sole hit
is the copy only when nothing says it is another frame's: no pairing
under that archive claims it for a folder this frame is not in, and
the hash stands at one path among the local roots, this frame's.
Otherwise which copy is the frame's is not known, and the frame is
noted as waiting for that archive with a row whose copy is empty and
whose reason says so, and nothing is written. One raw in a shoot and
in its selects, both backed up, each writes to its own copy and never
to the other's; the shoot alone backed up, the selects frame's sole
hit is the shoot's copy, and it waits rather than write into it. The
frame is spelled as the index spells it throughout (`by_path`'s row):
the window's file list is the folder's own listing, so a folder opened
through a symlink gives paths that are not canonical, and the pairing
and the twin count would miss them. A frame under an archive root
queues nothing unless the window follows it there: the editor works on
the local copy, and the archive's is written from it.

The queue is the latest sidecar per frame (`Sync::queued`), and one
job per frame is out at a time (`in_flight`); a save while one is out
replaces the queued one, and the landing sends it, so ten saves in a
burst are one write. A job set aside (quiet past §233's ten seconds,
`WRITE_WAIT`) is still out: the frame stays in flight until it is
heard done or lost, so a second job never runs beside the first and
writes an older sidecar over a newer one. When it lands late, the
newer write queued behind it goes. A save made before the window's
index reader is open (a launch preset's) waits in memory
(`Sync::deferred`) and is queued when the reader opens, never dropped.
In a test the job is queued and landed in place (`tests::land_sent`),
as `panel::archive`'s are.

**The job, per archive copy** (`write_through`, `write_one`). First
the frame is noted pending in the index with the reason "writing", so
a quit, a crash or a job that never answers leaves its row; the row is
cleared only when the write is recorded. The row is the job's first
statement, so a quit between the save and the job's start leaves
nothing; that is milliseconds, and the local file holds the save, so
the next save or Back up carries it. Then: the archive looked at
(`roots::answer_of`, three seconds); the sidecar read from the file
when it did not come from the window (a frame the window has let go
of, or holds as its row standing in); a sidecar with no revision
refused (written by a build from before §233, it cannot be compared);
the copy's sidecar checked by a stat alone against what we last wrote
(`check`): no file is `Absent`, the recorded size and mtime is `Ours`,
anything else `Changed`, and a stat that fails other than by the file
being away is `Failed` with its error, a row with the reason, never
read as an absence. `Ours` is held to one more thing, with nothing
read (`known`): the revision recorded there is in this sidecar's
lineage, its revisions or the ones it absorbed. The stat says the file
is what we last wrote; the lineage says what we last wrote came from
this sidecar. A window that read the file before a job settled it and
the copy behind its back, or a copy another machine rewrote within the
same second at the same size (which a stat cannot tell), fails it and
is compared, not written over. `Changed` is read and compared: `Same`
records and writes nothing; `Behind(Other)` writes; everything else
comes back to the window as a `Take` with the copy's sidecar. The
write is the XMP beside the copy regenerated from the sidecar's meta,
when XMP sidecars are on, then `Sidecar::write_to` through a temporary
of this write's own (`.<name>.<pid>-<n>.tmp`, since §243's fixed
`.json.tmp` would have two writers tread on each other), then
`record_archive_write` with the revision and the file's size and mtime
as it landed, and the row cleared. Any failure leaves the row with its
reason and its try count.

**A frame whose file is not where its row said** (`gone_or_moved`) is
looked for by its hash among the local roots, and the row moved to
where it is now. With no twin, its folder is asked: there, readable
and with something in it, the file not among them, is a frame deleted
by another hand, and the row is dropped with a log line; the folder
gone, or listing empty, is a disk unplugged or a share unmounted,
which takes the folder with it, or a fixed mount point with nothing
mounted on it, which lists empty, and the row is kept with the reason
"the frame is not here; its disk may be away". The folder and not the
root, since an empty mount point answers as a root. The editor's own
Delete drops a frame's pending rows with its row (`Library::forget`),
and Remove from library drops the rows of the frames under the root
and those owed to it as an archive (`clear_pending_under`), so a hash
is never held pending by a frame the editor itself took away.

**The landing** (`landed`, `settle`). A job lost leaves the frame
pending (the row was made before the attempt). A `Take` is settled
against what the window holds *now*, not what the job carried: ours
behind the copy's takes the copy's bytes whole (`write_to` over the
local sidecar, no new revision; when the local root is gone the window
takes it in memory and the file stays behind, as any copy away from a
save does), and a write goes from memory so the job finds the copy the
same and records it, clearing the row; ours ahead, or the same, goes
again; the two gone their own ways are joined
(`greycard_edit::sync::join`) and saved, which queues the write again,
and the copy is found behind it. On the frame on screen the join
becomes the panel's and is developed again
(`history::take_current_settled`), as an undo is, but culling goes on
when it is on. A frame the window holds but may not write (held for a
delete) waits for its next save or the next catch-up. A frame the
window does not hold is settled on a job (`Source::Settle`): read,
compared again with the copy's, taken whole or joined and saved to its
file there, then written to the copy. The join never runs on a share
from the window's thread. A join whose save here fails (the disk gone
under the frame) still goes to the copy from memory, so the copy holds
both edits and the row clears; and after every settle the frame on
screen is asked to follow (below), since with the disk gone no landing
would come to ask.

**The pending list and what we last wrote** are two tables in the
index, schema 6: `archive_writes` (hash, archive, copy, path,
revision, size, mtime, written) and `pending` (hash, archive, copy,
frame, since, tries, reason). `archive_writes` is keyed by the frame's
content hash, the archive root as `files` stores roots, and the copy's
path, so a frame with two copies on one archive has a record for each;
`pending` by those and the frame too, so two local frames of one hash
wait on rows of their own and one's write never clears the other's,
and the "which copy" row has an empty copy. No foreign key to a row: a
row forgotten or pruned does not take them along, and a rebuild keeps
them. A schema 5 library gains them empty in place. The API is
`record_archive_write`/`archive_write`, `add_pending` (a new row, or
the same row's try count up and reason replaced), `note_pending`,
`move_pending`, `clear_pending`, `clear_pending_under`,
`pending_for`/`pending_all`/`pending_counts`/`is_pending`; `forget`
takes a frame's pending rows with its row; `mtime_of` is public so the
editor records the mtime in the unit the index keeps; `open_current`
opens a library only as it is, for a caller that may not bring it up.
The stat check is as fine as the file system's mtime, and the lineage
check covers what it cannot tell: a copy another machine rewrote
within the same second at the same size carries a revision this
sidecar has not seen, and is compared. The lineage is §243's capped
list, so after more than its cap of saves with the archive away the
recorded revision has dropped off it, and the next write reads the
copy and joins once where it could have written: §243's accepted
cost, one needless read. The `pending` key gained the frame in place
during this part (schema 6 is unreleased): a library brought to 6 by
the two earlier commits of this branch keeps the old key, and its
pending rows fail with a logged warning; no library outside the
development scratch ever reached 6, so the version is not bumped.
`add_pending`'s failures are logged as warnings wherever it is called.

**The catch-up** (`catch_up`, `heard_from`, `heard_over`). An archive's
answer to a look made for something else is the cue, as the rejects
queue's is: a pass over it landing, a view's read, the poll; the hooks
are beside `rejects::heard_from` in `library::told` and
`rejects::heard_over` in `roots::land`, so the first read after launch
counts. Once an answer per offline spell (`caught_up`); a landing that
leaves rows waiting for an archive (a copy to join that could not be,
an archive that stopped answering mid-job) forgets the spell, so the
next answering look tries them again, and the frame's next save tries
its own. The pass reads the archive's pending rows, groups them by
frame, and queues one write per frame through the same queue a save's
goes through: a frame the window holds with a sidecar of its own goes
from memory; one it has let go of, or stands in for, from its file; a
row whose copy was not known is paired again, in case the roots say
now, by the same rule a save pairs by. A write already queued or out
for the frame is the latest and stands, so a catch-up never runs
beside a save's write. The status line's click (`retry`) forgets every
spell and runs the catch-up for each archive with something waiting.

**Saves made without the window's hand** are hooked too: a launch
preset (`preset_at_start`, when it wrote), an export recorded on a
frame the window has let go of (`record_export_on_disk`, from the
window and from a headless `--export`), and a stand-in un-rejected in
culling, each of which writes the file and not the window's sidecar.
The window's ones queue a write from the file (`after_disk_save`); the
headless run, which has no window and starts no job, notes the frame
as waiting in the user's index (`note_disk_save`) when the index and
the roots are there and an archive is among them, and the next
window's catch-up writes it. The index comes to the run through its
`Plan` (the editor's own path from `export`, None in a test), and is
opened only as it is (`Library::open_current`: there, and at this
build's schema, else refused; a lock waited on for a quarter second,
then the rows skipped and said): a run from the command line never
makes, migrates or rebuilds the user's library. A stand-in (a row
standing in for a sidecar not read) is never written to an archive:
the file is.

**Back up and Bring back** (`archive::compare_sidecars`,
`join_sidecars`) read the two `.gcd`s and compare them (§243's
`compare`) instead of their save counts: `Same` is nothing to do;
`Behind` copies the one ahead over the one behind, whichever side it
is on, so a copy that another machine went on from comes home in Back
up as it would in Bring back; `SameEdit` and `Diverged` are joined,
the join saved on the side the run is from (which makes the revision)
and written to the other, so the write behind the next save finds the
two the same. The run compares the `.gcd`s again before it acts on
them: the sheet was up between the plan and the run, and a rating made
meanwhile would otherwise be written over by a copy the plan judged
ahead; what goes is what the files say at that moment. A `.gcd` that
will not read is left and named, as before. The XMPs still go by
mtime, their format having no revision.

**The status line.** A property of its own, `sync-note`, beside the
status on the plate and in the grid's header, with a click
(`sync-retry`): the status text is one string that any later word
overwrites, and this one must stand while anything waits. It says
"3 edits waiting for Archive", one clause an archive, nothing when
nothing waits, and when some of them wait on the user rather than the
archive, "; 1 can't tell which copy is its own": a click on the note
names those frames in the log, with the answer (back up their folder,
so the pairing says which copy is whose). It is read from the index's
counts after every landing and when the index is opened at launch
(`library::open_reader` → `refresh_note`), so a window opened on a
list left waiting says so at once. It does not say "syncing": a write
behind every save would flicker the word on and off.

**Following the archive's copy** (`follow_offline`, `follow_check`,
`follow`), from `roots::land` when the offline set changes and from
every landing: the frame on screen, under a root now offline and not
under an archive, with one copy under an archive that answers, nothing
queued or out for it and nothing waiting in the index for its hash, is
checked on a job first: the copy's sidecar by its stat against what we
last wrote. Ours, or none, and the frame follows: its path becomes the
copy's (`files[c]`, `index_ids[c]`, the stand-in), the next save goes
there through the same check as any write, and the status line says it
once ("Shoot is offline: IMG_0001.CR3 is open from its copy on
Archive"). Changed, and the copy is read and compared: ours ahead is
written first, then followed; otherwise the copy is brought back and
settled as a `Take` is, and the frame follows at the next landing,
once the copy and the window agree. So a copy another machine saved
while the laptop was away is never written over by the first save
after the follow. With a write waiting the frame stays on its local
path and its saves queue, as §233 has it.

A followed frame does not follow back when its root returns: its saves
keep going to the copy until the frame is opened again. The local file
does catch up: the first look that finds the root answering again
queues, through the frame's own queue and off the window's thread, a
job that reads the local sidecar and writes the copy's (the window's)
over it when it is behind (`Source::Home`, `bring_home_followed`), once
a return, and the indexer reads the frame again as it does after a
save; a local sidecar that is ahead or apart (edited by another hand
while the root was away) is left for Back up or Bring back to join. So
a new window on the local frame shows the look the saves made.

**Quitting** with a followed frame's save out to an archive that is not
answering loses that save: its only other place is the local file, and
the local root is the one that went. The row is there before the
attempt, so the index knows the frame waits, and the next catch-up sends
what the local file has once its root is back; the saves made since the
follow are gone, as they would be from any frame whose two places are
both away. The editor warns of nothing at quit today (no close handler
but the import sheet's), so a followed frame gets no warning either;
adding one is a decision for the user.

**`--no-sidecars`** turns all of it off: no write queued, no catch-up,
no follow, nothing noted; a frame the window may not write (held for a
delete) is left for its next save.

**Decided here, §233 being silent.**
- The compare-and-join of a copy a write finds changed happens against
  the window's sidecar as it is at the landing, not as the job saw it;
  for a frame the window does not hold, on a job, from the file.
- Which of two copies under one archive is a frame's own is the backup
  pairing's to say; with none, nothing is written and the row says why,
  rather than guess by name and write one frame's edit over another's.
- A job set aside holds its frame: no second write for the frame until
  it is heard done or lost. The cost is a frame whose share went quiet
  waiting out the supervisor; the alternative wrote old over new.
- Rows left waiting after a landing are tried again at the next
  answering look and at the frame's next save, not on a timer: there
  is no polling of our own; the roots' poll and every read are the
  clock.
- A frame that followed to an archive's copy does not follow back; its
  local sidecar is brought up to the copy's when the root returns.
- Back up brings a copy that went on from the local one home, where
  before it was left and named; the user asked for the two sides to be
  the same, and the `.gcd`s can now say which is ahead. Both act on
  what the files say at run time, not at plan time; the sheet's counts
  may be off by the saves made while it was up.
- A headless export writes one kind of row to the user's index and
  nothing else: `--library` is still refused there, and the library is
  never brought up from the command line.
- A frame not where its row says, with no local twin, is deleted when
  its folder is there with something else in it, and away otherwise:
  an unplugged disk, an unmounted share and an empty mount point all
  look alike from here, and a row kept costs a try per catch-up where
  a row dropped costs an edit.

**Open.**
- A whole shoot folder deleted by another hand keeps its frames' rows,
  as does a folder whose last frame was deleted: the folder gone or
  empty is what an unplugged disk looks like. The index's
  `missing_since` mark could tell them apart after a pass over the root
  found the folder's parent there and the folder not; not wired.
  Remove from library or the editor's own Delete clears them.
- The archive write, and Back up and Bring back alike, compare and
  then rename: a save on the other side that lands between the
  comparison and the rename is written over. The window is a stat and
  a read wide, on one machine against another's save of the same frame
  at the same moment; the next write's lineage check finds the copy
  changed and compares, but the save in the gap is gone from the copy
  by then. Closing it wants a lock the file systems do not share.
- No warning at quit with a followed frame's save out to an archive
  that is not answering, which loses that save (there is no warning at
  quit for anything).
- A followed frame does not follow back when its root returns.

**Tests.** `greycard-library`: a schema 5 library gaining the tables;
the two tables' API per frame, archive, copy and frame, `forget` and
`clear_pending_under` taking the rows with them; `open_current`
refusing an older library byte for byte and making nothing.
`greycard-edit`: a save leaves no temporary behind, whatever its name.
`greycard-ui/src/archive.rs`: a pair edited on both sides joined by
Back up and the same to Bring back, a copy behind written over whole in
either direction; a save between the plan and the run kept.
`greycard-ui/src/sync.rs`: the stat check against our write, another's,
none, and a stat that fails; one frame paired with exactly one copy or
none; the write off any window (a copy with no sidecar, ours by its
stat, another machine's handed back to settle, the archive away waiting
with its reason and try count, a sidecar with no revision refused);
through the window, an edit written through with both sidecars equal
and a burst of three saves one write, the XMP beside the copy written
when XMPs are on and not when off; the archive away and back, the note
saying what waits, the list surviving a window closed and opened, the
catch-up converging and a second answer sending nothing; a copy saved
elsewhere joined here and written back; a `Take` landing after newer
saves keeping them; a quit with a write out leaving it pending and the
next launch catching up; a write set aside holding its frame and the
newer one going after; two frames with one hash keeping their own
edits, and nothing written while the pairing is unknown; a catch-up
racing a save queuing behind it; the frame on screen following only
after a copy another machine saved is settled, with its disk really
gone, and staying when a write waits, its sidecar brought home once
when the root returns and a local sidecar that went its own way left;
a save on disk, and a save with no window open, reaching the archive;
a row for a frame whose disk is away kept and written when it is back;
a frame deleted by the editor and one gone from a present folder each
losing their rows, and a frame gone from an empty folder keeping its;
one frame's unknown-copy row standing through another frame's write; a
sole hit that is another frame's copy not written; a window's save
after a settle job joining rather than writing over; a note with no
window open leaving an older library untouched; a save before the
reader opens queued when it does.
