# §296. The mixer's dropper is one pick (2026-10-11)

Issue #20: the color mixer's dropper stayed in hand after a pick, so
the next click on the picture picked again, and the cursor stayed a
dropper. With the view resized, a click meant for the picture became a
mixer edit, which then rode into exports.

Of the five droppers, two were already one pick: White and Range each
call `stop_placing` from their press. Two stay in hand on purpose
until Esc or their button: the Curve's, a point a click, and the
Defringe's, so several fringes can be tried with the pass off. The
Mixer's was the odd one out: it is a gesture, not a click (the press
picks the band, a drag up or down sets its saturation and sideways its
hue), so it cannot put itself down on the press the way White does. We
put it down on the release instead: `pick_released` takes what the
press chose, and when that was a mixer band it calls `stop_placing`
after the view is refreshed. A press off the picture leaves nothing
chosen, so the dropper stays in hand for a second try.

Its button again and Esc already put it down through `pick_started`
and `stop_placing`; the status hint keeps "Esc to finish", as White's
and Range's say, since Esc puts the tool down and undoes nothing.

A panel test, `a_pick_puts_the_mixer_dropper_down`, drives the
installed callbacks: the button, a stand-in for the press (which needs
a developed picture to sample), a drag, the release, then the button
twice and Esc.
