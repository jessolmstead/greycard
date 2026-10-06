# Masks

A mask limits an adjustment to part of the picture: a brighter face,
a darker sky, a warmer lamp. You build masks on the **Masks** tab.
Each mask has its own shapes and its own sliders, and the global edit
on the **Develop** tab stays under all of them.

## The Masks tab

The **ADJUSTMENTS** section lists **Global** and then your masks.
Click a row to choose it. The panel below edits what you chose.

- The dot before a mask switches it on and off.
- The bin on the chosen row deletes that mask.
- **New mask** adds an empty mask named "Mask 1", "Mask 2" and so on.
  A frame can hold up to 16.

With a mask chosen, you see, top to bottom:

1. **The mask's card:** its name, **Show mask** and **Invert**.
2. **Shapes:** one row per shape in the mask.
3. **The chosen shape's settings**, when it has any.
4. **Add a shape:** the buttons that add another shape.
5. **The mask's own sliders:** **WHITE BALANCE**, **LIGHT**,
   **COLOR**, **CURVES**, **COLOR MIXER**, **COLOR GRADING** and
   **TINT**, each headed with the mask's name ("SKY 1 · LIGHT").

### Naming, Show mask and Invert

A mask is named for its first shape ("Subject 1", "Linear 2"). Type
in the name field to call it something you'll recognize; a name you
type is kept.

**Show mask** paints the mask red over the picture, stronger where it
acts more. It turns on by itself while you are drawing a shape, and
goes back to how you had it when you finish.

**Invert** flips the whole mask: the adjustment then acts everywhere
the shapes are not.

### The shapes list

Each row reads as the way the shape joins (**+**, **−** or **∩**) and
its kind, such as "+ Radial".

- The dot switches the shape on and off without deleting it.
- The half-filled circle inverts that shape alone. It lights when on.
- **×** deletes the shape. It shows when the mask has more than one.
- Click a row to choose it and see its settings.

## Adding shapes

Click **New mask**, then a button under **Add a shape**. The first
shape always adds. From the second on, the row above the buttons sets
how the next shape joins:

| Mode | The mask becomes |
|---|---|
| **Add** | this shape and everything already in the mask |
| **Subtract** | the mask with this shape taken out |
| **Intersect** | only where the mask and this shape overlap |

Intersect is how you limit one shape to a place, such as a
**Luminance** shape that only reaches the top of the frame: add a
**Linear**, then a **Luminance** with **Intersect**.

Press **Esc** to put a drawing tool down, or click its button again.

### Linear

Drag across the picture. The adjustment is full where you start and
fades to nothing where you let go. Afterwards, drag the middle line to
move it, or an end line to turn and stretch it about the middle. Hold
**Alt** while dragging an end line to move that end alone.

### Radial

Press at the center and drag outward. Drag the center handle to move
it. Drag a handle at the end of either axis to change that radius and
turn the ellipse. **Feather** (50% to start) is how much of the
ellipse fades out toward its edge.

### Brush

Click **Brush** and paint. While a brush is chosen, the button reads
**Paint**, and new strokes go into the same brush.

| Setting | Does |
|---|---|
| **Size** | the brush's radius, as a share of the picture's width; the scroll wheel changes it too |
| **Feather** | how much of the radius fades |
| **Flow** | how much each stroke lays down |
| **Add**, **Subtract**, **Erase** | paint, take paint away by the flow, or clear paint completely under the brush |

### Subject and Background

**Subject** finds the main subject with a learned model. **Background**
is everything the Subject mask leaves out. Neither needs drawing: one
click makes it. A mask with both costs one run of the model.

### Sky

**Sky** finds the sky. It refuses a frame where it finds none rather
than inventing one; the status line says "no sky found". With a Sky
shape chosen, the button reads **Pick**: click sky it missed to add it,
or right-click what isn't sky to remove it.

It works least well on sky seen through a window or behind dense
blossom.

### Object

Click **Object**, then click the thing you want, right-click to
exclude something, or drag a box around it. With an Object shape
chosen, the button reads **Pick** and adds more clicks to it.

### Luminance

A luminance shape takes everything between its **Low** and **High**
lightness, fading out over **Low fade** below and **High fade** above.
A new one takes 70 to 100.

The scale runs from 0 (black) to 100, the brightest the camera
recorded at a global exposure of 0. Middle grey is about 57. The
picture on screen is shown a little brighter than that, so a bright
sky can sit in the 70s. A High of 100 takes in everything above it,
and a Low of 0 everything below. Dragging Low past High pushes High
along, and the other way round.

### Color

A color shape takes one range of hues, set by **Hue** and **Width** and
fading over **Hue fade**. It takes only colors at least as strong as
**Chroma**, fading out over **Chroma fade**, so greys stay out.

A new one starts on skin tones (hair of the same hue comes with it),
with the dropper in hand: click a color in the picture to center it
there, or press **Esc** to keep skin. **Pick** gives you the dropper
again later.

### What Luminance and Color read

Both read the picture as developed, before any mask's own sliders,
the curves or the look, so a mask doesn't shift as you edit under it.
Exposure moves them; dehaze and clarity can move them a little.

## A mask's own sliders

Under the mask, **LIGHT**, **COLOR**, **CURVES**, **COLOR MIXER**,
**COLOR GRADING** and **TINT** work as they do on the Develop tab, but
only where the mask reaches. **BLACK & WHITE** and the **Display
curve** switch are global only. A mask's tone sliders act even when the
global **LIGHT** section is switched off.

### White balance in a mask

Use this for mixed light: a room under lamps with a window, a stage
under two colors of light. Set the global white balance for one light,
then give a mask the other.

The mask's **WHITE BALANCE** section is off until you switch it on at
its title. It starts at the global white, so nothing jumps.

- **Temperature** (2000 to 12000 K) and **Tint** name the light under
  the mask.
- **Neutral** gives you a dropper: click something under the mask that
  should be grey. It switches the section on for you.

There is no **As shot** or **Auto** here. The values are absolute: a
mask set to 3200 K stays a 3200 K lamp when you move the global white
balance, so the masked area holds still.

Over blown highlights, a mask whose white is far from the global one
can tint them if you pull exposure well down.

## Models download on first use

Subject, Background, Sky and Object use learned models that greycard
downloads the first time you need one. A sheet names the model, its
size, where it comes from and its license before anything downloads;
choose **Download** or **Not now**.

| Shape | Download |
|---|---|
| Subject, Background | about 114 MB |
| Sky | about 96 MB, plus the Object model for a finer edge |
| Object | about 184 MB |

Each is downloaded once. The first Subject or Background mask of a
session takes a few seconds while the model loads; after that, masks
come quickly. Without the Object model, Sky still works, with a
coarser edge, and clicks can only remove sky.

## Masks and other frames

Masks are drawn around one picture, so syncing settings onto other
frames leaves them out unless you tick **Adjustments (replaces
masks)**, which replaces each frame's own masks. See
[Presets and history](presets-and-history.md).
