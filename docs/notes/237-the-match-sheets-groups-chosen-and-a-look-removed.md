# 237. The match sheet's groups are chosen, and a look can be removed (2026-10-03)

Two roadmap items from the Color track, built together because both are
about what the user decides to keep of a fitted look. Built by a sonnet
author and read twice by an opus reviewer.

**A box on each group.** **The choice.** Each body-and-style line on
the match sheet has a box, all checked at first. Fit runs over the
checked groups, and its button counts them ("Fit 3 groups", "Fit 1
group"); at none it is off, and a line under it says "No group is
checked". A folder of two bodies and five styles is ten fits of forty
frames each, and the user who wants one style no longer pays for the
other nine.

**An unchecked group is skipped as a small one is.** Not developed for
itself, never fitted, never written. It is still a line in the report
("not chosen, so not fitted"), as a too-small group is, so the list of
results accounts for every group on the sheet. `plan_chosen` takes one
flag a group (a missing flag is a yes, so the old `plan` is the same
call with everything checked) and the run takes the same flags, so the
sheet's plan and the run's cannot disagree.

**A borrower whose donor is unchecked still borrows.** The donor is
chosen once, on merit, from every group that could fit, checked or not
(§181): if unchecking a body moved its neighbors to another donor, the
table a borrower gets would depend on which boxes were ticked, and the
line the user read before the run would not be the table that came
out. A donor's frames are only read, so the run develops the unchecked
donor and fits its table in memory for the borrower (`Plan::Donor`),
and writes nothing of the donor's own. Both lines say so: the
borrower's ("that group is not being fitted, so its frames are read
for this look and its own table is not written") and the donor's. The
cost the user should know of is that the donor's forty frames are
still developed; the sheet says so rather than hiding it, and
unchecking the borrower too skips the donor entirely, since nobody
then needs it. A borrower that is unchecked borrows nothing and does
not make its donor read.

**Remembered per scope.** `settings.json` gains `match_unchecked`: a
list of `{scope, unchecked: [look names]}`, "library" for the library
scope and the folder's canonical path for a folder, the most recent
scope first and at most forty (the recent-folders rule: a list that
grows with use is capped). What is kept is the unchecked names, not
the checked ones, so a group not seen before is checked without any
special case, and a scope with everything checked keeps nothing.
Written as each box is ticked, as the other choices that should
survive a crash are (the last file, a folder as it opens), not
gathered at the close; the close's own write carries it over from the
file as it does for those, since the panel does not own it. Not read
or written in a batch run or a test, as the lens offer's answer is
not.

**The refit.** §235's "Refit under AgX..." opens the sheet narrowed to
one look's groups (that look's own group, plus the donor a borrowed
look needs). We kept that as it was: the sheet lists only those
groups, all checked, and remembers nothing, since the narrowed list is
not the scope's choice and a refit that unchecked its donor would
leave the look it was opened for with nothing to borrow. The boxes
still work in it, for the session, which keeps the code to one list
and one rule.

**A fix on the way.** The list's box took its height from the list's
own preferred height, which is computed before the lines wrap, so a
box could be as tall as one line a group and hide the rest. It now has
room for two or three lines a group, up to the same 200 logical
pixels.

**Removing a look.** **What goes.** A look is a name with up to one
table per display curve (§235), so removing one removes every table of
it together: `look::tables_of` is the one question, so `Neon.agx.cube`
goes with `Neon.cube` when its header declares AgX, and a film preset
someone called `Neon.agx` is a look of its own and stays. Only files
whose folder is the look folder are listed, and the move checks them
against it again (the delete's own check, which also refuses a link).
None has no files and no button.

**The sheet.** The Look section gets a "Remove this look..." button on
a row of its own (three buttons do not fit the panel's width), for a
chosen look that has files. The sheet names the look, lists the files,
and counts the pictures in the open folder that name it, from the
edits already in memory (the open picture by the panel's edit, which
may be ahead of its sidecar). A frame whose edit still stands in from
the index's row has no look in it, so the sheet counts those apart and
reads their sidecars on a thread of its own, updating its line when
the count lands ("Reading the edits of 12 more pictures..."); the
window's thread does no new disk walk. Return answers yes once and
Escape no, the sheet taking the keys with a FocusScope that focuses on
init (§234), added to `sheet-open()` with the window's `changed
look-remove-open` handler and Escape row.

**The move.** The files go in one call to the system trash through
`delete::delete`, the same function the delete sheet uses, with the
trash handed in so no test touches the user's: the look's own file is
the "frame" and the other tables its "sidecars". That brings the rest
of the delete's rules for free: where there is no trash the sheet says
so and deletes only on a click on the red button, never on Return; a
trash that refuses stops the move and remembers the folder, so the
next sheet offers the permanent delete beside it; nothing is deleted
in a capture, an export or a timing run.

**After.** The look list is read again, the cached tables are dropped,
and the viewport's look is dropped, so the open picture that named the
look shows it as "(missing)" and renders without it, as it would one
that came from another machine. No sidecar is rewritten: a sidecar
that names a removed look is telling the truth about what its picture
asked for, and putting the look back in the folder brings the picture
back as it was.

**Not done.** The count is of the open folder's list. A look used by
pictures elsewhere says nothing of them, and the sheet says only the
folder's.

**After review.** **The sheet reads the folder, not the list it was
handed.** The panel's look list is read again only when the LOOK
section opens, a run ends or a removal lands, so a table added since
(a second editor, a copy by hand) would have stayed behind as an
orphan variant that still listed as the look. `ask` lists the folder
itself, and the files the sheet shows are what is there.

**A link is never offered.** The move refuses a link (its target is
not the look folder's), so offering one would end in a refusal. The
sheet leaves links out and says "<file> is a link and stays"; a look
of nothing but links gets the same words in the status line and no
sheet. A refused trash names the table that was refused: the variant
when the look's own file had gone.

**The sheet's words** use the look's shown title, as its row does, and
say "in the library" in the all-roots view and "in this folder"
otherwise.

**Fit is honest about donors.** The button counts the checked groups,
and when unchecked donors are developed for a checked borrower, the
line under it says how many ("Also reads 1 group that is not checked,
for a borrow").

**The choice's key and its reads.** The scope key is the folder as the
browser spells it, not canonicalized: a call to the disk on the
window's thread would hang on a root that does not answer (§215,
§218). The choice is read in every run, so a capture shows what a
scope was left with; only the write is skipped in a batch run.

**Tests never read the user's settings.** `settings::path()` is none
in a test build unless the test points the thread at a scratch file
(`settings::use_file`), so `Settings::load()` gives the defaults and
`save()` writes nothing. A previews-cap test had been reading the real
`settings.json` through that fallback. The one test of the real path
logic calls it directly.