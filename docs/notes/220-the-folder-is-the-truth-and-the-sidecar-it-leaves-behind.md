# 220. The folder is the truth, and the sidecar a hand move leaves behind (2026-10-02)

From the evening's NAS test (§219's), a second thing the first real
cull over an archive turned up, and the user's question that followed
it: how the index ever gets back in step with what is on the disk.

**What went wrong.** Move rejects moved one frame of several and said
the rest were in the rejects folder already. They were not; their
sidecars were. The frames had been rejected once before, and with no
way back inside the editor the user had dragged the raws out of the
rejects folder by hand. The raws came home; the `.gcd` files stayed
behind, under the rejects folder's hidden `.greycard` (§153), where a
file manager does not show them. The next cull flagged the frames
again, and the move refused each one because a file of one of its
names was in the folder already: §123's rule, never a raw moved and a
file beside it not, and never a file written over.

**Now.** The rule is split by whose folder it is. In the shoot's own
rejects folder a sidecar under one of the frame's names with no raw of
that name beside it is an orphan of this very frame, left by an earlier
cull, and the frame's own sidecar, the one carrying the flag now, goes
over it (`cull::Orphans::WriteOver`). The raw's name taken still keeps
the frame whole where it is. On an archive nothing is ever written
over, so Remove rejects' move (§218) keeps the stricter rule: an
orphaned sidecar there leaves the copy whole and names the clash
(`cull::Orphans::Keep`). The shoot's folder is the user's working
folder and the archive the copy that has to outlive mistakes; the same
file means a different thing in each.

**How the index gets back in step.** The question the bug raised, and
the answer §207 implies without saying in one place. The index is a
cache of the folders and never a second truth; the folders are brought
to it by passes:
- at launch, a tree pass over every root that answers the look (§187,
  §215);
- on a local root, the watcher sees a change within seconds and hands
  the folder to the indexer (§188); a raw dragged out of a rejects
  folder is seen this way;
- on a network root, which is not watched, a tree pass on the poll
  (§188), when its chip is clicked, or when a folder under it is
  opened;
- after the editor's own moves and copies, the folders touched handed
  over at once: Back up, Bring back, Move rejects, Remove rejects, a
  delete.

A pass takes a file at its path with the same size and time as
unchanged; one with another size or time is read again and its whole
hash cleared (§217); a file gone from its path is marked missing, not
forgotten, so an unplugged drive keeps every edit (§72); a file found
elsewhere with the same content key is a move, and its row follows it.
Missing rows are kept for good: the editor never prunes them (the CLI
can), the views leave them out, and every archive lookup confirms a
find on the disk before it counts (§216), so a stale row cannot make a
frame look backed up. The editor's own deletes forget their rows
outright (§218).

**What a pass cannot repair.** The sidecar. The index caches what the
sidecar says, and a raw dragged home without its sidecar has none to
cache: its edits and its flag are in the orphan under the old folder's
`.greycard`. That is the hazard of §153's hidden placement, and it is
easy to walk into, since the folder shows nothing to carry. Two items
join the roadmap for it, both to be built as a section of their own:
- a move seen by the index carries the sidecar along: a pass that
  takes a raw gone from one folder and new in another for a move
  already knows where the row's sidecar was, and moves it to the new
  folder's placement when the new place has none, so a hand move
  anywhere keeps the edits and the flag;
- Move back, the inverse of Move rejects, on a frame in a rejects
  folder, with its sidecars, so the hand move is rarely needed at all.

Sidecars beside the raw by default would make them visible and was
considered and left: §153 chose the hidden folder to keep a shoot's
folder to its frames, and the first item above protects the user who
forgets, which is the case that matters.
