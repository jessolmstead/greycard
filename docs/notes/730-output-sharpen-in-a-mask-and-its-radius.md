# §730. Output sharpen in a mask, and its radius in the picture made (2026-11-22)

Three changes to the output sharpen of §663.

**The radius.** It was in pixels of the full-size picture and shrank
with an export, so a web-size export barely sharpened: the one case an
output sharpen is for. It is now in pixels of the picture made: the
export's own pixels, and the window's at a zoom under 1:1. The same
Radius crisps a 2048 px export as it does the full frame. An edit made
before this exports the same at full size and sharper when made
smaller.

**In a mask.** A mask's Detail carries an Output sharpen, −100 to
+100, at the picture's Radius and Threshold (since §736 to +200, and
at the mask's own Radius and Threshold where it has them). The amount at a pixel is

    clamp(base · Π(1 + w·s⁻) + Σ w·s⁺, 0, 2)

with `base` the picture's Amount, `s` a mask's share, and `w` its
weight times its opacity. A negative share scales: −100 takes the
picture's sharpening away under the mask whatever its Amount, −50
halves it. A positive share adds, and sharpens inside the mask with
the picture's own off. Takes multiply, additions sum, and the takes
apply to the picture's amount alone, so the masks' order does not
matter.

A per-pixel amount found a fault the single amount hid. Channels under
zero were held at zero as soon as the amount was above zero, so at a
mask's edge a weight of next to nothing moved an out-of-gamut color by
the whole hold, and two cards disagreed on which side of zero that
weight fell. The hold now comes on with the amount, over 0 to 1%
(`HOLD_RAMP`); from 1%, the panel's first step, nothing changed.
Since §735 the hold is AgX's rail's alone.

**The export sheet's Sharpen.** Both sharpened a smaller export, one
on the other. The sheet's Sharpen is now skipped, frame by frame, for
a frame whose edit has an Output sharpen that acts (the picture's own,
or a mask's positive share), and the sheet says so when one of the
frames to be exported has one. A set mixes: each frame goes by its own
edit, and the file's description names the sheet's Sharpen only where
it ran.

On the Masks tab the label column is at least 100 px, so every
section's tracks start at one x; at the panel's narrowest the tracks
are 32 px shorter there.
