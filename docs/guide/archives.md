# Archives and backup

An **archive** is a library root that holds the long-term copy of your
shoots, such as a NAS or a large external drive. Once a root is marked
as an archive, greycard can copy a shoot to it, bring a shoot back
from it, keep the two copies' edits in step, and clear your rejects
off it.

Use this when you shoot to a laptop and keep everything on a bigger
drive, or when you edit on two computers, such as a laptop and a
desktop ([Working on two computers](#working-on-two-computers)). If
all your work lives in one place, you don't need it. Roots themselves
are covered in [Folders and the library](library.md).

## Marking a root as an archive

With the grid up, right-click the root's row under **ROOTS** (Control+
click on a Mac) and choose **Use as an archive**. A small cloud
appears beside its count.

Choose it again to unmark it. Nothing on disk moves either way.

An archive's own view works like any root's. You can browse, rate,
cull, edit and delete there.

## Back up

**Back up** copies the frames that are not on the archive yet, with
their sidecars.

1. Open the folder, or select the frames, you want to back up. This
   works from a root, a folder in it, or a folder outside the library,
   but not from **All roots**.
2. Click the cloud button in the grid's header, or right-click a frame
   and choose **Back up to Archive...** (your archive's name). With
   two archives, pick one from the menu.
3. The sheet looks at what is already there, then says what it will
   copy and how big it is.
4. Check the folder under **To this folder:** and press the button,
   such as **Back up 120 frames**.

The header acts on the open folder and every folder under it, or on
the selection when more than one frame is selected. The menu acts on
the selection.

### What gets copied

- A frame already on the archive is not copied again, even under
  another folder or another name there. greycard recognizes it by its
  content.
- If one copy's edit is behind the other's, only its sidecar is
  updated, on whichever side it is. If both copies were edited, the
  edits are joined (see [One frame in two places](#one-frame-in-two-places)).
- A file of the same name that is a different picture is left alone,
  and the sheet names it.
- `rejects` folders are left out. Tick **Include the rejects folder**
  to copy them too.

Each file is read back and checked after it is copied. Back up never
writes over or deletes a picture on the archive.

The copy runs in the background, with a bar and **Cancel** at the
foot of the window. **Cancel** stops after the frame in hand. If the
archive stops answering for a few seconds, the copy stops after its
current frame and the status line says so. Run **Back up** again
later; it skips what already arrived.

### The header's count

Once a folder has been backed up, the header's button shows what has
changed since, such as "3 frames not on Archive" or "All on Archive".
Click it to back up the rest.

"Not known" in the count means greycard could not tell without
reading the whole file. The **Back up** sheet checks those fully.

The **Delete** sheet also says whether the frames you are about to
delete are on an archive: "All 340 are on Archive", or "3 of these
are on no archive".

## Where a shoot goes on the archive

The first time, the folder under **To this folder:** mirrors your
root: the archive, then the root's name, then the shoot's path under
the root. Change it to fit how your archive is laid out. It must be
inside the archive.

greycard remembers the folder you chose for that shoot. The next shoot
from the same root starts beside it. For example, after you back up
`Skye` to `Archive/2026/Skye`, the next shoot `Harris` starts at
`Archive/2026/Harris`.

## Bring back

**Bring back** copies frames from the archive to a local root, for
when you want to work on an old shoot again or you deleted it from
the laptop.

1. Open the shoot in the archive's view (its row under **ROOTS**, then
   the folder in the tree), or select frames there.
2. Click **Bring back...** in the header, or right-click and choose
   **Bring back to** and a local root.
3. Check the folder and confirm.

If the shoot was backed up from this computer, the folder is the one
it came from, and the sheet says "Where it was backed up from". If
not, greycard suggests a folder under the root and asks you to check
it.

A frame that is already here is not copied. Its sidecar is updated or
joined as with **Back up**. The same checks, bar and **Cancel** apply.

## Working on two computers

A laptop and a desktop can share their shoots through one archive,
such as a NAS both can reach. Each computer works on a copy of its
own, and the archive's copy carries the edits between them.

1. On each computer, add the NAS as a root and choose **Use as an
   archive**. Keep your shoots under a local root.
2. On the computer that has the shoot, **Back up** its folder.
3. On the other, open the shoot in the archive's view and
   **Bring back** a copy. You can also edit it on the archive
   directly, while the NAS is there.
4. Edit on either. Each save also goes to the archive's copy.

The other computer's edits don't come over by themselves, and
greycard doesn't check the archive when you open a frame or a folder.
They arrive when you:

- run **Back up** on the folder, or **Bring back** from the archive's
  view. With nothing else to copy, its button reads **Copy the
  sidecars of 12 frames**.
- save that frame here. greycard finds the archive's copy changed and
  joins the two.

So run **Back up** on a shoot before you work on it on the second
computer. A frame only the other computer changed then takes its edit
whole. Save a frame here first, even a rating, and this computer's
edit stays on top, with the other's one undo away in History (see
[One frame in two places](#one-frame-in-two-places)).

Collections, smart collections, and stacks go with the folders they
touch: to the archive from a folder you have backed up from that
computer, and to the other computer when it next looks the archive
over ([A root on a network share](library.md#a-root-on-a-network-share)).
A collection changed on both is kept twice, the earlier one named for
its computer, such as "Selects (on laptop)".

Some things stay where they were made:

- exported files, presets, fitted looks, and settings (copy presets
  and looks yourself, as [Presets](presets-and-history.md) and
  [Fitting your camera's look](camera-match.md) say)
- a delete: the frame stays on the archive and on the other computer
- the collections and stacks of a shoot copied to the NAS outside
  greycard, until you run **Back up** on its folder once

Keep both computers' clocks right, and both on the same version of
greycard. The clocks decide which edit is on top when both changed.
A frame edited by a newer version opens read-only and says so, and
**Back up** holds it. 0.12 and older can't read 0.13's edits at all,
and leave them alone.

## One frame in two places

A frame on your laptop and its copy on the archive are treated as one
frame.

- **All roots** lists it once, from the local copy. If the local copy
  is gone or its drive is offline, it is listed from the archive.
- Every time you save an edit, rating or flag, greycard also writes it
  to the archive's copy, in the background. A quick run of saves is
  one write.
- If the archive is offline, the status line counts what is waiting,
  such as "3 edits waiting for Archive". The list is kept when you
  quit. It is written when greycard next finds the archive answering,
  or when you save the frame again. Click the note to try now.
- Before it writes, greycard checks whether the archive's copy
  changed since it last wrote there. If it did, the two are compared
  instead of one written over the other.
- A copy that is only behind takes the other's edit. If you edited
  both copies separately, on two computers for example, greycard
  joins them instead of picking one. The edit of the copy saved last,
  by its computer's clock, is the one you see. Both histories are
  kept: the join is a step called "Reconciled with the copy on" and a
  computer's name, and one undo brings back that computer's edit.
  Ratings, flags and labels each keep their most recent change.
- If a frame's drive goes offline while it is open, greycard can carry
  on from the archive's copy, and the status line says so. When you
  quit, the last edit you made there is written to the archive's copy
  before greycard closes, waiting a few seconds at most for the
  archive to answer. If the archive is away too, that edit has nowhere
  to go and is lost.

If a frame has two copies on the archive and greycard can't tell which
is its own, the note adds "can't tell which copy is its own". Back up
that frame's folder once, so greycard knows where it goes.

## Remove rejects from an archive

A shoot is often backed up before it is culled, so the archive keeps
frames you later rejected. **Remove rejects from Archive...** finds
the copies of the open folder's rejects on the archive.

It is in the grid's header under the **Rejects** menu, and in the
**CULLING** section. It works on one folder at a time: pick one in
the tree, with **With subfolders** off.

The sheet lists each reject beside its copy on the archive, then
offers:

- **Move to rejects on Archive**: moves the copies into a `rejects`
  folder beside them on the archive. This is the default, and
  **Enter** presses it. Nothing is deleted.
- **Delete from Archive**: deletes the copies, to the trash where the
  archive has one. It is red and only a click presses it. If the trash
  has failed there before, **Delete from Archive permanently** is
  offered beside it, and can't be undone.

Copies greycard can't be sure of are listed as unverified and left
alone.

If the archive is offline, the sheet works from what greycard last
saw. A move is queued and runs when the archive answers again. A
delete is never queued: ask again when the archive is back.

To clear an archive's own `rejects` folders, open the archive's view
and use **Delete rejects folder...** there.

## Going back to an older version

Once greycard 0.4 has opened your library, an older version refuses
to open the library and leaves it as it is. Your raws and sidecars
are not affected. To go back to the older version, close greycard and
rename `library.sqlite`: it builds a new index from your folders. A
sidecar that 0.13 has saved with an edit, or exported from, can't be
read by 0.12 or older, which leave it alone. One holding only a
rating, a flag, a label, or a turn they read, and save again in their
own form when you change it.
