# §748. A shape Esc put down is out of hand (2026-11-26)

§545 let Esc put a chosen Linear or Radial shape's handles away so
the picture could be seen, and brought them back with a press on the
shape's body. That made the body a trap: with nothing drawn, a drag
meant to pan the picture over a gradient's band moved the gradient,
and Space did not help, since with no tool in hand it is the 1:1 key
(discussion 178).

**The rule.** Handles down, the picture behaves as with no shape
chosen. A press or drag anywhere on it pans, the body shows no move
cursor, and nothing on the picture brings the handles back. The
shape stays chosen in the panel, so its sliders still act on it.

**What brings them back** is an ask from the panel: a press on the
shape's row or its mask's row (also when that mask is already the
chosen one), a tool taken up, a new shape, another shape, mask, or
frame. An undo that lands on the same shape leaves them down.

One flag carries it: the test for a live body (`mask-body-live` in
the viewport) asks for the handles to be up, which gates the cursor
and the press together, for both kinds of shape. Brush masks, the
retouch tools, and the middle button's pan never went through it.
