# 267. Fit off after a run to its end (2026-10-08)

When a camera match run ended, the sheet planned its groups again
against the looks on disk, and the table the run had just written is
a fit of the current version from as many frames: the store's rule
(§260) lets a fit replace that, so every group the run fitted read
"fits" again and Fit stayed on and highlighted. Pressing it fitted
the same frames to the same table.

The sheet now remembers that a run over its choices went to its end.
Until a choice changes, Fit is off and not the highlighted button,
Close is, and the line under Fit says "Done. Change a group, the scope
or Replace existing looks to fit again." A box pressed, a scope
picked, Replace existing looks toggled or the sheet opened again
clears it. A run that was stopped, or failed, leaves Fit on, since its
choices are not done.

We kept the rule and changed the sheet. A fit from as many frames is
let through on purpose: a library that has grown past the sample's 40
frames gives a new sample under the same count, and that is a fit
worth having. Telling the two apart would mean keeping the sample's
frames with the table, for a case Replace existing looks and a new
choice already cover.

`fit_is_off_after_a_run_to_its_end_until_a_choice_changes` drives the
sheet through a run to its end, a box pressed, and a stopped run.
