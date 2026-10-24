# §418. The UI scale as any percentage

*2026-10-24*

A tester found 100% too small and 125% too large on one monitor. The
five steps we chose for the Scale row were a guess at what a tester
would name; the gap between two of them was a real size nobody could
reach. The row is now a number field, as the Library's Cell size is: a
whole percentage of the system's scale, taken on Enter or as the sheet
closes, held to 50 to 300. Text that is not a number puts the field
back and leaves the setting as it was. The settings file keeps
`ui_scale` as the integer it always was, so an old file opens at its
choice, and a hand-edited one out of range opens held to it. The next
start's line shows while the setting taken differs from the one the
window started at, not while a value is half typed.

The word "System" goes. It named 100%, which a number field shows as
100, and keeping it would have meant a special case in both directions
of the parse for a value the field already says plainly. Under 100% is
new: below the system's scale works the same way as above it, as the
factor is the system's times the percentage either way.

The About test, `the_about_category_says_the_version_and_the_folders`,
failed whenever the folders' paths were long, as they are under a
worktree's XDG folders, and was taken for an XDG_STATE_HOME bug. The
cause was the sheet. A path wraps onto a second line in its row, but
the About pane's preferred height counted each path row as one line:
Slint 1.18's height-for-width measures a wrapping text in a
HorizontalLayout at the row's width, not its own share of it. The pane
came out short by the extra lines, the Log row fell below the scroll
pane's content, and the test's search passes over clipped elements.
The real sheet cut the row off too, with no way to scroll to it. Each
folder row now takes its path's own preferred height as its least, and
the Log row shows under any set of folders.

The range's top is wide on purpose, and past about 210% on a 1080-line
screen the Settings sheet's least size no longer fits: the window
opens taller than the screen, with Settings still reachable to bring
the value down. The review measured it at 300 on a 1x output; we take
it, since a value that large is typed, not stumbled on.
