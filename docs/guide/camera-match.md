# Fitting your camera's look

Every raw file carries a JPEG the camera made from it, in the picture
style you had set: Canon's Faithful or Standard, Fujifilm's Provia or
Reala Ace, Nikon's Neutral, Sony's Standard, Panasonic's Natural.
**Fit this camera's look…** learns that rendering from your own raws
and keeps it as a look you can put on any frame. Use it when you like
how your camera's JPEGs look and want that as a starting point for
your raws.

Nothing is shipped: every fitted look is made on your machine, from
your frames.

## What a fitted look is

greycard develops a sample of your frames, lays each camera JPEG over
greycard's own version of the same frame, and learns the difference
in color and tone. The result is a look like any other in the
**LOOK** section, with a **Strength** slider.

There is one look for each camera body and picture style, named for
both, such as "Canon EOS R6 Mark II Faithful". Faithful and Standard on
the same body are different looks, because the camera renders them
differently.

The look sits at the end of the edit, after your sliders. With the
sliders at their defaults you get the camera's picture. After you move
a slider, you get the camera's rendering of the changed picture.

## What frames it needs

- **Canon `.CR3`, Fujifilm `.RAF`, Nikon `.NEF`, Sony `.ARW` or
  Panasonic `.RW2` raws.** greycard reads the picture style from these;
  other makers' raws, and Canon's older `.CR2`, are not fitted yet. A
  library indexed before a maker was read reads that maker's frames
  again on its next pass.
- **About 20 frames or more** of one body and one style. A group is
  sampled down to about 40. A group under 20 can borrow a look (see
  below) or is skipped.
- **The scene-adaptive settings off**, where you can. These change the
  camera's JPEG from scene to scene, so a frame shot with them on
  teaches the look less. They come in two kinds, below.
- **Variety.** Frames from many shoots, places and light make a better
  look than one session. The sample takes a few frames from every
  folder first for this reason, and the sheet warns when every frame is
  from one folder.

### The adaptive settings

Settings that bend the style's tone per scene leave the frame in its
style's group. Once a group has 20 frames with them off, the rest are
left out; a group with fewer uses frames with them on to make up its
sample.

| Camera | Tone settings |
|---|---|
| Canon | Auto Lighting Optimizer, Highlight Tone Priority |
| Fujifilm | Dynamic Range above 100% or on auto, D-Range Priority, Highlight Tone or Shadow Tone not at 0 |
| Nikon | Active D-Lighting, Contrast or Saturation set to A |
| Sony | Dynamic Range Optimizer (any level or Auto) |
| Panasonic | Intelligent D-Range (Intelligent Exposure on older bodies) |

Settings that render in place of the style, or draw over it, take the
frame out of every group: it was not rendered in that style, so it is
never used, and the sheet counts it with the raws that have no fixed
picture style. So does a style that adapts to the scene itself.

| Camera | Settings that replace the style |
|---|---|
| Canon | the Auto picture style |
| Nikon | the Auto Picture Control (and a custom one built on it), a scene mode, the Auto, scene and effects positions of the mode dial |
| Sony | the scene modes, Intelligent Auto and Superior Auto, Auto HDR, a Picture Effect |
| Panasonic | the scene modes and Intelligent Auto, the Auto photo style, HDR, a creative filter (Filter Settings) |

A setting greycard cannot read or does not recognize takes the frame
out of every group too, so a frame is never taken as fixed on a guess.

## Fitting a look

1. Open a frame, ideally from the camera you want to fit.
2. In the **LOOK** section, press **Fit this camera's look…**.
3. Choose where to look for frames: **Library** (every root of your
   [library](library.md)) or **This folder**. The library is the
   default when you have one, since it holds the most variety.
4. Read the list. Each line is one body and style: how many frames it
   has, how many have the adaptive settings off, and what the fit will
   do with it.
5. Untick the groups you don't want. **Fit** counts the ticked groups
   ("Fit 3 groups").
6. Press **Fit**.

Each group's result appears as a line when it is done. When the run
ends, the new looks are in the LOOK section's list.

### What the lines say

| Line says | Meaning |
|---|---|
| fits on N of them | Enough frames: it gets a look of its own |
| too few; borrows the look fitted on X | Under 20 frames, but another body with the same style has enough: it gets that body's look under its own name |
| skipped | Too few frames and nothing to borrow, or a look it should not replace (the reason follows) |
| not chosen | You unticked it |
| read only, for a borrow | Unticked, but a ticked group borrows from it, so its frames are still developed |

A borrowed look is not as close as a look of the body's own, but close
when the bodies share a sensor or a generation.

The ticks you leave are remembered for the library and for each
folder.

### Replacing looks you already have

By default a run leaves an existing look alone when it would be worse
to replace it: a look you added yourself under the same name, a body's
own look that a borrow would replace, or a look fitted from more frames.
The group's line says so. Turn on **Replace existing looks** to replace
them anyway.

### How long it takes

Each frame is developed and compared several times, a few seconds a
frame, so a group of 40 takes a few minutes and a library with many
bodies and styles can take much longer. A bar fills across the whole
run, with the count of frames done and the group under it; while a
group's look is fitted, the bar holds and the words count the frames
held out. **Stop** ends the run after the frame in hand, or after the
group whose look is being fitted; the groups already done are kept.

### Reading the result

A fitted group's line reads like "fitted on 38 frames, ΔE 0.013 held
out over every frame (0.011 fitted)". ΔE is how far the look lands from
the camera's JPEGs: smaller is closer, and figures around 0.01 to 0.02
are typical. The held-out figure is the honest one: it is measured on
frames the look did not learn from.

The line also says how many frames "did not register" (the JPEG would
not line up with the raw) and, for each lens, how the corners differ
from the camera's JPEG. That last figure is the camera's own
vignetting correction against greycard's; the look does not change it.

## Using a fitted look

Pick it in the **LOOK** section like any other look, and use
**Strength** to take it part way. A preset can carry it.

The look gives you the camera's color and tone, not its brightness.
Camera JPEGs come out at their own exposure from scene to scene, so the
fit leaves brightness to the **Exposure** slider.

A look fitted on one body works on another body's frames. It is not
swapped for that body's own look. When the look was fitted on another
body than the open frame's, the section says "fitted on" that body.

### The look list

Once you have fitted looks, the list is grouped:

- **None**;
- the open frame's body, open and marked "this camera";
- every other body with a fitted look, folded, with a count;
- **General**: film looks and any other `.cube` or `.png` look you
  added.

Click a heading to fold or open it.

### One table per display curve

A fitted look is learned under one **Display curve** (in the
**LIGHT** section), **Per channel** or **AgX**, and applies only to a
picture on that curve. A look can hold one table for each.

Each fitted look's row names the curves it has ("per channel · AgX").
A look with nothing for the open picture's curve is dimmed. If you pick
it, the section says it is off for this picture and offers **Refit
under AgX…** (or under per channel). That opens the sheet for just
that look's group, and the run adds a table for the picture's curve
beside the one the look has.

A refit needs the look's frames in the scope. If none are there, the
sheet says so and **Fit** stays off.

### Removing a look

Pick the look and press **Remove this look…**. The sheet lists its
files and says how many pictures in this folder (or the library) use
it. **Move to the trash** removes every table of the look together.

Pictures that used it show the look as "(missing)" and render without
it. Their edits are not changed, so putting the files back brings the
look back.

## Where looks are kept

In a `greycard/looks` folder under your data folder: `~/.local/share`
on Linux, `~/Library/Application Support` on a Mac, `%APPDATA%` on
Windows. A fitted look is a `.cube` file, and a table for AgX sits
beside it with `.agx` before the extension. Copy them along with the
look to use it on another machine.

## Limits

- Picture styles are read from Canon CR3, Fujifilm RAF, Nikon NEF, Sony
  ARW and Panasonic RW2.
- Canon's per-style contrast, saturation and color tone settings,
  Fujifilm's Color, Color Chrome and Clarity, a Nikon Picture Control's
  sharpening, contrast, saturation and hue steps, Sony's contrast,
  saturation and Creative Look settings, and a Panasonic Photo Style's
  own adjustments are not read, so a group may mix frames shot with
  different settings.
- Panasonic writes the same code for Standard and for its custom
  styles, so they share one group, "Standard or Custom", and a fit may
  mix them; a high held-out error on that group is the sign. Photo
  styles the camera writes that greycard has no name for (some newer
  ones, such as the Leica looks and the cine styles) are not fitted.
- Sony's Picture Profiles are not read, so a frame shot under one may
  still group under its Creative Style.
- A look fitted under **AgX** lands a little further from the camera
  than one fitted under per channel, since the camera's own tone is
  closer to per channel.
- A fit from one folder is usually one session's light and lens, and
  may not suit other shoots.
- While a run is going, the sheet stays open.

For how the camera match was measured, see
[camera-match.md](https://github.com/jessolmstead/greycard/blob/master/docs/camera-match.md).
