# 44. The navigator, and a snapshot of the window (2026-09-15)

**The navigator.** A left panel, new, 240 wide, as Lightroom and
darktable keep theirs (our ask, on seeing it on the right): the
whole frame as the screen shows it, with a rectangle over the part
the view has when that is less than all; a press or a drag in it
puts that point at the view's center, kept inside the frame while the
frame is the larger. The panel has room under it for what else an
editor keeps on its left. The
picture is the scopes' analysis image (§38): the frame drawn 512
wide through the viewport shader, under the edit, the display table
and the proof, which was already made whenever the edit changed and
only binned. It is now also copied to a staging buffer beside the
bins and mapped the same way, a frame later, into a Slint image;
700 KB a change, nothing a frame. So the navigator is exactly what
the viewport shows, small, and costs the scopes' pass nothing more
than the copy. The rectangle is worked out each frame from the
view's size, zoom and center as fractions of the frame, and drawn by
Slint over the picture, which is shown at the panel's width, or no
taller than it is wide for a portrait frame, centered.

**The snapshot.** `--screenshot` reads the viewport's texture back
and has never shown the panel, so the panel's look was always
our call. Slint's wgpu renderer can render the window to a
texture: `--snapshot PATH` takes it a moment after the first develop
is on screen, panel and filmstrip included, then quits. This is how
the navigator, the clipping marks and the histogram's corners were
checked; the sections below the fold still are not, since the
snapshot cannot scroll the panel.
