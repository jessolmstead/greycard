# Settings

The Settings sheet holds the choices that belong to your computer
rather than to a picture. This chapter lists each one in the order the
sheet shows it, then where greycard keeps its files.

Open it with **Settings...** at the bottom of the left panel, or press
**Ctrl+,**. Close it with **Done**, Esc or a click outside. A number
you typed is taken when you press Enter or close the sheet; anything
but a whole number puts the field back as it was.

## Interface

### Scale

How large greycard draws its type and controls, on top of the scaling
your system already applies.

| Choice | Type and controls |
|---|---|
| **System** (default) | The size your system's own display scaling gives |
| **125%**, **150%**, **175%**, **200%** | That much larger than **System** |

The change applies the next time greycard starts, and the sheet says
so until then. The whole window scales, panels, sheets, and menus alike;
a picture at 1:1 still shows one pixel of the image on one pixel of the
screen. With the `SLINT_SCALE_FACTOR` environment variable set,
greycard draws at that factor instead, and the sheet says so. A
capture taken with `--snapshot` or `--screenshot` is drawn at the
system's scale whatever the setting, so it looks the same on every
machine; set `SLINT_SCALE_FACTOR` for one at another size.

## Sidecars

### Edits go

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

### Move the open frames' sidecars

Shown when frames are open. The sheet says how many of their edit
files are in the other place. Click the button, then **Move 12
sidecars** to confirm, and they all move at once.

### Write XMP sidecars

Off by default. Turn it on to share ratings, labels and keywords with
Lightroom or darktable: greycard then also writes a standard `.xmp`
beside each raw. An `.xmp` that is already there is read either way.

Picks and rejects have no XMP field, so they stay in the `.gcd`.

## RAW+JPEG

### Treat a JPEG beside its raw as a separate frame

Off by default: the camera's JPEG beside its raw (`IMG_0001.JPG`
beside `IMG_0001.CR3`) goes with the raw as one frame, marked
**+JPEG**. Turn it on and each JPEG is a frame of its own, with its
own rating and its own sidecar, and moving or deleting the raw leaves
it where it is. The open folder changes at once. See
[A raw and its camera JPEG](library.md#a-raw-and-its-camera-jpeg).

## Crop

### Aspect

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

## Thumbnails

The first line says what the cache holds, such as "312 thumbnails
kept, 2.5 MB of 300 MB."

| Item | Default | What it does |
|---|---|---|
| **Clear** | | Empties the cache, the pictures of edited frames included. Each folder's thumbnails are made again when it next opens, and an edited frame's in the background. |
| **Keep up to** | 300 MB | Room for the grid's and filmstrip's thumbnails, about thirty thousand frames at one size. When full, the ones used longest ago go. 0 turns the cache off. |
| **Previews up to** | 8192 MB | Room for local previews of the frames in your library, about 0.5 MB a frame. They let you cull a shoot on a drive that is unplugged or on a slow network. 0 keeps none. |

A folder you open again fills in at once from the cache, even after you
move or rename it.

## Network folders

### Rescan every

10 minutes by default. A library folder on a network share can't be
watched for changes, so greycard looks it over this often instead. 0
means only when greycard starts. Folders on your own drives are
watched and need no rescan.

## Presets

Says whether any of the develop presets that come with greycard are
missing from your list. **Restore** puts back the ones you removed.

## Updates

### Check for a new version at launch

On by default. At most once a day, greycard asks GitHub whether a newer
release is out, sending only its version number. If there is one, a
button in the left pane says so and opens the release page. Nothing is
downloaded or installed.

Turn the switch off to stop the check. **Check now** checks at once,
whether the switch is on or off.

## Not on this sheet

Some choices live where you use them, and are remembered from one
session to the next:

- the monitor's color profile: the **MONITOR** section on the Develop
  tab ([Scopes and color](scopes-and-color.md))
- **With subfolders**, in the left pane's folder tree
  ([Library](library.md))
- the Export sheet's choices and export presets, and the folder
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
