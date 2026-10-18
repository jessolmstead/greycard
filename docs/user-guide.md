# greycard user guide

greycard is a raw photo editor for Linux, macOS and Windows. This page
gets you installed and started; the chapters below cover each part of
the editor.

It is a test build, so some things are missing. When a picture looks
wrong, or the editor does something you didn't
expect, please [report it](#reporting-a-problem).

## Before you start

- **A GPU.** Vulkan on Linux and Windows, Metal on a Mac.
- **A Mac with an Apple chip**, if you're on a Mac. There is no build
  for Intel Macs or Windows on ARM.
- **Raw files.** Tested with Canon, Fujifilm GFX and X-Trans, Sony,
  Panasonic and Nikon raws. X-Trans raws develop without noise
  reduction for now ([Demosaic](guide/develop.md#fujifilm-x-trans)).
  Nikon's High Efficiency raws (TicoRAW) don't open yet; only a build
  from source with the `nikon-he` feature reads them
  ([building.md](building.md)).
- **Copies or a backup.** greycard never changes a raw file, but it is
  a test build.
- **On Linux, a file chooser.** greycard asks the desktop's file
  chooser portal, which GNOME, KDE and most desktops run. On a window
  manager such as i3 started without one, install `zenity` (or
  `kdialog`) and greycard uses it instead, or run the session under
  `dbus-run-session` with `xdg-desktop-portal` and
  `xdg-desktop-portal-gtk` installed. A folder named on the command
  line, `greycard-ui ~/Pictures/shoot`, opens without either.

## Installing

Download the file for your system from the
[releases page](https://github.com/jessolmstead/greycard/releases) and
unpack it.

- **Linux:** run `sh install.sh` in the unpacked folder. `sh
  uninstall.sh` removes it.
- **macOS:** follow [testing-on-a-mac.md](testing-on-a-mac.md), which
  gets you past the security warning for an app not yet signed by Apple.
- **Windows:** double-click `install.cmd`. Run `register.cmd` too if you
  want greycard in **Open with** for raws. If Windows warns about an
  unknown publisher, click **More info**, then **Run anyway**.
  `uninstall.cmd` removes it.

## The basics

1. **Open a folder** with **Ctrl+O** or **Open folder...**. The
   filmstrip along the bottom holds the folder's frames.
2. **Browse** with the arrow keys. **Library** and **Loupe** at the
   top of the left pane (or **G**) switch between the grid
   and one frame. The loupe has two modes, **Cull** and **Develop**,
   first in the right panel's tab row. You see the camera's own preview
   first; greycard's develop replaces it a moment later.
3. **Edit** in the right panel's tabs after **Cull**: **Develop**,
   **Crop**, **Masks** and **Retouch**.
4. **Export** with **Export...** or **Ctrl+Shift+E**.

Your edits are saved as you go, in a `.gcd` file beside each raw. Copy
those along with the raws to move a shoot.

The window opens no taller than your screen, so on a 13 or 14 inch
laptop the buttons of a sheet such as Export stay in reach.

## The chapters

Working with your photos:

- [Folders and the library](guide/library.md): opening folders, library
  roots, the folder tree, filtering, collections, where edits are kept
- [Importing from a card](guide/import.md)
- [Culling and rating](guide/culling.md): ratings, flags, labels,
  the lightbox, stacks, rejects and deleting
- [Archives and backup](guide/archives.md)

Editing:

- [The Develop tab](guide/develop.md): every section of the panel
- [Crop and retouch](guide/crop-and-retouch.md)
- [Masks](guide/masks.md)
- [Scopes and color](guide/scopes-and-color.md)
- [Presets, history and several frames](guide/presets-and-history.md):
  presets, snapshots, the history, versions of a frame, sync and paste
- [Fitting your camera's look](guide/camera-match.md)

Finishing and reference:

- [Exporting](guide/export.md)
- [Settings](guide/settings.md)

## Keys

Press **?** in the editor, or choose **Help** > **Keys (?)** at the
foot of the left pane, for these on a sheet you can search. The sheet
is made from the table the editor reads its keys from, so it always
matches the build you have. **Help** also opens this guide and the
bug form.

### Moving around

| Key | Does |
|---|---|
| ← → | Previous or next frame (in the grid, the mouse's back and forward buttons too) |
| Shift+← → | Previous or next frame, added to the selection |
| ↑ ↓ | In the grid, the frame above or below (with Shift, added) |
| Home, End | The first or the last frame (with Shift, added) |
| Ctrl+click | Add a frame to the selection, or take it out |
| Shift+click, Ctrl+Shift+click | Select every frame from the open one to this one; with Ctrl, add them instead |

### Views

| Key | Does |
|---|---|
| G | Library (the grid), and back to the loupe as it was left |
| E | The loupe (one frame), in the mode it was left in: Cull or Develop |
| D | Develop: the loupe with the edit panel |
| L | The lightbox, and back |
| C | Cull; in the loupe, Cull or Develop in turn |
| V | In Cull, compare two or four frames |
| Enter | Open the frame chosen in the grid or the lightbox; from Cull, Develop |
| + − | Bigger or smaller tiles in the grid (Ctrl+wheel too); fewer or more across the lightbox |
| Tab | Hide the panels, the filmstrip and the status line, and bring them back (in the grid, the left pane) |
| F6, F7, F8 | The filmstrip, the left pane or the develop panel alone |
| Space | Fit or 1:1 |
| Z | Fit or 3:1 |
| J | Shadow and highlight clipping warnings |
| S | Soft proof (in the grid, a stack; below) |

### Rating and sorting

| Key | Does |
|---|---|
| 1 to 5, 0 | Set a rating, or clear it |
| P, X, U | Pick, reject, unflag |
| 6 to 9 | Red, yellow, green or blue label (press again to clear) |
| [ ] | Turn the frame a quarter left or right; not part of the edit |
| B | Add the selected frames to a collection, or take them out |
| Ctrl+G, Ctrl+Shift+G | Stack the selected frames; unstack them |
| S, Shift+S (in the grid) | Open or close a stack; make the frame its top |
| Ctrl+' | New version of the frame |
| Delete | Delete the selected frames, after a confirmation |

### Editing

| Key | Does |
|---|---|
| Ctrl+Z | Undo |
| Ctrl+Shift+Z or Ctrl+Y | Redo |
| Ctrl+C | Copy the open frame's settings |
| Ctrl+V | Paste them onto the selected frames |
| Ctrl+Shift+S | Sync the selected frames to the open one |

### Files, sheets and help

| Key | Does |
|---|---|
| Ctrl+F or / | Filter the folder by rating, flag, label or words |
| Ctrl+O | Open a folder |
| Ctrl+Shift+I | Import from a card |
| Ctrl+Shift+E | Export |
| Ctrl+, | Settings |
| ? | The keys sheet |
| Esc | Close a sheet, put a tool down, let go of a set, go back to one frame, or clear the filter |

On a Mac, Ctrl is ⌘, and the keys sheet says Cmd (⌘C, ⌘V, ⌘Z,
⌘+click). Control+click is a right-click, and delete (⌫) opens the
delete sheet.

## Known rough edges

- The default look and the tone sliders don't match Lightroom's feel
  yet. Highlights and Whites came closer in 0.7; tell us when a frame
  looks too dark, too flat or odd in a way you can name.
- Fujifilm X-Trans raws get no noise reduction, hot pixel repair or
  chromatic aberration correction yet.
- The high-quality denoisers are slow, taking tens of seconds on a
  large frame.
- On Windows and macOS the display is treated as sRGB unless you pick
  your monitor's ICC profile by hand. On a wide-gamut screen, colors
  may look too saturated until you do.
- On a Mac, double-clicking a raw or a `.gcd` in Finder launches
  greycard but does not open that file.

## Reporting a problem

Click **Report a problem...** at the bottom of the left panel, or
choose it from **Help** just above. It opens
the bug form in your browser with the version, OS and GPU already
filled in, and it shows the log file in your file manager so you can
drag it into the form. In the report, include:

1. What you did and what you expected, in a sentence each.
2. The log. The previous run's log is kept as `greycard-ui.log.1`.
3. The raw file and the camera model, if a picture renders wrong. This
   is the most useful report you can send.

If the form doesn't work, the logs are here:

| System | Log folder |
|---|---|
| Linux | `~/.local/state/greycard` |
| macOS | `~/Library/Logs/greycard` |
| Windows | `%LOCALAPPDATA%\greycard\logs` |

