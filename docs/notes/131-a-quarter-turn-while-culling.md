# 131. A quarter turn while culling (2026-09-21)

A camera's orientation tag is wrong often enough — a frame shot
straight down, a body with no sensor for it, a scan — that a culler
wants to put one right while arrowing through the shoot, without
leaving the mode and without developing anything. `[` turns a
quarter left, `]` a quarter right, on the selection, in the culling
loupe, the loupe proper and the grid alike.

**Where the turn lives, and why it is not an edit.** On the
`Sidecar`, beside the meta and the XMP mark: `turn`, 0 to 3 quarter
turns clockwise *on top of* the camera's own tag. §117's argument,
one field along. A rating is a judgment about a picture rather than
a step in developing one; a turn is a fact about the picture the
file got wrong, which is the same kind of thing. Put it in the
`Edit` and Ctrl+Z takes a frame back onto its side, a preset carries
one frame's turn onto the next, and a snapshot restores which way up
the picture was along with the exposure. So `record`, `undo`,
`redo`, `go_to` and `restore_snapshot` never touch it, setting it
records no state, and `Edit::to_json` has no "turn" in it, which is
what keeps a preset honest.

Stored as a turn on top of the tag rather than as the whole
orientation, for two reasons. A later build that reads a camera's
tag differently — rawler fixes a body, or a maker changes its mind —
still reads this edit as "a quarter right of whatever the file
says", which is what was meant. And the same number means the same
thing to the develop and to the camera's own JPEG, whose tag is the
same tag: the loupe and the viewport cannot drift apart.

The file rules are §117's too. Absent when it is 0, so a sidecar
written before today loads unchanged and developing a frame nobody
has turned does not grow a field. Read loosely (`meta::loose_turn`):
a word, a list, a negative, all read as no turn, and a seven reads
as three rather than refusing the file. `VERSION` stays 3 — a new
field with a default is not a change of meaning. The write is
prompt and goes through the same `Sidecar::save` the edit and the
meta use: one file, one struct.

**The keys.** `[` and `]`, unmodified. Nothing in the window bound a
bracket, so Lightroom's Ctrl+[ and Ctrl+] were not needed to keep
out of the way, and a hand on the arrows reaches these without
moving. They act on a set (`turn_into`, which mirrors §117's
`meta_into`) against the day the browser grows a multi-select; until
then the set is the selection. No word in the status line, for
§117's reason: the picture turning is the answer, and it is on
screen in every view. There is a pair of buttons in the CULLING
panel section, and another in the Crop tab under the ROTATE
section's own quarter turns, since a turn is worth reaching for
outside culling too. Two pairs of rotate buttons a finger apart
wants saying out loud, so the second pair is labeled "Turn left"
and "Turn right" and carries a line under it: the four above are a
step in developing the picture, these two say the camera recorded
the wrong way up.

**Everywhere the picture is shown.** One rule does all of it: the
eight EXIF tags are the square's eight symmetries, and every one of
them is some number of quarter turns after an optional mirror. So
the four unmirrored tags (1, 6, 3, 8 in tag order) are one cycle,
the four mirrored ones (2, 7, 4, 5) the other, and a quarter turn
steps along whichever cycle the tag is in and never crosses.
`Orientation::turned` is that, and `turns_from` is its inverse. It
is tested against `develop::orient` itself, for all eight tags and
all four turns, rather than against a table written twice.

The develop takes it at the one place the engine already turns a
picture: `DevelopSettings` gained an `orientation` override, the
worker passes the frame's tag composed with the turn, and `prepare`
uses it in place of `frame.orientation`. Nothing after that step
knows a turn happened, which is the point — the viewport, the
navigator, the scopes, the crop overlay and the export all read the
developed picture's size and get the turned one. A develop is
keyed on the turn as well as the edit (`Base`, and the export's kept
picture), since `same_base` cannot see a field that is not in the
edit. A picture that is not a raw was turned by its own tag at
decode, so its turn is a rotation of the loaded image instead. The
CLI develops a frame under flags rather than a sidecar, so it has
nothing to compose; the field is `None` there and says so.

The three views that draw the camera's own small picture — the
filmstrip, the grid and the culling loupe — do not turn it again.
They already draw through a `Geometry`'s matrix, for the edit's own
quarter turns and mirror, so the frame's turn is folded into those:
`Geometry::shown_turns` gives the quarter turns to draw with, which
is the edit's turns less the frame's, or plus them under a mirror,
since a mirror reverses which way a turn of the source reads. That
is what makes a turn free here. The cached preview is neither
re-turned nor re-requested and no texture is remade; the next frame
draws the same bytes through a different matrix. Measured on the
45 MP CR3 (`--turn`, the time from the key to the frame that shows
the picture turned): 1.1 ms of work, and 1 to 34 ms to the frame
itself over ten runs, which is the display's cadence rather than
anything this does — the redraw either catches the frame being built
or waits for the next. Turning a frame that is already developed
costs a develop, as it must: 1.34 s for the same frame.

**Everything in the edit turns with the picture.** Two spaces, and
they move differently.

The geometry is in fractions of the leveled plane, so the crop, the
keystone and a held aspect need no aspect of their own —
`Geometry::under_turned_source`, which is `turned` the other way
with the quarter turns put back, so the picture still turns while
the crop keeps the corner it was on. The one wrinkle is the mirror:
with `flip` set, a turn of the source reads backwards on the screen,
so the stored quarter turns take a half turn to keep the screen
turning the way the key asked. The test is not the formula but the
mapping: for a handful of geometries and all four turns, the plane
point lands on the same source pixel it did before, in the turned
source's coordinates.

The masks and the repair patches are in units of the developed
picture's *width*, both ways, and that is not an aspect-free space.
The first cut of this left them alone on the strength of the
module's own sentence — a shape "survives a crop or a turn, which
only change what is looked at" — and that was wrong, because the
turn that sentence is about is the edit's own. The edit's quarter
turns move nothing but the matrix the viewport draws the developed
picture through; the picture itself is what it was, and a mask in
its coordinates is still over the same pixels. A turn of the
*frame* re-orients the developed picture itself. The width becomes
the height, so a position left alone does not even hold its place
on the screen: a radial on the face of a portrait frame, at
(0.48, 0.42), comes out 48% across and 63% down and half again as
big, which the reviewer reproduced as a mask sliding off a face onto
a torso, and a repair patch healing pixels nobody asked it to.

So `mask::Turned` is the map, and it is the pixel rotation with the
change of unit after it: for a quarter clockwise of a `w` by `h`
picture, a pixel `(x, y)` goes to `(h - y, x)` and the new width is
`h`, which in width units is `(u, v) -> (1 - v·a, u·a)` with
`a = w / h`, every length times `a`. A half turn keeps the width, so
nothing scales. A patch's source is an offset rather than a point,
so it turns without being moved; a radial's angle grows clockwise on
the screen, so it takes the quarters straight. It carries the linear
gradients, the radials, the brush strokes and their radii, the
object clicks and boxes, and every patch. A subject mask has no
positions to move, and its raster is thrown away instead, along with
every other raster the window holds: those are keyed by adjustment
and component and know nothing of a turn, a brush's is brought up to
its strokes rather than remade, and both would go on masking the
pixels they masked before. They are cheap beside a mask in the wrong
place.

**Whose shape.** The map wants the picture's aspect, which the
sidecar has no way to know, so the caller passes it — and it has to
be *that frame's*, which took three goes to get right.

The file's own answer comes first. `decode::stance_path` reads a
frame's orientation tag and the size a develop of it comes out at
with no pixel decoded: for a raw that is rawler's `raw_image` under
its `dummy` flag, which fills in everything but the samples, and the
crop rectangle it leaves is exactly what a develop produces (checked
against the develop's own line: 8192 x 5464 for the 45 MP CR3, 5472
x 3648 for the 24 MP one). A metadata read a frame a session, kept
beside the orientation tag, and asked for only when a frame is
turned or its meta is written to an XMP. Half a millisecond warm,
thirty cold.

Before that, one shortcut: the develop on screen, when it is this
frame's. Not when it is the last frame's — the first cut trusted
`source_size` whenever the open frame was the one being turned, and
`source_size` is only written when a develop lands, so arrowing from
a landscape frame to a portrait one and pressing `]` before the
second develop arrived mapped the second frame's masks by 1.5 where
0.667 was wanted, silently, since there was an aspect to be had.
It is cleared on every select now, and starts at nothing rather
than at a square.

The camera's JPEG — the culling preview, the filmstrip thumbnail —
is only the last resort, for a file rawler will not measure. It is
the same *frame* but not always the same *shape*: a body writing
its embedded picture at 16:9 or square with the raw left full frame
would hand back 1.778 where 1.5 was wanted, and a mask mapped by
that lands nowhere.

With none of them, the geometry still turns and anything placed on
the picture is left where it is, with a line in the log naming the
file. That is also the case when an XMP's orientation is adopted at
a folder's opening, where nothing has been decoded yet: the turn is
taken, since with the rest of this the edit maps consistently either
way, and it is said out loud by name — with where the frame now
stands rather than how far it moved to get there, since a reader of
the log wants the frame and not the delta. A picture that turns
under the user without a key being pressed and with no state to undo
it is worth a sentence.

**Every state, not only the current one.** A turn records nothing —
it is not a step in developing the picture — and that promise only
holds if what undo, redo and a snapshot hand back means the same
pixels the current state does. The first cut turned `current` alone,
so an undo after a turn put a pre-turn crop onto a turned frame and
a redo handed back a rectangle of a different part of the picture.
`Sidecar::turn_by` now maps the history, the redo stack and every
snapshot with the same map. For the open frame the panel owns the
edit, so what it is holding is recorded first and the sidecar's
states are then turned together; the panel takes the result back
whole, since a turn moves masks and patches as well as the crop.
Nothing refits the crop afterwards — `under_turned_source` hands
back a rectangle that fits if the old one did, and the refit the
first cut called would invent a full-frame crop for a frame that had
none, so that `]` in the loupe and `]` in culling wrote different
sidecars and `]` then `[` did not come back. Checked in the editor:
a crop, a radial and a patch through `]` and then `[` are the file
they started as, with no history state. For the frame a
camera got wrong there is no crop and no keystone yet, and the edit
is not touched at all.

**The XMP.** `tiff:Orientation` is the seventh property of §124's
module and the only one that is not the meta: the `.gcd` holds a
turn on top of a tag, and every other tool expects the two composed.
So a write takes the camera's tag and steps it along by the turn,
and a read takes the value back apart — the quarter turns that carry
the camera's tag to the packet's, or none when no quarter turn does,
which is a packet claiming a mirror the frame has not got. It is in
the presence bitmask with the rest, so a packet silent about the
orientation moves no turn.

The camera's tag is the caller's to supply, because only the caller
knows the frame, and with none the property is neither written nor
believed: whatever the file says about the orientation is left where
it is and the other six fields still go. Reading it is lazy —
`adopt` takes a closure and calls it only for a packet that carries
the property at all — so a folder whose XMPs are silent about the
orientation opens without reading a byte of any raw. When they are
not silent it costs one metadata read a frame
(`decode::orientation_path`, about 1 ms on a CR3 warm, 32 cold),
once, since the adoption mark then matches and the file is not read
again. Writing it is under `xmp_sidecars` like the rest, and it goes
in turn or no turn: it is the field's own meaning rather than
something this build has to say, and a stale 6 left behind after a
turn was taken back would be worse than writing the plain truth
every time. Checked end to end against a real frame: a 45 MP CR3
whose tag is 8, turned right, writes `tiff:Orientation` 1; the same
file edited by hand to say 3 comes back on the next open as a turn
of three.

One reader answers for a file's tag. Three places read a
picture's with the `image` crate's own `orientation()` — the
orientation probe, the thumbnail and the culling preview — while
`picture::decode` reads it with rawler's TIFF reader, and on a file
with more than one directory the two can disagree. That would put a
`tiff:Orientation` into an XMP for a picture nobody is looking at,
and now that those same pictures are measured for the mask map it
would misplace a mask as well. All three go through the picture
module's reader, the one that decides which way up the picture is
drawn.

One thing fixed on the way past: the culling status line said the
JPEG's own size rather than the size as shown, so a frame turned on
its side still read "5464 × 8192". It now reports the plane's size,
which under culling's geometry — quarter turns and a mirror, nothing
else — is the JPEG's own pixels either way round.

One thing the learned denoiser needed, and it is the shape of every
bug in this: `Edit` has no turn in it, so nothing that compares two
edits can see one move. `Base` was keyed on the turn from the start;
`LearnedBase`, which keeps the network's picture and the plain one
beside it, was not, so a turn with the denoiser on kept the old
oriented pair and the viewport showed an unturned frame while every
other part of the window said turned — and the export took the same.
Anything cached against an edit has to carry the turn beside it.

**Left out.** No turn in the grid's context menu, because there is
no context menu. No mirror to go with the turns: a camera records a
mirror only through a tag this build already honors, and a picture
the photographer wants flipped is an edit, which the Crop tab has
had all along. No CLI command to set or clear a turn, and the CLI's
develop still takes flags rather than a sidecar, so there is nothing
there to compose a turn onto yet. The XMP's orientation is adopted
without the map for what is placed on the picture, for the reason
above; a folder that has both a foreign `tiff:Orientation` and
greycard masks is one this build wrote the XMP for, so the two
already agree.
