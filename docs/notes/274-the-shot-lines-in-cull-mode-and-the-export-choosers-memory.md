# 274. The shot lines in cull mode, and the export chooser's memory (2026-10-10)

Two small items from the roadmap, built together.

## The shot lines in cull mode

The edit panel's head shows, under the file's name, the body and lens and
then the exposure triangle. Culling blanked them: a frame under the loupe
is never developed, and the develop is what carries the file's shot
summary to the panel. We did not open the raw to read it, since stepping
through a shoot at the speed of the arrow keys is what the mode is for.

The library index already holds every frame's camera, lens, focal length,
aperture, shutter and ISO, so culling asks it. `shot_text` reads the row
by the id the window already holds for the frame (`State::index_ids`),
through the new `Library::by_id`, so the window's thread asks the disk
nothing: `by_path` canonicalizes the folder, and on a hung network mount
that would stall every arrow press. A frame under an offline root is not
looked up at all and reads empty. The row's `Exif::shot()` and the same
`Shot::summary` the edit mode uses make the words, so the two modes read
alike.

The lines are set where the focus lands (`cull_select`, which compare mode
also goes through for its focused frame), and again whenever the index
answers (`reread`), because a folder opened for the first time is culled
before its pass has reached the frame. A frame the index does not know
reads empty rather than wrong; an index that could not be asked just now
(busy, or no reader yet) is not an answer, so the refresh leaves the lines
that are up. The size line stays empty: it is the develop's and there is
none.

In compare we show the focused frame's lines, not one set per compared
frame; the panel head has room for one, and the focus is the frame the keys
act on. The edit panel's head carries no date, so culling shows none
either, which keeps the two modes the same.

## Where the export chooser opens

Nothing was lost on the way to the settings file: the sheet, its subfolder
and its presets were saved and restored all along. What was never kept was
the answer of the folder chooser. Every chooser opened on the first
frame's own folder, and the folder it was answered with was used once and
dropped.

`Settings::export_last_folder` keeps it (named so it does not clash with
`startup::export_folder`, which headless uses for another question). It
sits outside the `Sheet` on purpose: a preset is a sheet, and a preset that
carried a chooser's last answer would fix a place the user never chose for
it. A preset that names a subfolder never asks, so its location wins
whenever it is picked; a preset that asks opens the chooser where the last
export went. The folder is written to the file as the chooser answers, as
presets are, and at close it is taken from the state with the presets
(`take_state_choices`), since the panel's own settings are rebuilt there
and would otherwise write it empty. The set chooser, the single-file
chooser (the file's own name, in the remembered folder) and the queue's
chooser all start from it and all write to it.

The window's thread never checks that the folder is still there. The check
runs where the chooser is asked, on the thread of its own (`nearest_folder`
in `export::ask`), and opens on the nearest parent that exists. A folder
whose mount was removed therefore falls back up the path; a mount point
left behind with nothing mounted on it is still a folder and passes, so the
chooser opens on the empty directory. On the platforms that ask through the
native dialog the dialog's own handling of a missing start folder applies.
