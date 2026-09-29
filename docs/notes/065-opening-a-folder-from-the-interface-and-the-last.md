# 65. Opening a folder from the interface, and the last photo on launch (2026-09-18)

`Cli.path` was a required file or directory. Two roadmap items wanted
a way in without it: a folder chooser reachable from the window, and
picking up where the last session left off.

The portal's `OpenFile` with its `directory: true` option, rather
than a folder browser of our own: the app already speaks to
`org.freedesktop.portal.FileChooser` for export and for importing
presets, and a folder chooser is the same call with one more option
in the dict and no filter; it returns a single directory URI the way
a file chooser returns a file, so `choose_folder` sits beside
`choose_open` and shares `dialog()` with it. "Open folder..." at the
top of the left panel, above NAVIGATOR, since it is what one reaches
for before there is a picture to navigate or export; Ctrl+O does the
same.

The last file open is written to the settings as soon as it develops,
not gathered from the panel at window close as everything else in
the file is: a develop is the one point the file is certainly open,
and an eager write means a killed session still remembers, at the
cost of a small write on every file switch. Skipped under
`--screenshot`, `--snapshot` and `--export`, the rule those already
follow; a diff of the settings file before and after such a run shows
it untouched. The window-close save reloads the field from disk
rather than writing an empty one over it.

Opening a folder reuses the launch scan and, before swapping the
list in, does what `on_select` does on leaving a file (save its edit,
hold its picture on screen), so the old file's sidecar cannot land in
the new folder's array; `st.current` is cleared before the swap and
the new index selected after it. With no path on launch: the last
file's folder if the file still exists and the folder has files, else
the chooser once the event loop runs, else the empty editor, which
every call site already guarded against from when a single file
could fail to decode. A unit test covers the matching by canonical
path and both fallbacks; the chooser itself cannot be driven
headlessly.

Found in review: a thumbnail is delivered by index, and a slow one
from the folder just left would have landed on the new folder's
slot of the same number. The outcome now carries the path it was
made from, and the delivery is dropped unless that is still the
file at that index.
