# 242. An export into a subfolder, and a question before it writes over (2026-10-04)

Every export asked the desktop where to go: a save dialog for the
frame on screen, a folder chooser for a set. That is right for a
one-off and slow for the common case, the day's picks into an
`export` folder beside the raws, the same folder every time. The
export sheet now has a Subfolder field. Empty, it asks as before.
Typed, Export writes each frame into that folder under the frame's
own folder, made as its first frame goes in, and asks nothing.

**Beside each frame, not one place.** A set chosen across folders
(the tree with subfolders, the library) lands in one `export` per
source folder, each beside its raws; a single chosen folder would
mix shoots. This is Lightroom's "same folder as the original, put in
subfolder", and the reason it is the default people reach for. A
subfolder is a name going down: `export` or `export/web`; a whole
path or one that climbs (`..`) is refused on the sheet with the
reason, since it would land every frame of a set in one place and
the field would mean two things. A `.` is passed over, and a name
that is nothing but dots is refused, so an export never lands on its
own frame's name.

**The frame on screen goes as a set of one.** With no chooser, the
frame on screen has nothing to set it apart from a set: it goes
through the set's path (`start_set`), under its panel edit as a set
already takes it, and its name is the set's (`queue::names`): the
frame's own stem and the format's extension, without the `.greycard`
the editor writes beside a raw, since the subfolder is not where a
camera writes. Two frames of one name in one subfolder (a raw and
its camera JPEG) are still told apart, ` (2)` on the second.

**The sheet carries it.** The field is on `Sheet`, so an export
preset keeps its own subfolder (a Web preset into `web`, a Print one
into `print`) and the preset reads as edited when it moves. An entry
added to the queue with a subfolder asks no chooser either; its
folder is none and its sheet says where, so the run puts it beside
each frame as Export would. A settings or queue file from before
reads the field as empty, which asks. `--export DIR` names its folder
and ignores the field.

**The subfolder is made by the worker, and only it.** The worker
makes the folder (`create_dir_all`) as a frame goes in, and only
when the set has a subfolder: a chosen folder that is not there (a
drive unplugged since it was queued) must never be made on the disk
under its mount point, which is why the queue checks it first and the
worker never makes it.

**A question before writing over.** The chooser was the confirmation
for a file it named. With no chooser, and for a set into a chosen
folder, the editor names the files itself, and the sheet's Overwrite
policy would write over whatever was there in silence. Now, under
Overwrite, an export from the Export button looks first for the files
its set would write (`queue::names`, the names the set will use), and
when any is there, a sheet names every one of them by its whole path,
in a list that scrolls however long it gets. Keep both is the default
and Enter's: the set under Increment, each beside the one there.
Overwrite is red and only a click reaches it, the delete sheet's
rule. Escape is no. One sheet with every file on it rather than a
question a file: a set of two hundred over yesterday's export is one
decision, made with the list in view. Increment and Skip never write
over and never ask. A queue run does not ask: it runs unattended, and
its entry was queued under the policy it says.
