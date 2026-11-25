# §742. A sidecar with no state that 0.12 refuses (2026-11-24)

§734 made a sidecar from a newer greycard read-only to an older one.
0.12 and older tell a newer file only by a state's version: `migrate`
refuses a `current`, a history entry, or a snapshot past their schema.
A 0.13 file nobody developed holds no state, so 0.12 read it, saved
it again at schema 9 without the keys it does not know (an owed
turn, the shape, the tombstones, the swap time, a line's sources),
and removed 0.13's copy in the other place.

**The stop.** Where no line holds a state and the file holds a key
outside 0.12's lists, or a write floor above 10, we write
`"history": [{"version": 10}]` on the line. 0.12 migrates history
whether or not there is a `current`, and refuses the file as newer;
it then writes nothing and removes nothing. We read no history on a
line without a `current`, so the file reads back with no state and
writes again byte for byte, and a first edit records only its root.
The stop says 10, the first schema with floors, whatever ours is: a
later build that keeps the floors at 10 must not write one that 0.13
refuses.

**Not every file.** A file holding only a rating, a flag, a label, a
turn, or a plain virtual copy takes no stop: 0.12 loses nothing of
it, and a 0.12 and a 0.13 computer go on sharing those. `floors` and
`set_aside` do not count as lost; a file without floors is judged by
its states.

**Proved on 0.12's own reader.** Its `greycard-edit` at the release
tag, built apart, refused each file with the stop and left both
places' copies as they were; without the stop it read and rewrote
each. Every read in 0.12 that comes before a write goes through
that refusal, and 0.7 to 0.11 migrate history the same way.

Builds of master from 2026-11-22 to 2026-11-24, never released, read
history on a line without a `current`, and would show the stop as
one more undo step.
