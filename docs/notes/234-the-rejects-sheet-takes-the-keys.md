# 234. The rejects sheet takes the keys (2026-10-03)

The bug §228 found: CULLING's Move rejects button kept the focus after
its click, so Return while its sheet was up asked again and Escape did
not close the sheet. Built by a sonnet author, read twice by an opus
reviewer.

**What it was.** The Move rejects sheet had no FocusScope of its own,
unlike the delete sheet and the archive rejects sheet. The button that
opened it, whose own scope takes the focus on a click, kept the keys:
Return pressed the button again, and Escape never reached the window's
handler that closes the sheet.

**Now.** The sheet holds a FocusScope that takes the focus when it is
built, as the delete sheet's does: Return is its Move (once, not on a
repeat), Escape its no, and every other key stops there. The sheet is
an `if` on `rejects-open`, so it is built again and takes the focus on
every open. The window takes the keys back when the sheet answers.

**The grid header's path.** The review found the same trap one layer
away. The header's Rejects menu (§228) ended its Move item with
`menu-done`, whose timer focuses the window a millisecond later, for a
sub-menu's sake; by then the sheet had the focus, and the timer took it
away, so Return on a sheet opened from the menu answered nothing. The
item no longer calls it, as the menu's delete and archive items never
did, and the comment that explains why now names all three.

**A sheet closed from Rust.** The `--move-rejects` driver answers the
sheet from the Rust side, which destroyed the focused sheet and left
the window with no keys. The window now takes its keys back whenever
the move, delete, archive or archive-rejects sheet goes, however it
was closed, unless another sheet is up by then: a `changed` handler
runs after the turn, so a sheet that closed in the turn another opened
would otherwise take the keys from the new one. No path does that
today; the guard and its test keep it so.

**Checked.** A test that failed before the fix opens the sheet from
CULLING's button and has Return answer once and Escape close it, the
window's keys back after; a row of the grid menu's test does the same
from the menu, failing with `menu-done` left in. The older test of
CULLING's keys expected Return swallowed while the sheet was up and
was brought to the new contract. Move back has no sheet; the archive
rejects sheet already took the focus, and has a guard test now.
