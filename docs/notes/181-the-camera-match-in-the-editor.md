# 181. The camera match in the editor (2026-09-26)

The user-facing half of the camera match (§180): the tags in the index, the
runner, the Look section's action, the picker's note and the facet.

**The tags in the index, by migration rather than rebuild.** The
library's rule was that an older schema is dropped and rebuilt, since
the index is a cache. The maker's tags are the first change where that
is the wrong trade: a rebuild of a large library rehashes and reprobes
every file, hours, to add four columns that each want one small read of
the maker note. So schema 3 adds `maker`, `style` (the group key,
"Canon Faithful"), `style_fixed` (a fixed style with every adaptive
setting off, `CameraStyle::is_fixed`), `peripheral` (the camera's own
vignetting correction, null where the file does not say) and
`style_read`, and a schema 2 library is brought up in place by
`ALTER TABLE` under the same write lock the making takes. Every
migrated row has `style_read` at zero, and a pass that finds a file
unchanged reads its tags and nothing else: the report counts those as
`styled`, and the editor carries that count through a tree pass and says it in the index line. The rule for next time is in the code: a step that only adds
columns a pass can fill without rehashing is a migration; anything else
still rebuilds. A reader cannot migrate, and says so, as before. The
tags are read for raws only; a picture has no camera JPEG to fit
against. A file whose tags will not read is logged, not an error in the
report, and is not read again until it changes, as the probe's rule is.

Two queries serve the match: the groups (make, model and style, with
their fixed and unfixed counts) under the roots or the whole library,
and one group's frames by date with their lens and whether each is
fixed. A third counts the raws whose tags are not read yet, so the
sheet can say the next scan will fill them.

**Where the runner lives.** In the editor, `camera_match.rs`, not in
`greycard-edit`: the develop it needs is the export's own develop of a
frame that is not open, `develop_job` and the finish the export uses,
which live in the editor's worker with its AI and lens plumbing. The
worker gained `FrameDevelop`, a frame developed once on the CPU and
finished as often as asked, and `write_export`'s finishing was split
out so the export and the match run the same code. That matters for
the exposure step: the offset goes through the Exposure slider, which
is the finish, so a frame is developed once and finished twice, not
developed twice.

**The sequence per frame** is the trial's: the default edit (lens
corrections on, the learned denoiser off as default), finished at a
2048 long edge as a 16-bit sRGB TIFF would be with no output sharpening;
the camera's JPEG from the raw, turned by its orientation; the
registration, dropping a frame under the correlation cutoff; the block
pairs; the median offset in stops; the finish again at that offset;
the pairs again under the same registration, since the exposure moves
no pixel. On the test folder in release that is about 4.5 seconds a
102 MP GFX frame and less for the Canons, and every group's first frame
registered but one R5 Mark II frame at 0.86.

**Sampling.** About 40 frames a group: the fixed frames alone once
there are 20 of them, else every fixed frame and the rest from the
frames with an adaptive setting on. Within that, up to three from
every folder first, evenly through each, then the rest evenly through
the dates, so one long session does not fill the sample and a folder
of five frames is not lost among a thousand.

**Fit, borrow, skip.** A group whose candidates reach 20 is developed
and fitted if 20 register; the table is written as
`<camera> <style without the maker>` ("Canon EOS R6m2 Faithful"),
sanitized for a file name, written beside, flushed and renamed so the
picker never reads half a table. Its `TITLE` is the name with
"(fitted on <camera>, <n> frames)" after it; the picker shows the bare
name when the title only repeats it. The reported error is the fitted
mean ΔE and the mean over four frames each held out of its own fit,
which is four more solves a group, a few seconds in release, and the
result line says it is over four frames. A group under 20 whose maker
and style another body in the run fits borrows that table under its
own name, the title naming the body it was fitted on, and its own
frames are developed to measure the borrowed table on them, which is
the number the user wants before trusting a borrow. The donor is
chosen once, before the run, on merit: of the bodies that will fit
the style, the one with the most frames in the scope, the first on a
tie; the plan's line names it and the run borrows from that one and
no other. A group under 20 with no donor is skipped without
developing anything.

**What a run may write over.** A folder run or a borrow could
otherwise replace a better library-wide fit, or a table the user put
in the look directory under the same name. So the plan reads the
store first, and the title's body and frame count decide: a fit of a
body replaces that body's own fit only when it has at least as many
frames as the one there (asked again after the run, with the frames
that registered, since some candidates may not); a borrow never
replaces a body's own fit; a table whose title the match did not write
is never replaced. A table borrowed from another body is replaced by
either. Each refusal is the group's plan line and its result, and the
table is left as it was. "Replace existing looks" on the sheet, off by
default, turns each refusal into a replacement with the warning on
the line.
The radial term is regressed per lens within each group under that
group's model and reported as corner minus center; it goes nowhere
near the table.

**The sheet.** "Fit this camera's look…" in the Look section opens
it. The library is the default scope where one is indexed, else the
open folder, and the sheet says why. A folder under a root is surveyed
from the index, its own files only, when the index has a row for every
raw the folder lists and has read the tags of all of them; a raw
dropped in since the last pass sends the survey to the files, which at
folder scale is a second or two. Both surveys count the raws with no
fixed style (an adaptive one, or a maker whose styles are not read),
and the library's count of raws not read yet counts raws only, not the
pictures a migrated library also marks unread.
Each group's line says its frames, how many are fixed, and whether it
will fit, borrow (from which body) or be skipped, the adaptive frames
left out, and "all from one folder, so the fit will be narrow" where
that is so. Fit runs on a thread of its own with a Stop between
frames; each group's result lands as a line, and the picker is read
again at the end.

**The picker's note.** When the chosen look's title names a body and
the open frame is another, the Look section says "fitted on <body>".
Nothing for the same body, for a look with no body in its title, or
for a borrowed look, whose row already shows its whole title. A look
is never swapped for the frame's own body's; a preset carries it by
name as before.

**The facet.** The filter bar's facets are data-driven, so style is a
facet beside camera and lens in a few lines. One thing was not: the
bar's filter held its chips in an array written out as six long, which
matched `Facet::ALL` until Style made seven, and would have panicked on
the first chip read. It is sized from `Facet::ALL` now, so the next
facet cannot outgrow it.

**Not done.** A fitted group's error held out over every frame (the
four-frame figure is the cheap stand-in); the radial report into the
lens track's vignetting line; typing `style:` in the filter text (the
facet is chips only, like the other facets).
