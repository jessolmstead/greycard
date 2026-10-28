# Masks

A mask limits an adjustment to part of the picture: a brighter face,
a darker sky, a warmer lamp. You build masks on the **Masks** tab.
Each mask has its own shapes and its own sliders, and the global edit
on the **Develop** tab stays under all of them.

## The Masks tab

The **ADJUSTMENTS** list at the top of the tab holds **Global** and
then your masks. Click a row to choose it. The sections below edit
what you chose.

- The dot before a mask switches it on and off.
- The bin on the chosen row deletes that mask.
- **New mask** adds an empty mask named "Mask 1", "Mask 2" and so on,
  and takes the sections back to their top. A frame can hold up to 16.

The list stays put while the sections scroll under it, so **Global**
or another mask is one click away from any slider. It shows up to six
rows, fewer in a short window, and scrolls inside itself past them.
Click its title to fold it to one line, which names what you are
editing.

With a mask chosen, you see, top to bottom:

1. **MASK**, headed with the mask's name: its name field, **Show
   mask** and **Invert**, then **Shapes**, one row per shape in the
   mask, the chosen shape's settings when it has any, and **Add a
   shape**, the buttons that add another shape.
2. **The mask's own sliders:** **WHITE BALANCE**, **LIGHT**,
   **COLOR**, **CURVES**, **COLOR MIXER**, **COLOR GRADING**,
   **TINT** and **DETAIL**, each headed with the mask's name ("SKY 1 ·
   LIGHT").

### Naming, Show mask and Invert

A mask is named for its first shape ("Subject 1", "Linear 2"). Type
in the name field to call it something you'll recognize; a name you
type is kept.

**Show mask** paints the mask red over the picture, stronger where it
acts more. It turns on by itself while you are drawing a shape, and
goes back to how you had it when you finish.

When you move a slider in the mask's own sections, **Show mask** turns
off by itself, so you see the edit rather than the red. A shape's
settings, such as **Feather** or a range, **Invert** and the brush
leave it showing, since those are about the shape. Turn it on again
whenever you want to check the shape.

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
fades to nothing where you let go. Afterwards, drag anywhere in the
band between the end lines, or the middle line, to move it, or an end
line to turn and stretch it about the middle. Hold **Alt** while
dragging an end line to move that end alone. A drag outside the band
moves the picture.

### Radial

Press at the center and drag outward. Drag anywhere inside the
ellipse, or its center handle, to move it; the pointer turns to a
move arrow there. Drag a handle at the end of either axis to change
that radius and turn the ellipse. **Feather** (50% to start) is how
much of the ellipse fades out toward its edge.

### Brush

Click **Brush** and paint. While a brush is chosen, the button reads
**Paint**, and new strokes go into the same brush.

| Setting | Does |
|---|---|
| **Size** | the brush's radius, as a share of the picture's width; the scroll wheel changes it too, one step a click, and a touchpad scroll by as many steps as the same distance on a wheel |
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

On a frame wider than 6144 pixels, as from a 45 or 100 MP camera,
the Sky mask is made at twice the size, so twigs, wires and the rims
of leaves against the sky keep a clean edge at 1:1.

### Landscape

**Landscape**, beside **Sky**, looks over the picture once and offers
what it found, largest first, in a menu under the button: **Water**,
**Foliage**, **Mountain**, **Building** and **Sky**, each with the
share of the frame it covers ("Foliage 33%"). Only what it found is
offered; with nothing found there is no menu, and the status line
says "no landscape found". Choose one to add it to the mask, joined as
the row above the buttons says; press **Esc** to add none.

Each is a shape edged from the picture: water and foliage follow the
color line along their edge, mountains and buildings a softer one.
**Foliage** keeps off the sky that **Sky** finds, so the open sky
between branches stays out of it. These shapes take no clicks: to
take something out of one, subtract a brush, or intersect it with
another shape.

### Object

Click **Object**, then click the thing you want, right-click to
exclude something, or drag a box around it. With an Object shape
chosen, the button reads **Pick** and adds more clicks to it.

### People

**People** opens a menu: **Whole person** at the top, then a person's
parts in three groups:

| Group | Parts |
|---|---|
| Face | Face, Facial skin, Eyebrows, Eyes, Iris, Lips, Teeth |
| Body | Skin, Hair, Hands |
| Clothing | Top, Bottoms, Shoes, All clothing |

**Whole person** takes one person head to foot: hair, skin, clothes,
and usually what they hold. Where **Subject** takes everything that
stands out, people and things alike, Whole person takes the one person
you pick, so on a group you can brighten one person and leave the rest.
Where two people stand so close that the model sees them as one, each
takes their own side of the pair. Someone of whom only a head shows
gets their face. Its edge comes from the Subject model, so loose hair
against the sky is kept as Subject keeps it; where the person touches
someone else, the line between them is the People model's. Without the
Subject model on the machine, Whole person still works with a coarser
edge, and the status line says so. The first Whole person you ask for
offers the Subject model's download.

Clothing goes by where it is worn: **Top** takes a coat and the shirt
under it as one, **Bottoms** whatever is worn below the waist.
**Iris**, **Lips** and **Teeth** look closer, at each face and then
each eye or the mouth, so they take longer than the rest.

On a picture of one person, the shape is made at once, as Subject
is, and it is that person's. On a picture of more than one person,
each face is outlined and the status line says "click the person, or
All people". Click a person to make the shape theirs alone, click
**All people** above the picture to take everyone's, or press **Esc**
to make nothing. On a picture with no one in it, the shape is made at
once and is everyone's.

A People shape keeps who it is of, even when it was made on a portrait
of one person and you were never asked. On the picture it was made on,
that is the person whose face is where it was, however you turn or
edit the picture afterwards; a change of white balance, camera
profile, lens or noise reduction may ask you to click them once, the
face where theirs was outlined more strongly. Pasted or synced onto
another picture, it looks for the same person there by their clothes,
hair and skin, not by where they stood: pasted onto another frame of
the same person, it finds them, though someone dressed very like them
can be taken for them.

When it can't be sure, as on most pictures of several people, the mask
stays empty and the faces are outlined again, the likeliest person
outlined more strongly. When it doesn't find its person at all, as on
a portrait of someone else, it never takes whoever is there: the faces
are outlined, none more strongly, and the status line says "this mask
was made for someone else". Either way, click a person to settle it
for that picture, or **All people** to make that picture's copy
everyone's; press **Esc**, or carry on, and the mask stays empty. If
you were on another tab, the outlines come back when you return to
**Masks**. On a picture with no one in it, the mask stays empty and
the status line says so. Nothing stops a paste or sync onto many
pictures: each one asks when you open it. After a paste or sync onto
several pictures, a person clicked on one of them brings up **Use this
person on all N pictures** under the shapes: press it and greycard
looks for that person on the other pictures that still have the mask
as it came, by their clothes, hair and skin and by where they stood on
the picture you clicked, and gives it to them where it finds them
(each picture's history gets a step, "Person from" the picture you
clicked, that undoes on its own). Where it can't tell, the picture
keeps asking, and the status line counts them ("used this person on 6
of 8 pictures; 2 not found"); the cross beside it puts the offer away.
An export with a People
shape that hasn't found its person writes the mask empty and says so
in its result line and the log ("a People mask needs a person picked",
or "a People mask's person isn't in this picture").

A People shape made with **All people** stays everyone's wherever it
goes: pasted onto another picture, it takes everyone there.

On a machine without a GPU, a People shape takes several seconds,
and an iris, lips or teeth about thirty.

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
**COLOR GRADING**, **TINT** and **DETAIL** work as they do on the
Develop tab, but only where the mask reaches. **BLACK & WHITE** and
the **Display curve** switch are global only. A mask's tone sliders
act even when the global **LIGHT** section is switched off. Each of
the mask's sections has its own switch, **TINT** included, so you can
compare the mask with and without it.

A mask's curve editor shows the picture's histogram behind the curve,
as the global one does.

### Detail in a mask

The mask's **DETAIL** has **Texture**, **Clarity** and **Dehaze**, as
the Develop tab's does: smooth the skin of a face and leave the rest
sharp, or clear the haze from a distant ridge and leave the
foreground. It shows in the viewport, the export and the thumbnails
alike, and its steps land in the history like any other.

A mask over the whole frame gives the same picture as the global
slider, one slider at a time. A mask's **Dehaze** used together with
the same mask's **Texture** or **Clarity** comes out a little
differently from the global pair, since the mask reads the haze
before its own detail is applied.

### White balance in a mask

Use this for mixed light: a room under lamps with a window, a stage
under two colors of light. Set the global white balance for one light,
then give a mask the other.

The mask's **WHITE BALANCE** section is off until you switch it on at
its title. The first time, it starts at the global white, so nothing
jumps.

- **Temperature** (2000 to 12000 K) and **Tint** name the light under
  the mask.
- **Neutral** gives you a dropper: click something under the mask that
  should be grey. It switches the section on for you.
- The switch keeps the mask's light while it is off: off, the area
  under the mask takes the global white; on again, the mask's own
  light comes back. Use it to compare the two.
- **Double-click** **Temperature** or **Tint** while the section is on
  to forget the mask's light. The section switches off, **Follow
  global** goes off with it, and the mask follows the global white
  again; the next time you switch it on it starts there.

There is no **As shot** or **Auto** here. With **Follow global** off,
the values are a light: a mask set to 3200 K stays a 3200 K lamp when
you move the global white balance. With it on, they are an offset
from the global (+25 mired, a tint of +0.002), so a mask made warmer
stays that much warmer when the global moves. The offset is in mired,
so the same number is the same change of warmth at any global; plus
is toward the higher kelvin, as on the Temperature slider. Switching
it either way moves nothing on screen, and the dropper still makes
the patch grey.

Over blown highlights, a mask whose white is far from the global one
can tint them if you pull exposure well down.

## Models download on first use

Subject, Background, Sky, Landscape, Object and People use learned models that greycard
downloads the first time you need one. A sheet names the model, its
size, where it comes from and its license before anything downloads;
choose **Download** or **Not now**.

| Shape | Download |
|---|---|
| Subject, Background | about 114 MB |
| Sky, Landscape | about 96 MB, plus the Object model for a finer edge on Sky |
| Object | about 184 MB |
| People | about 1.04 GB, plus the Subject model for Whole person's edge |

Each is downloaded once. The first Subject or Background mask of a
session takes a few seconds while the model loads; after that, masks
come quickly. Without the Object model, Sky still works, with a
coarser edge, and clicks can only remove sky.

A learned mask that a Fill lies over is made again on the retouched
picture, so a person a Fill took out leaves the Subject mask too; a
Heal, a Clone, or a Fill elsewhere leaves it alone.

### On an older processor

The models run on ONNX Runtime, and the copy that comes with greycard
needs a processor with AVX2 (most Intel Core processors from 2013 on, and
AMD's from 2015 on). On an older one, on Linux or Windows, greycard
still starts and edits as usual, but the first time you ask for a
learned mask, the eraser or the learned denoiser, the same sheet
offers Microsoft's own ONNX Runtime instead: about 9 MB on Linux and
79 MB on Windows, under the MIT license, kept beside the models. It
runs the models on the CPU alone, so they are slower than on a GPU,
and the status line says so ("on CPU (slower)"). **Not now** leaves
the AI features off for the session, and the status line says why.
From the command line, `greycard models --fetch onnxruntime` fetches
it; an `--export` on such a machine before it is fetched writes the
picture without the learned masks and exits 2, naming it.

### On Windows on ARM

No copy of ONNX Runtime comes with greycard on Windows on ARM, so
there it is the same on every processor: the first time you ask for
a learned mask, the eraser, or the learned denoiser, the sheet offers
Microsoft's ARM64 build, 80 MB from github.com and 16 MB kept, under
the MIT license. The models run on the CPU alone, and the status line
says so. **Not now** and `greycard models --fetch onnxruntime` work
as above.

## Masks and other frames

Masks are drawn around one picture, so syncing settings onto other
frames leaves them out unless you check **Masks**, or one mask under
it by name. **Replace masks** takes each frame's own away; **Add to
masks** keeps them and puts the copied ones after. See
[Presets and history](presets-and-history.md).

A Subject, Background, Sky, Landscape or People mask copied onto another frame is
made again for that frame, from that frame's own picture. A paste of
masks onto the frame on screen shows at once.
