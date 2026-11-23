# Folders and the library

This chapter covers finding your frames: opening a folder, the ways
of looking at it, the library of folders greycard keeps track of, the
filter, collections, and where your edits are saved.

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

greycard runs as one window. A file or folder you open from your file
manager, by double-click or Open With, while greycard is running opens
in that window, which comes to the front; starting greycard again with
nothing to open brings the window up too. On Wayland the compositor
decides whether the window comes forward or is only marked. A
`greycard export`, and any run of the editor with flags, works on its
own and leaves the window alone.

## Filmstrip, grid and loupe

- The **loupe** shows one frame large, for editing. The **filmstrip**
  along the bottom holds the rest of the folder.
- The **grid** fills the window with thumbnails, for browsing and
  sorting a whole shoot.
- The **lightbox** lays the frames out large and as edited on the
  canvas, for judging a set side by side. Press **L**. See
  [the lightbox](culling.md#the-lightbox).

**Library** and **Loupe** at the top of the left pane switch
between the grid and the loupe, and the one you are in is lit. In the
grid, a double-click on a frame, **E**, **Enter**, or **Esc** also go
to the loupe. (With several frames
selected, the first **Esc** clears the selection.)

The loupe has two modes, **Cull** and **Develop**, first in the right
panel's tab row (Cull, then Develop's tabs): Cull for rating a shoot
fast, Develop for editing (see
[Culling mode](culling.md#culling-mode)). The loupe comes back in the
mode you left it in, and greycard remembers it for the next session.
The grid has no mode of its own: the rating, flag and label keys, and
**Move on**, work there whichever mode the loupe is in.

**G** goes from the loupe to the grid, and from the grid back to the
loupe in the mode you left it in; **E** goes from the grid to the loupe
the same way. **D** goes straight to Develop, from the grid or the
loupe. **C** goes from the grid to the loupe in Cull; in the loupe it
switches between Cull and Develop.

In the grid:

- The arrow keys move the selection, and **Home** and **End** go to
  the first and last frame. **Alt+←** and **Alt+→**, or a mouse's
  back and forward buttons, go back to the folder you came from and
  forward again, as a browser's do, each on the frame you left it on.
- A **middle click** on a frame syncs its edit onto the other selected
  frames, here and in the filmstrip (see
  [Sync settings](presets-and-history.md#sync-settings)).
- The scroll bar down the right edge, and the one under the
  filmstrip, show where you are in the folder; drag either, or click
  its track to page.
- The **Tile size** slider in the header, beside the size in pixels,
  sets the cell size anywhere from 96 to 512 px. **Ctrl+wheel**, or
  **+** and **−**, step it through six sizes, and the slider follows.
  On a narrow window the largest size is held to two frames across.
  greycard remembers the size. To type one, or to stop the wheel
  resizing, see [Settings](settings.md#cell-size).
- A color label tints the whole cell in the grid. In the filmstrip it
  shows as a thin colored line under the thumbnail.
- A frame greycard can't read says **Unreadable**.
- A version of a frame has a cell of its own right after the frame's,
  with its name on a tag at the top left. See
  [Versions of a frame](presets-and-history.md#versions-of-a-frame).

Drag the left pane's right edge or the right panel's left edge to
set its width, and the filmstrip's top edge to set the size of its
pictures; double-click an edge to put it back. The sizes are kept
between sessions. **F7** puts the left pane away, **F6** the
filmstrip, and **F8** the right panel. **Tab** hides all of them
and brings back the ones it hid, or all three when it hid none; over
the grid it is the left pane alone. **F11** takes the window full
screen and back, in any view; on a Mac the window's green button
does.

### Thumbnails show the edit

An edited frame shows its edit in the filmstrip, the grid, the cull
view and compare, not the camera's JPEG. Reset a frame's edit and the
camera's picture comes back. To reset several at once, select them,
right-click one and choose **Reset to Original**: each edited frame
goes back to its Original in one step, which its history can undo.

Some frames are edited without being opened: a sync, a preset or a
paste over a selection, an edit joined from another machine through an
archive, or a folder opened with its edits already on disk. Their
pictures are made in the background, for the frames on screen and
near them, and until then they show the camera's. Scroll and the
frames you scroll to are made next. greycard holds this work back
while you edit the frame on screen, so your own develop comes first.
While pictures are being made, the status line says how many are
left.

### Edited and exported marks

Small marks sit in the top right corner of a thumbnail, in the grid
and the filmstrip:

- a **pencil** on a frame whose edit is not the default;
- a **picture with a down arrow** on a frame that has been exported;
- a **bookmark** on a frame that is in a collection (see
  [Collections](#collections));
- **+JPEG** on a raw with the camera's JPEG beside it (below).

A frame shows every mark it has. The exported mark stays after you
undo past the export, or the export's row leaves the history. A
version wears its own pencil and exported mark, and its own bookmark.

### A raw and its camera JPEG

A camera set to RAW+JPEG writes two files for each press, such as
`IMG_0001.CR3` and `IMG_0001.JPG`. greycard lists them as one frame,
the raw, marked **+JPEG**; under the frame's name the panel says "With
the camera's JPEG, IMG_0001.JPG". The pair has one rating, flag,
label and set of keywords, kept in the raw's `.gcd`. Moving rejects,
Move back and deleting take the JPEG with its raw, and the delete
sheet counts the JPEGs. An export writes the raw's develop.

Culling shows the camera's JPEG when it is at least as large as the
preview inside the raw, and the status line says "the camera's JPEG
beside the raw". A JPEG with no raw of its name beside it is a frame
of its own. An Olympus or OM System High Res Shot saves its merge as
`.ORF` and one single frame as `.ORI`: both are frames, and the
camera's JPEG goes with the `.ORF`.

To list the JPEGs as frames of their own, with their own ratings, turn
on **Treat a JPEG beside its raw as a separate frame** under
**RAW+JPEG** in [Settings](settings.md#rawjpeg).

## Roots: your library

A **root** is a folder you add to the library, such as your Pictures
folder or a share on a NAS. greycard keeps an index of every frame
under its roots, so it can list and filter them without reading each
file again.

The roots are in the left pane while the grid is up, under **ROOTS**:

- **All roots** lists every frame in the library, with the total.
- Below it is a row for each root, with the count of every frame
  under it. Click a row to open the root as a click on it in the
  folder tree does: its own frames with **With subfolders** off,
  everything under it with the switch on.

**EXPORT QUEUE**, a header of its own under **COLLECTIONS**, counts
the frames waiting to be exported; click it to see them (see
[The queue in the grid](export.md#the-queue-in-the-grid)).

To add a root, click **+ Add a root...** at the foot of the section
and choose a folder. When the open folder is in no root,
**+ Add this folder** adds it in one click. The Import sheet's **Add
in place** does the same and opens the folder (see
[Adding a folder in place](import.md#adding-a-folder-in-place)).

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
Each view remembers whether you folded the section, so it can stay
open in the grid and folded in the loupe.

- Click a folder to open its frames. Click its triangle to fold or
  unfold it without opening it.
- The count beside a folder is its own frames when it is unfolded, and
  everything under it when it is folded.
- **With subfolders** off (the default) opens a folder's own frames
  only. Turn it on to open a folder with everything under it. The
  frame on screen stays open when it is still in the list. greycard
  remembers the setting.

A folder or root with no frames of its own opens empty, and the
status line suggests **With subfolders**.

The left pane's lists, Folders, Collections, Presets, Snapshots, and
History, share the pane's height. A list that has little to show
gives its room to the others, and folding one does the same. Drag the
gap between two open lists to move room from one to the other; the
split is remembered. Double-click the gap to even the two.

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

After you update to 0.7, the first look over each folder reads every
raw in it once more, to learn what kind of sensor made it. That takes
a millisecond or two a file on your own drives, and a while of quiet
reading in the background on a share. It happens once.

### The loading bar

A view that takes a moment to read shows a bar with what it is doing,
such as "Reading sidecars… 3,400 of 11,711". When it is held up by a
slow share, it says "Waiting on" and names the root. You can keep
working while it reads.

### An offline root

When a root's drive is unplugged or its share is away, its row is
dimmed and says "offline" in place of the count. You can still:

- open it and browse its folder tree, its grid and its thumbnails,
  an edited frame showing the last picture of its edit greycard made
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
**Lens**, **Style** (the camera's picture style), **Day**, and
**Keyword**. They fill in once greycard has indexed the folder.
Beside them is **Kind**, from the file's name: **Raw**, **JPEG**,
**PNG** or **TIFF**, shown when the frames shown hold more than one. Keywords are added in the **METADATA** section (see
[Keywords and a title](culling.md#keywords-and-a-title)) or brought
in with a Lightroom catalog or an XMP.

When the chips do not fit the row, it scrolls sideways: turn the
mouse wheel over it, drag it, or hold Shift and turn the wheel.

The count at the end of the row says how many frames are shown, for
example "120 of 640". **Clear** shows the whole folder again.
**Save...**, beside it, keeps the filter as a smart collection (see
[Collections](#collections)).

When a rating, flag or label key makes the frame fall outside the
filter, it leaves the view and the status line says why, such as
"IMG_0001.CR3 rated 0 and hidden by the filter (1 star or more);
Clear the filter to see it".

### ISO and focal length

**ISO** and **Focal** are sliders with two ends. In the **CULLING**
section they sit under the stars, flags, and labels; in the grid's
header, under its top row. In both they are above the **Camera**,
**Lens**, and **Day** rows. Each runs over the ISOs or focal lengths
the folder's frames were shot at, every value one step apart however
far apart the numbers are, with the low end's value on the left and
the high end's on the right.

- Drag an end to narrow the range. The filter changes when you let go.
- Double-click a slider to take in its whole range again.
- From the filter's text field, press **Shift+Tab** in the CULLING
  section, or **Tab** in the grid's header, to reach an end. The arrow
  keys then step it one value at a time; the filter changes when you
  let go of the key.

A range set writes its terms into the text field (`iso>=400`,
`focal<=70`), and terms you type there move the ends. A frame with no
ISO or focal length in its data, such as a scan, is shown only while
that slider covers its whole range. A slider is there once the folder
has two values or more.

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
| `mm` | The focal length, as `focal`: `mm<=35` |
| `rating` | 0 to 5 stars: `rating>=3` |
| `focus` | The [focus score](#sort-order), 0 to 1: `focus>0.6`; a frame on a network drive has one only if it is a CR3 or a RAF |
| `date` | A year, month or day: `date:2026-09`, `date<=2026-09-21` |
| `flag` | `pick`, `reject` or `none` |
| `label` | `red`, `yellow`, `green`, `blue`, `purple` or `none` |
| `kind` | `raw`, `jpeg`, `png` or `tiff`: `kind:raw`, `kind!=jpeg` |
| `has:versions` | Frames with at least one version |

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

## Sort order

**Sort**, the arrows at the end of the filter's row, sets the order of
the grid, the filmstrip, culling and the lightbox. The arrows point
the way the order runs, and the grid's header says what by beside the
frame count ("8 frames · by capture time"), as the CULLING section
does beside its count. Rest the pointer on the arrows for the whole
of it, as "Sort: Capture time, oldest first". The menu offers:

- **Capture time**, from each frame's own date in its EXIF, moved by
  its [shift](culling.md#shift-capture-time) when a camera's clock was
  off;
- **Name**, the file's name;
- **Rating**;
- **Date edited**, when the frame's edit was last saved;
- **Focus**, how sharp the sharpest part of the frame's embedded
  preview is, from 0 to 1, the sharpest first; the index scores each
  frame just after it lists it, and a frame not scored yet goes last;

and under them which way up: **Oldest first** or **Newest first**
(**A to Z** or **Z to A** for a name, **Lowest first** or **Highest
first** for a rating or focus).

Each kind of view keeps its own order, and greycard remembers them
when you quit:

- **A folder** is by name until you choose otherwise, which is the
  camera's numbering.
- **All roots, and a root or folder with its subfolders,** are by
  capture time, so the frames of two cameras from one day read in the
  order they were taken, wherever their folders are. A smart
  collection opens as one of these and uses the same order.
- **A collection** is in its own order (below), and its menu starts
  with **Hand order** to come back to it.

Frames that tie, such as a burst taken within one second, follow their
names. A frame with no capture time goes last, either way up, and so
does a frame with no edit when you sort by date edited. Date edited
is the last time the frame's sidecar was saved, so a rating given to
an edited frame counts too.

Changing the order keeps the frame you are on selected and scrolls to
it. A closed [stack](culling.md#stacks) sits where its top frame
falls, and an open one keeps its frames together behind the top. A camera's JPEG
beside its raw goes where its raw does, and a
[version](presets-and-history.md#versions-of-a-frame) right after its
frame.

The order is read when the list is: a frame you rate stays where it
is while you cull, and moves to its new place the next time the list
is read, when you choose the order again or open the view again. A
[capture time shift](culling.md#shift-capture-time) is the exception:
a list by capture time is read again as soon as the shift is saved,
since putting the frames in their right places is what it is for.

## Collections

A **collection** gathers frames from anywhere in your library, from
any root and any folder, into one list in an order you choose: a
portfolio, a set for a client, the frames for a print sale. The frames
stay where they are; a collection only points at them.

The collections are in the left pane while the grid is up, under
**COLLECTIONS**, below **ROOTS** and **FOLDERS** until you drag them
elsewhere. Each row shows the
collection's name and how many frames it holds. Click a row to open
it; the grid, the filmstrip and the lightbox then show its frames.

### Making one and filling it

- **+ New collection...** at the foot of the section makes an empty
  one.
- Right-click a frame (or a selection) and choose **Add to
  collection**, then a collection, or **New collection...** to make
  one with those frames in it.
- **B** adds the selected frames to the collection that is open, or
  else to the last one you added to or made this session. When every
  selected frame is in it already, **B** takes them out. With no
  collection yet, it asks for a name for a new one.
- While a collection is open, the frame menu has **Remove from**
  *collection*. The frames themselves are not touched.

A name with slashes, such as `Clients/2026/Cover`, puts the
collection in a set: the list sorts by set, and shows the set muted
before the name.

### Order

A collection opens in its own order, **Hand order** in the
[Sort](#sort-order) menu. Drag a frame, or the selection, in the grid
to move it; a bar marks where it will land.

Any other order from the Sort menu shows the frames that way instead,
without changing the order you made, and greycard remembers it for
that collection. While it is on, a drag does nothing. Choose **Hand
order** to go back.

Right-click the collection's row for its menu:

- **Rename...**
- **Capture order** is a quick way to the Sort menu's capture time,
  oldest first. It is checked whenever the collection is in an order
  you chose, and takes it back to hand order. While the collection is
  open, its row names the order it is in.
- **Delete...** deletes the collection after asking. Its frames stay
  where they are.

### Frames that move

A collection knows a frame by what is in the file, not only by where
it is. Move a shoot to another drive, rename it, or let another
program rewrite its metadata, and the frame stays in the collection.

A frame greycard can't find anywhere in the library is not shown, and
the status line counts it when the collection opens, such as
"Portfolio: 12 frames shown, 2 missing". It is kept, so it comes back
when its shoot does. To let such frames go, right-click the open
collection's row and choose **Forget 2 missing frames**. greycard
looks again before it forgets anything, and keeps a frame on a drive
that is offline or not answering, or one in a folder the library has
not finished reading. The item is not offered while the library is
being indexed.

### Versions in a collection

A [version of a frame](presets-and-history.md#versions-of-a-frame)
can be in a collection on its own: **B** or **Add to collection** on a
version's cell adds that version, and the bookmark shows on that cell
alone. The original and its versions can sit in one collection side
by side, each in its own place in the order.

A collection that holds a version can't be read by greycard 0.6 or
earlier, which leaves it alone rather than change it.

### Smart collections

A **smart collection** is a saved filter. Set up the grid's filter,
press **Save...** beside **Clear** and give it a name. It covers what
you were looking at: a root, a folder with the folders inside it, or
every root, and the sheet says which before you save.

A smart collection shows in the list with a sparkles mark and no
count. Click it to put its filter back on the bar and open the place
it covers. Its row stays lit while the bar shows its filter, and goes
dark when you change a chip. Its frames are whatever the filter finds
at the time, so it fills itself as you rate and label.

### Where collections are kept

Each collection is a small file in a `collections` folder beside the
library index (see [Settings](settings.md#where-greycard-keeps-its-files)).
A copy also goes into the shoot folders it touches, which is how it
reaches another computer through an archive
([Working on two computers](archives.md#working-on-two-computers)).

## Where your edits live

Your edits are saved beside each raw in a `.gcd` file, so
`IMG_0001.CR3` keeps its edit in `IMG_0001.CR3.gcd`. Ratings, flags,
labels and keywords are in it too. To move a shoot to another folder
or machine, copy the `.gcd` files along with the raws. Delete a
`.gcd` file to start that frame's edit over. greycard never changes
a raw.

A frame's versions live in the same `.gcd` (see
[Versions of a frame](presets-and-history.md#versions-of-a-frame)).

A newer greycard may write its `.gcd` files in a form an older one
can't read: 0.13's are refused by 0.12, as 0.12's were by 0.11. An
older version leaves such a frame alone rather than write over it:
it won't open it or change its rating, its status line says why, and
an export of it comes out unedited. From 0.13 on, a later version's file opens read-only
where it can be shown at all. Once a version has saved a frame, open it with that
version or a later one. A damaged `.gcd` is left alone the same way;
move it aside or delete it, then open the folder again, to start the
frame over.

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

### Lightroom edits in XMP sidecars

When a raw has a Lightroom XMP beside it (`IMG_0001.xmp`) with develop
settings in it and no edit of greycard's, the first time greycard opens
or indexes the frame it brings the edit across as one step in its
history, "Imported from Lightroom", just as a
[catalog import](import.md#a-lightroom-classic-catalog) does.
Lightroom's rating, label, keywords (with their full path), and title
fill whatever greycard has left empty; one you set here stays, unless
the XMP has changed since greycard last read it, when the XMP's wins
as it always has. It writes the frame's `.gcd` then, so it happens
once. A frame with an edit of greycard's is left alone, and an XMP
whose settings are where Lightroom starts a frame brings nothing.

Lightroom Classic writes XMPs only when told to, so one beside a raw
may be older than your catalog. Importing the catalog later replaces
an edit brought from an XMP when you have not edited the frame since:
the develop becomes the catalog's, and so does each rating, label,
keyword list, or title that came from the XMP or is empty. One you
set here stays. A frame you have edited is held back unless you force
the import.

### Profiles from RawTherapee

RawTherapee keeps each frame's edit in a `.pp3` beside it. When you
open a folder that holds them, a line under the folder's name says how
many frames have one ("RawTherapee profiles found beside 24 frames"),
counting the frames an import would change and would not hold back
(below). It is offered once a folder: **Not now** puts it away
for good, and so does importing any of the frames. Opening the sheet
with **Import...** and canceling it leaves the offer for the next time
you open the folder. Your answer is kept in greycard's settings, not
in the folder. A run started with `--no-sidecars` offers nothing.

A profile named after the raw with its extension replaced
(`IMG_0001.pp3`) belongs to the raw: the camera's JPEG beside it
doesn't take it.

ART's `.arp` profiles are found and read the same way, and imported as
"Imported from ART". ART's own tools, and the settings it keeps
differently (its black level, its saturation, and a white balance set
as multipliers), are listed as having no place here. When a frame has
both, the `.arp` is read and the report names the `.pp3` beside it.

**Import...** opens a sheet with what the import would do, a few lines
a frame: the profile and the RawTherapee or ART version that wrote it, what
comes across exactly (the stars, the color label, the trash mark as a
reject, the lens profile's switches, the angle and turns), what comes
across as a likeness (the exposure, the white balance, the crop, the
manual distortion), and what has no place here, by RawTherapee's own
name ("Local Contrast", "Exposure Curve"). The totals sit at the top.
A folder of more than 200 profiles lists the first 200 frames and says
how many more the totals count. The tone curves are never carried:
they do the job greycard's base curve does.

A frame you have developed here is left alone, and so is one whose
stars, label or flag set here the profile would change, so a cull you
made in greycard is never wiped by RawTherapee's unrated profiles.
When there are some, a switch appears, **Write over the N frames
developed or rated here already**. With it
on, the report says what a forced import would set, RawTherapee's
neutral settings included for a developed frame, and what it replaces
of your own culling ("5 stars to 0, the reject cleared"). **Write**
writes the sidecars, one history step named "Imported from
RawTherapee" a frame, which you can undo like any other. The frame on
screen picks up what was written at once. A slider you moved just
before pressing **Write** is saved first, so that frame counts as
developed here.

The import doesn't touch the hand-set chromatic aberration of a frame
unless the profile sets one: a lens profile's correction and the
amounts set by hand are separate.

The command line does the same: `greycard rawtherapee FOLDER` prints
the report, and `--write` writes it (`--force` for the frames
developed or rated here).

### A raw moved by hand

Under a library root, a raw you move or rename in your file manager
keeps its edit, rating and flag: greycard recognizes the file and
moves its `.gcd` along with it. An `.xmp` is not moved; move it
yourself.

## Thumbnails and local previews

greycard keeps the thumbnails it makes on disk, so a folder you open
again, move or rename fills in at once. Frames under a root also get
a **local preview**, a picture 2048 px on its long side, made in the
background while the drive is there: for the frames you scroll past
in the grid or the strip, and, while you cull in the Cull view or the
lightbox, for the frames around the one you are on. It lets you cull
an offline drive, and a shoot on a NAS without reading each raw over
the network. An edited frame's pictures of its edit are kept in the
same cache, beside the camera's.

Both have a size limit under **THUMBNAILS** in Settings:

| Setting | Default | Notes |
|---|---|---|
| **Keep up to** | 300 MB | About thirty thousand thumbnails at one size. 0 turns the cache off. |
| **Previews up to** | 8192 MB | About half a megabyte a frame. 0 keeps none. |

When the cache is full, the ones used longest ago go first. **Clear**
empties it.

The cache is under `~/.cache/greycard/thumbs` on Linux,
`~/Library/Caches` on a Mac and `%LOCALAPPDATA%` on Windows.
