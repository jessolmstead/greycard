# §757. History capped at the read, and the join checked (2026-11-27)

**The cost.** A sidecar of 100,000 `{}` history states, 300 KB on
disk, took 9.6 s and 207 MB to read in a debug build. Nothing was
parsed twice: a state written in three bytes is read as a whole edit,
about 88 allocations and 1.5 KB held, migrated and given an id like
any other. The only lever is the count.

**The cap at the read.** Every build has written at most 50 states a
line (`HISTORY`), by the record and by the join. So the read now cuts
a longer history to 50 before any state is migrated or read: the
first state stays and the oldest after it go, after differences are
made whole, on every line. The same file reads in 0.03 s with an 8 MB
peak. A file a build made loads whole. The cut is plainer than the
write cap's: it does not drop repeats first or hand export records
on, which matters only to a file no build wrote. We did not charge
states against the differences' budget, which would change what that
budget means, nor refuse the file, which would lose the frame's edit.

**The join's checks.** `sync::join_check` holds what a join must
keep: no line or version id held twice, the same result run twice and
from either side, each copy's original kept unless the other has seen
its last revision, no tombstone for something held, versions in the
order made, and a copy joined with itself unchanged. A fuzz target
(`join_json`) and a sweep of mutated pairs in the tests run it. It
found two faults:

- A line from before versions that nobody developed kept its id in
  memory, though none is written. Joined with the same file read
  back, a version of that id landed on the original or was held
  twice. `set_start` now clears what is not written.
- Two copies tied on time, revision, host, and current state joined
  differently from each side, the first argument winning. `newer_of`
  now breaks a full tie by the file as written and its lines' state
  ids. The same tie-break reaches the join of two copies' ratings and
  labels: where they tie in full and no field's time decides, the
  kept copy no longer wins by being first. Only files from before
  field times can tie so.

## Left open

- The fuzz target has not run under libFuzzer; the sweep ran 20,000
  pairs.
- Snapshots and versions have no cap at the read.
