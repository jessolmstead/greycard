# §729. A large object removed, and the crop's place in a Fill (2026-11-21)

A generative Fill over a large, strongly colored object gave the
object back, a little softened. Two causes, found on one frame.

**The start.** Our remove and the reference inpaint both start from
the window itself with noise added. At the first step about a tenth of
the picture survives the noise (0.11 at the full schedule, 0.23 at four
steps), and for a large, smooth, saturated object that is enough: the
model sees its color and draws it again. We now start from the window
with its hole filled in from its surround by push-pull (Gortler et
al., 1996), so nothing of the object reaches the model. Over three
seeds the hole's mean color distance from its surround fell from
0.035–0.082 to 0.005–0.010, at 512 and 1024 and at four steps. Small
holes come out as before. `FILL_VERSION` is 7, so every kept fill is
made again once.

**Past the crop.** The fill's window read the frame under the crop, so
an object that ran on past the crop's edge was the hole's surround
there, and came back. A fill now sees what the user sees (`Shown`):
what the crop hides of the window joins the hole and the mask, and is
left out of the level match and the grain measure. On the cropped
frame the same distance fell from 0.11–0.13 to 0.002–0.010.

A fill is therefore keyed by what the crop hides of its window too: in
memory, on disk, for failures, and for the reduced read. A crop that
hides none of the window keeps the slot it had, so most fills are not
made again for a crop.

A crop change asks for a develop only when it changes what a Fill's
window shows, and only once the change is over: a handle released, a
slider let go or come to rest, an aspect typed and accepted. While the
fill is made again the old one stands in, under the pending outline. A
fill refused for the card's memory shows the base, not the stand-in.

Left as it is: the stand-in lives in memory only; a window whose whole
surround the crop hides is left smooth, there being nothing to measure;
a remove already running finishes before the next begins; a refusal
for the card's memory is keyed by the window alone.
