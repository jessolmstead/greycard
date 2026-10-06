# Culling and rating

Culling is the first pass over a shoot: rate the frames, pick the
keepers, reject the rest, and clear the rejects out of the way. This
chapter covers ratings, flags and color labels, culling mode, the
rejects folder, and deleting frames.

## Ratings, flags and labels

Every frame can carry:

- **a rating**, 0 to 5 stars
- **a flag**: pick, reject, or none
- **a color label**: red, yellow, green, blue or purple

Set them with the keys, from the frame menu, or in culling mode. The
keys work in the loupe, the grid and culling alike.

| Key | Does |
|---|---|
| 1 to 5 | Set the rating |
| 0 | Clear the rating |
| P | Pick |
| X | Reject |
| U | Unflag |
| 6, 7, 8, 9 | Red, yellow, green or blue label; press again to clear it |

Purple has no key; set it from the frame menu.

**Right-click** a frame in the filmstrip, the grid or the picture for
the frame menu. **Rating** and **Label** are submenus, and **Pick**,
**Reject** and **Unflag** are on the menu itself. A check mark shows
what the frames already have.

### On several frames

The keys and the menu act on every selected frame (Ctrl+click or
Shift+click to select more than one). A label key on a mixed selection
sets the label on all of them; press it again to clear it from all.

### How they show

Small badges over each thumbnail show the flag, the stars and the
label. A labeled frame's tile in the grid is also tinted toward its
label's color, so a run of red frames stands out. In the filmstrip,
which sits beside the picture you are judging, the label shows as a
thin colored line under the thumbnail instead of a tint.

Ratings, flags and labels are saved in the frame's `.gcd`. With
**Write XMP sidecars** on in Settings, ratings and labels go into the
XMP too, for Lightroom or darktable; picks and rejects stay in the
`.gcd`.

## Culling mode

Press **C**, or click **Cull** in the grid's header, to cull. Culling
shows each frame's own embedded camera JPEG instead of developing the
raw, so moving from frame to frame is instant. The right panel shows
the **CULLING** section in place of the develop tools.

- **← →** move between frames.
- **Space** zooms to 1:1 to check focus, and back.
- **V** cycles through comparing 1, 2 or 4 frames side by side. The
  **Compare** buttons (**1**, **2**, **4**) do the same. Click a frame
  in the compare view to choose it.
- **Ctrl+Z** and **Ctrl+Shift+Z** undo and redo your rating, flag and
  label changes from this session. They don't touch any edit.
- **Enter** or **Esc**, or the **Develop** button, leave culling and
  develop the frame you are on. **C** leaves too.

A word over the picture confirms each key, such as the new rating.

The status line says what you are looking at: "the camera JPEG", or
"a small camera preview" when the camera stored only a small one.
For a frame on a drive that is offline or on the network, culling
shows "the local preview" greycard keeps of it, so you can cull frames
that aren't reachable right now, or are slow to read.

### The CULLING section

- **Turn** with its **[** and **]** buttons turns the selected frames a
  quarter left or right, for a shot the camera recorded the wrong way
  up. The **[** and **]** keys do the same, in culling, the grid and
  the loupe. In culling nothing is developed.
- **Show** filters the folder by rating, flag, label or words, the
  same filter as the grid's header. See [the library](library.md).
- **Move on after a rating, flag or label** steps to the next frame
  after each key, so rating a shoot is one key a frame. It is off
  until you turn it on, and acts only when one frame is selected.
- The rejects buttons and **Delete selection...**, described below.

## The rejects folder

Rejecting a frame only flags it. To get rejects out of the way, move
them into a `rejects` folder inside their shoot's folder. Nothing is
deleted.

The same actions are in two places:

- the **CULLING** section, in culling mode
- the **Rejects** menu in the grid's header. The button shows the
  count, such as "12 rejects", or just "Rejects" when there are none.

### Move rejects

**Move 12 rejects...** moves every frame flagged reject in what you are
viewing, whatever is selected. It asks first: "Move the rejects?",
with the count and where they go. **Move** (or Enter) moves them;
**Cancel** (or Esc) doesn't.

- Each frame takes its `.gcd` and its XMP with it.
- In a view of several folders (all roots, a root, or a folder with
  **With subfolders** on), each reject goes into the `rejects` folder
  of its own folder.
- A frame on a drive that is offline is left alone.
- A file is never written over. If the `rejects` folder already has a
  file of that name, the frame stays where it is, and the status line
  says so.

With no rejects the item reads **No rejects to move** and is greyed out.

### Move back

Open the `rejects` folder, select the frames you want back, and use
**Move back to** *folder* (or **Move 3 frames back to** *folder*). It
is on the frame menu, in the **CULLING** section and in the
**Rejects** menu, whenever part of the selection is in a `rejects`
folder. The frames go back into the folder above with their sidecars,
and their reject flag is taken off so the next Move rejects won't send
them out again. There is no sheet: nothing is written over.

### Delete rejects folder

**Delete rejects folder...** deletes the frames in the rejects folder,
and the folder itself once it is empty. It always asks first (see
[Deleting frames](#deleting-frames)).

What it covers depends on the view:

- **One folder:** that folder's `rejects` folder.
- **All roots, a root, or a folder with With subfolders on:** every
  `rejects` folder that view lists frames in. The sheet says how many
  frames and folders, such as "Delete 30 frames from 3 rejects
  folders?".

Frames on an archive are left out unless you are viewing that archive
itself, and the sheet says how many it left alone. Delete those from
the archive's own view.

### Remove rejects from an archive

When you have an archive, the menu also offers **Remove rejects from**
*archive*... for each one. It finds the archive's copies of this
folder's rejects and moves them into a rejects folder on the archive,
or deletes them. It works on one folder at a time. See
[Archives](archives.md).

## Deleting frames

**Delete selection...** in the **CULLING** section, or the **Delete**
key in the grid, culling or the loupe, deletes the selected frames from
disk. On a Mac, the key marked delete (⌫) does the same.

Both deletes ask first. The sheet names how many frames and sidecars,
and from which folder. If you have an archive, it also says whether
the frames have a copy there.

- A frame's `.gcd` and its XMP go with it. Nothing else in the folder
  is touched. An XMP a raw shares with its JPEG (`IMG.xmp` beside
  `IMG.CR3` and `IMG.JPG`) is kept.
- **Move to the trash** is the default, and Enter presses it. You can
  restore the frames from the system trash.
- The delete runs in the background, and the status line says when it
  is done. A frame on another drive may be copied into your home
  trash, which takes a while for a large set.

If the trash refuses a frame, the delete stops there. The frames
before it are in the trash; that frame and the ones after it stay
where they were, and the status line says which files went. The next
time, the sheet also offers **Delete permanently**, which can't be
undone. Enter never presses that one. Where there is no trash at all,
the sheet says "No trash here: this is permanent."

## Mouse buttons

In the grid, a mouse's back and forward side buttons step to the
previous and next frame, as ← and → do.
