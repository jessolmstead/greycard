# §738. Bound the Add to collection menu (2026-11-24)

Issue 203: with 392 collections at 150% scale, the frame menu's Add
to collection asked for an 18,941 px texture. The device's limit was
16,384, and opening the submenu ended the editor. femtovg draws a
menu's shadow into a texture the whole menu's size, and Slint's menus
neither scroll nor stop at the window's edge.

**The fix as contributed.** The submenu names ten destinations at
most, those added to last first, by id, so a rename keeps its place.
New collection... stays. Past ten, All collections... opens a sheet:
a field that narrows the list by name, sets and all, case aside, and
the list scrolled inside the window. Opening the sheet and changing
the recent list wait for the submenu to close.

**What we added.**

- *Every list-fed menu held to the window.* Edit in, Back up to, the
  grid header's archive button and its rejects menu, Move to group,
  and the recent folders made an item per entry too. `MenuFit` gives
  each as many rows as the window's height holds beside its own; the
  window's surface is already under the limit, so the menu is. Add to
  collection's cut is reached through All collections..., Move to
  group's through New group... (which takes an existing group's name
  as that group), and the recent folders' through opening the folder.
  Four have no other path: Edit in (the Settings sheet orders and
  removes apps, it does not launch them), Back up to and Bring back
  to, the grid header's archive button, and the rejects menu's
  archive choices. We accept that: twenty-odd apps or archives is not
  a setup we expect, and a crash was the alternative. No other menu
  in the window is fed from a list without a bound.
- *The sheet's list fast at any size.* It was a button a collection,
  and each key made a fresh model: 5,000 collections took 4 s to open
  and 7 s a key in a debug build. Rust now holds one model and narrows
  it in place, and the sheet makes only the rows in view. Opening and
  each key are now under 0.1 s of the sheet's own.
- *The keys.* As in the keys sheet, the keys stay in the field. The
  first match is lit; Up, Down, Page Up, and Page Down move the light
  and keep it in view; Return takes it; Escape closes; Tab goes round
  the field and the buttons, never the rows; text typed on a button,
  Backspace too, goes to the field (Space still presses it).
- *By id.* A pick hands back the collection's id, not its place in a
  list. One deleted before the pick, from the sheet or from the menu
  while it was up, is said in the status line by the name it was
  offered under ("... is no longer in the list"), and nothing is added.
- *The recent list lives with the library.* The settings are the
  machine's, and two libraries would share one list of ids that mean
  nothing in the other. It is `recent.txt` beside the collections,
  read afresh before each write so two windows' adds are both kept;
  a reload of the folder, as a Lightroom import does, keeps it. A
  settings file with the contributed `recent_collections` still
  loads: the list is carried once into the first library opened that
  has no file yet, and dropped from the settings once carried or once
  that library has its own; a folder that will not take the file
  leaves it in the settings, written back as read, for the next start.
  Once it is carried the field is left out. An older build reading a
  file without it shows the newest instead.
- *The fallback.* Slots no add has filled show the newest collections
  made, not the first ten by name.
