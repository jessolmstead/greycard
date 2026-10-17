# 337. Importing a Lightroom catalog, part 2: the sheet, the crop, the camera rules and the stacks (2026-10-17)

§329 built the command and left the sheet, the crop and the smart
collections on a camera or a lens open. This is those three, and the
first of the import's two follow-ups from §334: Lightroom's stacks.

## The sheet

**Where it is.** "Import a Lightroom catalog..." under the grid pane's
Import..., where a folder fills from a card; the file chooser takes a
`.lrcat`. Its progress goes under the button and on the status line,
as the card import's does, since the grid covers the status plate.

**One import, two voices.** The sheet decides nothing the command does
not. Every answer it takes is a field of `lightroom_catalog::Options`
with a flag of `library import-lightroom`: the roots (`--root`), the set
(`--set`), writing over frames with edits (`--force`) and, new here,
leaving the books, prints, slideshows and web pages out
(`Options::no_creations`, `--no-creations`), §329's last open question.
What it shows is the report's own lines: the report's `Display` became
`Report::lines(voice)`, the word and the text of each line, which the
command prints in its column and the sheet lays in two; the three lines
that name a flag ("say where with --root", "left alone without
--force", "--write to import") have a sheet's wording beside the
command's. A test holds the sheet's lines equal to the report's own
lines for the options the sheet built: it checks the plumbing, that the
set, the creations switch and the roots the sheet holds reach the
import as the command's flags would, and that no line on the sheet
names a flag; the two voices differ by those three lines by design.

**Its steps.**

1. *The roots.* Each root as the catalog spells it, with its name;
   the folder here it was found in or mapped to, a Choose... beside
   it; and what was found there ("found beside the catalog: 558 of
   its 558 files found"). The sheet opens on a dry run with no
   mappings, so the first thing it shows is the command's own guess,
   and each folder chosen runs the dry run again. Under the roots, the
   set: the catalog's name by default, empty for none, as §329's Open
   asked. The roots scroll and the set, the switches and the buttons
   stay under them, so a catalog of a dozen roots still shows its
   Check; a root shows its folder's own name over its path, which a
   long path's end would otherwise hide. A switch for the creations
   shows only when the catalog has
   one, and one for writing over frames with edits only when some are
   there, with their count.
2. *The report.* Check runs the dry run under the set and the
   switches as they stand and shows its report: the roots, the
   counts of frames found and not, the edits carried and the mapper's
   list of what it could not carry, the crops, the copies, the stacks,
   the collections, the smart collections translated and skipped with
   why, where new sidecars go, and anything left alone. Back returns to
   the roots; Write is off until a file of the catalog is found.
3. *Written.* Write runs the import on a thread of its own,
   `import_with` telling it how far it has come (looking for the files,
   then a frame at a time, then the collections, stacks and roots); the
   status line and the pane's line say it a tenth of a second apart at
   most. The sheet stays up while it writes, its buttons waiting, so
   nothing is edited under a write. The write is given the folders the
   report showed, guessed or chosen, as mappings, so it never guesses
   again and cannot land where the report did not say. When it lands
   the sheet shows what the write did first and the report under it,
   the report's lines scrolling with a word when it goes on past the
   sheet, and the window takes up what it wrote (below).

**The window takes up the write.** The window holds the sidecar of every
frame it lists, and the open frame's panel; a write under them left
them stale, and the next save wrote the stale state back. The review
found it: the editor open on a frame with no sidecar, a write that put
a state, an exposure of 1.45, a custom white balance and a crop on it,
then a rating key, and the sidecar was left with no history, no
exposure, no crop. Now, before the write, a panel moved and waiting on
its save is saved (the frame then has an edit of its own and is left
alone unless written over; a panel at rest is its sidecar already and
nothing is written); after it, every frame the window lists that the
write gave a sidecar to is read again from the disk (`rows::reread_file`
over `Written::frames`, the paths as found and as the index keys them,
made canonical off the window's thread), its cells' pictures asked
again, and the frame on screen opened again on what was written with
nothing of its old panel saved over it. Then the collections and the
stacks are read again, and each root it added is read from the roots
file, watched and passed over as the roots pane's own add does
(`roots::taken_from_file`). The review's run again: the state kept,
1.45, the custom white balance at 5800, the crop at -0.24 degrees, and
the rating 3 with them.

**The guess at a root is the command's.** A root not mapped is looked
for where the catalog says, taken there on any of its first five files
as before; then the guess. Its candidates are every folder of the
root's name beside the catalog, the catalog's folder itself when it
bears the name, beside the folder above it (the three catalogs we read
sit beside their pictures' folder, `<shoot>/<shoot>.lrcat` and
`<shoot>/<root>/`), and under the library's roots, three levels deep.
Each is asked for a sample of 25 of the root's files spread through
the catalog's order, and a candidate is the root only when it holds all
of them or all but one. The catalog keeps each file's size when it was
imported, the last field of `AgLibraryFile.importHash` (capture time,
name, size), and a candidate holding most of those it has the size of
at another size is another shoot's files of the same names and is
passed over. Of the candidates left, the one holding most, then most at
their sizes, then the nearest, is taken.

The first cut took the first folder of the name holding any of the
first five files, and the review showed what that does: a root renamed
`RAW`, a library root holding `2025-a/RAW` with one file of a
coincident name and `2026-b/RAW` with all 265, and the guess took
`2025-a/RAW` ("1 of 5 files looked for found, taken") and a write put a
sidecar on the unrelated file. Run again: with the 265 at their sizes,
`2026-b/RAW` is taken ("25 of 25 files looked for found") and the write
puts 265 sidecars there and none in `2025-a`; with the 265 as empty
files, as the review made them, both folders are passed over, the
second for its sizes, and the root is said not found. A folder holding
a few of a root's files is no guess either: the user chooses it, and a
chosen folder is taken on any file, as a `--root` is. When nothing is
taken, the nearest folder of the root's name is said with what it
holds ("…/RAW holds 23 of 25 looked for; choose it to take it", the
command's "--root it to take it"), and one held back for its sizes
says so: a shoot whose rejects were deleted on disk but are still in
the catalog is a near miss, not "no folder of its name". The size check
holds for raws, which nothing rewrites; a folder of DNGs or JPEGs
Lightroom wrote its metadata into has sizes other than the catalog's
import hash kept and can be held back for them, and is then chosen by
hand. The report says how each was found ("found beside the catalog",
"found under the library's root ..."), so the command finds the
folders the sheet offers, and the written step keeps the report's words
for each root though the write was handed them as mappings.

**Off the window's thread.** Each dry run and the write run on a
thread and post back; a dry run asked for later wins over one that
lands late. `--sheet lightroom`, `lightroom-report` and
`lightroom-written` over `--lightroom CATALOG` put the sheet up at each
step for a capture, the last writing as Write does, and a capture waits
for the sheet's work to land before it is taken.

## The crop

**Lightroom's.** Two opposite corners of the cropped rectangle, as
shares of the file's width and height in the file's own way up (before
its orientation tag or a turn in Lightroom), and `CropAngle`, which
turns the rectangle about its corner at `CropLeft`, `CropTop`: its
width runs along (cos θ, sin θ) and its height along (-sin θ, cos θ),
down positive (§333 read this off the catalog's cropped sizes). The
export is that rectangle sampled out, then turned by Lightroom's
orientation of the image (`Adobe_images.orientation`, `AB` for none,
`DA` for the frames tagged 8 in every catalog we hold).

**Ours** (`geometry.rs`). The source is the develop turned by its tag
and the frame's `turn`; the edit's quarter turns turn it on, the fine
angle turns that about its center counter-clockwise on the screen,
and the crop is an upright rectangle of that leveled plane, in shares
of its size. `to_source` takes the plane's x axis to the source's
direction (cos α, sin α).

**The mapping, on points.** Let the file be W by H in its own way up,
T its tag with the frame's turn, L Lightroom's orientation.

1. The rectangle's corners P_i in the file's pixels: P0 = (L·W, T·H),
   P1 = P0 + w·u, P3 = P0 + h·v, P2 = P1 + h·v, with u = (cos θ, sin
   θ), v = (-sin θ, cos θ) and (w, h) its size along them.
2. Each taken to the shown source by T, a dihedral map: T's linear
   part R_T is a quarter turn, mirrored for four of the eight tags.
   The rectangle's sides there run along R_T u and R_T v. A quarter
   turn commutes with a turn by θ, and a mirror reverses it, so R_T u
   is the axis R_T e_x turned by θ, or by -θ under a mirror: our
   angle is α = θ for an unmirrored tag and -θ for a mirrored one.
3. Lightroom's export stands as L(file); ours stands as T(file) turned
   by the edit's quarter turns. Where L is T turned q quarters
   clockwise (`Orientation::turns_from`), the edit takes 4 - q of its
   own, which count counter-clockwise; an L mirrored where T is not
   has no quarter turn and the crop is not carried. That is a Flip in
   Lightroom, and none of the three catalogs had one.

   The dry run, which reads no file's tag, says how the turns are
   taken and counts what it can from the catalog alone: the frames
   with a mirrored orientation (a Flip), those found, and any code it
   does not read.

   The quarter turns are not the crop's alone: a frame turned in
   Lightroom and never cropped stands turned in its export too. So every
   frame written takes Lightroom's turn (`lightroom_catalog::turn_onto`,
   `crop::turns_for`), one never developed in Lightroom as a state of
   its own; a write counts those turned, those flipped (their mirror
   not carried), and any orientation code it does not read, by its
   spelling. The codes are read as the corners they name: the shown
   picture's corners A to D clockwise from its top left, and the two
   letters where the file's own top left and top right land in it, so
   `AB` is the file as it is, `DA` a quarter turn counter-clockwise,
   and the four with the letters the other way round (`BA`, `DC`, `AD`,
   `CB`) mirrored; a test reads each of the eight back off the corners.
4. The corners through `Geometry::to_plane` under that geometry stand
   upright; their box, in shares of the plane, is the crop.

The tests take the plane's corners back through `to_source` and find
the file's rectangle at each of the eight tags, and check a file point
through a Lightroom turn against where L puts it. The shares are of
the catalog's `fileWidth` by `fileHeight`; where our develop's size
differs (a border one converter keeps), the corners are scaled onto
ours.

**On the picture.** Ours never shows past the turned picture, which a
crop held to its shape in Lightroom at an angle can just reach; a crop
that does not fit by the geometry's own tolerance is drawn in about its
center to the largest of its shape that does (`Placed::drawn_in`), and
counted. None of the frames measured needed it.

**Its shape.** Lightroom's `CropConstrainAspectRatio` holds the crop's
shape while it is edited. Held, the crop keeps the frame's own as
`Original` when it is within half a percent of it, one of the panel's
presets within half a percent (5:4, 4:3), else its own ratio to three
places, with `portrait` for a tall one; not held, it is free.

**In the import.** The crop is laid on beside the mapper's preset, in
`lightroom_catalog::crop_onto`, where the file is: the frame's size and
tag come from the decoder's stance, the catalog's size and orientation
from `Adobe_images`. "crop" leaves the mapper's not-carried list in the
report and gets a line of its own ("57, 57 of them found: each would be
carried with its angle where its file says its size"); a write counts
those carried, those drawn in, and those not carried by why (a file
that says no size, a Lightroom mirror). `imported_edit` takes the
catalog's shape of the file and carries the crop with it, or leaves it
off and named without one, for the color measurement of §333, which
cuts Lightroom's crop out of its render itself.

## The crop against Lightroom's exports

`greycard-ui --measure-lightroom CATALOG --crops` develops each cropped
pair's raw under the edit the import writes, crop and all, finishes it
at the export's long edge through the export's own geometry, and lays
the export onto it with nothing cut or turned by hand: the camera
match's registration, refined at up to 2,560 pixels across over scales
a fortieth of a percent apart and whole-pixel shifts, then the shift to
a fraction of a pixel by a parabola through the peak.

**The develop's own scale.** Every frame came out a few tenths of a
percent off in scale, and the uncropped frames measured beside them as
controls do too: the R5 Mark II's 23 cropped frames at 0.9947 to
0.9965 against its 10 controls' 0.9947 to 0.9955, the GFX 100S II's 29
at 0.9913 to 0.9997 against its 11 controls' 0.9938 to 1.0003. It is a
magnification about the frame's center wherever the crop sits, and it
is mostly Lightroom's: the review ran the same measurement with no lens
database, so with none of our corrections, and the R5 Mark II's scale
moved only from about 0.995 to 0.992 to 0.995, the GFX's not at all.
Lightroom's profile correction fills the frame its own way (the GFX's
spread is all of it), and ours brings the R5 Mark II a little nearer;
without ours the crop's own residual below was worse, 0.69 pixels on
average and 1.83 at most. So the shift is also given with
that magnification taken out: a scale s about the frame's center z (in
the render, from its center) moves the picture by (1 - s)z, and what
is left is where the crop itself stands off Lightroom's.

**What it said**, on the catalog of about 280, the one whose exports
are in the archive: every one of its 52 cropped pairs, all by the
export history, 17 of them turned (-1.67 to +1.76 degrees), and its 21
uncropped history pairs as controls, all 73 registered (correlation
0.86 to 0.99).

- *The size.* Our full-size crop was Lightroom's to the pixel on all
  52 (from Lightroom's corners on the catalog's frame: 4355 by 5444,
  8430 by 11240, ...), none was drawn in, and the render at the
  export's long edge was the export's size on all 73. Every export
  stood the way ours did, which is the tag's turn, the angle's sign
  and the orientation code all right at once.
- *The crop's own residual*, the develop's scale taken out: 0.36
  pixels on average and 0.99 at most, in the export's pixels at its
  5,000-pixel long edge; 0.49 average on the R5 Mark II, 0.25 on the
  GFX, 0.43 over the 17 turned crops (0.96 at most). That is under a
  pixel everywhere, at the refinement's own resolution of about a
  quarter of one.
- *With the develop's scale left in*, the farthest corner of an
  export sits 9.3 pixels on average and 28.7 at most from ours, all of
  it the lens correction's magnification, as the controls show.

## Smart collections on a camera or a lens

§329 skipped them: Lightroom names a camera and a lens as the file's
EXIF spells them ("Canon EOS R6m2", "RF50mm F1.2 L USM") where the
index keeps the decoder's names ("Canon EOS R6 Mark II", "RF 50mm
F1.2L USM"). The lens differs too, which §329 had not seen: the index
takes rawler's lens name where it has one.

**The names they go by.** For each camera and lens a rule could name,
the import gathers the index's name and the EXIF spellings it goes by
(`Known`):

- each name the catalog's images give (`AgHarvestedExifMetadata`
  through the interned camera and lens tables), put to one of its
  files found and read as an index pass reads it (the camera's and
  the lens's both, one file a name);
- a catalog name none of whose files is here, through the decoder's
  camera table (`decode::cleaned_camera`, rawler's models and their
  aliases to its clean make and model); a lens stands as spelled;
- the library's own cameras and lenses (its facets), each known by the
  decoder's EXIF spellings of its name (`decode::exif_models`), or by
  the name when the decoder has none. These are the whole library's,
  not the smart collection's scope's (one root's, when the catalog has
  one): a conservative choice, since a camera the library holds
  elsewhere can only make a test pass more cameras, and a test that
  then passes several and fails others is skipped rather than given a
  term that narrows it wrongly. So "camera contains II" translates with
  no library beside the import and is skipped with a library that holds
  an R6 Mark III.

**The rule asked of them.** Lightroom's test (is, is not, contains,
contains all, contains words, does not contain, starts with, ends
with; case ignored) is asked of each one's EXIF spellings, not of our
name: "contains II" is two of four cameras in Lightroom's spellings
and all four in ours. Then:

- words our names hold exactly as Lightroom's spellings do (a lens
  "contains USM") are those words (`lens:usm`);
- else a test passing one is that one (`camera="Canon EOS R6 Mark
  II"`);
- "is not" and "does not contain" leave each one out by name, since a
  filter's terms are asked together;
- a test passing several and failing others has no term, and is
  skipped with why;
- a test passing none matches nothing and is skipped and counted, as
  before.

**On the three catalogs.** None of them holds a smart collection of
its own (§329), so we made copies of each with a smart collection for
every camera and every lens its images name, an "is", a "contains" of
the name's last word and an "is not": 30 rules over the three. 28
translated; the 2 skipped are "is not" in the catalog of about 155,
which has one camera and one lens, so those rules match nothing, with
no library beside the import; with a library that holds other bodies
and lenses (one holding the R5 Mark II and the GFX 100S II) they leave
the R6 Mark II and its lens out by name, and all 30 translate. Every
translated filter, listed over an index of one raw of each of the four
bodies (carrying the three lenses) copied from the archive into a
scratch folder, found exactly the frames Lightroom's rule names. The report counts them on a
line of their own ("on a camera or a lens: 12 translated, 0 skipped").

## Stacks

Lightroom keeps a stack in `AgLibraryFolderStack` (`collapsed`) and its
images in `AgLibraryFolderStackImage` (`position`, the top first). In
the three catalogs every stack is a master and its virtual copy, 2, 1
and 13 of them, all expanded. The import writes each as a stack file of
ours (§334): the members whose files were found, in Lightroom's order,
a copy as the version the import made for it, open when Lightroom had
it expanded. Lightroom's stacks are one folder's, as ours are; one
across folders is not written, nor is one with fewer than two members
found, nor one with a member in another stack already (a frame is in
one stack at most). A stack there already with every one of these
members is counted there, so a second import writes none. A copy whose
version was not made, its file's sidecar left alone, is not a member.

## What a write did, by hand

Under a headless weston on a copy of the catalog of about 280, its root
renamed to `D:/Photos/2026/Shoot/` and trimmed to six of its raws
(three stacked masters, three turned crops) and their copies, the copy
beside a folder `Shoot` of hard links to those six, with a scratch
library: the sheet opened on the guess ("found beside the catalog: 6
of its 6 files found"), the report said 9 images found, 6 crops, 3
stacks; the write made 6 sidecars and carried 6 crops, four of them
turned (-0.24, +0.27, +1.3 and -1.05 degrees), one held tall at 4:5 as
the panel's 5:4 turned, the quick collection and 3 stacks, added the
root, and the window took them up: the folder in the pane, the stacks'
badges on the strip, the copies as versions, and the panel of the frame
on screen at the written white balance. Run again, it wrote nothing:
six left alone, the collection and the stacks there already, the root
a root already. The same catalog with eleven more roots, none of them
here, shows its first step with the roots scrolling and the set and
Check under them.

## Left open

- The RAW+JPEG follow-up of §332: a catalog row for the camera's JPEG
  beside its raw is still written as a frame of its own, a sidecar the
  window never reads. None of the three catalogs lists a JPEG (their
  files are 558, 155 and 265 raws), so there was nothing to measure it
  on; it wants a catalog made with "treat JPEG files next to raw files
  as separate photos" on.
- A guess wants all but one of 25 of a root's files: a folder holding
  some of a shoot (a selects copy, a half-copied card) is not guessed
  and is chosen by hand. A catalog with no import hash has no sizes to
  tell a namesake folder by, and is guessed on the names alone.
- The develop's scale against Lightroom's, half a percent on the R5
  Mark II with its 50 mm and up to nine tenths on the GFX 100S II, is
  Lightroom's profile correction's fill, not the crop's; it moves a
  crop's edges by up to 29 pixels at 5,000, and matching it is the lens
  correction's question, not the import's.
- A Lightroom Flip is not carried: the mirrored codes are read (`BA`
  across, `DC` up and down, `AD` and `CB` about a diagonal) so a flipped
  frame is counted, its mirror and its crop not carried; a code read as
  none of the eight is counted by its spelling. No catalog we hold has
  one.
- The Upright keys, at zero in every image of the three catalogs, are
  still counted and not carried; a crop over an Upright transform
  would be laid on a plane Lightroom had transformed first.
- A camera rule whose test passes several cameras and fails others is
  skipped: the filter has no "either". A union of terms in the filter
  language would carry it.
- "Is not" in our filter leaves out a file with no camera, where
  Lightroom's may keep it.
- The quick collection and the creations' frames still come as
  collections unless the creations are left out; a creation's layout is
  not carried.
