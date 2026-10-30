# Exporting

An export writes a frame as a JPEG, PNG or TIFF under its edit. This
chapter covers the Export sheet, export presets, exporting several
frames, the export queue and exporting from the command line.

greycard never changes the raw. The export is a new file.

## Opening the Export sheet

- Click **Export...** at the bottom of the right panel.
- Press **Ctrl+Shift+E**.
- Right-click a frame and choose **Export...**.

Export is not available while culling. With two or more frames
selected, the sheet's title says **Export 3 frames**, and each frame is
written under its own edit.

Enter presses **Export** unless a field or a control has the focus.
Esc presses **Cancel**, or first cancels a preset name or a queue
clear in hand.

## The sheet, row by row

| Row | Choices | Default | What it does |
|---|---|---|---|
| **Format** | **JPEG**, **PNG**, **TIFF 16-bit** | JPEG | JPEG for sharing. PNG is lossless. TIFF 16-bit keeps the most tonal detail, for printing or further work in another editor. |
| **Quality** | 50 to 100 | 92 | JPEG only. Higher is a larger file with fewer artifacts. |
| **Long edge** | **Full**, **4096**, **2048**, **1024**, **Custom** | Full | The size of the picture's longer side, in pixels. An export is never made larger than the picture. |
| **Pixels** | a number | 1600 | Shown with **Custom**. The sheet shows the size that comes out, such as 1600 × 1067. |
| **Sharpen** | **Off**, **Low**, **Standard**, **High** | Standard | Sharpens the picture after it is made smaller, since shrinking softens it. It does nothing at **Full** size, and the label is dimmed then. |
| **Color** | **sRGB**, **Display P3**, **Rec.2020**, **Adobe RGB** | sRGB | sRGB for the web and most screens. Display P3 for recent phones and Macs. Rec.2020 for the widest color. Adobe RGB for a print lab or a workflow that asks for it; it has its own gamma, and the file's profile says so. |
| **Embed profile** | on or off | on | Writes the color profile into the file, so other programs show its colors correctly. Leave it on. |
| **Metadata** | **All**, **No edit**, **None** | All | What the file says about where it came from. See below. |
| **Subfolder** | a folder name | empty | Where the file goes. See [Where the files go](#where-the-files-go). |
| **If it exists** | **Increment**, **Overwrite**, **Skip** | Increment | What happens when a file of that name is already there. See [When a file is already there](#when-a-file-is-already-there). |
| **Watermark** | **Off**, **Text**, **Image** | Off | A mark laid over the picture. See [Watermark](#watermark). |
| **Border** | **Off**, four colors, **Match** | Off | A band of color around the picture. See [Border](#border). |
| **Folder** | **Choose...** | the last folder chosen | Where **Export** writes, at the foot of the sheet. See [Where the files go](#where-the-files-go). |

### Metadata

- **All**: the camera's details (camera, lens, settings, date) and the
  edit that made the picture.
- **No edit**: the camera's details, without your edit. Use it for a
  picture you hand on without your recipe.
- **None**: nothing but the color profile. The camera, the time and
  any location are left out.

### Watermark

Choose **Text** to type a line, such as "© Your Name", and pick
**White** or **Black**. Choose **Image** and click **Choose...** to
pick a PNG; its transparency is kept.

Click a square in the 3 × 3 grid to place the mark (bottom right by
default). Then set:

- **Size**: the mark's width, 1 to 50 % of the export's long side
  (15 % by default).
- **Margin**: the gap from the edge, 0 to 10 % (2 % by default).
- **Opacity**: 5 to 100 % (60 % by default).

Because size and margin are a share of the long side, the mark looks
the same on a 2048 px export as on a full-size one. On a narrow crop it
shrinks to stay inside its margins.

The export stops, and says why, rather than write an unmarked file when:

- the text is empty or no PNG is chosen
- a character has no font that can draw it, such as a color emoji

Text is set in the system's sans-serif font.

### Border

A border frames the picture in a band of one color, as a mat does a
print. Click a swatch to turn it on: **White**, light grey, dark grey,
**Black**, or **Match**, which takes the picture's own average color.
**Off** takes it away.

**Width** is a share of the picture's long side, 0.5 to 25 % (5 % by
default), so a preset frames a 2048 px export and a full-size one
alike. The border is added outside the picture, never cut into it,
so the file is larger than **Long edge** by the band on each side. The
line under the slider gives the file's size: "Added outside the
picture; the file is 2252 × 1569."

A watermark goes over the border, and its margin is measured from the
file's edge, so a small margin puts it in the band, where a signature
on a framed print goes. To keep it on the picture, make the margin
wider than the border.

The border is not shown in the viewport. An export preset keeps it.

## Export presets

A preset keeps every choice on the sheet under a name, such as "Web
2048" or "Print".

- **Save as...** at the top of the sheet asks for a name. Press
  **Save**. If a preset of that name is there, the button reads
  **Replace**.
- **Preset** picks a saved one. **None** is no preset.
- **Delete** removes the preset shown.
- "(edited)" beside the name means the sheet no longer matches the
  saved preset. Save it again to keep the change.

A preset keeps its **Subfolder** too, so a Web preset can write into
`web` and a Print one into `print`. Presets are kept in the settings
file.

## Where the files go

### The Folder row

At the foot of the sheet, above its buttons, **Folder** names the
folder **Export** will write to, with **Choose...** beside it.
**Export** writes there at once, with no dialog, for one frame or a
set. The folder is the last one you chose, after a restart too, so a
day's exports land in one place. Before you have chosen one, it is
the frame's own folder.

**Choose...** opens on the folder shown. If that folder is gone, as on
an unplugged drive, it opens on the nearest folder above it that is
still there. A folder you choose is shown at once and kept. Choosing
one while **Subfolder** has a name in it empties the field, since the
field would otherwise win.

If the folder is missing or can't be written to when you press
**Export**, the status line says so and the chooser opens. Choose
another and the export goes there; cancel and nothing is exported.

### The file's name

Each frame is written under its own name with the format's extension:
`IMG_0001.CR3` becomes `IMG_0001.jpg`. Two frames of one name, such
as a raw and its camera JPEG, are told apart with `-1` on the second
(`IMG_0001-1.jpg`), `-2` on a third.

Written into the frame's own folder, the name is
`IMG_0001.greycard.jpg`, so an export can never be taken for, or
written over, the camera's own `IMG_0001.JPG`.

A file of that name already there follows **If it exists** (below),
for one frame as for a set.

### A version of a frame

A [version](presets-and-history.md#versions-of-a-frame) is written
with its name after the frame's: the version Warm of `IMG_0001.CR3`
becomes `IMG_0001-Warm.jpg`, and `IMG_0001-Warm.greycard.jpg` in the
frame's own folder. Characters a file name can't hold become `_`.
The export is recorded in the version's own history, and the status
line and the queue name it "IMG_0001 (Warm)".

### Into a subfolder

Type a folder name in **Subfolder**, such as `export` or `export/web`,
and each frame goes into that folder inside its own raw's folder, made
when needed. Frames from three shoots land in three `export` folders,
one beside each shoot. A name in **Subfolder** wins over the folder
chosen, so a preset with its own subfolder always writes there.

The **Folder** row says so, such as "edited beside each original",
and so does a line under the field: "Saved in edited beside each
original." Both show the name as it will be used, so `./export//web/`
shows as `export/web`: spaces around the name, a leading `./` and
doubled slashes are cleaned away. A long name is cut at the front in
the row to fit it; the line under the field has it whole.

The name must go down from the raw's folder. A full path, or one with
`..`, is refused: the reason shows under the field in yellow, and
**Export** is dimmed until the name is fixed.

## When a file is already there

**If it exists** decides:

- **Increment** writes beside it as `IMG_0001-1.jpg`.
- **Overwrite** replaces it.
- **Skip** leaves it and writes nothing.

Under **Overwrite**, greycard first checks for files it would replace.
If any are there, a sheet lists every one by its full path and asks:

- **Keep both** (the default, and what Enter does) writes each new
  file beside the old one under the next free name.
- **Overwrite**, in red, replaces them. Only a click reaches it.
- **Cancel** or Esc exports nothing.

## While it runs

The status line shows progress. For several frames, the **Export...**
button becomes **Stop export**. Click it or press Esc to finish the
frame in hand and leave the rest.

If an edit uses something this computer has not got, such as a
Subject mask whose model is not downloaded, the file is still written
without it, and the status line says so.

## The export queue

The queue keeps exports to run later, such as a day's picks to send
once you are done editing.

1. Set up the sheet as for an export.
2. Click **Add to queue**. The set goes to the **Folder** row's
   folder, or to **Subfolder** when it has a name. Nothing is exported
   yet.
3. Repeat for other frames or other presets.
4. When ready, open the Export sheet and click **Export the queue**.

What is kept for each set:

- the frames, each under its edit as it was when queued (later changes
  to a frame are not used)
- the sheet's choices and the preset's name
- the destination

The foot of the sheet lists the sets, one row each with its preset
(or format), its frame count, and its folder, above a count such as
"2 sets, 3 frames queued". The bin at a row's end removes that set at
once, without asking, except the set being exported. The queue
survives quitting and restarting, and never runs on its own.

When it runs:

- Sets are sent in the order they were added.
- **Stop export** or Esc stops after the frame in hand. The rest stay
  queued.
- A frame whose file is not there, such as on an unplugged drive,
  stays queued and is named in the status line.
- A destination folder or a watermark PNG that is not there stops the
  queue, with the reason. Nothing in it is lost.
- A frame that fails leaves the queue. The status line names it.
- It does not ask before replacing files: each set uses the **If it
  exists** choice it was queued with.

**Clear** empties the whole queue after asking.

While the queue runs, the status line counts across the whole run,
such as "exporting 3 of 12", and the line at the end counts every
frame it sent.

### The queue in the grid

Click **Export queue** under the roots in the grid's left pane to see
every queued frame as a tile. The tiles are grouped under a heading
for each destination, such as "JPEG quality 92, long edge 2048, sRGB,
to edited beside each original", with how many frames go there. Sets
whose choices and folder read the same share a heading. A frame
queued twice shows twice.

- **Delete**, or **Remove from queue...** in the frame menu, takes
  the chosen frames out of the queue after asking. Their files are
  not touched. A set left with no frames goes. The frames of the set
  being exported stay.
- While the queue runs, a frame already written shows the exported
  mark, and the one being written shows a running bar.
- Double-click a tile, or press **E**, to open the frame as it will
  export: under the edit it was queued with, not its edit now. The
  filmstrip shows the queued frames in the grid's order, without the
  headings. **G** goes back to the queue's grid. A line at the top of
  the right-hand panel says the frame is queued for export.
- The **Library** and **Loupe** buttons at the top of the left pane
  leave the queue's view, back to what you had open before it, on the
  same frame, with its tools.
- Editing is off in the queue's view, in the grid and the Loupe alike.
  The develop tabs, the presets, the history, and the snapshots are
  hidden. Undo, paste, sync, versions, turns, and presets do nothing,
  and the frame menu dims them. Zoom, Fit, the scopes, soft
  proof, and the rating, flag, and label keys still work, as does
  Cull. **Export** is for the queue's run here: an export of the
  chosen frames does nothing.

## An export in History

Each export adds a line to the frame's **HISTORY**, such as
"IMG_0001.jpg · Web" in italics, with the time. It sits on the state
the file was made from. Hover over it for the full path; click it to
go back to the look you sent out.

The line goes when its state leaves the history (undone and replaced,
or pushed past the history's limit). The frame's thumbnail keeps its
exported mark either way (see
[the library](library.md#edited-and-exported-marks)). To keep a look
for good, add it
under **SNAPSHOTS**.

## From the command line

An export can run from a terminal or a script, with no window. It needs
no GPU and no desktop, so it also runs over SSH.

```
greycard-ui IMG_0001.CR3 --export out.jpg --export-preset "Web 2048"
```

The file is written under the frame's saved edit, and the export is
added to its history, as in the window.

| Flag | What it does |
|---|---|
| `--export FILE` | Writes one frame to `FILE`. Its extension (`.jpg`, `.png`, `.tif`) sets the format. |
| `--export FOLDER/` | Writes into a folder: one that exists, or a path ending in `/`. Each frame keeps its own name. |
| `--also 2,5,7` | With a folder: adds more frames from the opened folder, counted from 0 in the folder's order. |
| `--all` | With a folder: writes every frame of the opened folder, in the folder's order. Give the folder's path; not with `--also` or `--preset`. |
| `--export-preset NAME` | Uses a saved export preset. Without it, the sheet as you last left it. |
| `--long-edge N` | Long side in pixels. Never enlarges. |
| `--on-exists WHAT` | `increment`, `overwrite` or `skip`. |
| `--version-name NAME` | Exports each frame's version of that name instead of the frame's own edit, named `IMG_0001-NAME.jpg` in a folder, and recorded in the version's history. A frame with no version of that name stops the run. |
| `--preset NAME` | Applies a develop preset first, saved as a step in the history of the frame opened: the file given, or in a folder the one you last had open. Not with `--all`; lay a preset over many files with `greycard presets --apply NAME <files>`, then export. |
| `--exposure EV`, `--develop-temperature K`, `--agx` | Change this export only, on the frame opened (with `--all`, on every frame). Not saved to the frame. |
| `--no-sidecars` | Reads and writes no `.gcd`: each frame exports at its defaults, and nothing is recorded. |
| `--sidecar-folder` | Writes the `.gcd` into the hidden `.greycard` folder for this run. |
| `--xmp-sidecars` | Also writes an XMP beside the raw for this run. |

An export preset with a border writes the border; there is no flag of
its own for it.

Give a folder as the path to open, rather than a file, to use `--also`
or `--all`.
The **Subfolder** field is not used from the command line: the path you
give decides.

The last line on the terminal says how it went, and the exit code says
the same:

| Code | Meaning |
|---|---|
| 0 | Every frame written as its edit asks (or skipped, under `skip`). |
| 1 | A frame was not written, or the run could not start. The reason is given. |
| 2 | Every frame was written, but some without something the edit names that this computer has not got, such as a look or a downloaded mask model. Each is named. |

Flags that only make sense with a window, such as `--snapshot`, are
refused with the reason.

On Windows the program is `greycard-ui.exe`.
