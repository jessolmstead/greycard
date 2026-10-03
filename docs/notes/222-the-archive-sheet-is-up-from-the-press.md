# 222. The archive sheet is up from the press (2026-10-02)

The tester's report: the grid header's Back up button does nothing on
the first click but a sheet flashes; the second click works.

**What the code could and could not say.** Only three things close the
sheet: the look failing or set aside, Cancel in any of its forms (the
button, Escape, a click on the backdrop), and the confirm. A synthetic
click on the button (`--keys click:X,Y`) opened the sheet and it stayed;
a second synthetic click, which lands on the backdrop, closed it, as a
click outside a sheet should. No path from the press to a sheet that
closes itself was found, and no synthetic sequence reproduced the
report.

**What fits.** The look between the press and the sheet asks the
archive's root whether it answers, reads its pairings' folders and stats
every frame's destination, over the wire when the archive is a NAS. It
took its second or two on the first press of the evening, and the only
sign of it was a line in the status plate. A tester who sees nothing
clicks again; the sheet comes up under the second click, which lands on
the backdrop and cancels it: the flash. The third click, with the
archive's caches warm, is answered before the hand moves.

**Now.** The sheet is up from the press, with its title and the
destination as the file has it, "Looking at what is on nas..." where the
plan's lines go, and the confirm reading "Looking..." and disabled; the
look fills it in when it lands, as a look after the folder field is
edited always did. A look that fails, or is set aside at 3 s, closes it
and says why in the status, as before, and now also in the log, which
said nothing of a look that went wrong. A second click still lands on
the backdrop and cancels, which is now a sheet visibly dismissed and not
one that never seemed to come. Remove rejects' sheet keeps the old
shape, up only when its look is in; it reads the same folders and would
take the same change if the report comes back for it.
