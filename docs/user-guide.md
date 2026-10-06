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
- **Raw files.** Tested with Canon, Fujifilm GFX, Sony, Panasonic and
  Nikon raws. Two kinds don't open yet: Fujifilm's X-Trans raws, and
  Nikon's High Efficiency raws (TicoRAW).
- **Copies or a backup.** greycard never changes a raw file, but it is
  a test build.

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
2. **Browse** with the arrow keys. **G** switches between the grid and
   one frame. You see the camera's own preview first; greycard's
   develop replaces it a moment later.
3. **Edit** in the right panel's tabs: **Develop**, **Crop**,
   **Masks** and **Retouch**.
4. **Export** with **Export...** or **Ctrl+Shift+E**.

Your edits are saved as you go, in a `.gcd` file beside each raw. Copy
those along with the raws to move a shoot.

## The chapters

Working with your photos:

- [Folders and the library](guide/library.md): opening folders, library
  roots, the folder tree, filtering, where edits are kept
- [Importing from a card](guide/import.md)
- [Culling and rating](guide/culling.md): ratings, flags, labels,
  rejects and deleting
- [Archives and backup](guide/archives.md)

Editing:

- [The Develop tab](guide/develop.md): every section of the panel
- [Crop and retouch](guide/crop-and-retouch.md)
- [Masks](guide/masks.md)
- [Scopes and color](guide/scopes-and-color.md)
- [Presets, history and several frames](guide/presets-and-history.md)
- [Fitting your camera's look](guide/camera-match.md)

Finishing and reference:

- [Exporting](guide/export.md)
- [Settings](guide/settings.md)

## Keys

| Key | Does |
|---|---|
| ← → | Previous or next frame (in the grid, the mouse's back and forward buttons too) |
| Shift+← → | Previous or next frame, added to the selection |
| Ctrl+click | Add a frame to the selection, or take it out |
| Shift+click, Ctrl+Shift+click | Select every frame from the open one to this one; with Ctrl, add them instead |
| G | Grid, and back (in the grid, Ctrl+wheel or + and − resize it) |
| C | Culling mode |
| Tab | Hide the panels, the filmstrip and the status line, and bring them back |
| Space | Fit or 1:1 |
| Z | Fit or 3:1 |
| 1 to 5, 0 | Set a rating, or clear it |
| P, X, U | Pick, reject, unflag |
| 6 to 9 | Red, yellow, green or blue label (press again to clear) |
| [ ] | Turn the frame a quarter left or right |
| Delete | Delete the selected frames, after a confirmation |
| J | Shadow and highlight clipping warnings |
| S | Soft proof |
| Ctrl+F or / | Filter the folder by rating, flag, label or words |
| Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y | Undo, redo |
| Ctrl+O | Open a folder |
| Ctrl+Shift+I | Import from a card |
| Ctrl+Shift+E | Export |
| Ctrl+Shift+S | Sync settings onto the selected frames |
| Ctrl+C, Ctrl+V | Copy the open frame's settings; paste them onto the selected frames |
| Ctrl+, | Settings |
| Esc | Close a dialog, drop the tool in hand, go back to one frame, or clear the filter |

On a Mac, Ctrl is ⌘ (⌘C, ⌘V, ⌘Z, ⌘+click). Control+click is a
right-click, and delete (⌫) opens the delete sheet.

## Known rough edges

- The default look and the Highlights, Whites and Shadows sliders don't
  match Lightroom's feel yet. That is the next release's work, so tell
  us when a frame looks too dark, too flat or odd in a way you can name.
- The high-quality denoisers are slow, taking tens of seconds on a
  large frame.
- On Windows and macOS the display is treated as sRGB unless you pick
  your monitor's ICC profile by hand. On a wide-gamut screen, colors
  may look too saturated until you do.
- On a Mac, double-clicking a raw or a `.gcd` in Finder launches
  greycard but does not open that file.

## Reporting a problem

Click **Report a problem...** at the bottom of the left panel. It opens
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

