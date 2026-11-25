# §745. The sharpen of what is seen first (2026-11-24)

A Clarity, Dehaze, or Detail move on a big frame waited for the
capture sharpen of the whole picture, the dearest stage after it: on
a 102 MP frame a Clarity move took 0.41 s on a 5070 Ti, most of it
the sharpen of pixels off the screen.

**Two rounds.** The view's develop now sharpens first only the tiles
the window shows (a *part*), with fewer iterations the further out
the zoom, never under eight, and shows that. A *settle* then sharpens
the whole picture and takes its place. The same move takes 0.17 s to
show. A part's tiles read only their own padded squares, and the
luminance, the threshold, the blend, and the statistics are the whole
picture's, so a tile of the part is the whole sharpen's tile to the
bit; the picture at rest is the one master made, and the render gate
holds unblessed.

**The first round is never the picture at rest.** It is not kept as
a thumbnail, a preview, or an edited picture; a mask's Detail delta
does not take its planes; the Original key keeps no picture of it;
the capture's `ready` waits for the settle; an export, the command
line, and the prefetch take no view at all. The CPU path does not
quicken.

**Giving way.** The settle gives way to the next develop, and to a
view moved past the first round's tiles, which gets a first round of
its own. A save waits for the newest settle. Leaving a frame with a
save waiting runs the settle inline and saves from it, so the frame
left is the edit, whole. At quit the settle cannot give way, so the
save is made and the stop is clean. An open forgets the view until
the new frame is drawn, and a develop sent before that is sharpened
whole.

**What is seen.** Panning right after a move shows the newly bared
regions soft until their first round or the settle lands. We accept
that for the speed.

Not measured again on the card after the merge with §741; the level
scale and the pedestal pass through the quickened options unchanged,
and the device tests pass on lavapipe and on the 5070 Ti.
