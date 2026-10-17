# §330. Importing presets from the window (2026-10-17)

The PRESETS section's Import... has been there since the first
release, one file at a time, its own read and save beside the command
line's. We made it the roadmap's item: a chooser that takes several
files, `.xmp` and `.gcp` together, each through the one importer the
command line uses, and the mapper's report shown once.

**One importer.** `preset::Store::import` reads the file as
`preset::import` always has (Lightroom's XMP through the mapper, ours
as JSON), and writes it into the store; `greycard presets --import` and
the window both call it. It returns the file written and the
`Imported`, the mapper's passed-over names with it, or an
`ImportError` that says what went wrong: the file would not read, it
is not a preset, it is one of ours from a newer build (its schema
named, not "not a preset"), its name is blank (refused, as a save
refuses one), the store has the name (the stored preset's name given,
as the list shows it), or the write failed (in the file system's
words, without the "sidecar:" the crate's I/O error puts before
them). The window words each of these itself; the command line prints
the error's own text, with the `--replace` hint on a name the store
has.

**Every file tried.** The command line used to stop at the first file
that would not come in. It now tries them all, as the window does,
prints each refusal where it falls, and fails at the end ("2 of 3
preset files not imported"), so one bad file in a folder's worth does
not hold back the rest.

**A name the store has is refused.** The old window import and the
command line both went through `Store::save`, which writes over a
preset of the same name. That is right for a save from the editor,
where the user is updating their own preset, and wrong for a file
brought in: the one in the list may be the user's, and a Lightroom
export named "Matte" is not necessarily a newer copy of it. So
`Store::import` takes `replace`. The window passes false: it has the
bin, and the refusal is the moment to use it. The command line has no
remove, and a script that re-imports a changed file should not have to
delete from the store folder by hand, so `presets --import` refuses
by default, says that `--replace` writes over it, and with
`--replace` writes the new one over the old in the old one's file. A
different name that slugs the same ("Warm, Film" beside "Warm
Film") takes the next free file name rather than the other preset's
file, which `save` would have written over.

**Nothing in it is not a preset.** A Lightroom preset of masks alone
maps to a preset that carries nothing. The old import kept it, a row
that does nothing when clicked; the importer now refuses it and names
what it held ("nothing in it has a place here (masks)").

**Masks in an XMP.** The mapper named a catalog's masks and spots but
never a preset file's: XMP keeps them as structures, and the reader
took only plain values and curves. A structure with something in it
is now kept as its key alone, as `read_table` keeps a catalog's
tables, so "masks", "spots" and "lens blur" are named for a preset
file too. An empty list is nothing to name.

**Shown once.** One preset in whose line fits the status line (80
characters, about what a narrow window shows) goes there, as before.
Anything else, several files, a long list of what had no place, or
any refusal, goes on a small sheet: the refusals first, in red as the
look sheets say theirs, then a line for each preset that came in with
what it passed over, and a note when anything was. OK, Enter, Escape
or a click outside closes it, and nothing of it is kept; the status
line keeps only the title. `--sheet preset-report` puts it up over
sample presets for a capture.

**Not done.** The brief asked for the imported preset to be selected.
The presets list has no selection, a click lays the preset over the
edit, so there is nothing to select it in; the list is re-read and
the new rows are in it, in name order.

**The chooser.** `export::choose_open_many` asks the portal with
`multiple` and reads every URI of the reply, and `rfd`'s `pick_files`
elsewhere; the one-file chooser and the folder chooser are the same
call taking the first. A test still opens no chooser: `ask` answers
it as a cancel, and the tests call `import_presets` with paths.
