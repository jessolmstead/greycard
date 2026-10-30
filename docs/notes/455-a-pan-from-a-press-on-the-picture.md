# §455. A pan from a press on the picture

*2026-10-29.* In the Loupe a drag that began on the dark canvas round
a picture smaller than the viewport panned the picture (discussion
51). The pan branch of the viewport's pointer handler took any drag
with no tool in hand; nothing asked where the press landed. Now the
window answers a pure question for the press, whether the point
falls inside the placed picture through the same zoom, center, and
mat the shader draws with (`zoom::on_picture`), and the answer is
latched for the drag: a press on the canvas pans nothing, a press on
the picture pans wherever the pointer goes after. A picture carried
wholly out of the view counts every press as its own, so it can be
dragged back (`zoom::in_view`). The zoom toggle's click uses the
same question in place of its own copy of the mapping. The crop,
the retouch, the brush, the picker, and a mask's drag take their
branches before the pan and are as they were. The cursor over the
canvas is the arrow, re-read on the next move, so a key zoom under
a still pointer can show the old one until then.
