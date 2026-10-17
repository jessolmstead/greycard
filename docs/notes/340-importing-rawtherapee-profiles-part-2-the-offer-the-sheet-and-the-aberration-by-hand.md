# §340. Importing RawTherapee profiles, part 2: the offer, the sheet and the aberration by hand (2026-10-17)

§328 built the mapper and the command and left three things open that
this takes up: reading the profiles when a folder opens, the command's
report as a sheet, and a part of its own for the chromatic aberration
set by hand. The pattern is §337's sheet for a Lightroom catalog.

## One import, two voices

The command's import moved into `greycard-library` as
`rawtherapee::import`, `import_with` (a frame at a time to a progress
callback) and `import_each` (each step told, a frame's report with
it), beside `lightroom_catalog`: `Options` (`write`,
`force`, and where a new sidecar goes), a `Report` of a `FrameReport`
a frame with a profile, and `Report::lines(Voice)`, the word and the
text of each line, which the command prints in its column. `Voice` is
the Lightroom import's, re-exported. The command's flags are as they
were, and so is its output, a frame's lines printed as the frame is
done through `import_each` and the totals at the end; a test pins the
text line for line, and another holds the lines told a step at a time
equal to the report's. Three lines name a flag and have a sheet's
wording beside the command's: a frame held back ("--force lays the
import over it", "writing over the frames developed or rated here lays
the import over it"), the held-back totals ("(--force to import over
it)", "left alone unless written over") and the dry run ("--write
writes it", "nothing written yet"). A fourth differs by design: the
sheet names a frame by its file's name, its folder being the sheet's
subtitle, where the command gives the path it was handed.

The report has its two halves apart, `frame_lines` (the files passed
over, then each frame's lines) and `totals`, and `lines` is the one
after the other, as the command prints them. The sheet lays the totals
over the frames' lines, which scroll: a folder of a few hundred
profiles is a few hundred lines a frame, and its totals are what a user
reads first. A test holds the sheet's two lists equal to the report's
two halves in the sheet's voice for the options the sheet built, and
their join to `lines`.

The command's helpers came with it (`profile_beside`, `frames_named`,
the sidecar a frame starts from) and its five tests, rewritten against
the library's report. The new-sidecar edit is the Lightroom import's
`starting_edit`; the CLI's own `sidecar_for` stays for `presets
--apply`. Where a new sidecar goes is the one thing the two callers
answer differently: the command has no setting and writes beside the
frame (`Placement::of`), as before; the sheet passes the editor's
setting, and a folder with a `.greycard` already gets its new sidecars
there, as the Lightroom import does (`placement_for`).

Two of the command's lines changed with the move, and we let them. A
sidecar that will not read was said with its path and the reader's
message twice, once in the context the command wrapped it in and once
as the cause; it is said once now ("reading the sidecar of
…/IMG_0001.CR3: sidecar: expected value at line 1 column 3"). And the
warning a raw whose ISO would not probe gave ("no iso from …, the
learned blend keeps its default") is gone: the new-sidecar edit is the
Lightroom import's `starting_edit`, which takes the default blend
without a word, as that import always has. The blend is the same
either way; only the line is lost.

## What a frame has here already

§328 held a frame back when it had a history here. That left a frame
culled here and never developed unguarded: its stars, label and flag
are meta, with no history, so an unforced import laid the profile's
`Rank=0`, `ColorLabel=0` and `InTrash=false` over them, every
RawTherapee profile's untouched values, and a five-star reject came out
unrated and kept, with no line to say so. The review found it. A frame
now has what it has here (`rawtherapee::Here`): nothing, meta (stars, a
label or a flag, what culling sets; keywords and the rest are not a
profile's to touch), or a history. A history holds the frame back
unless forced. Meta holds it only when the profile would replace some
of it: the import is laid onto a copy of the frame's meta first, and a
frame whose stars, label and flag would come out as they are (stars an
earlier import of the same profile set, or a cull that agrees with
RawTherapee's) takes the unforced path, its meta kept and no state for
it unless the profile brings an edit; it is not held, and the switch
does not count it. A frame held says which ("its stars, label or flag
are set here already") and is counted on a held-back line of its own,
the history's line as it was. Forced over meta alone, the edit carried
is what the profile sets, not its neutral settings too, since the
frame's edit is this engine's default; forced over a history, as
before. Either way the report says what the write replaces of the
frame's own culling ("replaces 5 stars to 0, the reject cleared").
The sheet's switch reads "Write over the N frames developed or rated
here already".

The offer counts what Write would write: `waiting_in` is the import's
own dry run over the folder, frame by frame, unforced, and counts the
frames it would write. So a folder culled here is not offered for the
frames an import would hold, and a frame whose profile has come in
already (its meta by an earlier import, and nothing new) is not
counted again: that closes a gap the first cut of this had, where a
meta-only import left no history and its folder, moved, was offered
again for a sheet with nothing to write. The dry run reads each
profiled frame's header for its size, which the count did not before;
it runs on the look's thread, so the window does not wait on it.

## A raw's profile, and its JPEG

RawTherapee names a profile `IMG_0001.CR3.pp3`, or `IMG_0001.pp3` when
told to replace the extension, and §328 looked for both beside every
frame. The second is the raw's name, and beside a raw and the camera's
JPEG of it (a pair, §332), the JPEG took it too, and with it the raw's
lens profile, aberration, white balance and exposure: a JPEG the camera
had already corrected and balanced, corrected and balanced again. The
replaced-extension name is now a raw's, and a picture's only when no
raw of its name is in its folder, so a folder of JPEGs edited in
RawTherapee still finds theirs. A picture's own `IMG_0001.JPG.pp3` is
its own either way. Each folder's raws are listed once a run, and only
for a picture with such a profile beside it. Since a picture can still
have one, the offer says frames, not raws.

## No sidecars, no import

`--no-sidecars` is a run that writes no sidecars, and neither import
sheet read it: Write wrote all the same. Now the offer does not come up
in such a run, and both sheets, this one and the Lightroom catalog's,
show their report with Write off and a line that says why ("This run
writes no sidecars (--no-sidecars), so nothing can be imported."); a
Write asked for anyway (a capture's step) writes nothing. The Lightroom
sheet's write would also have written collections, stacks and roots;
it is refused whole, since its sidecars are its point.

## The offer

**When.** Every list the browser takes ends in `recent::opened`, which
works out the one folder it is; that is where the offer looks, so a
folder opened from the button, the tree, Recently opened or the command
line all count, and nothing else does. The watcher's and the index's
passes merge into the list and never reopen it, so they never ask. A
thread off the window's counts the folder's own frames that an import
without `--force` would write (`rawtherapee::waiting_in`, the import's
dry run: nothing held for a history or a cull the profile would
replace, and something to bring in), and posts the count back. With
any, a line under the folder's name says "RawTherapee profiles found
beside N frames" over **Import...** and **Not now**. Neither the
folder's open nor the index's pass waits on it. A folder is looked at
once a run; opened again, it is offered from what that look found.
Never in a batch run, as the lens offer of §155 is not, nor in a run
with `--no-sidecars`. A folder under a root the window holds offline
is not looked at, and not counted as looked at, so it is looked at
once the root is back: `recent::opened` records an offline root's
folder without a look at the disk, and the count must not be the one
that asks it, since a mount that has stopped answering would hold the
thread, and a capture waiting on it, for as long as it likes.

**Where the answer lives.** In the settings, `rawtherapee_answered`, a
list of canonical folder paths, written as the offer is answered and
kept from the file on the close path as `recent_folders` is. We weighed
a note beside the sidecars, under the folder's `.greycard`, which would
travel with the shoot, and took the settings for two reasons. A
`.greycard` made to hold the answer would change where the folder's new
sidecars go from then on: a folder with one gets its new sidecars there
whatever the setting says (§337's `placement_for`), so a Not now would
move the user's sidecars. And the answer is this machine's user's, not
the shoot's: a folder on a card, a read-only share or an archive the
user only opened should not be written to because they put an offer
away. What does travel with the shoot is what the import wrote: every
frame it brought in has a history or meta and is not counted again, so a
folder imported here and opened elsewhere is not offered there. Moved,
a folder answered with Not now is offered again under its new path;
that costs one more Not now.

**Answered.** Not now puts the line away and says the command still
imports them. A write that brings any frame in answers the folder too.
Import... alone does not: it puts the line away and opens the sheet,
and a sheet canceled, or a write that brought nothing in (every frame
held back), leaves the folder to be offered again the next time it
opens. Opening the sheet is a look, not an answer; the first cut
answered on Import..., and a user who looked and canceled lost the
window's way in. A run with no settings file (a capture, a test) keeps
the answer in the window.

## The sheet

1. *The report.* The import's dry run over the folder, on a thread:
   the totals (frames found and with a profile, meta and edits that
   would be carried, those held back for a history and those for their
   culling, unreadable or failed, each name
   with no place here and on how many frames) over each frame's lines
   (its profile and RawTherapee's version, what is exact, approximate,
   neutral, or has no place, and what would be written or why not).
   The sheet lists the first 200 frames and then a line, "50 more
   frames with a profile, not listed here; the totals count them"
   (`Report::frame_lines_upto`): Slint lays out every line a model
   holds at once, and the archive's largest folder has 466 frames with one.
   The command lists them all.
   When some frames have a history here, a switch to write over them
   (the command's `--force`); turned on, the dry run runs again forced,
   so the report says what a forced write sets, RawTherapee's neutral
   settings among it, and the switch stays to be turned off. Write is
   off until a write would change a sidecar.
2. *Written.* Write runs the import on a thread of its own, its
   progress on the status line and beside the buttons a tenth of a
   second apart at most, the sheet up with its buttons waiting so
   nothing is edited under it. Before it, a panel moved and waiting on
   its save is saved, so that frame has an edit of its own and is left
   alone unless written over (§337's rule). Cancel and Escape close
   the sheet while a dry run reads, whose late answer the generation
   drops; only a write holds it. When it lands, the totals
   say what was written and the frames' lines what each took, and
   every frame written that the window holds is read again from the
   disk and the frame on screen opened again on what was written,
   through the Lightroom sheet's `reread_written`, now shared. The
   status line says "imported the RawTherapee profiles: 3 frames
   written, 1 left alone".

`--sheet rawtherapee-offer`, `rawtherapee` and `rawtherapee-written`
put up the offer (answered or not, in the capture's batch run), the
sheet at its report, and the sheet written as Write writes it, over the
folder open; a capture waits for the look, the dry run and the write.

## The aberration by hand, a part of its own

A preset's part is "one slider, or the few settings that make one
control in the panel"; `Lens: Chromatic aberration` held the lens
profile's switch for the aberration and both amounts set by hand, three
controls. §328's mapper wrote that part whenever it carried a lens
profile, so an unforced import with a lens profile set a frame's
hand-set amounts back to zero. They are two parts now:
`lens.profile-chromatic-aberration`, the switch ("Chromatic
aberration"), and `lens.manual-chromatic-aberration`, the red's and the
blue's amounts ("Manual aberration"). The mapper writes the second only
when the profile sets an amount, or forced with `[CACorrection]` there;
the report names it as before, "chromatic aberration by hand" when
exact and "no chromatic aberration by hand" at its neutral. A preset
saved before the split names the old `lens.chromatic-aberration`, which
is read as both new parts, so it carries what it always carried; the
old name is never written. The copy and paste sheet's Lens column has a
row more. A test lays a lens profile over a frame with amounts of its
own and finds them kept, then forced and found at zero.

## What it did, by hand

A scratch folder of seven frames hard-linked from the test frames,
six with a hand-written profile beside them (one by the
replaced-extension name, beside a raw with its camera JPEG), two
culled here by the command and one developed by it, under a headless
weston with scratch settings, the captures taken by the review on the
build with every fix above: the offer under the folder's name counted
the three frames a write would change; the sheet's report gave 7
frames found, 6 with a profile, 3 metas to carry exactly, 3 edits, 1
held back for its history and 2 for their stars, label or flag, with
the switch over those three; the camera JPEG beside its raw was not
listed. Write wrote 3 sidecars, the strip took their stars and labels,
and the frame on screen came back at the profile's custom white
balance, its history step "Imported from RawTherapee" above the
original. Opened again, every frame was held back and Write was off.
With `--no-sidecars`, no offer, and both sheets showed their report
with Write off and the note.

## Over the archive

The dry run over the author's archive, through the command and so the
library's import, never with `--write`, run again on the build with
the culling hold and the JPEG rule: the 4,436 profiles under 49
folders, every folder's run ending 0 and nothing panicking. 3,304 of
the profiles sit beside a frame and were read, none unreadable, none
held back (no frame there has a history here), 868 with meta and 3,192
with an edit to carry; the other 1,132 have no frame of their name
(766 written beside RawTherapee's own outputs, `*.out.pp3`, and 357
beside DNGs since deleted), and 6,688 frames were listed in all. 21 of
the 49 folders hold a profile beside a frame, so the offer would come
up on those 21. No profile there sets an aberration by hand, so the
split changes nothing on the archive; 3,189 carry a lens profile, each
of which before the split would have written the hand-set amounts at
zero. Neither later fix moves a count: every one of the 3,304 is
RawTherapee's appended name (`IMG_0001.CR3.pp3`), so the JPEG rule,
which is about the replaced one, has nothing to decide, and no frame
there has a sidecar, so nothing is culled here to hold. The outputs of
the two runs are the same line for line but for where the decoder's
warnings on stderr fall among the frames, now that the command prints
a frame as it is done. The names passed over most are Exposure Curve (3,069),
Perspective (963), Directional Pyramid Denoising (566), SoftLight
(516), Vibrance (455) and Exposure HighlightCompr (434). Part 1's sweep
over the same archive counted the same within six frames.

## Left open

- Once a folder is answered, the window has no way back into its
  sheet; a menu entry for the open folder would give one.
- RawTherapee's profiles kept in its cache rather than beside the raw
  are still not looked for (§328).
- Past the first 200 frames the sheet says how many more there are
  and no more; a lazily laid-out list would show them all.
