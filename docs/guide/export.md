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

## The sheet, row by row

| Row | Choices | Default | What it does |
|---|---|---|---|
| **Format** | **JPEG**, **PNG**, **TIFF 16-bit** | JPEG | JPEG for sharing. PNG is lossless. TIFF 16-bit keeps the most tonal detail, for printing or further work in another editor. |
| **Quality** | 50 to 100 | 92 | JPEG only. Higher is a larger file with fewer artifacts. |
| **Long edge** | **Full**, **4096**, **2048**, **1024**, **Custom** | Full | The size of the picture's longer side, in pixels. An export is never made larger than the picture. |
| **Pixels** | a number | 1600 | Shown with **Custom**. The sheet shows the size that comes out, such as 1600 × 1067. |
| **Sharpen** | **Off**, **Low**, **Standard**, **High** | Standard | Sharpens the picture after it is made smaller, since shrinking softens it. It does nothing at **Full** size, and the label is dimmed then. |
| **Color** | **sRGB**, **Display P3**, **Rec.2020** | sRGB | sRGB for the web and most screens. Display P3 for recent phones and Macs. Rec.2020 for the widest color. |
| **Embed profile** | on or off | on | Writes the color profile into the file, so other programs show its colors correctly. Leave it on. |
| **Metadata** | **All**, **No edit**, **None** | All | What the file says about where it came from. See below. |
| **Subfolder** | a folder name | empty | Where the file goes. See [Where the files go](#where-the-files-go). |
| **If it exists** | **Increment**, **Overwrite**, **Skip** | Increment | What happens when a file of that name is already there. See [When a file is already there](#when-a-file-is-already-there). |
| **Watermark** | **Off**, **Text**, **Image** | Off | A mark laid over the picture. See [Watermark](#watermark). |

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

### One frame

With **Subfolder** empty, the sheet's button reads **Choose file...**.
It opens your system's save dialog, with the raw's name and folder
filled in. The dialog asks before it replaces a file, so the sheet's
**If it exists** does not apply here.

### Several frames

With **Subfolder** empty, the button reads **Export 3 frames...** and
asks for a folder. Each frame is written there under its own name with
the format's extension: `IMG_0001.CR3` becomes `IMG_0001.jpg`. Two
frames of one name, such as a raw and its camera JPEG, are told apart
with ` (2)` on the second.

### Into a subfolder

Type a folder name in **Subfolder**, such as `export` or `export/web`,
to skip the dialog. Each frame then goes into that folder inside its
own raw's folder, made when needed. Frames from three shoots land in
three `export` folders, one beside each shoot.

The name must go down from the raw's folder. A full path, or one with
`..`, is refused with the reason.

## When a file is already there

**If it exists** decides:

- **Increment** writes beside it as `IMG_0001 (2).jpg`.
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
2. Click **Add to queue...**. Choose a folder, or let **Subfolder**
   decide. Nothing is exported yet.
3. Repeat for other frames or other presets.
4. When ready, open the Export sheet and click **Export the queue**.

What is kept for each set:

- the frames, each under its edit as it was when queued (later changes
  to a frame are not used)
- the sheet's choices and the preset's name
- the destination

The foot of the sheet counts what waits, such as "2 sets, 3 frames
queued". The queue survives quitting and restarting, and never runs
on its own.

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

**Clear** empties the whole queue after asking. The queue is counted,
not listed, so you cannot remove one set on its own.

## An export in History

Each export adds a line to the frame's **HISTORY**, such as
"IMG_0001.jpg · Web" in italics, with the time. It sits on the state
the file was made from. Hover over it for the full path; click it to
go back to the look you sent out.

The line goes when its state leaves the history (undone and replaced,
or pushed past the history's limit). To keep a look for good, add it
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
| `--export-preset NAME` | Uses a saved export preset. Without it, the sheet as you last left it. |
| `--long-edge N` | Long side in pixels. Never enlarges. |
| `--on-exists WHAT` | `increment`, `overwrite` or `skip`. |
| `--preset NAME` | Applies a develop preset first, saved as a step in the frame's history. |
| `--exposure EV`, `--develop-temperature K`, `--agx` | Change this export only. Not saved to the frame. |
| `--no-sidecars` | Reads and writes no `.gcd`: each frame exports at its defaults, and nothing is recorded. |
| `--sidecar-folder` | Writes the `.gcd` into the hidden `.greycard` folder for this run. |
| `--xmp-sidecars` | Also writes an XMP beside the raw for this run. |

Give a folder as the path to open, rather than a file, to use `--also`.
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
