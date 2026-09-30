# 206. The open folder named, and Recently opened (2026-09-29)

Roadmap line (Next 0.4.0): "The open folder named, and a Recently
opened list: the folder's name under the Open folder button (its path
as the hover text, the root's name when it is under one), and a list
of the last ten folders opened in settings.json beside `last_file`,
offered under the button and in the grid's header, with the one open
marked; a folder gone from the disk stays listed and says so when
chosen."

**What it was.** Nothing in the window said which folder was open. The
grid's header named the frame selected and the strip named each file,
but the folder itself was only in the title of the chooser that
picked it; a second folder open meant the chooser again, walked from
wherever the last one was. The settings file kept `last_file` and
nothing of the folders before it.

**What it is now.** Under the left pane's Open folder button, in small
type, the folder's own name; after it, muted, "in Archive" when the
folder is under a root, by the root's label as its chip has it; the
root itself goes by that label alone. The full path is the hover text,
after the pointer rests on the name for 600 ms. A chevron says the
name opens something: a click drops Recently opened below it, the last
ten folders, the one open checked. The same name sits beside the grid
header's Open folder button, as that button already stood in for the
left pane's. With nothing open and a list to offer (the empty editor,
its chooser canceled) the name reads "Recently opened", so the way
back to where one was is there before anything is open. A view of the
roots names the root, or "All roots"; it is not a folder and is not
recorded.

Each item is the folder's name and, after a dash, the root it is under
or else the folder it is in (with `~` for home), so two shoots both
called `raw` are told apart without the list growing to full paths.

**One place records.** Every list the browser takes goes through
`browser::open_loaded`: the Open folder button and Ctrl+O, a folder
chosen from the list, a root's view, the import's destination opening
after it, the desktop's open of files. `recent::opened` is called
there, once, after the view is set; it works out whether the list is
a folder at all (every file in the one directory, which a single file
opened by name also is) and records it. The launch's folder, the
command line's or the last file's, is built before the window runs and
never passes through `open_loaded`, so startup calls the same function
once. Nothing else records, so a way of opening a folder added later
is in the list without being told. The name depends on the roots too,
so `roots::show`, which runs whenever they change, redraws it: a root
renamed or added renames the name under the button at once.

**Settings.** `recent_folders: Vec<String>` beside `last_file`,
canonical paths, most recent first. `settings::push_recent` is the
rule: take the folder out wherever it is, put it at the front, cut to
ten. A file from before the list has none, and `#[serde(default)]`
already gives it an empty one. Like `last_file` it is written as it
happens, not gathered when the window closes: the close keeps the
file's list rather than the panel's, and the write goes through the
file as it is on disk, so a second window's folders are merged rather
than written over. The gate is `State::settings_file`, as the export
presets' is: a capture, a batch export and a test have none and keep
the list in the window only. The window's list is one model for the
session, its rows replaced only when they change, since `roots::show`
runs on every watcher change and index report and a menu may be open
over it. A launch that names a folder and the roots' view together
records nothing for the folder, which the view replaces at once.

**A folder gone.** Chosen, a folder no longer on the disk (a card out,
a drive unplugged, a folder moved) says "~/…/Harbor is not there any
more" on the status line, as the other failures to open do, and also
in place of the name under the button until the next folder opens: the
grid covers the status plate, and the grid's header is where the list
was offered from. The list is left as it was. Taking the entry out
would lose a drive's folders every time the drive was out, and the
next ten folders opened push it off the end anyway. It is checked
when chosen and not when the list is drawn, and on a thread of its own,
as a root to add is (`Roots::checked`): a stat of a path is cheap until
it is on a network mount that has stopped answering, and a dead share
is exactly what this list puts one click away. The folder is opened,
or said to be gone, back on the event loop. The browser's view generation is
taken when the folder is chosen, and a look that comes back after it
has moved on (another folder opened, a view of the roots, the same
folder chosen twice while the first look hangs) is dropped rather
than opened, or said to be gone, over what the user went to
meanwhile.

**The hover text.** Slint 1.18 has no tooltip, so the path is an
overlay of the window's own, set by the name after the pointer rests
on it. A name destroyed under the pointer (the grid closed, the pane
put away) never hears the pointer leave, so the window takes the text
down itself whenever the grid or the left pane comes or goes. None is
put up, or shown, while a sheet is over the window, which a key can
open while the pointer rests on the name. The list
is not opened over a sheet, as the frame menu is not.

**Left alone.** How a folder is read once chosen is `open_folder`'s,
unchanged here; the list only calls it. No keyboard shortcut for the
list, and no way to clear it or take one entry out: neither was asked
for, and the cap does the forgetting.

**Tests.** The settings round trip carries the list, and a file
without the key reads as an empty one
(`the_settings_round_trip`,
`a_file_from_another_version_keeps_what_it_knows`); the cap, the
dedupe and the move to the front
(`a_folder_opened_again_moves_to_the_front_and_the_list_keeps_ten`).
In `panel::recent`, on the headless window: two folders opened are
named and listed newest first with the open one marked, and choosing
the older brings it to the front; a folder under a named root carries
the root's name, and the root itself goes by it alone; a folder
removed from the disk and then chosen says so on the status line and
under the button, the list and the files open untouched, and the next
open clears the note; files from two folders are no folder; a click on
the name, found by its accessible label, opens the list, the press
that closes it opens nothing, and with the grid up the name is in its
header too; the hover text over the header's name goes when the grid
closes under the pointer (checked to fail without the fix); a folder
opened with a settings file to write goes through the file as it is,
keeping a folder another window wrote meanwhile and the rest of the
file.
