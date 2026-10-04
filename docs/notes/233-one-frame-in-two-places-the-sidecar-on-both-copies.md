# 233. One frame in two places: the sidecar on both copies, and two histories joined (2026-10-03)

The design for what §224 left: Next 0.4.0's line (the sidecar written
to both copies when both are there, the frame on screen following to
the archive's copy when its local root goes) and the library track's
line it waits on (edits synced between a shoot and its archive copy
by the sidecars alone). Written before the build, to be built in two
parts: the file's side in `greycard-edit`, then the editor's side.

The rules the roadmap already set stand: the hash is the identity and
the path a hint (§72, §216); no database crosses machines; no lock
file, no checked-out flag, no daemon.

**When the archive's copy is written: behind every save.** A sidecar
is written to the archive's copy whenever the local one is saved and
the archive answers. We weighed a periodic pass and a Sync button
against it. Both leave a window in which the copies differ for no
reason, and a button asks the user to remember a thing the editor
knows. What makes write-through costly is the size, not the count:
over the sidecars under the test folder the median is 22 KB and the
largest 5.4 MB. The large one is fifty history states of a 25 KB edit
(mask strokes, most of it), written pretty-printed; the same JSON
compact is 1.2 MB. Writing that over a network on the window's thread
after every slider release, to a share that may be asleep, would
stall the editor. So write-through is put behind the save:

- The local copy is saved as today, on the save timer, and stays the
  one written first. The editor works on the local copy whenever it
  is there.
- The save then queues the archive's write. The queue keeps the latest
  bytes per frame, so ten saves in a burst are one write. It runs on
  the network lane of §215: the root looked at first with the roots'
  own look, the write to a temporary name and renamed over the copy,
  and a write that says nothing for ten seconds given up.
- Before the rename, the archive's copy is checked against what we
  last wrote there: its size, its mtime and the revision we recorded
  (below), kept per frame and archive in the index. A stat answers it
  without reading a 5 MB file. If the copy is not the one we wrote,
  another machine has saved it since, and the write does not go
  ahead: the pair goes to the comparison below instead. Without this
  check write-through would itself be the thing that loses an edit
  made on the other machine. Between the stat and the rename there is
  a gap of milliseconds in which another machine could write; we
  accept it rather than add a lock.
- A write that cannot be made (the archive offline, the share gone
  quiet, the check sending it to the comparison) leaves the frame on
  a pending list in the index, so a quit does not lose it. The next
  pass to find the archive's root answering (the network poll of
  §188, on §215's lane) takes the list up first.
- The status line says how many edits wait for which archive ("3 edits
  waiting for Archive") while there are any, and a click on it retries
  them now. That is the Sync button, there only when it means
  something.

The archive's copy is written in its own placement, beside or under
`.greycard/`, as `Sidecar::find` finds it there; the XMP beside it is
regenerated from the joined sidecar, never merged.

**What a state is: a hash of its history.** To tell whether one copy
is behind the other, each state of the history gets an id: the
blake3 hash of its parent state's id, the edit's bytes as compact
JSON, and its label. The parent of the first state is a fixed zero.
This is how git names its commits, and it has three things over a
random id:

- A sidecar written before ids existed (every one so far) gets them
  without a migration. Both copies of a backed-up shoot compute the
  same chain over the states they share, so the first comparison
  after the build lands already sees which is behind, where random ids
  would make every pair look diverged and join them all.
- The same state reached on two machines from the same parent has the
  same id, so it is the same state and nothing is joined.
- A history altered by hand or by a bad copy no longer matches its
  chain.

The id is computed once, when the state is recorded, and stored on the
step and on the current state (`id`). It is never recomputed from a
loaded file: a field added with a default, or a serde alias such as a
sidecar's `"norm"` read as per channel (§232), changes the bytes, and
two builds recomputing would disagree about one history. The one
exception is a sidecar with no ids, which gets them on its first read,
from its first state forward, and keeps them. Undo and redo move a
state's id with it. The exports made from a state are not in the hash:
two machines exporting from the same state are still at the same
state, and their records are put together.

The history's cap of fifty keeps the first state and drops from the
second (`HISTORY`), and since ids are stored, the drop changes none of
them. A copy more than forty-nine states behind has lost the state it
would be found by and reads as diverged, which is safe: a join loses
nothing.

**What a save is: a revision.** State ids alone cannot say which copy
is later, because undo and redo move the current state without making
a new one. A copy at state 4 with state 5 to redo and a copy at state
5 are either the first having undone or the second having redone, and
the files cannot tell which. So each save also makes a revision: the
hash of the previous revision and the saved file's content, with the
time and the host's name. A sidecar keeps its last two hundred
revisions, a hash and a time each, a few kilobytes. A save that only
changes the meta (a rating) is a revision too, so the meta is ordered
with the edit.

The `saved` count of §161 stays for what it does now, choosing between
a sidecar beside the raw and one under `.greycard/` on one disk. Its
reviewer named the limit that rules it out here: a higher count means
more saves, not later ones.

**Comparing two copies.** In order:

1. The files' hashes equal: the same. Nothing to do.
2. The latest revisions equal: the same edit and history; the meta is
   joined field by field (below), and both copies are written.
3. One copy's latest revision is among the other's revisions: that copy
   is behind, and the other is copied over it whole.
4. Neither, and both copies carry revisions: they diverged; joined.
5. One or both from before revisions: by state ids. If one copy's
   current state and its whole history are in the other's history in
   order, it is behind. Otherwise joined. The undo case above cannot
   be told apart here and is joined, which keeps both.

**Joining two histories.** The newer copy is the one whose latest
revision is later by its time; on a tie, or a time that does not
parse, the local copy. Clocks on two machines can disagree, but this
decides only which branch is on top; the other is one undo away. The
joined history is the states the two share, then the newer copy's own
states, then the older copy's own states and its current, then the
newer copy's current recorded again with the label "Reconciled with
the copy on <host>". One undo brings back what the other machine had,
further undos walk its branch; the history reads as a log of what both
did rather than one line, which is the price of losing nothing. The
redo stacks are dropped, as any new state drops them. The shared
states are found by id, the last state whose id is in both. If the cap
has dropped every shared state but the first, the join is the first
state, then the two branches as above.

The meta joins field by field, the later write of each field winning:
rating, flag, label, keywords (as one field, so a keyword removed on
one machine stays removed), title, caption. That needs a time per
field, which the meta does not keep today; it gets one, read loosely,
and a meta from before it takes the file's latest revision time for
every field. Snapshots join by name; two different snapshots under one
name are both kept, the other copy's renamed "<name> (on <host>)".
Export records join by state id.

The join is written to both copies, so after it they are the same
file.

**The frame on screen when its root goes.** When the local root goes
offline with a frame open, and the archive's copy was not behind (no
write pending for it), the open frame's path becomes the archive's,
the next save goes there, and the status line says so once. If writes
were pending, the frame stays on the local path as §160 has it, its
saves queue, and the catch-up pass sends them when the root returns;
following the frame then would have it edit a copy that is behind.

**The two parts of the build.**
1. `greycard-edit`: state ids (computed on record, filled in on first
   read when missing), revisions on save, meta field times, and the
   comparison and the join as plain functions over two `Sidecar`s, with
   tests for every case above: the undo ambiguity, the cap dropping
   the shared states, a pre-id pair, a pre-revision pair, a meta field
   changed on each side, a snapshot name on both.
2. The editor: the queued write behind the save, the stat check, the
   pending list in the index, the catch-up pass, the status line, the
   comparison in Back up and Bring back and on a root's return, and
   the open frame following to the archive's copy.

**Not decided here.** Writing sidecars as compact JSON would make the
largest one about a quarter of its size, for every save and not only
the archive's; it is a format change of its own, and the pretty form
is what makes a sidecar readable when someone opens one. Keeping a state's edit as a difference
from its parent would shrink it further. Neither is needed for this
design to work.
