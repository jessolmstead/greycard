# Folders and the library

This chapter covers finding your frames: opening a folder, the three
ways of looking at it, the library of folders greycard keeps track of,
the filter, and where your edits are saved.

A folder you open with **Open folder...** is enough to start. The
library is for when your shoots live in a few big folders, on more
than one drive or on a NAS, and you want to browse and filter all of
them in one place.

## Opening a folder

Press **Ctrl+O**, or click **Open folder...** at the top of the left
pane, and choose a folder of raws.

Under the button, the pane names the open folder, with "in" and the
root's name after it when the folder is in your library. Rest the
pointer on the name to see the full path.

Click the name for **Recently opened**: the last ten folders you
opened, the open one checked. A folder that is no longer on the disk
stays in the list. Choosing it says so, and if its drive is
unplugged, "isn't reachable; is its drive connected?".

## Filmstrip, grid and loupe

- The **loupe** shows one frame large, for editing. The **filmstrip**
  along the bottom holds the rest of the folder.
- The **grid** fills the window with thumbnails, for browsing and
  sorting a whole shoot.

Press **G** to switch between them. In the grid, **Loupe** in the
header, a double-click on a frame, **Enter** or **Esc** also go back
to the loupe. (With several frames selected, the first **Esc**
clears the selection.)

In the grid:

- The arrow keys move the selection, and **Home** and **End** go to
  the first and last frame. A mouse's back and forward buttons step
  through the frames too.
- The scroll bar down the right edge, and the one under the
  filmstrip, show where you are in the folder; drag either, or click
  its track to page.
- **Ctrl+wheel**, or **+** and **−**, change the cell size. There are
  six sizes, from 96 to 512 px; the header shows the current one, and
  greycard remembers it.
- A color label tints the whole cell, in the grid and the filmstrip.
- A frame greycard can't read says **Unreadable**.

**F7** puts the left pane away, **F6** the filmstrip and **F8** the
right panel. **Tab** hides all of them, or over the grid, the left
pane alone.

## Roots: your library

A **root** is a folder you add to the library, such as your Pictures
folder or a share on a NAS. greycard keeps an index of every frame
under its roots, so it can list and filter them without reading each
file again.

The roots are in the left pane while the grid is up, under **ROOTS**:

- **All roots** lists every frame in the library, with the total.
- Below it is a row for each root with its count. Click a row to see
  every frame under that root.

To add a root, click **+ Add a root...** at the foot of the section
and choose a folder. When the open folder is in no root,
**+ Add this folder** adds it in one click.

Right-click a root's row (Control+click on a Mac) for its menu. The
first line is the root's full path.

- **Rename...** gives it a shorter name in greycard. The folder on
  disk is not renamed. Leave the name empty to go back to the
  folder's own.
- **Use as an archive** marks it as a backup copy of your shoots. See
  [Archives and backup](archives.md).
- **Remove from library** forgets the root. Its files stay where they
  are.

## The folder tree

When you are looking at a root, or a folder inside one, the left pane
shows its folders under **FOLDERS**, in the grid and the loupe alike.

- Click a folder to open its frames. Click its triangle to fold or
  unfold it without opening it.
- The count beside a folder is its own frames when it is unfolded, and
  everything under it when it is folded.
- **With subfolders** off (the default) opens a folder's own frames
  only. Turn it on to open a folder with everything under it. greycard
  remembers the setting.

A folder with no frames of its own opens empty, and the status line
suggests **With subfolders**.

### Subfolder tiles

In the grid, the folders inside the open folder appear as tiles above
the thumbnails, each with the number of frames under it. Click a tile
to open that folder. Right-click it for **Reveal in file manager**.
With **With subfolders** on, there are no tiles, since everything
under the folder is already listed.

A shoot's `rejects` folder shows as a tile and in the tree like any
other folder, so the frames you moved out while culling are one click
away. See [Culling](culling.md).

## Network and offline roots

### A root on a network share

A local root is watched, so a frame added or changed there shows up
right away. A share (NFS, SMB and the like) can't be watched, so
greycard looks it over every 10 minutes instead. Change that under
**NETWORK FOLDERS**, **Rescan every**, in **Settings...** (0 for
never).

If a share stops answering, greycard sets it aside after about three
seconds and carries on with your other roots. The status line says,
for a root named Archive, "Archive isn't reachable; showing its last
indexed frames".

### The loading bar

A view that takes a moment to read shows a bar with what it is doing,
such as "Reading sidecars… 3,400 of 11,711". When it is held up by a
slow share, it says "Waiting on" and names the root. You can keep
working while it reads.

### An offline root

When a root's drive is unplugged or its share is away, its row is
dimmed and says "offline" in place of the count. You can still:

- open it and browse its folder tree, its grid and its thumbnails
- filter it and read the ratings, flags and labels it had
- cull it in the loupe and the compare view, from the local previews
  made while it was there (below)

Developing or exporting a frame needs the raw itself, so wait until
the drive is back.

## The filter

The filter narrows what the grid and the filmstrip show. It sits in
the grid's header, and in the **CULLING** section while you cull.
Press **Ctrl+F** or **/** to type in it.

### Chips

- **Stars**: **Any**, **1+** up to **5**. Click **or more** to switch
  to **exactly**, which shows only that rating.
- **Flag**: **Pick**, **Reject** and **Unflagged**.
- **Label**: the five colors and **No label**.

Each chip shows how many frames it would leave. Chips in one group
add up (Red or Green); different groups narrow each other (Red and
3+). A chip that would leave nothing is dimmed.

Under them are chips read from the cameras' data: **Camera**,
**Lens**, **Style** (the camera's picture style), **ISO**, **Focal**,
**Day** and **Keyword**. They fill in once greycard has indexed the
folder.

The count at the end of the row says how many frames are shown, for
example "120 of 640". **Clear** shows the whole folder again.

### Typing

Words in the text field find frames whose file name or keywords
contain them. Several words must all match.

You can also type tests, with no spaces inside them:
`camera:R6 iso>=3200 rating>=3`.

| Term | Matches |
|---|---|
| `camera:`, `make:`, `model:`, `lens:` | Part of the body or lens name, such as `lens:RF` |
| `name:`, `folder:` | Part of the file name, or of the folder's path |
| `keyword:` | Part of a keyword |
| `iso`, `focal`, `aperture`, `shutter` | A number: `iso>=3200`, `focal<=35`, `aperture:1.8`, `shutter<1/60` |
| `rating` | 0 to 5 stars: `rating>=3` |
| `date` | A year, month or day: `date:2026-09`, `date<=2026-09-21` |
| `flag` | `pick`, `reject` or `none` |
| `label` | `red`, `yellow`, `green`, `blue`, `purple` or `none` |

The tests are:

- `:` means "contains" for words and "equals" for numbers.
- `=` is the exact value, `!=` is anything else.
- `<`, `<=`, `>`, `>=` compare numbers and dates.

Put a value with a space in double quotes: `lens:"RF 24-105"`.
`!=` leaves out frames that don't record the field at all.

A test greycard can't read, such as `rating>=9`, is searched for as a
plain word, and the status line says what was wrong with it.

**Esc** in the field clears the text and gives the keys back to the
frames. greycard remembers the filter when you quit and restores it
next time.

## Where your edits live

Your edits are saved beside each raw in a `.gcd` file, so
`IMG_0001.CR3` keeps its edit in `IMG_0001.CR3.gcd`. Ratings, flags,
labels and keywords are in it too. To move a shoot to another folder
or machine, copy the `.gcd` files along with the raws. Delete a
`.gcd` file to start that frame's edit over. greycard never changes
a raw.

### A hidden folder instead

If you would rather not see a `.gcd` beside every raw, open
**Settings...** at the bottom of the left pane (or press **Ctrl+,**)
and set **Edits go** to **Hidden folder**. Edits then go into a hidden
`.greycard` folder inside the shoot's folder, which travels with the
shoot the same way.

greycard reads a sidecar in either place. Changing the setting moves
nothing by itself: each frame's sidecar moves the next time it is
saved. To tidy the open frames at once, use **Move the open frames'
sidecars** in the same sheet. It says how many are in the other place
and moves them when you confirm.

### XMP for other programs

To share ratings, labels and keywords with Lightroom or darktable,
turn on **Write XMP sidecars** in Settings. greycard then also writes
a standard `.xmp` beside each raw. An `.xmp` that is already there is
read either way. Picks and rejects have no XMP field, so they stay in
the `.gcd`.

### A raw moved by hand

Under a library root, a raw you move or rename in your file manager
keeps its edit, rating and flag: greycard recognizes the file and
moves its `.gcd` along with it. An `.xmp` is not moved; move it
yourself.

## Thumbnails and local previews

greycard keeps the thumbnails it makes on disk, so a folder you open
again, move or rename fills in at once. Frames under a root also get
a **local preview**, a picture 2048 px on its long side, made in the
background while the drive is there. It lets you cull
an offline drive, and a shoot on a NAS without reading each raw over
the network.

Both have a size limit under **THUMBNAILS** in Settings:

| Setting | Default | Notes |
|---|---|---|
| **Keep up to** | 300 MB | About thirty thousand thumbnails at one size. 0 turns the cache off. |
| **Previews up to** | 8192 MB | About half a megabyte a frame. 0 keeps none. |

When the cache is full, the ones used longest ago go first. **Clear**
empties it.

The cache is under `~/.cache/greycard/thumbs` on Linux,
`~/Library/Caches` on a Mac and `%LOCALAPPDATA%` on Windows.
