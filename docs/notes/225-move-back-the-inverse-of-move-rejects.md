# 225. Move back, the inverse of Move rejects (2026-10-02)

The second of §220's two roadmap items for the sidecar a hand move
leaves behind, beside §223's carry: a way back out of a rejects folder
inside the editor, so the drag in the file manager that orphaned the
sidecars in the first place is rarely needed at all.

**What it is.** A "Move back to <folder>" item on the frame menu,
shown when any of the selection is in a rejects folder, and the same
words on a button in CULLING under the move-rejects button: "Move back
to shoot" for one frame, "Move 2 frames back to shoot" for more, in
both places. Both act on the frames of the selection that are in a
rejects folder; the others are left out, so a set that mixes the two
is safe to ask over. Each frame goes into the folder above its rejects
folder with its `.gcd` and its XMPs (`cull::move_back`), by the same
code Move rejects moves with (`move_into`): a sidecar under the rejects
folder's hidden `.greycard` goes under the folder above's own, one
beside stays beside (§153), and the XMPs are always beside. A frame
under an offline root is not offered, as Move rejects leaves one alone.
There is no sheet: Move rejects asks first because it acts on every
flagged frame of the folder at once, many of them off screen, while
Move back is over what the user has chosen, writes nothing of another
frame's over, and is undone by Move rejects. The folder is named from
the path alone, since the menu and the button ask at every change of
the selection; only a frame opened by a bare relative name, whose
folder above is ".", has its folder looked up for a name.

**Never a file written over.** §123's rule holds in the inverse: a raw
of the frame's name in the folder above leaves the frame whole where
it is, and the status line names the clash ("1 left where it was
(b.tif is in /shoot already)"), as Move rejects' does. The orphan is
the case §220 settled for the move out, met from the other side: a
sidecar in the folder above under one of the frame's names with no raw
of that name beside it. It goes the same way, the frame's own sidecar
written over it (`Orphans::WriteOver`). The folder above is the shoot,
the user's working folder, so §220's reasoning for it applies whole:
a sidecar with no frame is an earlier copy of this frame's, left when
it was culled or dragged about by hand, and the one in the rejects
folder is the one that has traveled with the frame since, with the
flag and the newer edits on it. Keeping the frame whole for it would
refuse the move every time for a file nobody can see, which is the
very bug §220 was about. A frame with no sidecar of its own leaves an
orphan there alone and takes it up: it is this frame's, and the only
edit it has.

**Under either placement.** The first cut looked for an orphan only
where the frame's own sidecar would land. A frame's sidecar beside it
in the rejects folder, with an orphan under the shoot's `.greycard`,
then left both after the move; `Sidecar::find` takes the one saved
more times, which was the orphan, so the frame read the old edit, the
flag stayed on the shadowed copy, and the next save settled the frame
by deleting its own. Now both placements are looked at, as §223's carry
does: written over, the frame's sidecar takes the place under its own
placement and an orphan under the other goes; kept, on an archive
(`Orphans::Keep`), either one leaves the copy whole and is named. Move
rejects has the same fix.

**Which files a frame there can own.** A `.gcd` and an XMP's long name
(`IMG.CR3.xmp`) carry their raw's whole name, so nothing but that raw
can own one, and with the raw's name free one there is always an
orphan; an unrelated `IMG.CR3.jpg` or `IMG.JPG` beside it changes
nothing. The short XMP name (`IMG.xmp`) is different: it is a frame's
only while nothing else in its folder answers to the stem, and in the
destination the camera's `IMG.JPG` can be there. Matched as the XMP
reader's own test is, but without regard to case, since the disks of
two of the three platforms match names that way, and with ours and
`.tmp` files no picture. When such a file is there, the frame stays
whole and the file is named, either way: with an `IMG.xmp` there, it is
the JPEG's, and writing over it would lose it ("B.xmp is in /shoot
already"); with none, the frame's short XMP moved in beside the JPEG
would be nobody's, since the reader no longer takes the short name for
either, and the frame's ratings would be lost to every tool that reads
it ("c.jpg there answers to C.xmp too"). Moving it and saying so was
the other choice and was left: the status line would report a loss
rather than prevent one, and the rule since §123 is that a frame moves
whole or not at all. The case is rare, since the editor writes the long
name when a stem is shared, and the named file tells the user what to
move by hand. Move rejects had the hole too, writing a JPEG's XMP over,
and has the fix.

**A raw and its JPEG together.** The review found the rule splitting
a pair. A.CR3 and A.JPG of one shot, each with its `.gcd`, share one
A.xmp, which is nobody's while both are in the folder. Both flagged
and moved, the raw goes first; the JPEG is then alone, A.xmp reads as
its own, and at the destination the raw just moved answered to the
stem and refused it, leaving the pair half in each folder. A file this
same move brought in is no other frame's claim on the short XMP, so it
is passed over (`stem_owner`'s `arrived`): the three move together,
out and back, in either order. A file there before the move, or an
A.xmp already there with such a file, still refuses. An A.xmp already
there with no file of the stem but the ones the move brought is an
orphan, and goes under the frame's own as any other. With the raw
alone chosen, the XMP was nobody's while the pair was together and so
is not the raw's to take: it stays with the JPEG left behind, whose it
then is, and the raw goes with its `.gcd`. Moved back alone, the raw
finds the JPEG answering to A.xmp, which is not its own, and nothing in
its way; the pair is whole again.

**The flag.** A frame moved back has its reject flag taken off, in its
sidecar at the new path, after the move and only for the frames that
went. Move rejects leaves the flag alone, since the flag is the reason
for the move and the rejects folder is where flagged frames live; the
inverse is the user saying the frame is not a reject, and a flag left
on would send it straight out again at the next Move rejects, its count
on the button saying so. Taking it off before the move would leave a
frame that stayed (its name taken) unflagged and still in the rejects
folder, so it comes after. The write is the window's own when the
sidecar in memory is the frame's; under a view of the roots, where the
frame's row may still be standing in for an unread sidecar, the
sidecar is read from the disk, changed and written back where it was
found. It is not an undo step: the frame leaves the list, and Move
rejects puts it back. The move keeps the sidecar's placement and the
flag's write is a save like any other, so it settles where the setting
says (§153); under the beside setting, a sidecar that came back under
the hidden folder is written beside at once and the hidden copy goes.
With sidecars off nothing is written, so the flag stays Reject on the
disk and the next Move rejects takes the frame out again; with XMPs
off an XMP that says reject is left as it is, as every save leaves it.

**The list and the index.** The frames that went leave the browser's
list as Move rejects takes its frames out (`drop_files`), so the
selection goes on to the nearest frame left. The index is then told of
each move directly (`Library::file_moved`, sent through the indexer's
own channel as a delete's forgetting is, §190): the row at the old path
is taken to the new path, folder and name with its id, size, time and
hash, found again if it was missing, and read again where it is now; a
row already at the new path, from a file that was there once, goes.
Hash detection is not enough here. A pass that finds a file new to a
folder looks for a missing row of its content, but a pass over a folder
the move emptied takes the empty folder for a drive that is away
(§160) and marks nothing missing, so Move back of the only reject left
the old row standing and the new path got a second one: a ghost frame
under All roots, with Move back offered on it. The editor knows both
paths, so it says them. Move rejects tells the index the same way. The
two folders, the one each frame left and the one it went to, are then
handed over at once as folder changes, as §220 says the editor's own
moves are, so the counts and the tree follow; in a view of the roots
the merge brings the frame back into the list at its new path. Move
rejects did not hand its rejects folder over before, relying on the
pass over the open folder, which under a view of the roots is none,
and on the watcher; it now does. §223's carry and these moves do not
fight: the carry moves a sidecar only when the new place has none, and
after the editor's move the frame's sidecar is there already.
