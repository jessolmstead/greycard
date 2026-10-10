# 271. A person picked on a zoomed view keeps the view (2026-10-10)

**The bug.** Zoomed in on a group, choosing a part from the People menu
and clicking the person it is of sent the view back to Fit. It read as
the People mask resetting the view when it finished, since the mask
takes a few seconds to come and the reset is what the user notices next.

**The cause.** Not the mask landing at all. The person is picked on the
press: the viewport's touch area hands the press to `place-pressed`
while `placing` is "Part", and `parts::pressed` picks the person and
puts the picking down there and then, clearing `placing`. On the
release the touch area looked at `placing` again, found it empty, saw a
press that never dragged, and took it for a plain click on the picture:
`toggle-zoom`, which from a zoom goes to Fit. The droppers had the same
shape of bug once and were fixed by latching on the press
(`picked-down`); the placing tools were never latched, because every
other one keeps `placing` until its release.

**The fix.** The touch area latches `placed-down` on a press that went
to a placing tool or a repair, as it does `picked-down` for a dropper.
Its release goes to `place-released` (which finds nothing in hand and
returns) rather than to the zoom, and a drag after it neither pans nor
draws a crop. A plain click on the picture still zooms in and out.

We checked the landings themselves while we were there. A learned
mask's raster (People, Parts, Subject, Sky) lands in `Outcome::Mask`,
which stores it and redraws and touches neither the zoom nor the
center. The develop after it, the learned denoiser's included, lands in
`Outcome::Developed`, which keeps a zoomed view of the same source in
place, refits only for the picture held from the last file, and
re-centers for a new source size or a view already at Fit. Camera match writes a look and never the view. None
of them shared the cause.

**The test.** `parts::tests::a_person_picked_on_a_zoomed_view_keeps_the_view`
zooms to 2:1 off center, delivers the people found on a picture of two,
clicks inside the first face's own outline on the viewport through the
real pointer events, and checks the part is hers and the zoom, center
and frame size are what they were. It then lands the part's raster and
the develop after it, and does the same for a picture of one, where the
part is made with no click, checking the view each time. Last, a plain
click on the picture goes back to Fit, so the latch cannot pass by
having switched the zoom off. Before the fix it failed at the pick's
release, at Fit and centered.
