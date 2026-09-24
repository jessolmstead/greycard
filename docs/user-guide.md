# greycard: a guide for testers

greycard is a raw photo editor for Linux, macOS and Windows. This is a
test build, so some things are missing and some are wrong. What we need
most from you is a report whenever a picture looks wrong or the editor
does something you did not expect.

## Before you start

- **A GPU.** Vulkan on Linux and Windows, Metal on a Mac. There is no
  software fallback.
- **A Mac with an Apple chip**, if you are on a Mac. There is no build
  for Intel Macs or for Windows on ARM.
- **Raw files.** It is developed against Canon `.CR3` and Fujifilm
  `.RAF`, and most other formats open too. Some don't open yet: X-Trans
  Fujifilm bodies, and Nikon's High Efficiency raws, which are the
  default setting on the Z9, Z8 and Z6 III.
- Try it on copies of your raws, or on a shoot you have backed up.
  greycard never changes a raw file, but it is a test build.

## Installing

Download the file for your system from the
[releases page](https://github.com/jessolmstead/greycard/releases) and
unpack it.

- **Linux:** in the unpacked folder, run `sh install.sh`. Everything
  goes under `~/.local`, and `sh uninstall.sh` removes it.
- **macOS:** follow [testing-on-a-mac.md](testing-on-a-mac.md). It
  covers the security dialog you will see, since the app is not signed
  by Apple yet.
- **Windows:** double-click `install.cmd`. If you want greycard offered
  in Open with for raw and `.gcd` files, also run `register.cmd`.
  Windows may warn that the app is from an unknown publisher; click
  **More info**, then **Run anyway**. `uninstall.cmd` removes it.

## The basics

1. **Open a folder** with Ctrl+O, or with the Open folder button in the
   grid. The first frame is developed, and the filmstrip along the
   bottom holds the rest of the folder.
2. **Browse.** Use the arrow keys to move between frames, and **G** to
   switch between the grid and the single frame. When you choose a
   frame, you see the camera's own JPEG first (marked "camera
   preview"), and greycard's develop replaces it a second or two later.
3. **Edit.** The right panel has four tabs: **Develop**, **Crop**,
   **Masks** and **Retouch**. Slider changes show up in the picture as
   you make them.
4. **Export** with the **Export…** button at the bottom of the panel, or
   Ctrl+Shift+E. You can write a JPEG or a TIFF, in sRGB, Display P3 or
   Rec.2020.

Your edits are saved beside each raw in a `.gcd` file, so
`IMG_0001.CR3` keeps its edit in `IMG_0001.CR3.gcd`. To move a shoot to
another folder or machine, copy the `.gcd` files along with the raws.
Delete a `.gcd` file to start that frame's edit over.

If you would rather not see a `.gcd` beside every raw, open
**Settings...** at the bottom of the left panel (or press Ctrl+,) and
set **Edits go** to **Hidden folder**: edits then go into a hidden
`.greycard` folder inside the shoot's folder, which travels with the
shoot the same way. A sidecar in either place is read, and changing
the setting moves nothing by itself: a frame's sidecar moves to the
chosen place the next time it is saved, so a folder tidies itself as
you work through it. To tidy the open frames at once, use **Move the
open frames' sidecars** in the same sheet; it says how many are in
the other place and moves them when you confirm.

Ratings, flags and labels are kept in the `.gcd` too. To share ratings,
labels and keywords with Lightroom or darktable, turn on **Also write
an XMP** in Settings. With it on, greycard writes standard `.xmp`
sidecars beside the raws; one that is there is read either way. Picks
and rejects have no XMP field, so they stay in the `.gcd`.

## Several frames at once

Click a frame to open it. **Ctrl+click** adds a frame to the
selection or takes it out again, and **Shift+click** selects every
frame from the open one to the one you click; hold both to add a run
to what is already selected. **Shift+←** and **Shift+→** (and the
arrows in the grid) move to the next frame and keep the selection.
The selected frames are highlighted in the filmstrip and the grid,
and the frame on screen has the brighter outline. A plain arrow or a
plain click goes back to one frame, and so does **Esc**.

Ratings, flags, labels and the [ ] turns act on every selected frame.

**Sync settings** copies the open frame's edit onto the other
selected frames. Edit one frame, Ctrl+click or Shift+click the
others, then press **Sync…** under Presets, or Ctrl+Shift+S. Choose
which sections to copy: all are ticked except Adjustments, because
masks are drawn around one picture's subject. The crop, the
straighten and the retouch are never copied. Each frame keeps the
sync as one step in its history, so Ctrl+Z on that frame undoes it.

Export works on the open frame only, not the whole selection.

## Culling

Press **C** to cull. In this mode greycard shows the camera's embedded
JPEG without developing the raw, so moving from frame to frame is
instant.

- **Space** zooms to 1:1 so you can check focus.
- **V** compares two or four frames side by side.
- **Enter** or **Esc** leaves culling.
- **Move rejects…** moves rejected frames and their sidecars into a
  `rejects` folder beside the shoot. Nothing is deleted.

## Keys

| Key | Does |
|---|---|
| ← → | Previous or next frame |
| Shift+← → | Next frame, keeping the selection |
| Ctrl+click, Shift+click | Add a frame, or a run of frames, to the selection |
| G | Grid, and back (in the grid, Ctrl+wheel or + and − resize it) |
| C | Culling mode |
| Tab | Hide the panels, the filmstrip and the status line, and bring them back |
| Space | Fit or 1:1 |
| Z | Fit or 3:1 |
| 1 to 5, 0 | Set a rating, or clear it |
| P, X, U | Pick, reject, unflag |
| 6 to 9 | Red, yellow, green or blue label (press again to clear) |
| [ ] | Turn the frame a quarter left or right |
| J | Shadow and highlight clipping warnings |
| S | Soft proof |
| Ctrl+F or / | Filter the folder by rating, flag, label or words |
| Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y | Undo, redo |
| Ctrl+O | Open a folder |
| Ctrl+Shift+E | Export |
| Ctrl+Shift+S | Sync settings onto the selected frames |
| Ctrl+, | Settings |
| Esc | Close a dialog, drop the tool in hand, go back to one frame, or clear the filter |

On a Mac these are the Ctrl keys, not ⌘, for now.

## Downloads on first use

Some features download what they need the first time you use them, and
each one shows you its license first:

- the lens correction database, which is under a megabyte
- the learned denoiser and the Subject and Object mask models, which are
  5 to 200 MB each

Each one is downloaded once. The first Subject mask of a session takes
a few seconds.

The lens database is the one greycard offers without being asked: the
first time a picture opens and there are no lens profiles on the
machine, it asks whether to download them, since without them no lens
is corrected. Answer **Not now** and it will not ask again; the
**Get lens profiles** button in the Lens section is there whenever you want
them.

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

Click **Report a problem…** at the bottom of the left panel. It opens
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

