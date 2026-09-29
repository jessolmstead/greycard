# 135. The dark window behind the camera's picture (2026-09-21)

A tester arrowing through a mixed shoot saw it on the Fujifilm and
the Sony frames and not on the Canons: "the preview doesn't really
load and it is dark until the full photo develops". The camera's
picture was not the culprit — culling draws those frames, and so
does the placeholder; what was wrong is what the viewport does in
the moments when no camera picture stands in.

**What went dark.** A select clears the frame's source size, which
§131 wanted so that a turn taken before the develop lands cannot map
this frame's masks by the last frame's shape. The viewport read the
same field for the picture it was drawing — which at that moment is
not this frame at all but the develop still on the GPU, under the
edit it was made with, `held` for exactly that purpose — and
`Geometry::frame(0, 0)` is a rectangle one pixel across. Every pixel
of the viewport falls outside it, so the shader returns the canvas,
and the window goes to flat grey until a develop lands. The
navigator goes with it, drawn from the same frame.

So §134's sentence — "the wait is given up, the picture on screen
stays where it was" — was not true of any of the three cases it was
written for. A raw with no embedded preview, a decode that came back
with nothing, and a develop that failed all took the placeholder
down onto a dark window rather than onto the picture that was there.
A JPEG or a TIFF, which gets no placeholder by design because its
develop is the same decode, was dark for the whole of that develop:
on a 10000 × 6667 plate, a second of grey where the frame before it
should be standing. And every select had a flash of it, from the key
to the frame that shows the camera's picture — 40 to 270 ms here,
which is the tenth of a second §134 measured and called an
improvement, drawn as grey rather than as the picture it improved on.

**Why the Canons were fine, and why they were not.** They were not:
they flash the same. What differs is how long the flash lasts, and
it is the file and not the make. The camera's JPEG cannot be had
until rawler has mapped and populated the whole raw, so the wait
before the placeholder goes up rises with the file: the Fujifilm GFX
frames in that folder are 86 to 96 MB and the Sony 37 against 17 to
25 for the Canon CR3s, and those are also the frames whose develops
run longest, so the grey has the most room to be noticed. The same
folder holds two Nikon Z frames this build cannot decode at all
(`HighEfficencyStar` compression), and there the failed develop took
the placeholder down and left the grey window up for good, which is
the "it hangs" of the same report.

**The fix.** The size the viewport draws from is its own question,
and it is asked of the picture that is on the GPU rather than of the
frame that has no picture yet: `placeholder::drawn_source` takes the
open frame's own developed size when it has one and the size of the
develop on screen until then. The open frame's size still goes to
nothing on every select, so nothing that measures *this* frame —
the mask map above all — can take the last frame's shape for it.
`State::shown_size` is written where a develop is handed to the
window, beside the `pending` picture it will upload, so it names
whatever is about to be drawn whether that is the open frame's
develop or the one before it.

**Tests.** Pure: the rule itself, and why it is worth having — the
open frame's size alone sends `Geometry::frame` to one pixel, which
is the canvas over the whole viewport. Through a headless window: a
develop delivered, then a frame chosen whose file has no camera JPEG
in it, and the size the viewport would draw from is still the
picture on screen. Checked in the editor besides, on a folder of a
CR3 and a 10000 × 6667 TIFF, since a picture file gets no
placeholder and its develop is long enough to catch: before, the
viewport is the canvas with "decoding..." under it; after, the cliff
that was there is still there, fitted, with its navigator and its
histogram, until the plate lands.

**Left out.** A frame whose embedded preview cannot be read still
says nothing about it in the develop view — the culling loupe names
it ("no camera preview (...)") and the log warns, but the status
line here just says "developing...". And the two Nikon files remain
undecodable; that is rawler's to fix, and upstream's.
