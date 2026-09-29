# 197. Archive roots: back up, bring back, and the rejects on the archive (2026-09-28)

The design, written before the build; the built sections will follow
with what they measured. 0.3.0 made a root on a network share safe to
have (§187, §188, §192). This is what such a root is for. Most shoots
end up in two places: the folder they were culled and edited in, on a
laptop or a desktop, and a copy on a NAS or in a mounted cloud folder
that is meant to outlive the machine. Today greycard treats both as
plain roots and knows nothing of the relationship, so the copy is made
with rsync or a file manager, the sidecars come along only if the
person remembers the hidden folder (§153), and the rejects culled at
home stay on the archive forever. The three actions here are the ones
that workflow needs and nothing more: Back up, Bring back, and Remove
rejects.

**What an archive root is.** A root the user has marked as an archive.
The mark lives in `roots.json` under a key of its own, `archives`, a
list of paths, exactly as the names do (§185): a build that knows
nothing of archives reads the file as a list of ordinary roots, which
is what an archive is to it, and the version stays at 1. An archive is
not a different kind of folder on disk. It is a root with two rules
attached: greycard never deletes a file under it, and never writes
there except through the three actions below, each behind a sheet. It
is passed over at launch and by the poll like any other root, so the
index holds its files with their hashes, which is what the actions key
on. It is not watched, but a share is not watched anyway (§188).

The mark is set from the root's chip menu, beside Rename ("Use as an
archive", a toggle), and the chip carries a small mark. Turning it off
turns off nothing but the rules; the files are where they were.

**Where the archive's frames show.** In the archive's own chip view,
as any root's do. Not in All roots, for now: with a shoot on both
sides, All roots would list every frame twice, and the view would be
no use for the thing it is for, judging a body of work. One frame in
two places (the roadmap's next line) is what lets an archive's copies
back into All roots, shown once with the local copy preferred; until it
lands, an archive root is left out of All roots and its chip says so
when hovered. That is a stopgap and is named as one.

**Back up.** An action over the open folder, or the selection when
there is one, from the grid's header menu and the frame menu: "Back up
to <archive>…", with a pick when there is more than one archive. The
sheet lists what would go before anything does:
- the frames not on the archive, with their sidecars, and the total
  size;
- the frames already there whose sidecar here is newer, sidecar only;
- the frames already there and the same, skipped, with a count;
- the frames whose archive sidecar is the newer one, skipped and named,
  since copying over it would lose an edit made from the archive's
  side. §161's rule decides newer: the `saved` counter when both carry
  one and they differ, mtime otherwise.

Where a frame goes on the archive mirrors where it is here:
`<archive>/<root's label>/<path relative to that root>`, the label
being the root's name when it has one (§185) and its folder's name
otherwise. A folder open from outside any root goes under its own
name. The mirror is what makes Bring back and one frame in two places
cheap later: the relative path is a hint, and the hash is the
identity, as §72 has it for everything.

A copy is verified by hash, and the hash here is the whole file, not
§72's head-and-size key: the key tells two frames apart and cannot
tell a truncated copy from a whole one, which is the failure a backup
exists to catch. BLAKE3 is taken of the source as it is streamed to
the destination, and the destination is read back and hashed once it
is closed; a mismatch removes the copy and names the frame in the
report. Reading back doubles the traffic on the archive's side and is
the point. The `.gcd` goes to the same placement it has here, beside
or under `.greycard`, and the XMPs beside, as Move rejects carries them
(§190). Never a delete on the archive: a file there that is not here
stays, and the report never mentions it.

The copy runs off the window's thread, one file at a time (latency is
the limit on a share and a second stream buys little), with §192's bar
filled by bytes rather than files so a 100 MB frame does not look like
a stall, a Cancel that takes effect between files, and a report in the
status line. When it is done the destination folders are handed to the
indexer as folder passes, so the archive's rows are current without
waiting for the poll. Nothing runs on open, on a timer or on a save;
Back up is a verb the user says.

**Bring back.** The inverse, from the archive's chip view over a
selection or a folder: "Bring back to <root>…", with a pick of the
local roots. The mirror is unwound where it can be (a frame under
`<archive>/Photos/2026/Skye` goes to the local root labeled Photos, at
`2026/Skye`), and otherwise the sheet asks for a folder under the
chosen root. The sheet shows the size before the confirm, and the
frames whose hash is already under a local root are skipped and named
with where they are, since bringing a frame back beside its own copy is
never what was meant. The same whole-file verification, the same
sidecar rule with the sides swapped, the same bar and Cancel. The
destination folders are indexed when it is done, and the folder is
offered to open.

**Remove rejects.** Culling leaves a `rejects` folder beside the shoot
(§80, §190). The archive still holds those frames under their old
names, and going through the archive by hand to find them is exactly
the chore this is for. The action sits beside Delete rejects folder in
the CULLING section: "Remove rejects from <archive>…". It takes the
frames in the rejects folder, finds each one's copies under the
archive by content hash in the index (`by_hash`, narrowed to the
archive's root), confirms each is still on disk at that path with that
size, and shows the list: the reject here, its copy there. On confirm
each copy moves into a `rejects` folder beside itself on the archive,
its sidecars with it, exactly as Move rejects moves a frame here; a
name already taken there leaves the frame where it is, said. Nothing
is deleted. Deleting the archive's rejects folder is the archive's
own Delete rejects folder, run from its chip view, behind §190's
sheet, and is not this action.

A removal the user has confirmed while the archive is offline is
queued, in `archive-queue.json` beside `roots.json`: the archive's
path, and for each frame its hash, its name and the size it had. When
the archive answers again (the launch's look, the poll's, or a chip
clicked) the queue is run through the same code path with the same
on-disk check, said in the status line, and what could not be done is
left in the queue and named. The confirmation was given when the
sheet was, and moving a copy into a rejects folder on the archive is
the one thing here that is allowed to happen without the user
watching, because it deletes nothing and is undone with a drag. Back
up and Bring back are never queued: a copy that starts on its own
when a share appears is the behavior §72 set out not to build.

**Cloud.** A mounted cloud folder (rclone mount, a provider's own
client) is an archive root like any other, and the roadmap's rule
stands: a mount or a configured command, never a provider's API. A
command run after a Back up with the paths that landed (`rclone copy`
to a bucket the mount does not cover) is the same hook as the pool's
post-export command and is not built here.

**What the index has to be trusted with, and what it is not.** Back
up and Bring back decide what to copy from the disk on both sides
(exists, size, then the sidecars read), not from the index: an archive
offline at the last launch has stale rows, and a copy made on a stale
answer would either skip a frame that is not there or copy one that
is. The index is used to find things: which frames under the archive
carry a reject's hash, and which local root already holds a frame
being brought back. Every find is confirmed on disk before anything
moves.

**Two landings, not one.** The archive mark with Back up and Bring
back first, since they are one copy with the direction reversed and
share the sheet, the verification and the bar. Remove rejects with its
queue second. One frame in two places waits on the first, not the
second.

**Not decided, left to the build.** Whether the whole-file hash of the
source is worth keeping in the index once it has been computed (a
column, for a later verify-the-archive action) or thrown away; whether
Back up over a folder should include its `rejects` subfolder (no, by
default: the rejects are what the archive should not have); and how a
frame whose file changed under the same head and size (a re-exported
JPEG in a folder of exports) is told apart from its copy, since the
key does not see it. The size and mtime do, and that is likely the
answer, but it is a case to test rather than assume.
