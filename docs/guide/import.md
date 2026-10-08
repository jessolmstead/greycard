# Importing from a card

The **Import** sheet copies the frames off a camera card into a folder
on your computer, names them the way you choose, and can make a second
copy on a backup drive. Use it at the start of a shoot's work, before
you cull.

greycard never writes to the card or deletes anything from it. When
the import is done, you format the card in the camera as usual.

## Opening the sheet

Press **Ctrl+Shift+I**, or click **Import...** at the top of the left
pane while the grid is up. You don't need a folder open first.

When the sheet opens, greycard looks for a card: the first mounted
drive with a `DCIM` folder on it. If it finds none, the **From** line
says "No card found" and you choose the folder yourself.

## The sheet, line by line

| Line | What it does |
|---|---|
| **From** | The folder the frames come from. A card's `DCIM` folder fills in by itself; **Choose...** picks any folder of pictures instead. Every folder under it is read. |
| *(note under From)* | How many frames it found, and how many files are left on the card. |
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

A `/` in **Folders** makes a folder inside a folder: `{yyyy}/{date}`
gives `2026/2026-09-24`. The extension is always kept, so a pattern
like `{date}-{seq}` gives `2026-09-24-0001.CR3`.

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
| `--subfolder PATTERN` | Folders to make inside `--to`, as **Folders** takes them. None without it, unlike the sheet |
| `--name PATTERN` | The new file names, as **Name** takes them. `{name}` without it |
| `--backup DIR` | A second, checked copy of every file |
| `--preset NAME` | A develop preset laid on every imported frame |
| `--verify` | The sheet's **Verify against the card** |

The command line checks the same things the sheet does before it
copies anything. It prints the summary line when it's done, and exits
with 0 when the import finished and 1 when it stopped on an error.
