# 79. Importing a Lightroom catalog (2026-09-19)

The capstone of the library plan (§72) rather than a nice-to-have.
Nobody stays in Lightroom for the develop sliders; they stay for
twenty years of stars, keywords and collections, and the develop
settings are redone anyway. An import that carries the metadata over
exactly is what makes leaving possible.

**What the catalog is.** An `.lrcat` is a SQLite database whose
core tables have barely changed since Lightroom 2, so this is a
read-only query, not reverse engineering. Images, files and folders
in their own tables, the root folders with absolute paths; rating,
pick flag, color label and the virtual copy's name on the image
row; keywords in a hierarchy table joined to images; collections,
collection sets and their members; smart collections as a rule
string; develop settings as a serialized table per image, often with
the full XMP packet beside it; history steps and snapshots; stacks.
Faces, publish services and the cloud sync tables are ignored. The
catalog is locked while Lightroom runs, so the tool works on a
copy.

**Where each piece lands.** Rating, flag, label, keywords, title and
caption go to the sidecar's `meta` section, exactly: the part with
the value, and lossless. Collections and sets go to the collections
file, exactly; a smart collection translates where the query
language has the rule (rating, label, keyword, camera, lens, date)
and is otherwise skipped with a message. Develop settings go through
the mapper in `lightroom.rs` (§52), which reads the same Camera Raw
keys from an XMP file and reports what it could not carry; the
catalog's own serialization needs a small parser to reach the same
key and value pairs. The edit arrives as the approximation §52
describes, since the tone pipelines differ, so it is written as the
first history state, named for the import, for the user to see and
redo: exact metadata, approximate edits, said plainly. Virtual copies
wait on the versions item; until then the master imports and the
copies are counted and skipped. Folders become library roots. The
catalog's paths are absolute with the volume names of the machine
it lived on, so the tool asks where each root now lives, confirms by
checking a few files exist, and the content hash finds anything
moved since.

**Shape.** A command first: the catalog, the root mappings, and a
dry run that prints the report before anything is written: images
in the catalog, found, missing, with edits, virtual copies skipped,
smart collections that did not translate. Then the same behind a
sheet in the window. The dry run matters more than the sheet: the
one thing a migration must not do is half-import silently. The tool
knows which Lightroom versions it was tested against and refuses an
unknown schema with a message rather than guessing.

**Not the reverse.** Writing a catalog Lightroom will open is a
different and much larger job; the XMP export of ratings and
keywords already on the list is what anyone going the other way
needs.

Waits on the meta section, the index, roots and collections, so it
sits at the end of the library list. Once those exist, a day on the
schema, a day on the mapping and the report, a day on the path
resolution, one more on the sheet.
