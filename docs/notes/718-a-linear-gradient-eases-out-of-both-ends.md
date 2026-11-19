# §718. A linear gradient eases out of both ends (2026-11-18)

**What was wrong.** A linear gradient's weight was a straight clamped ramp,
and the jump in its slope where the ramp meets the flat showed as a line: a
dark one along the 0% end when the look brightens, a light one at the 100%
end (issue 182). No pixel was darker than the unmasked picture (0 of 2.7
million on a real sky); the eye makes the line from the crease, a Mach
band. On that sky the second difference at the 0% end was about 30 of
65,535, the ramp's own slope.

**Decided.** The ramp is Hermite's step, `1 - smoothstep(0, 1, d)`, the
easing the radial feather and the brush already use, on the CPU and in the
viewport's shader, the only two places that compute it. The ends and the
50% middle stay; the middle is half again as steep. The second difference
at the ends falls under one of 65,535.

We kept smoothstep over smootherstep: what it leaves cannot be seen, and
smootherstep would steepen the middle further (1.875 against 1.5 times the
straight slope) and differ from the other shapes.

Existing gradients render a little differently, with no version in the
sidecar: the old look was the fault. Kept pictures of an edit that holds a
linear gradient are made again once, through a key part of their own
(`LINEAR_RAMP`), as the whites' and midtones' shapes are; no other frame's
are.

**What it does not do.** A linear gradient that brightens a corner the
lens darkened still leaves a valley near its 0% end: the vignette is still
falling there and the lift starts from nothing, so brightness drops before
the lift catches up, whatever the ramp. The straight ramp put a creased
bottom on the line; the eased one puts a rounded bottom a little inside
it, about seven times gentler, and on a made sky 18% deeper. A radial
gradient, the Vignette slider, or the lens correction is the tool for a
vignette.
