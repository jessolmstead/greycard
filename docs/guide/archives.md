# Archives and backup

An **archive** is a library root that holds the long-term copy of your
shoots, such as a NAS or a large external drive. Once a root is marked
as an archive, greycard can copy a shoot to it, bring a shoot back
from it, keep the two copies' edits in step, and clear your rejects
off it.

Use this when you shoot to a laptop and keep everything on a bigger
drive. If all your work lives in one place, you don't need it. Roots
themselves are covered in [Folders and the library](library.md).

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
- If the copy there has an older edit, only the sidecar is updated.
  If both copies were edited, the edits are joined (see below).
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

## One frame in two places

A frame on your laptop and its copy on the archive are treated as one
frame.

- **All roots** lists it once, from the local copy. If the local copy
  is gone or its drive is offline, it is listed from the archive.
- Every time you save an edit, rating or flag, greycard also writes it
  to the archive's copy, in the background.
- If the archive is offline, the status line counts what is waiting,
  such as "3 edits waiting for Archive". They are written when it
  answers again. Click the note to try now.
- If you edited both copies separately, on two computers for example,
  greycard joins them instead of picking one. The more recent edit is
  the one you see, and both histories are kept, so the other is a
  click away in History. The join is a step called "Reconciled with
  the copy on" and the other computer's name. Ratings, flags and
  labels each keep their most recent change.
- If a frame's drive goes offline while it is open, greycard can carry
  on from the archive's copy, and the status line says so.

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
are not affected.
