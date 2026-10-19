# §370. The Lightroom catalog sheet in a sheet's shape (2026-10-18)

The import sheet shared the report builder with the command and
swapped a few words a line, so what the user saw was the command's
output in a dialog: a heading sentence, each root with its catalog
path, its name, where it was found and how many of its files, a
two-line caption under the Set field, and then the report's key and
value lines, catalog, schema, log, roots again, counts of every kind.
The first catalog the editor's own user brought in read as wordy, and
it was the first thing a Lightroom user would meet.

The sheet now has words of its own, built from the report by one
function that gives a warning, a summary, a line for what a write
did, and the details, and the window lays those out. One line a
root: its folder's name, a state in a few words, "found, 5 of 400
checked", "found", "found, no files in the catalog", "not found",
"not found; nearest: …", "not found in …", and Choose..., with the
folder taken under the name, and the catalog's path as well when two
roots share a name. The field is Collection set, its caption gone to the
guide. The report is one summary of two or three sentences, "1,240
frames, 1,198 found. 312 with edits. 13 collections, 3 stacks, 7
virtual copies.", each part only when its count is above zero, and
a Details fold holding everything else the report knew, each line
only when it applies. After a write, one line, "Wrote 1,198
sidecars; 42 frames not found.", with "30 written over" on a forced
write, and the same fold. The one thing that can lose work, a
catalog copied without its -wal, stays visible above the summary on
both steps as one sentence and one line of what to do. The command's
output is unchanged.
