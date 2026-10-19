# §376. Export a whole folder with --all (2026-10-19)

A user asked for batch export from the command line. `--export DIR`
already wrote a set, the frame opened and the `--also` rows, but the
only way to write a whole folder was to list every row by hand, which a
script has to count first and a person gets wrong.

We added `--all`: with a folder given as the path and `--export` into
a folder, every frame of the folder is written, in the folder's order,
each under its own edit and its own name, as a set from `--also` is.
The set is formed in the one place it always was, `headless::plan`,
from the same listing (`files::list_files` with the remembered
`jpegs_apart`), so `--all` writes exactly the frames `--also` could
have named; with it the rows are `0..files.len()` rather than the frame
opened plus the `--also` rows. The exit codes and the per-frame lines
are unchanged, and a frame with no version `--version-name` names still
stops the run before anything is written, as with `--also`.

The command line's overrides were laid on the frame opened alone, and
in a folder that is the frame the user last had open there, not the
first. For a set from `--also` that stays. With `--all` the set is the
whole folder, so the transient overrides (`--exposure`,
`--develop-temperature`, `--agx`), which touch no sidecar, go on every
frame of it. `--preset` is different: it writes a step into the
history of the frame it lands on, and laid on one remembered frame of
a whole-folder run it would change a frame nothing on the command line
named, saying nothing. So `--preset` with `--all` is refused, pointing
at the batch apply that already exists (`greycard presets --apply NAME
<files>`, then export with `--all`).

What is refused, with the reason: `--all` with `--also` (clap's
`conflicts_with`: one names the whole set, the other part of one);
`--all` without `--export` (clap's `requires`, as `--version-name` is);
`--all` with no path ("--all exports a folder: give its path"), rather
than taking the remembered last file's folder as a bare `--export`
does, since a run that writes a whole folder should name it; `--all`
with one file as the path ("open the folder, not a file"); `--all`
with `--export FILE`, one frame's file, worded as `--also` is refused
there; and `--all` with `--preset`. `--also` with a file opened is
refused only for a row past 0; `--all` is refused outright, since a
set of one from a flag that asks for a whole folder is more likely a
mistake than a wish.

The user guide's command-line table has a row for `--all`, the rows for
`--preset` and the transient overrides say which frame they go on, and
the sentence on giving a folder covers both set flags. The help for
`--preset` and `--agx` said "the first file", which is wrong for a
folder with a remembered file, and now says the frame opened.

Verified: a test writes three DNGs of our own into a folder and checks
that `--all` writes the same three names (`IMG_0000.jpg` to
`IMG_0002.jpg`) the `--also 1,2` run writes, both runs succeeding; a
test that clap refuses `--all` with `--also` and without `--export`; a
test that `--all` with a file opened, with `--export` to one file, and
with no path (a last file remembered) is refused with the reason; and
a test that `--all` with `--preset` is refused naming the batch apply,
and that `--all --exposure 1 --agx` puts exposure 1 and AgX on all
three frames of the plan when the remembered frame is the middle one.
`cargo test -p greycard-ui headless`, the export and `also` tests,
`cargo clippy --all-targets -p greycard-ui` and `cargo fmt --all` are
clean.
