# Importing from a card

The **Import** sheet copies the frames off a camera card into a folder
on your computer, names them the way you choose, and can make a second
copy on a backup drive. Use it at the start of a shoot's work, before
you cull. It can also add a folder already on your computer to the
library where it is, copying nothing: see
[Adding a folder in place](#adding-a-folder-in-place).

To bring your shoots over from Lightroom Classic, ratings, edits,
collections and all, see
[A Lightroom Classic catalog](#a-lightroom-classic-catalog) at the end
of this chapter. RawTherapee's `.pp3` profiles are offered when you
open a folder that holds them; see
[Profiles from RawTherapee](library.md#profiles-from-rawtherapee).

greycard never writes to the card or deletes anything from it. When
the import is done, you format the card in the camera as usual.

## Opening the sheet

Press **Ctrl+Shift+I**, or click **Import...** at the top of the left
pane while the grid is up and choose **From a card or folder...**. You
don't need a folder open first. The same menu has **A Lightroom
catalog...**, described [below](#a-lightroom-classic-catalog).

When the sheet opens, greycard looks for a card: the first mounted
drive with a `DCIM` folder on it. If it finds none, the **From** line
says "No card found" and you choose the folder yourself.

The choice at the top is **Copy**, which this page describes first,
or **Add in place**. The sheet always opens on **Copy**.

## The sheet, line by line

| Line | What it does |
|---|---|
| **From** | The folder the frames come from. A card's `DCIM` folder fills in by itself; **Choose...** picks any folder of pictures instead. Every folder under it is read. |
| *(note under From)* | How many frames it found, how many files are left on the card, and any folder that could not be read (its frames are not imported). |
| **To** | The folder the frames go into. The first time, your Pictures folder. |
| **Folders** | Folders to make inside **To**, as a pattern. Starts as `{date}`. Leave it empty to put everything straight into **To**. |
| **Name** | The new file name, as a pattern. `{name}` (the default) keeps the camera's name. |
| *(preview)* | What the first frame will be called and where it will go, for example "IMG_0001.CR3 becomes ~/Pictures/2026-09-24/IMG_0001.CR3". |
| **Preset** | A develop preset laid on every imported frame. **None** leaves them unedited. |
| **Backup** | A second folder that gets a copy of every file. **Clear** goes back to "None: one copy only". |
| **Verify against the card** | Compares frames already in **To** with the card byte for byte. Off by default. See [Verification](#verification). |

The button at the bottom says how many frames it will import, such as
**Import 214 frames**. It stays off until there is a source with
frames, a destination that exists, and patterns that make a name.

The sheet remembers **To**, **Folders**, **Name**, **Preset** and
**Backup** for next time.

### What gets copied

- Every raw and picture greycard can open.
- The camera's JPEG of a raw (`IMG_0001.JPG` beside `IMG_0001.CR3`)
  goes with the raw and takes the raw's new name. A JPEG with no raw
  beside it is a frame of its own.
- An XMP goes with the frame it belongs to.
- Videos and other files greycard can't open stay on the card. The
  note under **From** counts them.

### Patterns

**Folders** and **Name** take these tokens, with any text between
them:

| Token | Becomes |
|---|---|
| `{date}` | The day the frame was taken, `2026-09-24` |
| `{yyyy}` `{mm}` `{dd}` | The year, month and day on their own |
| `{name}` | The camera's file name, without its extension |
| `{camera}` | The camera model, or "Unknown camera" |
| `{seq}` | The frame's place in this import, `0001`, `0002`, … |
| `{folder}` | The frame's folder under **From**, such as `ceremony` or `reception/late`; nothing for a frame directly in **From** |

A `/` in **Folders** makes a folder inside a folder: `{yyyy}/{date}`
gives `2026/2026-09-24`. `{folder}` alone in **Folders** keeps the
source's subfolders as they are, and `{date}/{folder}` puts them
under the day. In **Name**, its `/` becomes `_`. For a frame directly
in **From**, `{folder}` is empty and takes a `-`, `_`, space, `.` or
`/` beside it with it, so `{folder}-{name}` gives `IMG_0001.CR3`
there. The extension is always kept, so a pattern like `{date}-{seq}`
gives `2026-09-24-0001.CR3`.

If a pattern has a typo, such as an unknown token, the preview says
"Not a pattern" and the import button stays off.

The date is the day the frame was taken, or the day the file was
saved when the frame doesn't say. Characters some systems refuse in a
name (`:` or `?`, for one) become `_`, so the names work on Linux,
macOS and Windows alike.

### Preset

The preset is applied to each newly copied frame as one step in its
history, "Preset: *name*", so you can undo it per frame. A preset with
a camera profile made for another camera has that profile left off;
the status line says how many frames that happened to. Frames that
were already imported are not touched.

### Backup

The backup gets the same folders and names as **To**, copied from the
new copies rather than read off the card again. If the backup is on
the same drive as **To**, the sheet warns "Backup is on the same
drive.", since one drive failing would take both.

If a remembered **To** or **Backup** drive isn't plugged in, the sheet
says "Destination missing; is the drive connected?" (or "Backup
missing"), and the import button stays off. The sheet never makes
those folders for you.

**To** and **Backup** can't be on the card, can't contain the source
folder, and can't be inside each other.

## Verification

Every copy is checked. greycard reads each copy back from the drive
and compares it with what it read from the card, and the file only
appears under its name once the two match. So a stopped import, a full
disk or a failing card never leaves a half-written frame behind. The
backup copy is checked the same way.

**Verify against the card** is about frames that are already in
**To**, as when you import the same card twice. Normally greycard
recognizes them by their size and the start and end of the file, which
is quick. With **Verify** on, it compares every byte instead, which
reads the whole card again. Turn it on if you suspect an earlier copy
was damaged.

## Frames already there, and names already taken

- **A frame you imported before** is skipped and counted as "already
  there". greycard recognizes it under any name inside **To**, and in
  any folder your library holds. Run an import again after it stops,
  and it picks up where it left off.
- **A name another file already has** is never written over. The new
  frame gets `-1` before its extension (`-2` and so on if needed),
  and the status line says how many.

## While it runs, and when it's done

The import runs in the background. Its progress shows under
**Import...** in the left pane (the button becomes **Stop import**),
and on the status line in the loupe. **Stop import**, or **Esc** when
nothing else is using it, finishes the frame in hand and stops. If you
close the window during an import, greycard waits for the frame in
hand before it closes.

When it finishes, greycard opens the folder that took the most frames.
If you moved to another folder while it ran, it leaves you there and
says which folder to open instead. The status line sums it up, for
example "imported 214 frames to ~/Pictures (5400 MB at 85 MB/s), 214
backed up, 12 already there".

If one file fails, the import stops there and names the file. The
frames before it are in place, and running the import again carries
on from there.

If a folder under **From** could not be read, the import copies the
rest and the status line ends "1 folder could not be read, its frames
not imported". Those frames are still only on the card.

## Adding a folder in place

**Add in place**, at the top of the sheet, adds a folder already on
your computer to the library where it is, as
[+ Add a root...](library.md#roots-your-library) does, and opens it.
Nothing is copied or renamed, and its subfolders come with it.

1. Click **Add in place**.
2. Click **Choose...** beside **Folder** and pick the folder. The note
   under it counts its frames and its subfolders, such as "214 frames,
   3 subfolders". A subfolder greycard can't read is passed over and
   counted: "1 folder could not be read".
3. Click **Add 214 frames**. The sheet closes, the folder becomes a
   root, and the grid shows its frames as the library reads them.

A folder already under a root is not added twice: greycard says which
root holds it and opens the folder there. Nothing is written beside
the frames until you edit, rate or label them, as with any folder you
open.

## From the command line

`--import` runs an import without opening a window, for a script or a
card reader on a server:

```
greycard-ui --import /media/me/EOS_DIGITAL/DCIM --to ~/Pictures/shoots --subfolder "{date}" --name "{date}-{seq}"
```

| Flag | What it does |
|---|---|
| `--import SRC` | The folder to import from: a card's `DCIM`, or any folder of frames |
| `--to DEST` | Where the frames go. Required. Made if it isn't there |
| `--subfolder PATTERN` | Folders to make inside `--to`, as **Folders** takes them, `{folder}` too. None without it, unlike the sheet |
| `--name PATTERN` | The new file names, as **Name** takes them. `{name}` without it |
| `--backup DIR` | A second, checked copy of every file |
| `--preset NAME` | A develop preset laid on every imported frame |
| `--verify` | The sheet's **Verify against the card** |

The command line checks the same things the sheet does before it
copies anything. It prints the summary line when it's done, and exits
with 0 when the import finished, 1 when it stopped on an error, and 2
when it went through but a folder under the source could not be read:
the line then says so, and those frames are still on the card, so
don't format it.

## A Lightroom Classic catalog

greycard reads a Lightroom Classic catalog (a `.lrcat` file) and
brings its frames into your library with what you did to them in
Lightroom. It only reads the catalog, and it never changes a raw:
what it brings is written to greycard's own sidecars beside the
frames, to your collections and to your library's roots.

Quit Lightroom first: a catalog Lightroom has open is refused. If you
import a copy of a catalog, copy the `-wal` file beside it too when
there is one, since Lightroom keeps its newest changes there until it
quits. greycard reads catalogs from Lightroom Classic 15.2 to 15.5,
and says so when a catalog is from another version.

### What comes across

- Ratings, picks and rejects, color labels, keywords (with their full
  path, such as `Places/Scotland/Skye`), titles and captions.
- A capture time you corrected in Lightroom, as a
  [shift](culling.md#shift-capture-time) on the frame, so it sorts and
  groups by the time you set there; the camera's own time stays in the
  file.
- The develop settings, as one step in each frame's history,
  "Imported from Lightroom", which you can undo. They are translated
  as a [Lightroom preset's](presets-and-history.md#importing-presets-including-lightrooms)
  are, so they are a starting point rather than an exact match, and
  the report names what has no place here, such as masks, healed spots
  and color noise reduction. A camera Lightroom shows brighter than
  greycard does, such as the Fujifilm GFX 100S II, has its exposure
  corrected on the way in.
- Crops, with their angle, and a frame turned in Lightroom stands
  turned here too. A flipped frame keeps neither its flip nor its
  crop.
- Virtual copies, each as a
  [version](presets-and-history.md#versions-of-a-frame) of its frame,
  named for the copy. A version shares its frame's rating, flag and
  label, so a copy rated apart from its master takes the master's.
- Stacks, as greycard [stacks](culling.md#stacks), open or closed as
  Lightroom had them.
- Collections, in a set named for the catalog unless you change it
  (below), with the catalog's own sets inside it. The Quick Collection
  comes as "Quick Collection". Books, prints, slideshows and web pages
  come as collections of their frames, unless you leave them out.
- Smart collections, as greycard's
  [smart collections](library.md#smart-collections), when each of
  their rules has a filter term here (rating, flag, label, keywords,
  capture date, camera and lens) and they match all of their rules.
  Any other is skipped, and the report says why. Lightroom's six
  stock smart collections are left out.
- The catalog's folders, each added to the library as a
  [root](library.md#roots-your-library).

### Importing one

1. Click **Import...** at the top of the left pane, choose **A
   Lightroom catalog...**, and pick the `.lrcat` file.
2. The sheet lists the catalog's folders as the catalog names them.
   Beside each is the folder greycard found it in on this computer,
   and how many of its files are there. greycard looks where the
   catalog says, then beside the catalog and under your library's
   roots, for a folder of the same name holding the same files. Where
   it found none ("not found"), or the wrong one, click **Choose...**
   and pick where that folder lives now; the sheet looks again at
   once.
3. Under the folders, **Set** names the set your collections go into:
   the catalog's name to start with, or empty to put them at the top
   of the list. When the catalog has books, prints, slideshows or web
   pages, a switch below brings them in as collections or leaves them
   out.
4. **Check** shows what the import would do, writing nothing: how many
   frames it found and didn't, the edits, crops, copies, stacks and
   collections it would bring, the smart collections it would skip and
   why, where new sidecars would go, and the frames it would leave
   alone. **Back** goes back to the folders.
5. **Write** writes it. It stays off until at least one of the
   catalog's files is found. The sheet stays up while it writes, and
   the button in the left pane reads **Importing the catalog...**.
   When it is done, the sheet says what was written, and the grid, the
   folders and the collections show it at once. **OK** closes the
   sheet.

On a catalog whose pictures are on a network share, finding the
folders can take a while. The sheet says how far it has got, and
**Cancel** stops it while it looks or checks. Once **Write** has
started, it runs to the end.

A frame's edit goes into the sidecar it has already, if any. A new
sidecar goes into the folder's hidden `.greycard` folder when the
folder has one, and otherwise where **Edits go** in
[Settings](settings.md#edits-go) puts it.

### Frames you already worked on

A frame that already has an edit, a version, or a rating, flag, label
or keyword set in greycard is left alone, so an import never undoes
your work here, and the report counts those frames. When there are
some, the sheet offers a switch to write over them too, with their
count. With it on, Lightroom's edits, ratings and capture times
replace yours on those frames.

Importing the same catalog a second time writes nothing that is
already here: the frames imported the first time are left alone, and
collections, stacks and roots already there stay as they are.

### The catalog from the command line

`greycard library import-lightroom CATALOG` prints the report the
sheet's **Check** shows, and `--write` writes it:

```
greycard library import-lightroom ~/Lightroom/Shoot.lrcat --root "A:/Photos=/home/me/Photos" --write
```

| Flag | What it does |
|---|---|
| `--root CATALOG=HERE` | Where one of the catalog's folders is now: its path as the catalog has it, a folder above it, or its name, then `=` and the folder on this computer. Give one for each folder that moved |
| `--set NAME` | The set the collections go into; `--set ""` for none |
| `--no-creations` | Leave out books, prints, slideshows and web pages |
| `--write` | Write the sidecars, the collections and the roots |
| `--force` | With `--write`, write over the frames you already worked on |
