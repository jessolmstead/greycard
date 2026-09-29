# 68. Never overwrite an export silently (2026-09-18)

An export could land on a file already there in four places, and
only one of them asked. The editor's Export goes through the
desktop's SaveFile portal: on this machine (GNOME 50)
xdg-desktop-portal-gnome delegates FileChooser to
xdg-desktop-portal-gtk 1.15.3, which calls
`gtk_file_chooser_set_do_overwrite_confirmation` with TRUE, so GTK
asks "A file named ... already exists. Do you want to replace it?",
read out of the backend's binary rather than assumed. The other
three were silent: the portal-less fallback `export_path`, which
writes NAME.greycard.jpg beside the raw; the editor's `--export`;
and the CLI's `--dng`, `--output` and `--preview`. `greycard
presets` writes sidecars, not exports, and is left alone.

The policy is one enum for both front ends. It cannot live in the
editor's export.rs: greycard-ui is a bin-only crate and greycard-cli
cannot reach into it, so `OnExists` (Increment, Overwrite, Skip) and
`Resolved` sit in `greycard-core::output`, the one crate both depend
on, and export.rs re-exports `OnExists` so the editor still names it
in one place. Increment takes the first free " (2)", " (3)"... before
the extension, counting from a stem that already ends in a number
rather than numbering it twice, so a second export of frame (2).jpg
is frame (3).jpg; with no free name in ten thousand it skips rather
than replaces.

The sheet keeps "If it exists" beside the other export choices,
Increment by default, remembered as `export_on_exists`. It governs
the paths the editor picks itself, the portal-less fallback and
`--export`. A path the user picked in the portal is taken as
confirmed and written over: the chooser has just asked them, and a
second in-app sheet would ask the same question twice. If a backend
that does not confirm turns up, that is where to add one.

The worker resolves the path before it develops, so a Skip costs
nothing, and delivers `ExportSkipped`; the status line says so
rather than falling quiet, and a headless `--export` quits on it.
The CLI takes `--on-exists increment|overwrite|skip` (default
increment) and names on stderr the path it wrote, renamed around or
skipped, per output. The editor's `--export` takes the same flag,
over the sheet's choice for that run.

Verified: unit tests in export.rs for the naming (with and without
an existing " (2)", a dotted stem, no extension, a dotfile) and the
skip decision; and both binaries run twice into a scratch directory
under each of the three policies.
