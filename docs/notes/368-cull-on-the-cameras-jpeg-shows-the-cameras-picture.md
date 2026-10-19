# §368. Cull on the camera's JPEG shows the camera's picture (2026-10-18)

A frame edited to black and white showed black and white in Cull
with the switch on Camera JPEG. The camera's picture was color, and
opened straight into Cull it drew in color; after a develop had
landed it did not. The cause was older than the switch: §270 made
every place culling and the library show a frame show the edit when
one is kept, and the Cull's wants went through the same fetch, which
hands the kept edit picture ahead of the camera's JPEG. Once the
frame's develop had landed and its edit was saved, that picture
existed, and Cull showed it under the name of the camera's.

The switch now means what it says. A Cull want asks for the camera's
picture alone, and the fetch leaves the edit pictures out for it; a
late edit picture landing in a camera Cull is turned away, and a
camera Cull's copy is no longer dropped when an edit picture is kept,
since it has no use for one. Each kept picture remembers which kind
it is, so a cell takes only a picture that fits it and a copy of the
wrong kind stays up until the right one lands rather than going
blank; when the right kind cannot be made, an offline file with no
local preview for its camera's picture, the wrong kind stays, which
is better than nothing. The versions compare is the one exception,
kept: its tiles differ only in their edits, so they show them.
Leaving Cull, the camera copies of edited neighbors are let go before
the cache becomes the develop view's stand-ins, so a stand-in under
Developed is the edit's picture as before, and the frame on screen
keeps its camera copy until the develop or the edit's picture lands. The Developed side puts the kept edit
picture up at once while the develop lands, as the stand-in always
has, so culling on edits stays quick. The strip, the grid, the
lightbox and the develop view's stand-in show the edit as before.
