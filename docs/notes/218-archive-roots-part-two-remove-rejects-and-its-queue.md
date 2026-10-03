# 218. Archive roots, part two: Remove rejects and its queue (2026-10-01)

The second of §216's two landings: Remove rejects from an archive,
with both buttons, and the queue of moves confirmed while the archive
did not answer. §217 built the mark, Back up and Bring back; this uses
their lookup, their look and beat, their card and their per-archive
mark.

**What it was.** Culling left a `rejects` folder in the shoot (§123),
and the archive kept every one of those frames under its old name. Going
through the archive by hand to find them was the chore. Deleting them
there took a trip to the archive's chip view for its own Delete rejects
folder, after a move made by hand.

**The action.** The CULLING section has an entry per archive under
Delete rejects folder: "Remove rejects from Archive...". The grid
header's menu has no Delete rejects folder, so the CULLING section is
the only place it goes (until §228 put a Rejects menu in the header). It works in the folder view only, over the same
folder §190's Delete rejects folder takes, in the spelling the reads kept. An open
folder on the archive itself is refused: its rejects folder is the
archive's own, and Delete rejects folder is the way to take it off.

**The look.** Off the window's thread, on part one's `send`: the archive
is looked at with the roots' 3 s look, then the rejects folder is listed
here (a rejects folder that is a link is refused, as §190's is). Each
frame becomes an entry: its content key (from its row while the row is
the file's, else read from its head), its name, its size, and what else
the index knows of it (the whole hash while the row is current, and its
time). Its copies are the rows under the archive that carry that key,
at any path (`by_hash_under`). When the archive answered, each is
confirmed on the disk:
- at that path, a file of that size;
- of that head (the row's key while the row is the file's, else read);
- the same by the whole hashes when both are known, else by the time.
  A backup keeps the time, and two seconds of slack cover a share that
  rounds it.

A pair that only a whole read would settle is not known. It is listed
on the sheet and never acted on. When the archive does not answer, the
copies are as the index last saw them: the row's size, the whole hashes
it keeps, its time. A copy in the open folder's own rejects folder is
never its own copy, however its path is spelled.

**The sheet.**
- "2 rejects here have 2 copies on Archive, 93.3 MB:", then each reject
  beside its copy's path, up to eight and "and N more".
- Named: the rejects with no copy found, those not known, and those
  that could not be read.
- When the archive did not answer, a line says the copies are as the
  index last saw them, that a move is queued, and that a delete cannot
  be done.
- The note says what each button does, and §190's trash or permanent
  wording, naming the archive.

There are two buttons:
- **"Move to rejects on Archive"**: the default, and Enter's.
- **"Delete from Archive"**: red, a `DangerButton` (now exported from
  the delete sheet), no FocusScope. The sheet's FocusScope takes every
  key, Tab among them, so no key reaches it. The offer is
  `delete::offer`'s: the trash; the trash and "Delete from Archive
  permanently" beside it once the trash has refused one of these
  folders this session (`State::trash_refused`); permanently alone with
  no trash. An answer the sheet did not offer is a no, checked in Rust.
  With nothing to move, Enter does nothing. The delete buttons are not
  offered from a look the archive did not answer.

**Same list, same confirmation.** A run from the sheet acts on the
copies the sheet listed and no others (`archive::rejects::Item`, an
entry with its listed paths). Each is confirmed again on the disk at
the moment it goes, by the same check. A listed copy that is no longer
the copy is named and left. A copy the index picked up between the
sheet and the click was never shown, and is not touched. The queue's
runs alone look a reject up again by its hash, since the paths may
have changed while the archive was away.

**The move.** Each copy goes into the `rejects` folder of its own
folder on the archive by Move rejects' own move (`cull::move_rejects`,
one copy a call). Its `.gcd` keeps its placement and its XMPs come
along. A name taken there, the frame's or a sidecar's, leaves the copy
whole where it is and is said. A copy already in a rejects folder there
is left and counted. Nothing is deleted. A copy whose folder is not
inside the archive as the disk has it (a folder swapped for a link
since the pass) is left and named. So is one whose destination is not
inside it: a `rejects` folder there, or its hidden folder, that is a
link, or that leads out of the archive.

**The delete.** The listed copies confirmed again, then `delete::plan`
and `delete::delete`. The allowed folders are the
copies' own folders, canonical, and only those inside the archive as
the disk has it. A copy that is a link is refused, as is a folder that
leads out of the archive. The trash's first refusal stops the run, as
§190's does, and never turns into a permanent delete; the folder is
remembered for the next sheet's offer. The deleted copies' rows are
forgotten on the indexer's thread.

**The run.** Both go on part one's look and beat:
- the archive is looked at again first;
- a beat goes at every frame and every wait on the lock;
- a run quiet for 3 s is set aside, told to stop after its frame, and
  holds its archive alone until it lands late, said as what it was;
- a run whose thread ends without a report is heard as lost and lets
  go of the card.

The card's bar is by frames, since the moves are renames (`Running`
gained `doing` and `by_files`), and its Cancel takes effect between
frames. The report goes in the status line, and the folders touched go
to the indexer as folder passes.

**The queue.** A move confirmed while the archive does not answer is
written to `archive-queue.json` beside `roots.json`:

    {"queue": [{"archive": "/mnt/nas/Photos",
                "frames": [{"hash": "...", "name": "A.CR3", "size": 47854166,
                            "whole": "...", "mtime": 1758270600000000000}]}],
     "version": 1}

`hash`, `name` and `size` are §197's. `whole` and `mtime` go in when
known: they are what the on-disk check needs once the frame here may be
gone. The queue runs when the archive answers again:
- the launch's look (taken as due, and run once the watcher's build
  lands with the window);
- a view's read over the roots (a chip clicked);
- a pass over the archive landing (the poll's, or a change's).

It runs through the same code as the sheet's move, with the same check
on the disk, and only once between offline spells.

What Cancel means for the queue:
- A hand Cancel holds. What the run did not reach stays in the queue
  until the archive next comes back from not answering, or the next
  launch; the passes that follow and the next poll do not rerun it.
- A queue run set aside or lost is tried again at the next answer.
- A sheet's move set aside and landing late queues the frames it never
  reached. The archive's silence stopped it, not the user, so those
  frames were confirmed while the archive did not answer, which is
  §197's case. Not when the user had pressed Cancel before it was set
  aside, and not for a move the user canceled outright. An archive heard not
answering (a pass skipped, a look that found it offline) is due again.
What was done leaves the queue. What could not be done stays and is
named in the status line, with the first reason. An entry whose frame
is no longer in the rejects folder here still runs, since nothing in an
entry reads the frame.

The queue is read loosely. An archive's entry with no path, or a frame
with no hash, name or size, is dropped, said, and the file written back
without it; a duplicate hash is kept once. A file that is not a queue
is set aside as `archive-queue.json.unreadable` and said, as
`roots.json` is.

The queue only ever moves. A delete confirmed while the archive does
not answer is refused ("not deleted: Archive is not answering. A delete
is never queued; ask again when it answers"), whether the sheet's look
or the run's own look is the one that found it. A run nobody is at
(`deletes_allowed` false) never runs the queue.

**The rule.** Nothing under an archive is deleted except behind its
own sheet: this one's red button, the chip view's Delete rejects
folder, and a frame's Delete selection there. Back up, Bring back and
the queue never delete.

**What the review changed.** Three rounds: one the author ran itself
before handing the branch over, then two from a fresh reviewer, and a
short third on the Cancel rules. The first:
- **A queue run that started on its own could be overtaken.** It
  started at a poll's pass while a Back up sheet was up, and that
  sheet's confirm took the card from it. The run went on with no card
  and no Cancel, and a backup ran on the same archive at once. Now:
  - part one's confirm is refused while another job holds the card, or
    one set aside holds that archive;
  - a queue heard due waits while any archive sheet is up, and runs
    when it closes;
  - the Remove rejects confirm checks the same, before it takes the
    sheet's state, so a collision keeps the sheet.
- **The open-folder-on-the-archive guard compared two spellings.** On
  Windows `fs::canonicalize` adds `\\?\`, so the guard never fired, and
  the open folder's own rejects were listed as their copies; Delete
  would have taken them. Now the folder is in the roots' spelling before
  the guard, and the look drops any copy in the folder it looked at,
  compared as the disk has it.
- The move gained the containment check the delete had.
- A sheet built while the archive did not answer offered a delete over
  copies seen only in the index; it no longer does.
- The move is not offered when every copy is in a rejects folder there
  already.
- The frames after a trash refusal are said.
- The key test's second half had gone to the window's keys after the
  first answer gave them back; it now has a sheet of its own.

The second (verdict: ready after the must-fix):
- **The run deleted copies the sheet never listed.** It threw away the
  look's copies and looked each reject up again by hash, so a copy the
  indexer picked up between the look and the click was deleted unseen.
  Now the sheet's runs carry the listed paths and act on those alone,
  as above; a test adds a copy after the look and changes a listed one.
- **The destination `rejects` folder was not checked.** A link there
  would have taken the copy out of the archive. Now it, and its hidden
  folder, are refused when a link or outside the archive, tested both
  ways.
- **A queue run canceled, set aside or lost was not tried again** (the
  archive stayed marked tried), and a set-aside sheet move dropped the
  frames it never reached. Both fixed as above, and tested.

A third pass on the user's Cancel (verdict: ready, two should-fixes):
- A hand Cancel followed by a set-aside queued the frames the user had
  canceled. The set-aside now records whether Cancel was already
  pressed, and a late landing queues only when it was not. Tested.
- A hand Cancel of a queue run cleared the archive's tried mark, so the
  passes that followed reran it at once. Only a set-aside or a lost run
  clears it now. Tested: the queue holds through another answer and
  runs after the archive goes away and comes back.
- **The window tests never pressed the real buttons.** The red button
  now has an `accessible-role` and `accessible-label` (still no focus
  and no default action). The tests click the CULLING entry, "Delete
  from nas permanently" and "Move to rejects on nas" by their labels.
  A test drives the indexer's words: a pass skipped makes the queue
  due, a pass landed runs it.
- **The ask asked the disk on the window's thread.** It canonicalized
  the open folder (`shoot_rejects_dir`) before the guard. Now the
  guard is lexical, against the open folder in the reads' spelling and
  the first file, and the rejects folder is joined from that spelling;
  nothing on the window's thread asks the disk.
- **A refused confirm left the sheet up without the keys** (the
  window's answer handler takes them back). It now closes the sheet and
  says why.

**Decisions the design left open.**
- One entry per archive in the CULLING section, a button each; the
  grid header has no Delete rejects folder, so no entry goes there
  (reversed in §228).
- A frame's copies are all the confirmed rows, not the first: two
  backups of one shoot leave two copies, and both are moved or deleted.
- The queue entry carries the frame's whole hash and time beside §197's
  hash, name and size, so a queue run can make the same check as the
  sheet's run with the frame here gone. An entry without them, written
  by hand, is not known at its copy and stays.
- A reject with copies moved and another of its files not known is
  done, the unknown one named and left alone. One with only unknown
  copies stays in the queue.
- An entry whose copy is found nowhere stays in the queue and is named
  at each run, which is once per offline spell. Dropping it would guess
  that the copy was removed by hand rather than not indexed yet.
- The queue of an archive whose mark is turned off stays and is not
  run. When the mark comes back, it runs its earlier confirmation with
  no new prompt; the queue only ever moves, so that is accepted.
- The Remove rejects look never reads both files whole: it runs from a
  button, but its pairs are §217's count's and the Delete sheet's, and a
  rejects folder can be large. An archive filled by a copy that did not
  keep file times therefore shows every reject as "not known".
- The cache's thumbnails are left on a delete from the archive. A
  backup keeps the time, so the copy's stamp is the reject's here, and
  `Thumbs::remove` would take the local frame's pictures with it.
- `--sheet archive-rejects` opens the sheet for the first archive, for
  a snapshot; never answered.

**Tests.**
- The core, on temporary folders:
  - a reject's copy found by hash under another folder;
  - a copy written since at another time not known, then settled by
    both whole hashes;
  - a copy whose size changed refused at the look and again at the run;
  - the move with a `.gcd` beside and one under the hidden folder plus
    an XMP, placements kept;
  - a sidecar's name taken there leaving the copy whole;
  - the delete through the plan, a linked copy refused and a folder
    leading out of the archive not allowed;
  - Cancel between frames;
  - offline from the index;
  - a run held mid-frame behind a short wait, set aside and landing
    late with its second frame not begun;
  - the queue read loosely, written back, emptied, and set aside when
    it is not a queue.
- The window:
  - the sheet's words and buttons, and Enter's move with the card and
    the report;
  - an answer not offered doing nothing;
  - the trash refused earlier offering both, the permanent delete
    taking the copies and their sidecars, nothing queued;
  - offline: no delete offered, one asked for refused, the move queued;
    one offered online and confirmed once the archive was away refused
    by the run's own look;
  - the queue run when the archive answers, with a reject gone here
    still moved, an entry that cannot be done left and named, and a
    malformed one dropped and said; not run again without an offline
    spell;
  - a malformed-only queue dropped and the file removed;
  - the chip clicked running the queue;
  - the launch's look running it, but not in a run nobody is at, and
    not under an open sheet until it closes;
  - a queue run not overtaken by a Back up confirm;
  - an open folder on the archive refused;
  - a run set aside holding the archive and landing late;
  - Cancel through the card, and a lost run letting go;
  - Enter only ever the move, Tab and space reaching no delete, Escape
    the no.

**Checked in the editor.** A scratch shoot of one raw, two raws in its
rejects folder, and copies of those two (times kept) under
`nas/clients/shoot` with `nas` marked an archive.
`--sheet archive-rejects` shows the sheet: "2 rejects here have 2
copies on nas, 93.3 MB:", each reject beside its copy, Cancel, the red
"Delete from nas", and the blue "Move to rejects on nas". Neither button
was pressed in the editor; the tests press them. A capture does not
pass over the roots, so the archive's rows came from an `--all-roots`
capture first. Before that, the sheet said no copy was found, which is
right for an index that has never seen the archive.

**Not done.**
- Settling "not known" pairs, for an archive filled without file times:
  the Back up sheet's settle (both files read whole, the hashes kept)
  is the way, and a follow-up should let it cover the rejects too.
- No real NAS. The hang is the tests' slow fake, and the trash on a
  share (the home trash taking a copy over the network, §190) was not
  tried.
- A delete from the archive leaves the deleted copies' thumbnails in
  the cache for the eviction.
- One frame in two places, and the sidecar-sync line, as before.
