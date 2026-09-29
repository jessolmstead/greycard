# 139. The grid's own button and its own grey (2026-09-22)

**Open folder in the grid.** The button lived at the top of the left
panel, and the grid is drawn over the whole window, panels included,
so in the one view where a folder is looked at as a whole there was no
button to ask for the next one. Ctrl+O always worked there, because
the window's key handler sees it with the grid open or shut, which is
what made the gap easy to miss. The grid's header now starts with the
same button calling the same `open-folder` callback, first in the row
as the panel has it, handing focus back to the grid's keys as Cull and
Loupe do. Nothing on the Rust side changed: opening a folder replaces
`thumbs`, and the grid already lays itself out again when that
changes.

**A sheet, not a hole.** The grid painted `Theme.canvas`, the #0f0f0f
behind everything and the loupe's default surround, and under a
panel-grey header it read as a hole cut in the window. A contact sheet
is a surface worked on rather than a frame's surround, so it has its
own token, `Theme.sheet`, #232323, between the panels' #1e1e1e and a
control's #262626. It stays a dark neutral, because a lighter grey
starts to change how shadows read in a view made for judging frames
side by side, and a cell's hover and its selection still step up from
it. The loupe keeps its surround; only the grid changed. Checked in a
snapshot of the grid on eight CR3s.
