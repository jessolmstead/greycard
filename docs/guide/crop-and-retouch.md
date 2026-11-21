# Crop and retouch

The **Crop** tab frames the picture: crop, straighten, turn and
correct perspective. The **Retouch** tab removes dust, blemishes and
small distractions. Both work on one frame at a time. Sync and paste
never copy the crop, the straighten or the retouch to other frames.

## The Crop tab

The tab has three sections: **CROP**, **ROTATE** and **PERSPECTIVE**.
**Reset all**, at the bottom, puts all three back at once; the reset
arrow in a section's header puts back that section alone.

### Cropping

1. Click **Crop**. The whole frame shows, with the area outside the
   crop shaded and a rule-of-thirds grid inside it. The button now
   reads **Done**. **Guides** picks another guide for the grid's
   place: the golden ratio, the golden spiral, diagonals, triangles,
   or none. **O** steps through them, and **Shift+O** turns the
   spiral and the triangles. The choice is the editor's, not the
   picture's.
2. Adjust the crop:
   - Drag a corner or an edge to resize it.
   - Drag inside the rectangle to move it.
   - Drag anywhere outside the rectangle to draw a new crop from that
     point.
   - Click **Draw**, then drag anywhere on the picture, inside the
     crop too, to draw a new one from where you press. This is the way
     to start over when the crop already fills the frame. Draw lets go
     after the one drag, or when you press Esc.
   - Hold **Alt** as you drag inside the rectangle to draw a new crop
     the same way. Some desktops keep Alt and a drag for moving
     windows; use **Draw** there.
3. Click **Done**, press Esc, or switch to another tab. The picture
   shows cropped. With **Draw**, **Level**, or a guide in hand, the
   first Esc puts it down and the next leaves the crop.

**Aspect** holds the crop to a shape:

| Aspect | Meaning |
|---|---|
| **Free** | Any shape. |
| **Original** | The frame's own shape. |
| **1:1**, **3:2**, **4:3**, **5:4**, **16:9**, **65:24** | That ratio. 65:24 is the XPan panorama. |
| **Custom** | Type your own ratio as width:height, such as `7:5`. |

A frame with no crop of its own opens its crop at **Original**, so
dragging a corner keeps the frame's shape; only opening the crop
leaves the frame's edit as it was. The **Aspect** row in
[Settings](settings.md#crop) changes that: **Last used** opens every
uncropped frame at the aspect you last picked from the list, kept
from one session to the next, and **Free** at any shape. A frame you
have cropped keeps its own crop and aspect, and **Reset all** puts a
frame back to no crop.

**Portrait** means the crop is tall: on, 3:2 becomes 2:3. It reads
the same way under **Original**, a ratio and **Custom**, so on a photo
shot on end it is on for the frame's own shape. Under **Free** the
crop can be any shape, so the switch is greyed and does nothing.

A ratio picked when the aspect was **Free** or **Original** starts the
way the photo is: tall on a photo shot on end, wide otherwise. A ratio
picked over another keeps the switch as you left it.

When you pick another ratio or turn **Portrait**, the crop takes the
new shape around the same center and keeps about the same area, so
turning **Portrait** twice gives back the crop you had. When you change
the angle, the crop stays where it is if it still fits; if not, it
shrinks to fit around the same center. While the **Angle**,
**Vertical**, or **Horizontal** slider moves, by drag, wheel, or key, a
grid lies over the picture so a horizon or an edge can be brought
level or upright against its lines; it goes when the slider rests.

### Straightening

In **ROTATE**:

- **Level** gives you a ruler. Drag along something in the picture
  that should be level or upright, such as a horizon or a wall. The
  picture turns to straighten it. greycard takes whichever is nearer:
  level or upright.
- **Auto level** does the same by itself: it finds the longest straight
  line near level or upright, a horizon or a wall, and turns the
  picture to it. Nothing happens when no line stands out.
- **Angle** turns the picture by hand, −45° to +45°.

### Turning and mirroring

The row of four icon buttons in **ROTATE** (rest the pointer on one
for its name, "Rotate left: part of the edit" and so on):

- turn a quarter left
- turn a quarter right
- mirror left to right (**Flip**)
- mirror top to bottom (**Flip vertical**)

These are part of the edit, and **Reset all** undoes them.

The **Turn left** and **Turn right** buttons under the row are
different. They fix a frame the
camera recorded the wrong way up. They are not part of the edit:
**Reset all** leaves them. They act on every selected frame. The **[** and **]** keys do the same.

### Perspective

Use this when the camera was tilted, so that the sides of a building
lean in, or a facade shot from one side narrows toward one end.

**Vertical** and **Horizontal** (each −40° to +40°) correct the tilt
by hand.

The guide finds them for you:

1. Click **Verticals** or **Horizontals**.
2. Drag a line along an edge in the picture that should run straight
   up (or straight across), such as the side of a building.
3. Drag a second line along another such edge, away from the first.

greycard sets the angle and the tilt so that both lines run true. If
the two lines tell it nothing (the same line twice, for example), the
status line says "the guide needs two different lines" and the tool
stays in your hand. Escape puts **Level** or a guide down.

Correcting perspective leaves empty corners. Crop them away
afterward.

## The Retouch tab

**RETOUCH** has three tools:

| Tool | What it does | Use it for |
|---|---|---|
| **Heal** | Copies texture from nearby, matched to the color and light of the spot. | Dust, blemishes, small spots on even surfaces. |
| **Clone** | Copies another part of the picture as it is. | Repeating a pattern or edge exactly. |
| **Fill** | Makes up new content from what is around the area. | Removing a small object where nothing nearby would match. |

The section's switch turns every repair off at once, for a before and
after.

### Making a repair

1. Click **Heal**, **Clone**, or **Fill**. The pointer shows the
   repair's size and where its soft edge begins.
2. Click on a spot, or drag over a longer flaw.
3. Click the tool's button again, or press Esc, when you are done. With
   **Fill**, press Enter first: the first Esc drops what is painted.

**Heal** and **Clone** repair as soon as you let go. **Fill** waits, so
you can cover a large or awkward area in several strokes first:

1. Paint over what you want gone, in as many strokes as it takes. Each
   is outlined, and nothing is made yet.
2. Press Enter, or click **Generate** under the Fill's buttons. Strokes
   that touch become one fill; strokes apart become fills apart, so
   each can be removed on its own later.
3. Keep painting the next area while a fill is being made.

Only Enter or **Generate** makes a fill. Until then the strokes wait on
their frame: put **Fill** down, go to another tab or frame, and they
are there when you come back and take **Fill** again. They are not part
of the edit, so an export leaves them out, and quitting drops them.

Enter, Esc, and Ctrl+Z work on the strokes only in the Develop view,
where they are drawn; in the grid, the lightbox, the crop, and culling
they keep their own meaning. Ctrl+Z takes back the last stroke. Esc drops
everything painted and keeps **Fill** in hand; Ctrl+Z next brings it
back, if nothing else was done in between. One Generate is one step of
the history, however many fills it makes.

To add to a fill, choose it in the list first: strokes that touch it
join it, and it is made again as one area. With no repair chosen, a
stroke over an old fill makes a new one on top.

While a tool is in hand, the mouse wheel changes the size, one step a
click; a touchpad scroll changes it by as many steps as the same
distance on a wheel. Ctrl with the wheel still zooms. To move the
picture without putting the tool down, as when working over dust at
200%, hold **Space** or **Shift** and drag.

For **Heal** and **Clone**, greycard chooses where to copy from. To
change it, choose the repair in the list with the tool in hand. Two
pins show the repair and its source, joined by a line. Drag the source
pin to copy from somewhere else, or the repair's pin to move the
repair.

### The repair's settings

**Size**, **Feather** and **Opacity** show while a tool is in hand,
and set the next repair. With a Heal or Clone chosen in the list, they
change that one instead. A fill chosen with a tool in hand is left
alone: the sliders, the wheel, and the bracket keys set the brush. To
change a fill, put the tool down and choose it: the sliders show for
it, and it is made again when you let go of one.

- **Size** (0.2% to 30% of the frame). On a fill of several strokes,
  every stroke grows or shrinks with it.
- **Feather** (0 to 100%, default 30%): how soft the edge is.
- **Opacity** (0 to 100%, default 100%): how strongly the repair
  covers what is under it.

### The repair list

Each repair is listed by its tool and a number, such as "Heal 1" or
"Fill 2".

- Click a repair to choose it. Its outline and the start of its soft
  edge are drawn on the picture.
- The bin icon on the chosen row deletes it.
- Escape lets go of the chosen repair.

### Fill and its model

**Fill** runs a learned model. It takes a second or more to fill an
area, and the status line says when it is working ("the fill model is
at work on Fill 3").

The model downloads the first time you choose **Fill**: about 210 MB,
once. greycard shows you its license first. If a sidecar with a fill
opens on a machine without the model, the area is left as it was until
the model is downloaded, and the status line says so.

There are two engines. LaMa fills on any machine and at any size. On a
machine with a GPU, the generative remove makes up what was behind what
you remove, working in a window around the area up to 1024 pixels
a side; a window up to four times that is scaled down to it and back,
and a bigger one is still LaMa's.

With **Fill** in hand, or a Fill chosen in the list, three buttons above
the list say what makes it:

- **Patch** fills from the picture around the area, at once, on any
  machine.
- **Fast** runs the generative remove in four steps: a few seconds, but
  rougher on large areas. It is a further 390 MB beside the remove.
- **Best** runs the generative remove in full, 25 steps.

The choice changes the Fill chosen and is kept for the next one, and the
Settings sheet's Editing category has it too. Before you choose, it is
Best, or Fast on a machine whose GPU shares its memory, where Best takes
a minute on a recent Mac and up to four on an M1, and Fast a quarter of
that; the first fill after a launch adds the model's load, a minute or
so on an M1. Where the GPU shares 16 GB or more, the load starts the
moment you take **Fill** in hand, so it runs while you draw. The status
line says "preparing..." while it lasts, as it does whenever a model
takes more than a moment to load. Choosing one whose model is not
downloaded offers the download; choose it again once it is here. On a
machine without a GPU the two removes are greyed, and the tooltip says
why. So they are on a graphics card with less memory than the remove
needs, 6.3 GiB: the tooltip says "Needs 6.3 GiB VRAM, 4.0 available.
Use Patch." with your card's figure, a new Fill is **Patch**, and a
Fill already marked Fast or Best is left as it was. A card big enough
but busy right now (another program holding its memory) leaves the
fill as it was too, once greycard has let go of what it can, and the
status line says the same sentence with the memory free. An export
of the frame is not written on a card too small, and says the same; on
a busy card it lets go and waits a little, and is written if the card
frees up. The
generative remove's models are 16-bit, so the GPU must run
16-bit floats in its shaders: NVIDIA cards before the GTX 16 and RTX
20 series (a GTX 1080, say) cannot, and a fill asked of them fails at
once with the log saying so; **Patch** still works there. A Fill drawn
before a model arrived keeps what made it until you choose again.

The generative remove is a second download, 5.5 GB under the CreativeML
Open RAIL++-M license, offered once the first time you choose **Fill**
with LaMa in hand. **Not now** keeps LaMa. A fill it made opens on a
machine without it, or without a GPU, left as it was.

Every fill takes on the photo's grain, measured around the area, so
it does not stand out as a smooth patch.

A frame with several generative fills to make shows its picture as
soon as it develops, each fill still to come outlined faintly where it
will go, and the fills land one by one after it. The status line
counts them ("Fill 5, 4 of 15") and its bar shows the one being made.
Leaving for the Library pauses them; coming back goes on.

A fill the generative remove made has **Another** on its row in the
list: each press makes a new version of it, and undo takes you back to
the last one.
