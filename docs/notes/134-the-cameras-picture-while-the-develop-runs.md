# 134. The camera's picture while the develop runs (2026-09-21)

Choosing another frame in the develop view left the last frame's
picture on screen under the last frame's look until the new develop
landed: 1.6 seconds on a 45 MP CR3 on this desktop, and the roadmap's
"switching photos is still a bit too slow to develop". §115 pulled the
binned proxy develop and wrote down what would take its place — a
placeholder and not a mode, up only while a develop is pending,
replaced by the develop the moment it lands — and said it pairs with
the camera's JPEG the culling mode (§123) already decodes and draws.
This is that, and it needed no new picture-making at all: culling's
encoded path in the shader, its prefetch threads and its cache do the
work, and the develop view is a second caller.

**What is drawn.** The frame's own camera JPEG, fitted, through the
monitor profile alone, and turned exactly as the develop will be: the
camera's tag composed with the sidecar's turn, and the edit's own
quarter turns and mirror folded in by `Geometry::shown_turns`, which
is §131's rule and the reason the picture does not move when the
develop replaces it. Nothing else of the edit is in it — no crop, no
tone, no masks, no repair — because it is the camera's rendering of
the frame and not ours. A word in the corner of the viewport says
"camera preview" and the status line says the rest, in the culling
loupe's words: "camera preview: the camera JPEG, 5464 × 8192, fitted,
through the monitor profile only; developing...". Both go the moment
the develop lands, which is the only signal that says the picture on
screen is now the export's.

That line is not written into the status line but onto a property of
its own, which the window shows in the status line's place for as
long as the word is up. Everything that asks for a develop writes
"developing..." where the status goes — a slider, a turn, an undo, a
preset, leaving the culling mode — and the picture on screen is not
that develop yet, so the word over the picture and the line under it
would have disagreed for as long as it took the next frame to put
them right. One property, set where the picture is, and they cannot.

The turn is in and the crop is out, and that is the line: which way
up a frame stands is a fact about the file that the camera's JPEG
carries as surely as the raw does, so honoring it costs nothing and
not honoring it would spin the picture a second later; a crop is a
decision about the frame, and a camera JPEG shown through one would
be a half-developed picture claiming to be less than it is. So a
frame cropped to a third of itself does change shape when the develop
arrives, and a frame nobody has cropped — which is most of them
while the arrow is moving — does not move at all.

**What the panel does meanwhile: nothing over the picture.** The crop
rectangle, a gradient's outline, a radial's ellipse, the shape
handles, the repair pins and the perspective guide are all placed in
the developed picture's own coordinates, and the camera's JPEG is
neither that size nor that picture. So they are not drawn, and a
handle that is not drawn is not pressable either, which is the half a
test can hold (§132's lesson). One Slint property, `placeholder`,
gates every one of them, and a press that would place a shape or read
a color off the picture is ignored while it is set; panning and
zooming still work, as they do in culling.

The navigator and the scopes wait rather than lie. They read the
develop: a histogram of the camera's rendering — its own tone curve,
its own white balance, its sharpening — read as this frame's develop
is worse than no histogram, and the navigator's rectangle is a
fraction of a picture with a crop in it that this one has not got. So
the navigator, the scope, the histogram behind the curve editor and
the two clipping lamps all go blank for the second the placeholder is
up and come back with the picture they describe. Culling empties the
same panels for the same reason, so the two paths agree.

Blanking the pictures is not enough on its own, which the review
found: the bins the scopes were drawn from are kept on the state for
the curve editor, which redraws its backdrop from whatever is in hand
every time a point is moved, so one touch of the curve painted the
last frame's histogram back behind a picture it did not describe.
They are dropped with the rest, and the next develop brings its own.

**The swap rule.** The developed picture replaces the placeholder and
never the other way: the placeholder comes down the moment a develop
reaches the frame, and nothing it does can put it back up. Which
develop that is has to be asked of the develop itself, and the first
cut asked the state instead — the picture waiting to go to the GPU
carried no generation, so any delivered develop took any placeholder
down. That is a race with a name: a develop delivered for frame A and
an arrow to B taken in the same turn of the event loop would drop B's
freshly armed wait before B's JPEG arrived, and B's JPEG would then
arrive to nothing. So the picture carries the generation it was asked
for (`Landed`), and the wait is asked about that one. A develop that
lands for a frame no longer selected is thrown away on its generation
before it gets that far, as it always was. A second frame chosen before the first has developed asks for the
second frame's JPEG, and — this is the one wrinkle — keeps the first
frame's picture on screen until the second's is decoded, rather than
blanking the viewport for a tenth of a second. That is the rule the
developed picture has always followed, one picture further along:
what is on screen stays until there is something better. `Wait` holds
both, the frame waited on and the frame showing, and a jump to a frame
whose neighbors are not cached leaves the developed picture up
instead. Leaving the culling mode is the same question asked the
other way: the loupe's picture stands in only where the loupe had one
of that frame to hold, since a mode left before its first decode has
nothing to show and gating the panel off a developed picture that is
still on screen would be a lie about it. Export never sees any of this: it reads the worker's
developed picture, so §18's viewport-equals-export holds untouched —
at rest the fit view is the export, and the placeholder is only ever
what is up while there is no develop to show.

**Where the decisions live.** `placeholder.rs`, pure and tested
without a window: the stages a wait goes through and what moves it
along, whether a develop replaces it, what the overlays do, the words
of the status line, how many frames either side keep their picture,
and whether a file is worth standing in for at all. `panel/cull.rs`
holds the glue beside the mode it borrows from — the select asks, the
delivery puts it up, `cull_frame` draws it, `drop_placeholder` takes
it down — and `main.rs` grew two fields: `hold`, which is the
camera pictures the develop view keeps (what culling left behind and
what a select decoded, renamed from `cull_hold`, which is now only
half of what it does), and `placeholder`, the wait itself.

**Tests.** Pure: the stages a wait goes through and what moves it
along, the develop that replaces it and the one that does not, what
the overlays do, the words of the line, the window either side, and
which files are worth standing in for. Through a headless window: a
frame chosen and its picture delivered, with the readings blanked and
the swap after it; a second frame chosen before the first develops; a
frame with no camera JPEG, and the same frame chosen again; a picture
file chosen, and the window still trimmed; a mask handle neither
drawn nor pressable over the camera's picture; a develop that lands
before the decode, and the late decode staying down; an older
develop leaving the placeholder standing; a slider, an undo, a redo,
a snapshot restored and a turn over it, each keeping it up and the
line with it; the curve editor asked to redraw and not painting the
last frame's histogram back; and the culling mode entered over a
placeholder and left again, with a picture and without one. The
crate's suite is 695 passing and 14 ignored with them in it.

**Cost, and who pays it.** The decode is `cull::Prefetcher`'s, on its
own threads, never the UI thread and never the worker, which is busy
with the develop that matters. Only the selection's own JPEG is asked
for: nothing is decoded ahead in the develop view, where the cores
belong to the develop. What culling decoded is kept and reused, so a
frame just culled is on screen in the next frame with no decode at
all, and the frames within two rows of the selection are kept as the
selection moves, which is the arrow back. That bounds what the develop
view holds at five previews of the view's own size — about 30 MB at
the 1404 px long edge this window runs at, and three times that at
4K, against the mode's 256 MB budget for a dozen either side. The
trim runs on every selection, whether or not that frame gets a
placeholder of its own, or a run through a folder of JPEGs would hold
on to whatever the raws before it left; the textures are dropped when
the develop lands, since nothing draws them until another frame is
chosen.

A JPEG, a PNG or a TIFF gets no placeholder. There is no camera JPEG
inside one — what the culling loupe shows of such a file is the file —
so the placeholder would be the same decode the develop is doing this
moment, done twice on another thread and arriving no sooner. A raw
with no embedded preview (a DNG written without one) is the same
picture from the other end: the decode comes back with nothing, the
wait is given up, the picture on screen stays where it was (it did
not, until §135 the same evening: the viewport went dark instead), and the
frame is remembered as having none for as long as the folder is open,
so choosing it again costs no second decode to be told the same
thing. The record goes when the pictures do — a folder opened, or the
rejects moved out from under the numbering. Both cases leave exactly
what the editor did before there were placeholders.

**Measured**, release, the 16-core desktop with the RTX 5070 Ti, on a
folder of four 45 MP CR3s (`--time-select`, hidden as the mode's own
`--time-cull` is: the arrow every tenth of a second, the time from the
key to the frame that shows the picture). Before, three runs: mean
1585.8 ms to the develop (1522–1667), 1578.8 (1518–1658), 1581.3
(1524–1656), and nothing of the new frame on screen for any of it.
After, four runs: to the camera's picture, mean 121.0 ms (107–136),
114.1 (101–121), 111.2 (103–116), 119.9 (117–126); to the develop,
1576.2 (1507–1666), 1592.0 (1520–1653), 1578.9 (1503–1667), 1576.0
(1514–1656). The frame is up fourteen times sooner and the develop
costs nothing it did not cost before — the decode is a tenth of a
second of one core beside a second and a half of sixteen. Taken again
after the review's fixes: 106.2 ms (102–110) to the camera's picture
and 1584.2 (1516–1672) to the develop, which is the same.

The first file of a run gets one too, which was not the point but is
the best of it: the editor opens on a picture in 105 to 127 ms rather
than 1.55 to 1.64 s. A frame whose picture is already held costs no
decode at all: leaving the culling mode is that case, and §123's
numbers stand — the loupe's picture is up, the develop replaces it
1.4 s later. On a mixed folder the Sony ARW's 1616 × 1080 embedded
preview stood in at 18.8 ms with its develop at 1.05 s, and the JPEG
in the same folder got no placeholder and developed in 22.9 ms.

**Left out.** Nothing is decoded ahead in the develop view: the frame
the arrow is about to land on is not started until the key. The
window of two either side is what makes an arrow back free, and a
prefetch here would want the same thought about the budget the mode's
got, against a develop already using every core. The placeholder is
fitted and stays fitted: a zoom asked for while it is up arrives with
the develop, because the frames either side are kept at the view's
size and a magnified screen-size copy would be a blur that jumps
twice. Entering the culling mode throws the develop view's pictures
away rather than seeding the mode's cache with them, since the mode
makes its own at its own size. The placeholder is not cropped, for
the reason above, so on a cropped frame the swap changes the picture's
shape as well as its rendering; drawing the edit's crop on it is the
one piece of the edit that could honestly go on a camera JPEG, and it
can be had later from the same `View` the compare tiles use if it is
wanted. There is no blend or fade between the camera's picture and
ours, by §123's argument: they differ by design and the swap is the
point. And the placeholder has no say in the
strip or the grid, which have had the camera's picture as their
thumbnail all along.

**The way out.** Found on the way: a process that exits while the
worker is in the middle of a develop can die in the driver rather
than at its own hand — the worker was inside `greycard-gpu`'s CA
correction dropping a wgpu buffer, with a segfault instead of an
exit. `--snapshot-placeholder` reached it by quitting on the picture
it had just captured, which is the one moment a develop is certainly
running; it now takes its picture and lets the run go on to the
develop it stood in for. Closing the window on a develop did not
reproduce it in 25 runs, here or on master, so this is insurance
rather than a bug fixed: the worker has a `stop` — a flag on its
queue, a wake, and a wait of up to five seconds for the thread to
come back and drop the engine's GPU context where it was made — and
the editor calls it as `app.run` returns. A clean quit pays 15 ms of
it. A job that outlasts the five seconds is left where it was, which
is what leaving did before there was any waiting at all.
