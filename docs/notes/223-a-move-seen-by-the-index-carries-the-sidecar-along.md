# 223. A move seen by the index carries the sidecar along (2026-10-02)

The first of §220's two items: a raw dragged by hand out of a rejects
folder, or anywhere, came home without its sidecar, which stayed under
the old folder's hidden `.greycard` where a file manager does not show
it, and the next cull flagged the frame again.

**Where.** A pass over a folder finds a file new to it, hashes it, and
asks the index for a row with that hash whose file is gone from a folder
still in use: a move, and the row follows (§207). The sidecar is
carried at that moment, in the pass's first phase, before the new
place's sidecar is read, since the second phase holds the write lock
and asks the disk nothing (§217). So the row's meta, written in the
second phase, is the carried sidecar's: the rating, the flag and the
edit come with the frame in the same pass that sees it move.

**The rule.** The sidecar the old path has, beside it or under its
folder's `.greycard`, goes to the new path in the placement it had, the
hidden folder made at the new place when that is where it goes; the
same as Move rejects keeps the placement across a cull (§153). Nothing
is written over: a sidecar at the new place under either placement
leaves the old one where it is, and so does a name taken there by the
time the move is made, since the move is a hard link, which fails on a
name in use, then the unlink; across devices or on a file system
without links, a copy into a file that must be new, then the unlink.
The XMP is left: it is a visible file beside the raw, and the hand that
moved the raw and not the XMP can move it. The report counts what was
carried, and the log names each.

**What it does not cover.** A folder renamed or moved whole is not a
move to the index, since the old folder gone looks like an unmounted
drive (§160); its sidecars came along inside it anyway. A copy from an
unmounted drive is not a move either, and nothing is carried from a
drive that is not there. The second phase may still refuse the move,
when another writer changed the old row between the phases; the sidecar
has been carried by then, and the new row reads it, which is right: it
is this frame's own, by hash.
