# 58. Object masks with more than one box (2026-09-18)

The log while editing showed `the model answered with the wrong shape:
pred_masks [1, 2, 3, 256, 256]`, then `[1, 3, 3, ...]`, `[1, 4, 3, ...]`,
once for every pointer move, and between them a reshape failure inside
the decoder. That is SAM 2.1's decoder treating each box as an object
of its own: `input_boxes` of shape `[1, n, 4]` gives `n` masks, and
the wrapper expected one. With clicks beside two or more boxes the
model could not shape its prompt at all, since the points come in one
prompt batch and the boxes in another. So an object mask worked for
one box or for clicks alone, and silently gave nothing past that,
which is most of why the tool read as unusable.

`Sam::decode` now groups the prompts into objects: each box with the
clicks inside it (a click in two boxes goes to both), and the clicks
in no box as one more object when a positive one is among them,
negatives alone saying nothing. One decoder call an object, a few
milliseconds each, and the masks joined by the larger logit; the
score reported is the worst over the objects, which nothing reads.
Checked against the model in the store with the disc test: two boxes,
one of them on the flat background, keep the disc at 0.95 inside,
and boxes with clicks at 0.99. The ignored model tests take the store
from `GREYCARD_MODELS` and pass silently without it.

What the bug list still holds for the tool is the drawing: the
viewport shows an outline for a linear and a radial mask, and nothing
for an object's boxes or picks, during the drag or after.
