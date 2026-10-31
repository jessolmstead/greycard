# §459. A start folder of folders

*2026-10-30.* The Start in folder of §403 was listed from the disk,
its own frames, and a folder with none fell back to the last folder:
the design assumed the chosen folder would hold the frames itself.
The first tester to use it chose a root organized into folders by
year and shoot, and every launch opened the last folder instead
(issue 58). Now a start folder that exists with no frames of its own
opens at launch as its row in the folder tree does, when it is a
root or lies under one: the library's view of it, its folders as
tiles with the status line's hint, or with With subfolders on the
frames under it. The launch carries that view the way `--all-roots`
carries the roots' view, opened once the index reports open, so the
roots file is read before the files are. A start folder with frames
of its own is listed from the disk as before, fast and before the
index. A folder of folders is one with a subfolder not hidden by a
dot: an empty one, §403's card-dump folder emptied after each
import, still falls back to the last folder, as does a folder of
folders under no root or a missing one, each with a log line that
says which. The folder is made canonical before it is matched to a
root, as the roots are; a folder opened by hand before the index
reports open takes the start view's place; and an index that fails
to open opens the last folder rather than leaving the window empty.
The resolution stays a pure function over a listing, now with three
answers rather than two, and the choice of view beside it a pure
function over the roots.
