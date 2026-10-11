# §303. The brush and the patch size by the wheel's accumulated delta (2026-10-11)

With the brush or the retouch patch in hand every wheel event used to resize it by 1.1x, so a touchpad scroll, which arrives as many small deltas, resized it a dozen times in a gesture. We now size it as the slider takes its notch: a step of 1.1x for each 60 px of vertical delta, a wheel click's size, added up across events. One click is one step as before; a touchpad gesture of 60 px is one step.

The count starts over when the direction turns, so a small scroll and its reversal do not step, and when the brush is put down or the retouch tool changes. The clamps are unchanged. The one accumulator serves both tools, since only one is in hand at a time.
