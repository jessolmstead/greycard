# 232. Hold hue to white taken out, unreleased (2026-10-03)

§226 added the display curve on a norm behind a switch, "Hold hue to
white", as the first part of the scene-referred tail. It lost on every
frame it was judged on. On the two sets of §230 it was harsh or too
bright in the whites on about forty frames and won once, a neon sign
whose chroma per channel clipped; AgX holds that sign without the
per-channel skew (§231). The
question it asked, a hue held through the roll-off to white, is what
AgX answers (§231), and its fault, the mean norm and the cube's
geometry taking a saturated light's chroma a stop and a half early,
was never going to be fixed by a control once AgX was in.

So it is taken out, not hidden. It never shipped: the switch came
after 0.3.0, and the one sidecar in the library that names it was a
trial of the switch. Hiding it would have kept about four hundred
lines on the CPU (`tail.rs`, `finish::tone_slope`) and a hundred in the
shader for pictures that do not exist, and kept the parity bug's worst
spot in the code while taking it out of the test that would see it
move: two of the three failing seeds in the bug sat at its chroma
weight.

What changes. `DisplayCurve` is per channel or AgX; a sidecar that says
`norm` reads as per channel (a serde alias, pinned by the switch's
test), so such a picture renders under per channel and the switch shows
it so. The LIGHT section's choice is two-way. `--hold-hue` is gone. The
shader's curve mode is 0 per channel, 1 a clip for a picture already
rendered, 2 AgX. The random parity test draws AgX half the time, last
as before. Seeds 1 to 10 pass on NVIDIA and on lavapipe with the new
draw. `tools/compare-transforms.py` no longer writes a hold-hue column,
and `tools/transform-sheets.py` no longer lays one out.

Kept: §226 as the record of what was tried and measured, and git's
history for the code. The one idea in it worth taking again is the
chroma scaled by the curve's slope, which the tail's master curve on a
norm (the roadmap's part (2)) would want, and the slope's formula is
in the history of `finish.rs`.
