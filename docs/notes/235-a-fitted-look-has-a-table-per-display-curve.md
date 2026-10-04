# 235. A fitted look has a table per display curve, and knows its body (2026-10-03)

§230 made the camera match depend on the display transform: a table
is fitted on what the display curve renders, so it corrects that
curve's faults toward the camera's JPEG, and under another curve it
corrects faults that are not there and misses the ones that are. The
call was that a fitted table records the curve it was fitted under,
the finish applies it only to a picture on that curve, the Look
section says so on a mismatch and offers the refit, and the tables
fitted so far, which say nothing, read as per channel. This section
is how that landed, with one change to the call (a table per curve
under one look name, rather than one table that a refit replaces),
and with it the roadmap's look list by body, which needed the same
header.

**What the fit was really fitted under.** §230 said the fit action
"develops through the editor's own pipeline, so it fits under
whatever transform the run is set to". Half of that was true. The run
develops each frame through the export's own develop and finish
(`FrameDevelop`), and the finish reads the edit's display curve, so a
frame finished under AgX is the AgX picture. But the edit it finished
under was `Edit::default()`, always: nothing in the sheet or the run
carried a curve, so every table ever fitted was fitted under per
channel whatever the open picture was on. That is also why "untagged
reads as per channel" is not a guess about the old tables but a fact
about them. The run now takes a curve: the sheet reads it from the
LIGHT section's switch when it opens (the open picture's, and the
switch rather than the recorded edit, so a curve switched a moment
ago counts), says "Fits under AgX, the open picture's display curve"
on a line of its own, develops every frame under it, and the same
value is what the table declares. A test finishes one synthetic frame
under each curve through the fit's own edit and finds them 25 levels
apart on average, so the curve does reach the render the fit sees;
the review confirmed it on a real R5 Mark II frame.

**The header.** The `.cube` format has nowhere to put this but
comments, and core already honors two of its own (`# encoding:` and
`# primaries:`). A fitted table now carries, under the line
"fitted from the camera's embedded JPEG by greycard-match" that every
table the match has written since its first commit carries:

    # display_curve: agx
    # make: Canon
    # model: EOS R6 Mark II
    # style: Canon Faithful
    # fitted_on: Canon EOS R6 Mark II
    # frames: 40

The key is the edit schema's field name and the value the sidecar's
word for the curve (`channels`, `agx`), both taken from serde so the
table and the sidecar cannot drift apart. Read back, the words the
editor shows ("Per channel", "per channel", "AgX") are taken too, in
any case, since a table tagged by hand is tagged in whichever words
its author saw; a word this build has not got, from a table fitted by
a later build under a transform this one lacks, reads as "made for
something else" and the table is off under every curve here. `make`
and `model` are the body the table is for, as the decoder names them,
which is how the index and the open frame name a body too; for a
borrowed table that is the borrower, and `fitted_on` the donor, so
the two facts the title ran together are apart. `fitted_on` and
`frames` repeat what the title says, so the store's rules about what
a run may replace (§181) now read them from the header and fall back
to the title only for a table from before it.

**The core rule held.** Core's LUT reader learned nothing about
curves, bodies or the match. It keeps every comment line above the
table as text (`Lut3d::comments`, and the same on `Info` so a listing
has them from the header read alone) and offers `lut::declared(key)`,
the value after `key:` or `key =`, last one winning as the encoding's
does. What a key means is the consumers': `greycard-edit` reads the
curve and the body, `greycard-match` writes whatever key and value
the editor hands it. The match crate cannot depend on the edit
schema, so the marker line lives in both crates as a constant and a
test in the editor holds them to the same text.

**One look name, a table per curve.** The first cut had a refit under
AgX write over the per-channel table of the same name, with a
warning. That is wrong once AgX becomes the default: sidecars written
before the flip keep reading as per channel (§231's plan), and every
one that names a fitted look would silently lose it the day its table
was refit. So a look is a name with up to one table per display
curve, and a run never touches another curve's table. The edit, the
sidecar, presets, sync and snapshots still name one look; the curve
picks the table.

The files: the per-channel table is the look's own file, `<name>.cube`,
which is where every table fitted so far already sits and what every
sidecar already names; another curve's is `<name>.<curve>.cube` beside
it, `Canon EOS R6 Mark II Faithful.agx.cube`, the curve in the
sidecar's word. We chose a suffix in the same folder over a folder per
curve or one file holding several tables: the store stays one flat
folder the user can see and copy, a look's tables sort together, the
old files need no move, and a `.cube` stays a plain `.cube` any other
application reads. A file is taken as a look's variant only when its
stem ends in a curve's word *and* its header declares that curve, so a
film preset someone named `Neon.agx.cube` is a look of its own called
"Neon.agx", not a variant of a look called "Neon"; the listing and
the finish apply the same test. A variant for per channel
(`<name>.channels.cube`) is read if someone writes one, but the match
writes per channel's to the look's own file.

**Resolution, and the gate.** `LookLut::look_under(curve)` is the one
call both consumers make, the viewport's cached look (keyed now on the
choice and the curve together) and the export's: the look's variant
for the curve when there is one that declares it, else the look's own
file through `look::gate`, which keeps it only when its table applies
under the curve (`MadeFor`: the curve it declares; per channel for a
table with the match's line and no declaration; any for the rest). So
the decision is made once, before either the CPU finish or the shader
is handed a table, and no new arithmetic exists on either side to
drift: where the picture's curve has no table, both are simply handed
no look. The agreement test renders the parity frame on the GPU and
the CPU under both curves with a table fitted under AgX and a general
one, and finds the fitted table level for level absent on per channel
(the GPU's picture equal to its no-look picture, the CPU's likewise),
15 levels from no look on average where it applies, and the GPU
within 0.57 of a level of the CPU in every case. In the editor under a
headless weston, on a copy of the user's looks with an AgX table added
for the R6 Mark II's Faithful: on an AgX picture that look moves the
viewport 5.9 levels on average (its AgX table), on a per-channel
picture it moves it 1.6 (its own table, a different correction), and
the R5's Faithful, which has only a per-channel table, leaves an AgX
picture identical to no look (mean and largest difference 0).

**A third party's table keeps applying.** The gate is about fitted
tables, and a film preset or someone else's `.cube` is a preference
laid over whatever the picture is; it must not stop applying because
the user switched to AgX. We tell the two apart by the match's own
comment line, not by the title: the line has been on every table the
match ever wrote, it is a comment no one else writes, and it is not
the text the picker shows, so a user who edits a fitted look's title
does not turn it into a film preset. A title in the match's shape on
a table without the line is not taken as fitted, by the gate, the
list or the store's rules: a run will not write over such a table
unless "Replace existing looks" is on. The one exception is
deliberate: a table from anyone that declares `display_curve` is
gated by it, since the declaration is exactly the statement "made
under this transform", and a colorist who writes one wants it
honored. A HaldCLUT has no comments and is always general.

**On a mismatch.** When the chosen look has no table for the
picture's curve the Look section says "No table fitted under AgX for
this look yet, so it is off for this picture; it has one fitted under
per channel.", and below it "Refit under AgX…" opens the camera
match's sheet for that look. A look someone else made for another
curve gets "Made for per channel, so it is off for this picture under
AgX." and no button, since there is nothing of ours to refit. A batch
export logs the same words when it writes a picture without its look
for this reason.

**The refit.** The run's curve is the picture's, and the run is
narrowed to the group that makes the look: the group whose name is the
look's, or whose name the look's title gives before "(fitted on", or
whose body and style the look's header declares. So a look the user
renamed is still found, and its new table is written under the name
the user gave it (`My R6.agx.cube`), beside the one it has. When the
group has too few frames for a fit of its own the run is it and the
donor a borrow would take, by the plan's own rule (most frames, first
on a tie), and the donor gets its own table for the curve on the way.
When nothing in the scope makes the look (a table whose body cannot
be read, like an early trial's titled in a spelling no body in the
index has, or a body whose frames are not in the open folder) the
sheet says so and Fit is off: a run over every group is the sheet's
own job, not the refit's. Within one curve the §181 rules stand; the
only new refusal is a table that declares another curve sitting where
this curve's would go, which only a hand could put there.

**The look list by body.** The list is grouped, one row a look
whatever tables it has: None, then the open frame's own body, open and
marked "this camera", then every other body that has a fitted look,
folded with its count, then the general looks (film presets, anyone's
`.cube`, and a fitted table whose body cannot be read) in a group of
their own, open. Each fitted look's row names the curves it has a
table for ("per channel · AgX"), and a look with none for the
picture's curve is dimmed, not hidden, so the refit is found from the
list as well as after picking it. A group the user opens or folds
stays so for the session, whichever frame is open (the first cut
stored a flip from a default that depended on the frame, so a group
opened on one body's frame showed folded on its own); the group
holding the chosen look is always open, and a click on its heading
changes nothing and is not kept. With no fitted table at all there
are no headings and the list is the plain one it was. The camera
profile list hides other cameras' profiles behind a count because a
profile is a measurement and does not apply to another body (§122); a
look is a preference and does (§180), so the other bodies fold rather
than vanish.

A look's body is what its header declares. A table from before the
header has only its title, and an old title names the body the table
was *fitted on*, which for a borrowed table is the donor: the user's
store has R7 and R8 Faithful borrowed from the R6 Mark II, and taking
the title's body put them in the R6 Mark II's group and called the R7
table "this camera" on an R6 Mark II frame. So the title's body is
taken only when the look's name starts with it; otherwise the longest
known body the name starts with, the known bodies being the open
frame's and those the other tables name; otherwise none, and the
table goes with the general looks rather than under a guessed body. On
an R7 frame the R7 table is in the R7's group; on any other it is
general until a refit gives it a header. Reading the index's list of
bodies would place it everywhere, at a query on the panel's thread;
we left that out. A name matches a body only where the body ends at a
word that is not "Mark", so an R6 Mark III table borrowed from an R6
Mark II is not put under a plain R6 when no R6 Mark III frame is open.

The grouping is a pure function over the listing, `look::grouped`,
and its order, its folding and the old borrowed tables are tested
there rather than by clicking.

**Renaming a look.** The roadmap has "Rename a look". A look is now a
name over several files, so a rename must move the look's own file
and every `<name>.<curve>.cube` with it, together, and the sidecars,
presets and snapshots that name it. A rename that moved only the
own file would leave the AgX table behind under the old name, where
it is a variant of a look that no longer exists and is listed as
nothing. The refit already copes with a look renamed by hand in the
folder (it finds the group by the title or the header and writes
under the new name), but the rename itself is the place to keep the
files together.

**Not done.** A refit runs over the scope the sheet opens on; the
library is the default where there is one, and a refit from a folder
without the body's frames says so and fits nothing. Old borrowed
tables are general on other bodies' frames until refit, as above.
