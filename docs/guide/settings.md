# Settings

The Settings sheet holds the choices that belong to your computer
rather than to a picture, in five categories down its left side:
Interface, Library, Editing, Keys, and About. Click one, or press the
up and down arrows or Ctrl+Tab, to show it; Tab goes on into its
rows. The sheet opens at the category you left it at. It also lists
the lasting choices you make elsewhere: the canvas, the monitor and
soft proof profiles, and the export's format and color space. A
change in either place shows in both. This chapter lists each choice
in the order the sheet shows it, then where greycard keeps its files.

Open it with **Settings...** at the bottom of the left panel, or press
**Ctrl+,**. Close it with **Done**, Esc or a click outside. A number
you typed is taken when you press Enter or close the sheet; anything
but a whole number puts the field back as it was.

## Interface

### Scale

How large greycard draws its type and controls, as a percentage of
the scaling your system already applies: 100 (the default) is the
size your system's own display scaling gives, 150 is half again as
large, and 75 is smaller. Type a whole number from 50 to 300; it is
taken on Enter or when the sheet closes, and anything else puts the
field back.

The change applies the next time greycard starts, and the sheet says
so until then. The whole window scales, panels, sheets, and menus alike;
a picture at 1:1 still shows one pixel of the image on one pixel of the
screen. With the `SLINT_SCALE_FACTOR` environment variable set,
greycard draws at that factor instead, and the sheet says so. A
capture taken with `--snapshot` or `--screenshot` is drawn at the
system's scale whatever the setting, so it looks the same on every
machine; set `SLINT_SCALE_FACTOR` for one at another size.

### Theme

The window's panels and text in **Charcoal** (default), a mid
**Grey**, or **Light**, changed at once; the blue accent is the same
in all three. The picture's surround keeps its own **Canvas**
setting, so the theme never changes what a picture is judged against.

### Scroll bars

**Fade** (default) draws a list's or a panel's bar as a thin thumb
and lets the grid's and the filmstrip's fade away a moment after you
stop scrolling. **Stay** keeps every bar drawn, its track too, so a
panel that holds more below always shows it. Either way a bar widens
under the pointer.

### Scroll bar size

How thick every scroll bar is: **Thin** (default), **Medium**, or
**Wide**, changed at once. A wider bar is easier to see and to grab;
the filmstrip grows a few pixels taller to keep its bar off the
frames.

### One section open at a time

Off by default. On, opening a section of the develop panel folds every
other section of its tab, so the one you are working in has the room.
The Develop, Crop, Masks, Retouch, and Cull tabs each fold their own,
but a section on both Develop and Masks (Light, Color, Curves, Mixer,
Grading, Tint, and Detail) is folded or open on both at once. The left
pane's sections are not touched. Turning it on folds nothing until the
next section opens.

## Library

### Start in

Where greycard opens when you start it without naming a folder.

| Choice | Opens |
|---|---|
| **Last folder** (default) | The folder of the frame you last had open |
| **Chosen folder** | The folder you pick in the chooser it opens, shown under the row |

Click **Chosen folder** again to pick another. If that folder is gone,
say on a drive that is unplugged, or holds no raws, greycard opens the
last folder instead.

### Show the filter bar

On by default. Turn it off and the filter bar leaves the grid's header
and the Cull tab, and the grid takes its room. A filter you set stays
on, so the grid may show fewer frames than the folder holds; turn the
bar back on to see or clear it. **Sort** moves to the grid's top row
while the bar is off.

### Cell size

The grid's cell, in pixels, from 96 to 512; 176 by default. Type a
size and press Enter, or close the sheet. **Ctrl+wheel**, **+** and
**−** change it too, and the row shows where they left it.

### Resize with the wheel

On by default. Turn it off and **Ctrl+wheel** scrolls the grid and the
lightbox as the wheel alone does, never resizing them. **+** and **−**
still resize.

### Sidecars

#### Edits go

Where a frame's edit file (its `.gcd`) is written.

| Choice | Where |
|---|---|
| **Beside the raw** (default) | `IMG_0001.CR3.gcd` next to `IMG_0001.CR3` |
| **Hidden folder** | a hidden `.greycard` folder inside the shoot's folder |

Both travel with the shoot when you copy the folder. greycard reads an
edit from either place, so changing this loses nothing. An edit file
saved by greycard 0.8 can't be read by 0.7 or earlier (see
[Where your edits live](library.md#where-your-edits-live)).

Changing it moves nothing by itself. Each frame's edit moves to the
new place the next time it is saved, so a folder tidies itself as you
work through it.

#### Move the open frames' sidecars

Shown when frames are open. The sheet says how many of their edit
files are in the other place. Click the button, then **Move 12
sidecars** to confirm, and they all move at once.

#### Write XMP sidecars

Off by default. Turn it on to share ratings, labels and keywords with
Lightroom or darktable: greycard then also writes a standard `.xmp`
beside each raw. An `.xmp` that is already there is read either way.

Picks and rejects have no XMP field, so they stay in the `.gcd`.

### RAW+JPEG

#### Treat a JPEG beside its raw as a separate frame

Off by default: the camera's JPEG beside its raw (`IMG_0001.JPG`
beside `IMG_0001.CR3`) goes with the raw as one frame, marked
**+JPEG**. Turn it on and each JPEG is a frame of its own, with its
own rating and its own sidecar, and moving or deleting the raw leaves
it where it is. The open folder changes at once. See
[A raw and its camera JPEG](library.md#a-raw-and-its-camera-jpeg).

### Thumbnails

The first line says what the cache holds, such as "312 thumbnails
kept, 2.5 MB of 300 MB."

| Item | Default | What it does |
|---|---|---|
| **Clear** | | Empties the cache, the pictures of edited frames included. Each folder's thumbnails are made again when it next opens, and an edited frame's in the background. |
| **Keep up to** | 300 MB | Room for the grid's and filmstrip's thumbnails, about thirty thousand frames at one size. When full, the ones used longest ago go. 0 turns the cache off. |
| **Previews up to** | 8192 MB | Room for local previews of the frames in your library, about 0.5 MB a frame. They let you cull a shoot on a drive that is unplugged or on a slow network. 0 keeps none. |

A folder you open again fills in at once from the cache, even after you
move or rename it.

### Network folders

#### Rescan every

10 minutes by default. A library folder on a network share can't be
watched for changes, so greycard looks it over this often instead. 0
means only when greycard starts. Folders on your own drives are
watched and need no rescan.

## Editing

### Canvas

The surround behind the picture, **Black**, **Dark grey**, **Mid
grey**, or **Light grey**: the same choice as the swatches beside
**Canvas** in the Develop tab's **MONITOR** section.

### Monitor

The monitor's color profile, the same choice as the **MONITOR**
section's, with a chosen file's path and **Choose...** under the row
when it is **File**.

### Soft proof

The profile and the intent, **Perceptual** or **Relative**, the soft
proof shows the picture through: the same choices as the **SOFT
PROOF** section's, which still turns the proof on and off.

### Crop

#### Aspect

The aspect a frame's crop opens at when the frame has no crop of its
own: as you press **Crop**, **Draw**, **Level**, **Verticals** or
**Horizontals** on the Crop tab, or move to another frame while you
crop.

| Choice | The crop opens at |
|---|---|
| **Original** (default) | The frame's own shape, so dragging a corner keeps it |
| **Last used** | The aspect you last picked from the **Aspect** list, on any frame, this session or an earlier one |
| **Free** | Any shape |

Opening the crop changes nothing in the frame's edit until you drag,
draw or pick an aspect, with one exception: under **Last used** at a
ratio, pressing **Crop** crops the frame to it at once. A frame you
move to while cropping is not cropped until you touch its crop. A
frame you have already cropped keeps its crop and its aspect. See
[Cropping](crop-and-retouch.md#cropping).

### Presets

Says whether any of the develop presets that come with greycard are
missing from your list. **Restore** puts back the ones you removed.

### Export

#### Format

The format an export is written in, the same choice as the Export
sheet's **Format** row.

#### Color

The color space an export is written in, the same choice as the Export
sheet's **Color** row.

## Keys

Every key you can move, under the heading of its group: click an
action's key, then press the new one. See
[Keys](../user-guide.md#keys).

## About

The version you are running, then the check for a newer one, then
the folders greycard keeps its files in (see
[Where greycard keeps its files](#where-greycard-keeps-its-files)).

### Updates

#### Check for a new version at launch

On by default. At most once a day, greycard asks GitHub whether a newer
release is out, sending only its version number. If there is one, a
button in the left pane says so and opens the release page. Nothing is
downloaded or installed.

Turn the switch off to stop the check. **Check now** checks at once,
whether the switch is on or off.

## Not on this sheet

Some choices live where you use them, and are remembered from one
session to the next:

- **With subfolders**, in the left pane's folder tree
  ([Library](library.md))
- the Export sheet's other choices and export presets, and the folder
  exports go to ([Exporting](export.md))
- how many frames go across the lightbox
  ([Culling](culling.md#the-lightbox))
- the Loupe's mode, Cull or Develop, and Cull's **Picture** switch
  ([Culling mode](culling.md#culling-mode))
- the grid's sort order, for each kind of view
  ([Sort order](library.md#sort-order))

## Where greycard keeps its files

Your edits are beside your raws (see [Edits go](#edits-go)). Everything
else is in these folders. Deleting a cache folder costs only time:
greycard makes it again, and downloads models again when needed.

| What | Linux | macOS | Windows |
|---|---|---|---|
| Settings, export presets, the export queue | `~/.config/greycard` | `~/Library/Application Support/greycard` | `%APPDATA%\greycard` |
| Develop presets | `~/.config/greycard/presets` | `~/Library/Application Support/greycard/presets` | `%APPDATA%\greycard\presets` |
| Library index and its list of folders | `~/.local/share/greycard` | `~/Library/Application Support/greycard` | `%LOCALAPPDATA%\greycard` |
| Collections | `~/.local/share/greycard/collections` | `~/Library/Application Support/greycard/collections` | `%LOCALAPPDATA%\greycard\collections` |
| Fitted looks | `~/.local/share/greycard/looks` | `~/Library/Application Support/greycard/looks` | `%APPDATA%\greycard\looks` |
| Camera profiles (`.dcp`) | `~/.local/share/greycard/profiles` | `~/Library/Application Support/greycard/profiles` | `%APPDATA%\greycard\profiles` |
| Thumbnails and local previews (cache) | `~/.cache/greycard/thumbs` | `~/Library/Caches/greycard/thumbs` | `%LOCALAPPDATA%\greycard\thumbs` |
| Downloaded models (cache) | `~/.cache/greycard/models` | `~/Library/Caches/greycard/models` | `%LOCALAPPDATA%\greycard\models` |
| Saved masks and denoiser results (cache) | `~/.cache/greycard/masks`, `~/.cache/greycard/denoise` | `~/Library/Caches/greycard/masks`, `.../denoise` | `%LOCALAPPDATA%\greycard\masks`, `...\denoise` |
| Lens database (cache) | `~/.cache/greycard/lensfun` | `~/Library/Caches/greycard/lensfun` | `%LOCALAPPDATA%\greycard\lensfun` |
| Logs | `~/.local/state/greycard` | `~/Library/Logs/greycard` | `%LOCALAPPDATA%\greycard\logs` |

The settings file is `settings.json`, the export queue
`export-queue.json` and the library index `library.sqlite`. The
current log is `greycard-ui.log`, and the previous run's is kept as
`greycard-ui.log.1`.

The library index is kept out of the cache, because rebuilding a large
library takes a long time.

On Linux, and on any system where they are set, the standard
`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` and
`XDG_STATE_HOME` variables move these folders.
