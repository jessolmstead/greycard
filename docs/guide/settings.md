# Settings

The Settings sheet holds the choices that belong to your computer
rather than to a picture. This chapter lists each one in the order the
sheet shows it, then where greycard keeps its files.

Open it with **Settings...** at the bottom of the left panel, or press
**Ctrl+,**. Close it with **Done**, Esc or a click outside. A number
you typed is taken when you press Enter or close the sheet; anything
but a whole number puts the field back as it was.

## Sidecars

### Edits go

Where a frame's edit file (its `.gcd`) is written.

| Choice | Where |
|---|---|
| **Beside the raw** (default) | `IMG_0001.CR3.gcd` next to `IMG_0001.CR3` |
| **Hidden folder** | a hidden `.greycard` folder inside the shoot's folder |

Both travel with the shoot when you copy the folder. greycard reads an
edit from either place, so changing this loses nothing.

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

## Thumbnails

The first line says what the cache holds, such as "312 thumbnails
kept, 2.5 MB of 300 MB."

| Item | Default | What it does |
|---|---|---|
| **Clear** | | Empties the cache. Each folder's thumbnails are made again when it next opens. |
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
- the Export sheet's choices and export presets
  ([Exporting](export.md))

## Where greycard keeps its files

Your edits are beside your raws (see [Edits go](#edits-go)). Everything
else is in these folders. Deleting a cache folder costs only time:
greycard makes it again, and downloads models again when needed.

| What | Linux | macOS | Windows |
|---|---|---|---|
| Settings, export presets, the export queue | `~/.config/greycard` | `~/Library/Application Support/greycard` | `%APPDATA%\greycard` |
| Develop presets | `~/.config/greycard/presets` | `~/Library/Application Support/greycard/presets` | `%APPDATA%\greycard\presets` |
| Library index and its list of folders | `~/.local/share/greycard` | `~/Library/Application Support/greycard` | `%LOCALAPPDATA%\greycard` |
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
