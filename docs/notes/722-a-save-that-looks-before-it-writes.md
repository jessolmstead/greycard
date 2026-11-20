# §722. A save that looks before it writes (2026-11-20)

**Asked.** A save wrote the whole sidecar from memory without asking
whether the file was still the one read, so a second writer on a
loaded frame (another window, a machine on a shared folder, a sync
tool, `greycard presets apply`) was undone, silently, at our next
change (review, 2026-11-11).

**Decided.** The save checks the file first, and never merges
histories. Four outcomes, in `sidecar/checked.rs` (`checked_against`):

1. **Written.** The file is the one we read or wrote (stat, then
   bytes), or behind ours: ours is written, as before.
2. **Joined.** The other writer changed only the meta (rating, label,
   tags, marks, the frame's turn kept equal): ours is kept and the
   meta is joined by field times.
3. **Took.** We changed nothing in the lines and the file went on
   from ours: theirs is taken whole, our meta joined into it.
4. **Kept.** Anything else: the other writer's file is copied beside
   the frame as `<name>.gcd.other` (then `.2.other`, and on), and ours
   is written. At most four are kept a sidecar, the oldest going
   first, identical bytes kept once, each written whole and synced. A
   copy that cannot be kept refuses the save. The status line says
   where the copy went.

Also in the check: a sidecar that cannot be read is set aside as
`.bad` and the edit here saved; one from a newer greycard refuses the
save with its reason; batches and the CLI go through the same rule.
What a save puts aside moves, renames, and goes to the trash with its
frame; a copy to the archive leaves it.

**Why no merge.** Five rounds tried to join the two histories on the
save path, as the archive sync does (§243), and each review found a
loss. The cause is one fact: recorded states are rewritten under
their ids. A quarter turn rotates the crop, masks, and patches in
every state; a look rename, the ISO seed, a migration at read, and an
older build's save all change states in place. So "this id is that
content" is false for any frame ever turned with a crop, and every
rule built on it misfired: a turn with masks left a geometry neither
side made, and two windows never settled. Until states are never
rewritten (the roadmap's item, with a design drafted), the save keeps
both files and merges nothing.

**How it tells "we changed nothing".** By content, not by ids or a
flag: the lines print, a hash over the file's bytes of every
top-level field but the nine meta fields, unknown fields counted as
lines. A file whose bytes we would not write the same (an older
build's, a migrated one) fails the comparison and falls to Kept, the
safe side. A turn on a frame with no geometry leaves the print
unchanged, so the turn is compared on its own.

**Two windows settle.** Rule 4 records the other file's revision in
`set_aside`, a field only rule 3 reads: the other window, with
nothing new, finds its revision there and takes ours. It is not
lineage: the first version put it in `absorbed`, and the archive sync
then read our file as ahead of the other machine's copy and wrote
over it, where master joined. `compare`, `knows`, and `join` never
read `set_aside`; after a Kept the archive sync says Diverged and
joins, as before.

**Measured.** The matrix, 13 window changes by 20 other-writer
changes on states carrying a crop, keystone, masks, a patch, a look,
a snapshot, and a virtual copy: 78 Written, 123 Kept, 36 Joined, 10
Took, 13 refused as newer; in every Kept cell the kept file is the
other writer's bytes and the archive's verdict equals master's. Two
windows saving in turn for 40 rounds: only Written or Took after the
second, one copy kept. 400 conflicts on one frame end on the newest
four copies. A read costs about 20% more on a small sidecar (the
print and a second hash; 131 to 160 µs); the library's scan does not
pay it.

**Left.** The rename and move found by the index list the old folder
once a frame; two processes keeping in the same instant can pick one
number; the words are provisional until signed off.
