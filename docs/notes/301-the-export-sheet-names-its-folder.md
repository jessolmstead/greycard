# §301. The export sheet names its folder (2026-10-11)

Issue #22: a tester chose a folder for an export and was asked again at
the next one, and the next. The chooser did remember its last answer
(§274), but only as the place it opened on: every Export with the
Subfolder field empty still asked, a save dialog for the frame on
screen and a folder chooser for a set. The button that did so read
"Choose file...", which says nothing of exporting.

## A Folder row, and Export writes there

The sheet now has a Folder row above its buttons: the folder Export
will write to, with a Choose... button beside it. Export reads
"Export" for one frame and for a set alike, and writes into that
folder with no dialog. The chooser is reached only through the row's
button; an answer is the new folder, shown at once and written to the
settings file at once, as a preset is.

**What the row shows.** With the Subfolder field typed, "export beside
each original": the field still wins, as §242 and §274 have it, so a
preset that names a subfolder keeps its own place. With the field
empty, the folder last chosen (`Settings::export_last_folder`, which
§274 added); with none chosen yet, the folder of the frame on screen,
which is where the chooser opened before anything was chosen. We did
not invent an `export` subfolder as the first default: the Subfolder
field is how a user asks for one, and a default that typed it for them
would make every preset read as edited.

**One function decides.** `sheet_folder` is the row's folder, the
export's, the queue's, and where the chooser opens; the row is made
from it through the same reading of the Subfolder field the export
goes by (`Sheet::subfolder_shown`), so the row cannot name a place the
export does not write to. A path too long for the row is cut at the
front, so the folder's own name shows; the text also elides what the
system font still makes too wide.

**Choose with a subfolder typed.** A folder chosen clears the field,
since the field would otherwise still win and the choice would do
nothing; the preset reads as edited, which is true.

**Why the footer.** The row sits in the sheet's fixed footer, above
Cancel and Add to queue, not at the top of the choices. It is what the
Export button acts on, so it belongs beside it, and on a short window
(§298) the choices scroll while the footer stays: a row among the
choices could be scrolled out of view as Export was pressed.

## One frame goes as a set of one

The frame on screen no longer has a path of its own. It goes as a set
of one into the folder, as it already did into a subfolder (§242), and
the worker exports it from the open picture as it did. Its name is the
frame's own stem in the format's extension, the name the save dialog
used to propose. When a file of that name is there, the sheet's If it
exists choice holds, as for a set: Increment writes `X-1.jpg` (§262),
or `X.greycard-1.jpg` in the frame's own folder (below), Skip writes
nothing, and Overwrite asks first on the overwrite sheet, which names
the file. Before, a file named in the save dialog was confirmed by the
dialog and always overwritten.

**The frame's own folder is beside it.** With nothing chosen yet the
row's folder is the frame's own, and a raw's `X.jpg` there would be the
camera's `X.JPG` on a case-blind file system (macOS, Windows): under
Overwrite the sheet would offer to write over the camera's original
with one click. A chosen folder that is the frame's own therefore names
files as no folder does, `X.greycard.jpg`, in every format and under
every policy (`queue::named`); any other folder takes the frame's own
name. Whether the chosen folder is a frame's own asks the file system,
so it is asked once for each folder of the set rather than each frame.

**The line still names the file.** A set ends on its count ("exported
1 frame into out in 1.2 s"), which drops what the frame on screen's own
line always said. A set of one now ends on that line: the file's name,
the rename Increment made ("X.jpg was there already: wrote X-1.jpg"),
and what the edit went without; under Skip, "X.jpg is there already:
nothing exported" rather than a count of nothing. With Increment the
sheet's default, a quiet rename is the likely case, and the line is
where it shows.

## A folder that cannot take the files

The row's folder is looked at when Export is pressed, on a thread of
its own (a share may keep it waiting): it must be there, be a folder,
and take a file, which we find by making one and removing it, since
permission bits do not say what a share or a read-only mount allows.
When it fails, the status line says why ("/Volumes/Card/out is not
there") and the chooser opens on it; an answer exports there and is
the folder from then on, a cancel exports nothing and says why. The
folder is never made: a drive unplugged since it was chosen must not
get a folder under its empty mount point (§242's rule for a chosen
folder). The probe's name carries the process and the moment; one
taken already is passed over for a second, never read as a folder that
takes files.

**A look that never answers holds nothing up.** A share that hangs
would keep the look's thread waiting for ever, and if Export waited on
it, Export, Add to queue and Choose would be dead for the session.
The look holds no flag; each one has a number, and an answer whose
number is not the latest (another Export since, or a Choose) is
dropped. Choose opens while a look is out, and overtakes it: its line
says "the export waiting on X was not started", and Export is pressed
again for the folder chosen. A look that answers with another export
under way since (the queue's) starts nothing and says "not exported:
another export is under way".

**The row follows the frame.** It is made from the frame's name and
its row, so two frames of one name in two folders (a tree with a
subfolder for each card) show their two folders.

## Add to queue

Add to queue takes the row's folder too, with no chooser, and no
longer has an ellipsis. Whether the folder is there is still looked at
when the queue runs, which stops on one that is not (§213); it is not
checked for writing at queue time.

Open: the single-frame job (`Job::Export`) lives on only for the
headless parity test (and the history tests that hand its outcome in);
the window sends every export as a set. Moving that test onto
`ExportSet` and deleting the variant, its worker arm and its queue and
blame arms (about 55 lines in the worker) is a follow-up. The line
under the Subfolder field ("Saved in export beside each original.")
repeats what the row says when a subfolder is typed; it is kept for its
warnings and because it sits by the field it explains.
