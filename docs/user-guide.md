# greycard user guide

greycard is a raw photo editor for Linux, macOS and Windows. This page
gets you installed and started; the chapters below cover each part of
the editor.

It is a test build, so some things are missing. When a picture looks
wrong, or the editor does something you didn't
expect, please [report it](#reporting-a-problem).

## System requirements

### The editor

Opening, culling, developing, and exporting.

| | Minimum | Recommended |
|---|---|---|
| **Operating system** | Linux with glibc 2.39 or newer (Ubuntu 24.04, Fedora 40, Debian 13, or later); macOS 13.4 or newer; Windows 10 or newer, 64-bit | The same |
| **Processor** | Any 64-bit Intel or AMD processor; Apple silicon or an Intel Mac; Windows on ARM | The same; for the AI features, see below |
| **Graphics** | None: greycard draws in software and develops on the CPU, slowly | A GPU with Vulkan (Linux, Windows) or Metal (Mac) |
| **Memory** | 8 GB | 16 GB |
| **Graphics memory** | 4 GB, with fallbacks | 8 GB |
| **Disk** | 1 GB | 10 GB |

- **Without a GPU** greycard still opens, and says so in the status
  line at the start, once more after the first develop, and in the
  log. A 24 MP frame takes about 14 s on a 2012 quad-core.
- **On Linux**, the tarball and the AppImage both need glibc 2.39.
- **On a Mac**, the Intel build is newer than the Apple silicon one.
  A Mac's graphics memory is its memory.
- **On Windows on ARM**, take the `aarch64-windows` files. It is a new
  build and not yet tried on a real machine.
- **A 4 GB card** develops; when it runs short, greycard lets go of
  what it can make again.
- **The disk** holds the program (under 200 MB unpacked) and its
  caches: thumbnails up to 300 MB, and previews of your library's
  frames up to 8 GB, about 0.5 MB a frame. Both limits are in
  [Settings](guide/settings.md#thumbnails).

### AI features

The learned denoiser, the learned masks, and the two removes download
their models the first time you use them. Each needs this beyond the
editor:

| Feature | Graphics memory, or the CPU | Download | Not available |
|---|---|---|---|
| **Learned denoiser** | 2 to 5 GB; a smaller card runs it in smaller pieces, slower. Without a GPU, on the CPU, slower again | 5 to 20 MB a tier, and up to 8 GB of saved results | Fujifilm X-Trans raws |
| **Learned masks** | Sky 0.8 GiB, Object 2.5, People 4.4, Subject and Background 5.9; a card short of it runs the mask on the CPU. There a People shape takes several seconds, an iris about thirty | Sky 96 MB, Subject 114 MB, Object 184 MB, People 1.04 GB | |
| **Patch fill** (the eraser) | 2 GB, or on the CPU about 3.5 GB of memory | 210 MB | |
| **Generative remove** | 6.3 GiB, and 16-bit floats in shaders: on NVIDIA, a GTX 16 or RTX 20 series card or newer | 5.5 GB, and 390 MB for **Fast** | Intel Macs, Windows on ARM, machines without a GPU, and older NVIDIA cards |

- **The processor.** The AI features run on ONNX Runtime. The copy
  that comes with greycard needs AVX2: most Intel Core processors from
  2013 on, and AMD's from 2015 on. On an older one, and on Windows on
  ARM, greycard offers Microsoft's runtime instead (about 9 MB on
  Linux, 80 MB on Windows) and runs the models on the CPU alone,
  slower ([On an older processor](guide/masks.md#on-an-older-processor),
  [On Windows on ARM](guide/masks.md#on-windows-on-arm)).
- **On an Intel Mac**, the denoiser and the Sky mask run through Core
  ML; Subject, Background, Object, People, and the patch fill run on
  the CPU alone ([On an Intel Mac](guide/masks.md#on-an-intel-mac)).
- **Every model** at once is about 7.5 GB, 5.9 of it the generative
  remove. With them and full caches, plan on 25 GB.

## Before you start

- **Raw files.** Tested with Canon, Fujifilm GFX and X-Trans, Sony,
  Panasonic, Nikon, and Olympus and OM System raws. X-Trans raws develop without the learned
  denoiser for now ([Fujifilm X-Trans](guide/develop.md#fujifilm-x-trans)).
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
[releases page](https://github.com/jessolmstead/greycard/releases).

- **Linux:** unpack the `.tar.gz` and run `sh install.sh` in its
  folder. `sh uninstall.sh` removes it.
- **Linux, without installing:** `greycard-<version>-linux-x86_64.AppImage`
  is the editor in one file, nothing to unpack. Make it executable
  (`chmod +x`) and run it, with a folder or a raw after it if you like;
  `--cli` first runs the command line, `greycard`, instead. It needs
  FUSE, which most desktops have; where it is missing, as in many
  containers, add `--appimage-extract-and-run`. It puts nothing in your
  menus or Open with unless your launcher takes in AppImages itself.
  Its settings, cache, and downloaded models are the same ones an
  installed copy uses, so the two can be swapped. It needs the same
  glibc as the tarball ([System requirements](#system-requirements)).
- **macOS:** open `greycard-<version>-aarch64-macos.dmg`, or
  `-x86_64-macos.dmg` on an Intel Mac, and drag greycard onto
  Applications. The app is signed and notarized, so it
  opens with one question the first time. For the `greycard` command
  too, take the `.tar.gz` instead and run `sh install.sh`;
  [testing-on-a-mac.md](testing-on-a-mac.md) has both step by step.
- **Windows:** run `greycard-<version>-x86_64-windows-setup.exe`, or
  `-aarch64-windows-setup.exe` on Windows on ARM. It installs for you
  alone, without asking for an administrator: the Start Menu entry,
  `greycard` on your Path, and greycard in **Open with** for raws. If
  Windows warns about an unknown publisher, click **More info**, then
  **Run anyway**. Remove it from **Settings > Apps > Installed apps**.
  Your settings, presets, and downloaded models stay either way.
- **Windows, without installing:** take
  `greycard-<version>-x86_64-windows.zip` (or `-aarch64-windows.zip`),
  unpack it anywhere, and run `bin\greycard-ui.exe`.

To check a download, put the release's `SHA256SUMS` beside it and run
`sha256sum -c SHA256SUMS --ignore-missing` there (`shasum -a 256 -c
SHA256SUMS` on a Mac).

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

Text and controls too small? **Scale**, under **Interface** in the
Settings sheet (**Ctrl+,**), makes them up to twice the size, from the
next start ([Scale](guide/settings.md#scale)).

## The chapters

Working with your photos:

- [Folders and the library](guide/library.md): opening folders, library
  roots, the folder tree, filtering, sorting, collections, a raw's
  camera JPEG, where edits are kept, RawTherapee profiles
- [Importing](guide/import.md): from a card, a folder where it is, or
  a Lightroom Classic catalog
- [Culling and rating](guide/culling.md): ratings, flags, labels,
  the lightbox, stacks, rejects and deleting
- [Archives and backup](guide/archives.md): a NAS or a big drive as
  the long-term copy, back up and bring back, and
  [working on two computers](guide/archives.md#working-on-two-computers),
  such as a laptop and a desktop, with edits synced through it

Editing:

- [The Develop tab](guide/develop.md): every section of the panel
- [Crop and retouch](guide/crop-and-retouch.md)
- [Masks](guide/masks.md)
- [Scopes and color](guide/scopes-and-color.md)
- [Presets, history and several frames](guide/presets-and-history.md):
  presets, snapshots, the history, versions of a frame, sync and paste
- [Fitting your camera's look](guide/camera-match.md)
- [Merging frames](guide/merge.md): HDR, panorama and focus stack

Finishing and reference:

- [Exporting](guide/export.md)
- [Settings](guide/settings.md)

## Keys

Press **?** in the editor, or choose **Help** > **Keys (?)** at the
foot of the left pane, for these on a sheet you can search. The sheet
is made from the table the editor reads its keys from, so it always
matches the build you have. **Help** also opens this guide and the
bug form.

To change a key, open **Settings** (Ctrl+,) and choose **Keys**:
click an action's key, then press the new one, modifiers and all;
**Esc** keeps the old. The grid's stack key follows the soft proof's
(S). The frame step (← →), the way back and forward (Alt+← →), and
the turn ([ ]) are a key each way; the step's Shift form follows its
keys, and the step moved is the Loupe's: the grid and the lightbox
keep the arrows. With a brush or a repair in hand the brackets size
it wherever the turn is bound. The
other lines with more than one key (the up and down arrows, the
ratings, flags, and labels, + −, F6 to F8, Redo, and the filter's),
**Enter**, **Tab**, and **F11** cannot be moved, but an action can
take one; **Delete** or **Backspace** alone, while a key listens,
means None; a focused slider keeps its own keys. **Esc** cannot be
taken. A key that does two things says so in red; of two actions,
the higher in the list gets the press. **Reset to defaults** puts
every key back. The tables below are the defaults; the **?** sheet
shows your own.

### Moving around

| Key | Does |
|---|---|
| ← → | Previous or next frame |
| Shift+← → | Previous or next frame, added to the selection |
| ↑ ↓ | In the grid, the frame above or below (with Shift, added) |
| Home, End | The first or the last frame (with Shift, added) |
| Alt+← → | In the grid, back or forward through the folders visited |
| Ctrl+A | Select every frame shown |
| Ctrl+click | Add a frame to the selection, or take it out |
| Shift+click | Select every frame from the open one to this one (with Ctrl, added) |

In the grid, **Alt+←** and **Alt+→**, or a mouse's back and forward
buttons, go to the folders visited, as a browser's do: back returns
to the folder a subfolder was opened from, and forward goes there
again. In Cull, the mouse's buttons step the frames instead.

### Views

| Key | Does |
|---|---|
| G | Library (the grid), and back to the Loupe as it was left |
| E | The Loupe (one frame), in the mode it was left in: Cull or Develop |
| D | Develop: the Loupe with the edit panel |
| L | The lightbox, and back |
| C | Cull; in the Loupe, Cull or Develop in turn |
| V | In Cull, compare two or four frames |
| Enter | Open the frame chosen in the grid or the lightbox; from Cull, Develop |
| + − | Bigger or smaller tiles in the grid; fewer or more across the lightbox |
| Tab | Hide the panels and the filmstrip, and bring them back (in the grid, the left pane) |
| F6, F7, F8 | The filmstrip, the left pane or the develop panel alone |
| F11 | Full screen, and back (on a Mac, the window's green button) |
| Space | Fit or 1:1, where the camera focused when the raw says |
| Z | Fit or 3:1, the same way |
| F | A mat of the canvas around the picture at Fit, or none |
| J | In the Loupe, shadow and highlight clipping warnings; in the lightbox, the badges |
| Shift+J | In the Loupe, focus peaking: the sharpest parts painted over the picture |
| S | Soft proof (in the grid, a stack: below) |

**Ctrl+wheel** over the grid changes the tile size as **+** and **−**
do.

With a repair or a mask's shape in hand, **Space** or **Shift** held
with a drag moves the picture; a tap of **Space** still zooms.

### Rating and sorting

| Key | Does |
|---|---|
| 1 to 5, 0 | Set a rating, or clear it |
| P, X, U | Pick, reject, unflag |
| 6 to 9 | Red, yellow, green or blue label (again to clear) |
| [ ] | Turn the frame a quarter left or right; not part of the edit |
| B | Add the selected frames to a collection, or take them out |
| Ctrl+G | Stack the selected frames |
| Ctrl+Shift+G | Unstack them |
| S, Shift+S | In the grid, open or close a stack; make the frame its top |
| Shift+[ Shift+] | In the grid, move the frame up or down in its stack |
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
| R | Crop, and Done |
| W | White balance from a neutral patch: the pipette |
| M | New mask with a Linear, placed by the next drag |
| Shift+M | New mask with a Radial, placed by the next drag |
| [ ] | Brush size, smaller and bigger |
| Shift+[ Shift+] | Brush feather, less and more |
| Ctrl+[ Ctrl+] | Brush opacity, less and more |
| O | Show the mask; in the crop, the next guide over it |
| Shift+O | In the crop, turn the golden spiral or the triangles |
| ↑ ↓ ← →, + − | Nudge the focused slider, or its typed value with ↑ ↓; Shift on an arrow for more, Ctrl on the slider for less, and Shift on a drag for finer |
| Wheel | Nudge the focused slider, any slider with Shift; Ctrl for finer steps |
| , . | The previous or the next slider |
| Tab, Shift+Tab | In a slider's typed value, the next or the previous one |

### Files, sheets and help

| Key | Does |
|---|---|
| Ctrl+F or / | Filter the folder by rating, flag, label or words |
| Ctrl+O | Open a folder |
| Ctrl+Shift+I | Import from a card |
| Ctrl+Shift+E | Export |
| Ctrl+E | Export with the last settings |
| Ctrl+, | Settings |
| ? | The keys sheet |
| Esc | Close a sheet, put a tool down, leave the crop, let go of a set, or go back to one frame |

In the filter's text field, **Esc** clears the text and gives the keys
back to the frames.

On a Mac, Ctrl is ⌘, and the keys sheet says Cmd (⌘C, ⌘V, ⌘Z,
⌘+click). Control+click is a right-click, and delete (⌫) opens the
delete sheet.

## Known rough edges

- The default look and the tone sliders don't match Lightroom's feel
  yet. Highlights and Whites came closer in 0.7; tell us when a frame
  looks too dark, too flat or odd in a way you can name.
- The learned denoiser doesn't run on Fujifilm X-Trans raws yet, and
  their chromatic aberration correction takes out less of a fringe
  than on other raws.
- The high-quality denoisers are slow, taking tens of seconds on a
  large frame.
- On Windows the display is treated as sRGB unless you pick your
  monitor's ICC profile by hand. On a wide-gamut screen, colors may
  look too saturated until you do.
- On a Mac, double-clicking a raw or a `.gcd` in Finder launches
  greycard but does not open that file.

## Reporting a problem

Click **Report a problem...** at the bottom of the left panel, or
choose it from **Help** just above. It opens
the bug form in your browser with the version, OS and GPU already
filled in, and it shows the log file in your file manager so you can
drag it into the form. In the report, include:

1. What you did and what you expected, in a sentence each.
2. The log. The three runs before are kept as `greycard-ui-1.log`
   (the latest), `greycard-ui-2.log`, and `greycard-ui-3.log`.
3. The raw file and the camera model, if a picture renders wrong. This
   is the most useful report you can send.

If the form doesn't work, the logs are here:

| System | Log folder |
|---|---|
| Linux | `~/.local/state/greycard` |
| macOS | `~/Library/Logs/greycard` |
| Windows | `%LOCALAPPDATA%\greycard\logs` |

