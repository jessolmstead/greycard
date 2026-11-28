# §760. Clear history (2026-11-28)

A tester asked for a way to drop a frame's history (discussion 167):
50 steps on a brush mask write 1.1 MB of sidecar, and 90 KB once
cleared.

**What it keeps.** Clear history, in the frame's menu beside Reset
to Original, acts on the version on screen. It keeps the first state,
the current state with its id, and every snapshot, and drops the
steps between and the undone steps. The picture does not change. The
frame's other versions are left alone: a clear of something not on
screen would destroy what the user cannot see. It asks nothing first.

**Undo.** History is itself the undo record, so the cleared steps go
to a stash on the line, held for the session and never written. Undo
right after puts them back where they were and lifts the tombstones;
Redo clears again. The next step, a snapshot restored, a move through
the history, a join, or a fresh read drops the stash. A clear is
weighed against a culling mark by when it was made, as an edit is.

**The join.** Without more, a clear lasts only until a copy edited
elsewhere joins: its states pass as capped away and come back
(§244). So each cleared state leaves a tombstone, `cleared` on the
line, an id and a time, capped at 500 and joined as the snapshots'
are (§734). A state's id names one state on every machine (§724).
The join drops the named states from both copies before its walk,
never a copy's first or current state, and keeps the tombstones less
any for a state it holds. `compare` and `behind` filter alike, or a
cleared copy with no revisions would read as behind and be written
over. The join's checks (§757) hold that nothing cleared returns from
either side or a third copy, and that an edit made elsewhere is kept.

**Older builds.** We did not raise the write floor. 0.13 ignores the
key and drops it at its next write, so there the steps can come back
from a stale copy; the edit is never lost. Locking 0.13 out of every
cleared file was the worse trade.

## Left open

- A copy that synced between a clear and its Undo keeps the
  tombstones, and drops the restored steps at its next join.
- Past 500 tombstones on a line the oldest go, and a stale copy with
  an edit of its own can bring those steps back.
- The open frame only; a selection is asked for.
