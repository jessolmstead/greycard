# Culling and rating

Culling is the first pass over a shoot: rate the frames, pick the
keepers, reject the rest, and clear the rejects out of the way. This
chapter covers ratings, flags and color labels, culling mode, the
lightbox, the rejects folder, and deleting frames.

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

A rating, flag and label belong to the file, so a frame and its
[versions](presets-and-history.md#versions-of-a-frame) share them: set
one on a version and every cell of that file follows.

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

## Keywords and a title

The **METADATA** section holds the frame's **Title** and its
**Keywords**. It is under the CULLING section in culling mode, the
last on the Develop tab, and in the left pane beside the grid,
under the import buttons.

Type a title and press **Enter** or **Tab**, or click a frame, to keep it;
**Esc** puts back what was there. In the Keywords field a comma or
**Enter** ends a word, which goes on at once and shows as a chip under
the field's label; the **x** on a chip takes it off. As you type,
words already in your library that start with the same letters are
offered under the field: **Up** and **Down** move through them, and
**Tab**, **Enter** or a click takes one. **Esc** leaves the field, and
the keys are rating keys again.

Both act on every selected frame, as a rating does. With several
selected, a keyword only some of them carry is dimmed (click its word
to put it on the rest), and titles that differ show as **Mixed**; a
title typed there replaces all of them. In culling mode **Ctrl+Z**
takes back a keyword or a title like a rating.

Keywords and the title are saved in the `.gcd`, and with **Write XMP
sidecars** on they go into the XMP as well. The filter's **Keyword**
chips and its `keyword:` term find the words you add.

## Culling mode

Press **C**, or click **Cull** in the grid's header, to cull. Culling
shows each frame's own embedded camera JPEG instead of developing the
raw, so moving from frame to frame is instant. A frame you have
edited shows its edit instead, as it does in the grid. The right
panel shows the **CULLING** section in place of the develop tools.

Under the file name at the top of the panel, culling shows the body
and lens, then the focal length, aperture, shutter and ISO, as
editing does. In compare, they are the chosen frame's.
They fill in once greycard has indexed the folder.

- **← →** move between frames.
- **Space** zooms to 1:1 to check focus, and back. At 1:1 you always
  see the camera's full JPEG, even on an edited frame.
- **V** cycles through comparing 1, 2 or 4 frames side by side. The
  **Compare** buttons (**1**, **2**, **4**) do the same. Click a frame
  in the compare view to choose it.
- **Ctrl+Z** and **Ctrl+Shift+Z** undo and redo your rating, flag and
  label changes from this session. They don't touch any edit.
- **Enter** or **Esc**, or the **Develop** button, leave culling and
  develop the frame you are on. **C** leaves too.

A word over the picture confirms each key, such as the new rating.
When the key makes the frame fall outside the filter, the word says
so too ("No stars; hidden by the filter").

### Comparing versions

To choose between a frame's
[versions](presets-and-history.md#versions-of-a-frame), right-click it
or one of its versions and choose **Compare versions**. Culling opens
on the original and its versions alone, side by side, each tile its
own edit: two up for one version, three up for two, four up for three
or more. With more than three versions, the first three are shown and
the status line says how many were left out. Each tile's name reads
like "IMG_0001 (Warm)", and the status line names the chosen version.

Press **Delete** on a version's tile to delete that version, after a
confirmation; the view closes up on the rest. To keep a version as
the frame's own edit, use **Swap with the original** on it. An arrow
or a click past the file's cells, or **V**, ends the comparison.

The status line says what you are looking at: "the camera JPEG",
"a small camera preview" when the camera stored only a small one, or
"the edit" for an edited frame.
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

## The lightbox

The lightbox is for judging a set as a whole: whether a run of frames
holds together in color and tone, before you send it out. It shows
each frame large and as edited, on a dark ground, with nothing else
drawn: no names, no badges, no marks, no header. A
[stack](#stacks)'s badge is the one exception, so a closed stack is not
taken for a single frame.

Press **L** in the grid, the loupe or culling, or click **Lightbox**
in the grid's header. The lightbox lays out whatever the grid shows,
as the filter leaves it and in the grid's
[order](library.md#sort-order): a folder, a root, a
[collection](library.md#collections), or only your picks (the
**Pick** chip, then **L**).

- **+** and **−**, or **Ctrl+wheel**, set how many frames go across,
  from two to six. It starts at four, and greycard keeps your choice
  for the next session.
- The arrow keys, **Home** and **End** move through the frames. A ring
  marks the frame you are on, and a thinner one the rest of the
  selection.
- **Return**, or a double-click, opens the frame in the loupe.
  **Return**, **Esc** or **L** there bring you back to the lightbox
  where you were.
- The rating, flag and label keys work as everywhere.
- **Ctrl+Shift+S** syncs the frame you are on onto the rest of the
  selection: choose the frame that is right, Ctrl+click the ones that
  stand out, and sync. The frame menu has the same as **Sync settings
  of** *frame* **onto 3 other frames...**.
- In a collection in its hand order, a drag still reorders it.
- **Tab** shows or hides the left pane.
- **Esc**, **G** or **L** leave for the grid, as does **Leave
  lightbox** on the frame menu. With several frames selected, the
  first **Esc** goes back to one.

A frame you have edited shows its edit; one you haven't shows the
camera's JPEG. While a new picture of an edit is being made, after a
sync for example, the tile keeps its last picture dimmed. A tile with
no picture yet says "Developing".

## Stacks

A stack puts several frames of one folder behind one: a burst, a
bracket, five tries at one expression. The grid, the filmstrip,
culling, the lightbox and the filter then see one frame, the stack's
top, until you open it.

- **Make one:** select the frames (Ctrl+click, Shift+click) and press
  **Ctrl+G**, or choose **Stack 3 frames** on the frame menu. The frame
  you are on goes on top. The frames must be in one folder, and a
  collection's view makes none.
- **The badge:** a closed stack's top carries a layers mark and how
  many frames it holds, at its picture's bottom right. Click it, or
  press **S** in the grid, to open the stack: its frames follow the top,
  marked "2/5" and so on. Click or **S** again to close it. In culling
  and the loupe **S** is the soft proof, so open a stack there by its
  badge on the filmstrip.
- **Set as top of stack** on the frame menu, or **Shift+S** in the
  grid, puts a frame on top. **Remove from stack** takes the selected
  frames out; a stack left with one frame is no stack. **Unstack**, or
  **Ctrl+Shift+G**, undoes it.
- **Compare stack** on the frame menu opens culling on the stack's
  frames side by side, two, three or four up.
- What you do to a closed stack, a rating, a flag, an export, a
  delete, Add to collection, is done to its top alone. A closed stack
  passes the filter when its top does, and the filter's counts leave
  out the frames behind it.
- **Move rejects** moves every rejected frame of the folder, including
  rejected frames behind a closed top. A frame moved out, or deleted,
  leaves its stack; Move back brings it back as a frame of its own.
- Stacks are kept with your collections, on this machine. A stack
  whose frame you rename in its folder follows it; a frame moved to
  another folder leaves its stack.

`greycard library list` shows a closed stack's top alone, marked
"stack of 3"; `--stacks expanded` lists every frame.

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

Deleting a frame takes its versions with it. To delete one version
and keep the frame, see
[Versions of a frame](presets-and-history.md#versions-of-a-frame).

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
