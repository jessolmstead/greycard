# §356. The file chooser out of the export (2026-10-18)

The first step of the render crate (§351). `export.rs` held two
things: making a file from a developed frame, and asking the
desktop where it goes. The second is the window's, and only the
window's: on Linux the file chooser portal over D-Bus, then zenity
and kdialog where the portal cannot be reached, everywhere else
`rfd`'s dialog awaited on Slint's event loop because AppKit and the
Windows common dialogs want the window's thread. A render crate
holding the export cannot carry zbus, rfd or Slint, so the chooser
had to leave before the export could.

It left whole: the chooser's types, its Linux and native paths and
their eleven tests, plus the percent-decoding one that sat further
down, are `chooser.rs` as they were, under a header of their own,
with the one import only they used. The eight callers name
`chooser::` instead of `export::`. The export keeps what describes a
file: the Software tag, and each format's globs, which the chooser's
tests still read to check that every format offers a filter, so
they are crate-visible now. `export.rs` names nothing of Slint, the
desktop or the target OS, which is the condition for its move; it
went from 2,998 lines to 1,952.

The test count is unchanged at 1,414, since tests only moved. The
rfd path was checked by reading: a Windows-target clippy needs a
MinGW gcc for ring that this machine lacks, so the Windows and macOS
CI builds are its first compile.
