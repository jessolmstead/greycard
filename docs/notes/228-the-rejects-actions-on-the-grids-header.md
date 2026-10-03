# 228. The rejects' actions on the grid's header (2026-10-03)

The roadmap tweak: Move rejects, Delete rejects folder and the archive's
Remove rejects should be at hand on the grid screen too. They lived in
the CULLING section alone, and the grid is drawn over both panels, so
from the grid there was no way to them short of going back to the loupe.
§218 had left the grid header without them on purpose; this reverses
that.

**Where.** A Rejects button in the grid's header, between the archive's
Back up / Bring back and Cull, opening a menu, the way the archive's
button opens one when it has more than one choice. Its words carry the
count ("2 rejects", "1 reject", plain "Rejects" with none), so the
header says how many are waiting without being opened. The menu holds,
in CULLING's order: Move N rejects... (grayed as "No rejects to move"
when none are flagged), Move back to <folder> (only while some of the
selection is in a rejects folder), Delete rejects folder..., and one
Remove rejects from <archive>... per archive (none with no archive).

**Why the header and not the frame menu.** The frame menu is about the
selection: what a right-click over a frame offers on the frames chosen.
Move back belongs there, and has been since §225, because it acts on
the chosen frames in a rejects folder. Move rejects, Delete rejects
folder and Remove rejects act on the folder: every flagged frame, many
off screen, whatever is selected. Putting them on a frame's menu would
say they act on that frame. The header is where the grid's folder-level
action already is (the archive's button), so they sit beside it. One
button with a menu rather than three or four buttons, so the header's
first row keeps its width on a narrow window; even so, with an archive
button, a count and Cull all present the row clips under about 870
pixels where it clipped under about 755 before, and the window has no
minimum width. Move back is in the menu as well as on the frame menu,
as CULLING has it under Move rejects: the menu is the whole of
CULLING's ways out, in one place.

**One implementation.** The grid forwards each item to the window's own
callback, the very one the section's button calls (`rejects-asked`,
`move-back-asked`, `delete-asked("rejects")`, `archive-rejects-asked`),
and reads the same properties for its words (`reject-count`,
`back-count`, `back-to`, `archive-rejects-choices`). Nothing new on the
Rust side: the same sheets open, behind the same checks, and Delete
rejects folder with no rejects folder says so in the status line as it
does from CULLING. No key is taken.

**The focus, and why two items do not hand it back.** Move rejects and
Move back end with `menu-done`, as the frame menu's items do. Delete
rejects folder and Remove rejects do not. Their sheets take the focus
in `init`, and `menu-done` starts the 1 ms refocus timer (§208), which
then found the window's keys without the focus and took it back from
the sheet: Return on the sheet answered nothing, though Escape, which
the window handles, still did. The timer is for a pick from a sub-menu,
where two popups close at once and Slint does not give the focus back;
this menu has no sub-menu, so Slint gives it back itself, and the
sheet then takes it. None of the frame menu's items opens a sheet that
takes the focus, which is why it had not come up. The review found a
neighbor of it that was there before, in CULLING itself: the Move
rejects button keeps the focus after its click, so Return while the
sheet is up asks again and Escape does not close it. That is on the
roadmap as a bug; the sheet wants a FocusScope of its own, as the
delete sheet has.

**What the header's actions say.** An action's answer goes to the
status line ("there is no rejects folder at ...", "open the folder
first", Move back's "1 left where it was ..."), and the status plate
is the viewport's, which the grid covers: from the grid's menu a click
that only said something looked like nothing. The header now shows the
status line after the selection's name, elided, as the import's line
is shown when the left pane is away; the same property, nothing new
held. It shows whatever the line says, a decode in progress among it,
as the plate does in the loupe; the line never clears on its own, so
the header carries the loupe frame's develop chatter too. Measured, it
takes width from the selection's name only: the count, Rejects, Cull
and Loupe sit at the same place with no status and with a long one.

**The sheet's line.** Move rejects' sheet said there was no undo and a
frame came back by hand. Move back is the undo now, offered once the
rejects folder is open and the frame chosen, so the sheet says that:
"Nothing is deleted. To bring a frame back, open the rejects folder and
use Move back."

**Tests.** Three, in `panel::cull`: each item, picked from the opened
menu, calls the same window callback as the section's button does with
the same counts; the menu grays Move rejects with none, leaves Move
back out with none of the selection in a rejects folder, and keeps
Delete rejects folder; and Delete rejects folder and Remove rejects,
picked from the grid's menu and from CULLING, let the 1 ms timer pass
and are answered by Return either way, which fails with `menu-done`
put back on the Delete item. The menu's items are picked with the
arrow keys and Return, not clicked: the testing backend reports a
popup's items in the popup's own coordinates, so a click at them would
land on whatever of the window is under that point.
