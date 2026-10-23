# §400. A mask's white as an offset from the global

*2026-10-23*

A mask's white balance named an absolute light: a mask at 3200 K
stayed a 3200 K lamp when the global white moved, so the masked area
held still. A tester asked for the other behavior as well: a mask that
is so much warmer than the global, whatever the global is. The
section gets a Follow global toggle.

## Modes, not a flag

The offset is two new modes of the mask's white, `relative` and
`kept_relative` (switched off with the offset kept), beside the
existing absolute and off. A flag beside "absolute" would have had an
older build read an offset like -1500 as a real light and clamp it to
2000 K; every build since 0.4.0 reads a mode it does not know as off,
so an old build drops the offset silently and renders nothing wrong.
The schema version stays, since no existing field changes meaning.

## Mired

The offset is stored and shown in mired, not kelvin. In kelvin, "800 K
warmer" is about 75 mired at a 3000 K global and 15 at 7000 K, so the
mask's warmth would change several-fold as the global moved and the
toggle would not do what it says; and a kelvin number that drifted on
its own as the global moved would read as a bug. The sync sheet's
relative white is in mired already, with the same sign (plus toward
the higher kelvin), so the repo has one meaning of "relative". The
slider runs ±450 mired, which covers the range's whole span, and
reads "+25 mired"; tint stays a Duv difference.

## Exact at the global

Switching the toggle converts the current values at the panel's
current global, through 1e6/K, so nothing moves on screen; a zero
offset is the global exactly, and the sum is clamped to the range
widened to include the global itself, since an As shot can resolve
past the panel's range (tint beyond ±0.05 on some profiles). The
dropper stores the clamped light it finds minus the global. The
double-click reset turns the toggle off.

The mask's effective light takes the global from the panel's current
white, not the last landed develop's, so while the global slider
drags, the masked area moves with the rest of the picture instead of
holding still and catching up in steps when a develop lands. The
reviewer found that one; the first version read the base's white.

## Not done

- A relative mask applied through a preset or the copy sheet carries
  its offset, which is the point; the Lightroom and pp3 imports write
  absolute lights, unchanged.
- The offset is stored to the slider's precision only on the slider's
  own moves; a conversion can leave a fraction that displays rounded.
