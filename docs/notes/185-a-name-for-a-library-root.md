# 185. A name for a library root (2026-09-27)

The roadmap's v0.7.0 line: "A name for a library root, set from the
UI, shown wherever the root's folder name is: "Archive" for a root
whose folder is called Photos." Few drives name their folders for
what is on them. A library with two roots called `Photos`, one on each
drive, has two chips a person cannot tell apart.

**A label and nothing more.** The root is its path everywhere: the
view, the watcher, the index's rows, the chip's callbacks and the
file all go by the path, as before. The name is looked up beside it
(`Roots::name`, `Roots::label`) and used only where a root is shown.
Naming a root never moves, renames or re-indexes anything. The sheet
says so, because in any file manager "rename" on a folder means the
folder. An empty name, or one of only spaces, takes the name away,
and the root goes by its folder's name again.

**The file.** `roots.json` keeps its list as it was and gains a map
beside it:

    {"version": 1, "names": {"/x/Photos": "Archive"}, "roots": ["/x/Photos", "/x/b"]}

serde_json keeps an object's keys sorted, so the names come out by
path, and before the list.

The version stays 1. The previous build reads `roots` and ignores a
key it does not know, so a library named in this build still opens
there with every root in place. A higher version would have been
refused outright, losing the whole list for the sake of a label. What
the previous build does lose is the names, if it saves the file (an
add or a remove), since it writes only what it read. The paths
survive and the labels go. The map is written only when a root has a
name, so a library with none keeps its file byte for byte.

Items in `roots` stay plain strings rather than becoming objects with
a name inside. The previous build takes an object where it expects a
string as "not a roots file", and would set the whole file aside as
`roots.json.unreadable`. A name for a path that is not a root, or a
name that is not text, is dropped on reading and the rest is kept.

A name belongs to its root and goes with it:
- removing the root drops its name;
- a folder added above named roots takes their place without their
  names, since the new root is a different folder;
- a root removed and added again comes back unnamed.

Two roots may share a name. The path tells them apart in the menu,
the sheet and the status line. The name is set inside `Roots::edit`,
in one read and write of the file, so two editors on one library
keep each other's names as they keep each other's roots.

**Where it is set: the chip's menu alone.** A right-click on a root's
chip (Control+click on a Mac) opens a menu. The row has one
`ContextMenuArea` of its own, zero-sized and never enabled itself,
opened where the right-click was, as §171's frame menu is one for the
window. It offers the path (grayed), Rename... and Remove from
library. The press handling follows the grid's cells:
- a left press that closes the menu does nothing more on any pill it
  lands on: the root's chip, its cross, "+ Add a folder...", "+ Add
  this folder" or "All roots". Without that rule a click to dismiss
  the menu would open the chooser, add the folder open, switch the
  view or take the root out;
- a right-click on another root with the menu up opens that root's
  menu.

Rename... opens a small sheet (`ui/panel/root-sheet.slint`), titled
"Name this library folder" rather than "Rename", which reads like a
rename on disk. It shows the full path, and the name field starts
with the current name selected, so typing replaces it and Enter keeps
it. The field's placeholder is the folder's own name.

The first cut also put the name field where a root is added: both
add buttons opened the sheet, and a plain add took an extra Enter.
The review turned that down. Most roots never need a name, and the
roadmap's line is met by the chip's menu alone, so adding a root is
again a single action. The status line after an add says a name can
be given with a right-click on the chip.

Escape closes the sheet. The field holds the focus, and in Slint 1.18
a key goes only up the focused item's ancestors, so it never reached
the window's `keys` scope, where the other sheets' Escape lives. In
the first cut Escape did nothing while the field had the focus. The
card now has a `FocusScope` of its own that takes Escape. It wraps the
card and not the scrim, so a click beside the card still closes it.
The preset sheet had the same flaw, since its name field also takes
the focus when it opens, and now has the same fix. A test covers each.

**Where it shows.** The name shows on the chip, which is the only
place a root's folder name was shown; the grid's header and the filter
chips never showed a root. A long name is cut to 240 px with an
ellipsis. In the first cut a 95-character name made a 770 px chip and
pushed the other roots off a 1000 px window. The whole name is in the
sheet, and the path is in the menu's first line and the sheet, since
Slint 1.18 has no tooltip. The chip also carries the path as its
accessible description, for a screen reader. The status line says a
named root as "Archive (/x/Photos)", so a root covered or removed is
never named only by something another root may share.

`--sheet root-name` opens the sheet over the first root, for a
snapshot.
