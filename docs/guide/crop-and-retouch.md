# Crop and retouch

The **Crop** tab frames the picture: crop, straighten, turn and
correct perspective. The **Retouch** tab removes dust, blemishes and
small distractions. Both work on one frame at a time. Sync and paste
never copy the crop, the straighten or the retouch to other frames.

## The Crop tab

The tab has three sections: **CROP**, **ROTATE** and **PERSPECTIVE**.
**Reset all**, at the bottom, puts all three back at once.

### Cropping

1. Click **Crop**. The whole frame shows, with the area outside the
   crop shaded and a rule-of-thirds grid inside it. The button now
   reads **Done**.
2. Adjust the crop:
   - Drag a corner or an edge to resize it.
   - Drag inside the rectangle to move it.
   - Drag anywhere outside the rectangle to draw a new crop from that
     point.
3. Click **Done**, or switch to another tab. The picture shows
   cropped.

**Aspect** holds the crop to a shape:

| Aspect | Meaning |
|---|---|
| **Free** | Any shape. The default. |
| **Original** | The frame's own shape. |
| **1:1**, **3:2**, **4:3**, **5:4**, **16:9**, **65:24** | That ratio. 65:24 is the XPan panorama. |
| **Custom** | Type your own ratio as width:height, such as `7:5`. |

**Portrait** turns the ratio on end, so 3:2 becomes 2:3.

When you change the angle or the aspect, the crop stays where it is if
it still fits. If not, it becomes the largest crop of the new shape
around the same center.

### Straightening

In **ROTATE**:

- **Level** gives you a ruler. Drag along something in the picture
  that should be level or upright, such as a horizon or a wall. The
  picture turns to straighten it. greycard takes whichever is nearer:
  level or upright.
- **Angle** turns the picture by hand, −45° to +45°.

### Turning and mirroring

The row of four icon buttons in **ROTATE**:

- turn a quarter left
- turn a quarter right
- mirror left to right
- mirror top to bottom

These are part of the edit, and **Reset all** undoes them.

**Turn left** and **Turn right** are different. They fix a frame the
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

1. Click **Heal**, **Clone** or **Fill**. The pointer shows the
   repair's size and where its soft edge begins.
2. Click on a spot, or drag over a longer flaw.
3. Click the tool's button again when you are done.

While a tool is in hand, the mouse wheel changes the size.

For **Heal** and **Clone**, greycard chooses where to copy from. To
change it, choose the repair in the list with the tool in hand. Two
pins show the repair and its source, joined by a line. Drag the source
pin to copy from somewhere else, or the repair's pin to move the
repair.

### The repair's settings

**Size**, **Feather** and **Opacity** show while a tool is in hand.
They set the next repair. With a repair chosen in the list, they
change that one instead:

- **Size** (0.2% to 30% of the frame).
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
