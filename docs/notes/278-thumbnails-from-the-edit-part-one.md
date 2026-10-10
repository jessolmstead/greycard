# 278. Thumbnails from the edit, part one: the key, the entries, the open frame (2026-10-10)

The first step of §270's order, built: `Edit::develop_key`, the
edited entries in the thumbnail cache beside the camera's, and the
open frame's picture kept for nothing when its edit is saved or the
frame is left. A frame with an edit now shows it in the strip, the
grid, the culling loupe and compare; a frame without one, or with its
edit reset, shows the camera's JPEG. No marker, no setting. The reduced
develop, the queue for frames edited without being opened, the ΔE check
and the lightbox are part two's and three's.

**The key.** `Edit::develop_key(lenses)` (`greycard-edit/src/key.rs`)
is the first eight bytes of a BLAKE3 over: a fixed prefix,
`DEVELOP_RECIPE` (1, raised when the develop or the finish comes to make
another picture from the same edit), the edit's own JSON, for every file
outside the sidecar the develop reads by name the file's name and the
BLAKE3 of its whole contents (or a mark that it is not there), and,
while the edit corrects the lens by its profile (`lens.enabled &&
lens.profile`, which a default edit does), the lens database's name. The
files are the look's table in the look directory under whichever
extension is there (`.cube` or a HaldCLUT; the camera match's fitted
tables are looks like any other) and its variant for the edit's display
curve, and a DCP from the profile directory. A file's hash is kept in
memory by its path, length and mtime, so a 65-point table is read once
a session and not once a frame. `Sidecar::develop_key` is the same for
the sidecar's current edit.

The lens database's name is the editor's to give, since greycard-edit
does not know the database: the directory it is read from (the first of
the store's candidates with any XML in it, the one `Store::load` reads)
and the fetch's `timestamp.txt` there (`edited::lens_database`). A
picture kept before the database was fetched is of an uncorrected lens,
and nothing would remake a key already kept; now a fetch or an update
moves the name, every key found is read again, and a frame whose edit
corrects the lens shows the camera's until its picture is kept anew.

§270 said "a hash of what `same_develop` compares, the turn and the
crop". `same_develop` compares the base, the retouch, the detail and
the sharpen, which is what decides whether the worker develops again;
the brightening that decided the design is the exposure, which is the
finish's and not in it. So the key takes the whole edit, less the two
things in it that reach no pixel: the schema version it was written at,
and an adjustment's name. Serde writes a struct's fields in their order
and nothing in an edit is a map, so equal edits write equal bytes: an
undo and a redo back to the same picture give the key they gave before,
and a sidecar written and read back gives the key of the edit in memory.
The history, the ids, the meta and the exports are the sidecar's, not
the edit's, so a rating changes no key.

The turn is not in it either. The first cut had the frame's quarter
turns (the sidecar's `turn`), as §270 asked, and the review found what
that cost: a picture is kept the camera's way up, with the frame's turns
taken back out, and drawn turned like the camera's, so the turn is not
in the picture kept; and a turn moves the crop and the masks inside the
edit (`Sidecar::turn_by`), which the key does see. With the turn in it,
an edited frame turned in the strip while it was not open fell to the
camera's JPEG (the reviewer measured the cell 2.1% from the camera's
entry and 23.6% from the edit's), and the loupe missed the same way. The
worker still matches the turn when it takes a develop in hand, since that
picture is turned by it.

The tests: an undo and a redo, by a copy of the edit and by setting the
same value again, and a round trip through the sidecar's JSON; a rating,
a keyword, the schema version, a mask's name and the frame's turn change
nothing, and the crop and an adjustment switched off each do; a look's
table changes the key by being there, and again by a refit of its
contents, its variant for another curve does not while the edit is under
per channel, and a DCP the same; the lens database's name is in it while
the profile corrects and not otherwise.

A picture made without something its edit asks for is not kept at all:
a learned mask or the learned denoiser whose model is not downloaded or
failed, a fill whose model is missing, a look not in the directory
(what the develop's `left_out_of` and the export's own list name). Kept
under the edit's key it would stand for good; the frame shows the
camera's until the next develop of it lands, or the next save, with everything in hand.

**The entries.** One per made size (128, 176, 256, 360) and one at the
local previews' 2048, under the file's content hash like the camera's,
with `EDITED_RECIPE` (0x8001, the high bit for "made from the edit") in
the recipe's place and the develop key in the stamp's:
`<hash>-<size>-r32769-<key>.thumb`. The camera's entries stay: they
stand in until the edit's land and are the picture again after a reset.
The format did not change by a byte: an edited entry is an entry like
any other, counted, capped and evicted by the same rules, its 2048 one
against the previews' cap and the rest against the thumbnails', and
the settings sheet's count and Clear take them with the rest. So a
cache written before this reads as it did (it holds no entry of the new
recipe, and every frame shows the camera's until its edit is kept), and
a build that does not know the recipe never asks for those names and
counts and evicts them like its own. Checked by running master's
editor (built from the branch point) over a cache that held edited
entries: it listed, drew and quit with nothing said.

The thumbnails are kept as the camera's tag turns the frame and no
further, like the camera's: the picture is made from the develop, which
is turned by the frame's own quarter turns and the crop panel's turn and
mirror, and those (`Geometry::shown_turns`) are taken back out before it
is kept, a turn by the turn the other way and a mirror with a turn by
itself. The strip, the grid and the loupe then turn it at draw time as
they turn the camera's. The crop and the straighten stay in it. A made
size is the long edge exactly, area averaged from the 2048 one; the
camera's are a whole factor down, so an edited thumbnail is a few pixels
larger than the camera's it replaces (176 by 117 for a 600 by 400 frame
where the camera's is 150 by 100). The JPEGs are the cache's own
qualities, 90 for a thumbnail and the previews' 88 for the 2048.

**Stale entries.** When a frame's pictures are kept under a new key,
the pictures of the key that frame showed before go at once
(`Thumbs::remove_tag`, every size of that one tag), and a reset takes the
pictures of the key it showed; the count follows each removal. Only that
frame's old key: the first cut took out every other key of the content,
and the review found it thrashing two hard links of one raw edited two
ways, and an unrelated folder's copy, each save taking the other's
pictures. Another key of the same content may be another copy's, and is
left to the eviction, as is an old key the window never found for the
frame (one edited and saved before its pictures were ever looked up),
and the edit's pictures of a file deleted from disk, which the delete
leaves as it leaves the camera's of another stamp.

**Which frames show their edit.** `edited::Edited`, held by the worker,
knows by path which frames of the list in the window are edited: the
window says, as each list lands, the index's `edited` for a frame
standing in from its row and the sidecar's edit against the frame's
default for one read from disk, and again when a row or a read changes
that (`edited::refresh`, from `rows::take` and `rows::apply_row`). "An
edit" is §276's rule, the one the cell's pencil goes by
(`greycard_library::is_default_edit`), the learned blend judged against
the frame's ISO from its row, so the mark and the picture agree. A
sidecar that changes under a frame not open has its key read again: a
row that comes in with another sidecar hash (`RowMeta::sidecar_hash`,
new, read from the column the index already keeps), and a sync that
takes the archive copy's sidecar whole, which now goes through the
save's own hook.

The key of a frame held as edited is found on the thumbnail's own thread
the first time a picture of it is looked up, by reading its sidecar as
the window reads it (the XMP beside it included), and kept; a reset there
takes the frame back to the camera's. A frame whose sidecar cannot be
read, its root offline among them, shows the picture of its edit kept
last for its content (`Thumbs::newest`): one key for every size, the one
whose entries were written last by their birth time, which a hit does
not move (on a filesystem without birth times the modification time
stands in, and that is the last used). The local preview's lookup for
the offline and network loupe does the same, ahead of the camera's
preview.

The pool's lookup and its making both look the edit's picture up first
and fall back to the camera's entry, so every size the strip and the
grid make is the edit's once it is kept. The culling loupe and compare
show the 2048 at a view's size, said on the plate as "the edit, 2048 ×
1365"; 1:1 still opens the camera's full JPEG, for focus, and a local
preview's words are unchanged.

**The open frame's picture, for nothing.** A save of the open frame
(`library::sidecar_written`, and a followed frame's save) sends the
worker a `Job::Keep` with the saved edit, its turn and the frame's ISO,
and so does a develop landing in the window when what it developed is
the frame's saved edit (`edited::landed`), which covers a frame edited
before this build or with its pictures evicted: opened, it has them
again. The worker takes saves ahead of a develop or an open, so a frame
left at once still has its picture in hand. It then:

- takes a reset edit's pictures out and says so;
- does nothing more when every picture of the key is kept already (a
  rating) or is being made (a develop landing, then a rating);
- takes the picture it has when that is of the same develop at the same
  turn: the develop's own on the CPU (`Last`), or the viewport's texture
  when the GPU ops made it (`ShownTexture`), read back a band of rows at
  a time and box-averaged a whole factor down on the way, so the full
  picture is never on the CPU. The texture is the one the viewport draws
  from (it holds it as its `source`): the worker's is a second handle on
  it and costs the device nothing while the window holds the same
  develop, in culling too; only a develop the window drops for a stale
  generation is held one develop longer than it would be;
- keeps nothing when the develop or the masks left out something the
  edit asks for (above);
- otherwise holds the save until a develop of it lands, or until the
  frame is left, when the frame is held as edited under its new key and
  shows the camera's until its pictures are kept.

The making runs on a thread of its own, one at a time, the newest save
in place of one still waiting: the reduction by a whole factor that
keeps the frame's long edge at 2048 or more, the geometry, a Lanczos
resize to 2048, the finish exactly as an export at that size is
finished (`export::render_framed`, the masks, the guide plane and the
vignette placed by the full frame, no output sharpening, sRGB), the
turns taken back out, the made sizes from that, the encodes outside the
cache's lock and the five writes under it. Then the cells showing the
frame ask again (`Outcome::EditedKept`) and the loupe drops its copy.

**Measured.** Release, the ignored test
`worker::tests::the_edit_s_pictures_of_the_samples` over a 24 MP R6 II
frame (6000 by 4000, reduced by 2) and a 45 MP R5 frame (5464 by 8192
portrait, reduced by 4), each developed once on the CPU under a
brightening, four runs, the user working on the machine meanwhile; the
time is from the picture in hand to the five entries kept:

| | from the CPU's picture | from the viewport's texture: read back | then made and kept |
|---|---|---|---|
| 24 MP | 158 to 199 ms | 35 to 85 ms | 146 to 217 ms |
| 45 MP | 107 to 126 ms | 68 to 119 ms | 96 to 126 ms |

The 45 MP frame is the cheaper of the two because a factor of 4 brings
its 8192 to 2048 exactly and the Lanczos pass has nothing to do; the
24 MP one is reduced to 3000 and resized from there. None of it is on
the window's thread, and only the read back is on the worker's, ahead of
the next develop. In the debug editor the same is 3.2 to 4.1 s for the
24 MP frame (the debug build's resize) and 1.3 s for the 45 MP one, read
back from the GPU on an RTX 5070 Ti in 270 to 430 ms. The five entries
are 864 KB for the 24 MP landscape and 502 KB for the 45 MP portrait.

On rendered output: the debug editor on a headless weston, hard links to
the two frames in a folder each with a second frame beside them, every
`XDG_*` folder under the work dir. A preset of +1.5 EV laid on the first
frame and a rating pressed (`--preset --press`), the snapshot held a few
seconds with `--keys wait…`: the strip's first cell went from a mean of
140.1 to 171.1 in the session (the mean absolute difference from the
camera's cell 33.7 of 255), and opened again in a new session the same,
from the cache with nothing made. The grid, the loupe ("culling: the
edit, 2048 × 1365") and compare showed the edit. A preset back to 0 EV
and another rating took it back to the camera's in the session (140.5,
the difference 1.9, which is the stars and the edited mark drawn over
the cell) and in the next, and the cache held the camera's entries
alone. The 45 MP folder did the same (130.0 to 149.9 and back).

The review's finding on the turn, again after the fix: the frame
brightened and kept, then a culling launch (where no frame is open) and
`] ]`, a half turn, pressed over it. The strip's cell and the loupe
("the edit, 2048 × 1365") show the edit, turned; the cell, turned back,
is 4.6% from the edit's cell and 12.9% from the camera's (the stars and
the edited mark, drawn in place, are what is left of the 4.6%).

The tests in `edited` hold it without a window, on a PNG through a real
worker: a saved brightening is the strip's picture at the made size and
the loupe's at 2048, a rating makes nothing and asks no cell, a second
edit replaces the first's entries, a crop is in the picture, a reset is
the camera's again with the edit's entries gone, a frame turned a
quarter and one turned and mirrored are kept the camera's way up, a save
before its develop lands is kept when it does, a frame left before then
shows the camera's, and a picture without a look its edit names is not
kept. And through the window, headless, with the window's own hooks: a
frame opened, brightened and saved by the panel's save shows its edit in
the strip, a reset saved the same way the camera's, and the frame turned
in the strip (`turn_frames`) after another is opened keeps its edit's
picture. Taking the save's hook out of `library::sidecar_written` fails
that test; the first cut's tests passed without it.

**Found on the way, on master as here.** A launch in the culling mode
(`--cull`, `--cull-compare`) over a frame with an edit writes the
panel's default over the frame's edit on the way out: a new history
state with the exposure at 0 and the learned blend at 1.0. Reproduced
with master's editor built from the branch point, with no part of this
change in it. It looks like the culling mode's stand-in panel being
recorded as the frame's state at the quit. The worse form: a develop on
an edited frame A, `c` into culling, the arrow to an unedited frame B,
and quit: A's edit is written into B's new sidecar. Not fixed here; it
is fixed on master in its own section.

**Left to part two.**
- Frames edited without being opened (a sync, a preset over the
  selection, a paste, a join): held as edited under their new key, they
  show the camera's until opened. The reduced develop and its queue are
  what fill them; `sidecar_written`'s non-open branch and the sync's
  take and join are where the queue is fed. A hook for it is there:
  `Edited::shows` says which key a frame wants and `all_kept` whether it
  is kept.
- The ΔE check. The picture kept here is the export's finish at 2048 of
  the window's own develop, read back from the GPU when the GPU made it;
  nothing yet holds the GPU's read back against the CPU's.
- A frame's sidecar on an offline root cannot be read for its key, so
  the picture kept last for its content stands for it, and another copy's
  later edit of the same content wins it. A key in the index's row (a
  schema step) is the real fix.
- An old key the window never found for a frame, and the edit's pictures
  of a deleted file, go by the eviction and not at once.
