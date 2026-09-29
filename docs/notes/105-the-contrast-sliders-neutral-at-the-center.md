# 105. The Contrast slider's neutral at the center (2026-09-19)

Asked, as a nit: the other Light sliders put their zero at the
center of the track and Contrast did not. Contrast is a slope about
mid grey, 0.5 to 2 with 1 neutral, and the track was linear in that
number, so neutral sat a third of the way along, the left half of
the travel covered 0.5 to 1.0 and the same distance on the right
only 1.0 to 1.5. The range is symmetric in stops, not in the
multiplier: halving and doubling are the same distance from 1.

The fix is in the slider, not the edit. `EditSlider` has a
`logarithmic` flag: the thumb sits at the log of the value over the
log of the range, a press or a drag maps back through a power, and
a nudge (an arrow key, a wheel notch) multiplies by two to the step
rather than adding it, so the step is in stops too; Contrast's 0.02
is a seventieth of a stop a notch, about what the linear step was
at the neutral. The stored value, the sidecar, the shader, the mask
blend and the Lightroom mapping are as they were; the readout still
says the multiplier, so the ends read 0.5 and 2 with 1 in the
middle, which is what the control is. A double click resets to the
default, the center. Checked with a snapshot of the LIGHT section:
the Contrast thumb sits under the others' zeros.
