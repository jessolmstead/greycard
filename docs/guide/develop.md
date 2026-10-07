# The Develop tab

The **Develop** tab holds the edits for the whole picture. This
chapter goes through it top to bottom. For edits to part of the
picture, see [Masks](masks.md). For presets and undo, see
[Presets, history and several frames](presets-and-history.md).

## Working with the panel

### Camera preview first

When you open a frame, you first see the camera's own JPEG, marked
"camera preview". greycard's develop replaces it a second or two
later. Droppers and drawing tools wait for the develop, but you can
pan and zoom straight away.

### Sliders

- **Drag** the handle, or click the track to jump there.
- **Double-click** a slider to put it back to its default.
- **Type a value:** click the number at the right, type it in the unit
  shown and press Enter. Escape leaves it as it was.
- **Fine steps:** click a slider, then use the left and right arrow
  keys or the mouse wheel. Shift with the wheel moves any slider
  without clicking it first.

### Sections

- Click a section's title to fold it away or open it again.
- Most sections have a **switch** at the right of the title. Off, the
  picture is as if the section were not there. Its settings stay, dimmed,
  for when you turn it back on. Use it to compare before and after.
- Some sections have a **Reset** button that puts the whole section
  back to its defaults.
- A section marked "raw only" does nothing on a JPEG, PNG or TIFF.

## White balance

Sets the color of the light, so that whites and greys come out
neutral.

| Control | What it does |
|---|---|
| **As shot** | On by default: the camera's own white balance. Turn it off to set your own. |
| **Temperature** | 2000 to 12000 K. Higher makes the picture warmer. |
| **Tint** | −0.05 to +0.05. Positive makes it more magenta, negative more green. |
| **Auto** | Reads the white balance from the picture. |
| **Neutral** | A dropper: click something in the picture that should be grey. |

**Auto** reads the light from the edges in the picture, so a large
sky or sea does not throw it off. It takes most of the warmth out of
tungsten and candle light; to keep that warmth, set the white by hand.

Press Escape to put the **Neutral** dropper down.

## Camera profile

You do not need one. The default, **Embedded**, uses the color matrix
the camera maker put in every raw file. That is what most people
develop with.

A DCP profile gives Adobe's rendering of your camera, or a profile
made for your own copy of it. Put `.dcp` files in the folder the
section names (`greycard/profiles` in your data directory). The list
shows the profiles made for the open frame's camera.

Where to get profiles:

- **Adobe's own**, free with the DNG Converter or Lightroom. Their
  license does not allow anyone else to pass them on, so copy yours by
  hand: on Windows from
  `C:\ProgramData\Adobe\CameraRaw\CameraProfiles`, on a Mac from
  `/Library/Application Support/Adobe/CameraRaw/CameraProfiles`. On
  Linux, run the DNG Converter once under Wine to unpack them.
- **RawTherapee's**, published by that project for the cameras it
  covers.
- **Your own**, from a ColorChecker shot run through dcamprof or the
  chart maker's tool.

If you choose a profile made for another camera, a warning says so.

## Light

| Slider | Range | What it does |
|---|---|---|
| **Exposure** | −5 to +5 EV | Brightens or darkens the whole picture. |
| **Contrast** | 0.5 to 2 | 1 is neutral. Above 1 adds contrast, below 1 takes it away. |
| **Highlights** | −2 to +2 EV | Darkens or brightens the bright areas. Pull it down to bring back a sky. |
| **Shadows** | −2 to +2 EV | Lifts or deepens the dark areas. |
| **Whites** | −2 to +2 EV | Moves the white point: where the brightest tones reach white. |
| **Blacks** | −30% to +30% | Moves the black point. |

**Highlights** and **Shadows** work on areas, not single pixels, so
the texture inside a sky or a shadow keeps its contrast.

### Display curve

At the foot of **LIGHT**, on a raw, the **Display curve** switch
chooses how the picture's tones and colors are fitted to the screen:

- **Per channel** is the default. Very bright, saturated colors
  shift on the way to white: blue toward magenta, red toward yellow.
- **AgX** takes bright, saturated colors to white more gradually and
  keeps their hue. Neon, stage lights and deep blue skies often look
  more natural.

The choice belongs to the picture. Presets, sync and paste leave it
alone. A fitted look is made for one display curve; see
[LOOK](#look).

## Color

| Slider | Range | What it does |
|---|---|---|
| **Vibrance** | −100% to +100% | Strengthens or mutes colors, more on pale colors and less on skin. |
| **Saturation** | −100% to +100% | Strengthens or mutes every color alike. −100% is grey. |

## Curves

Choose **Parametric** or **Point** at the top. **Reset** puts back
the curve you are looking at.

### Parametric

Four sliders, each −100 to +100: **Highlights**, **Lights**,
**Darks** and **Shadows**. Each raises or lowers its part of the
tonal range. You can drag the three dividers between the parts on the
graph. Double-click a divider to put it back.

### Point

Pick a channel: **RGB**, **Red**, **Green**, **Blue**, **R/G** or
**B/Y**.

- Click the curve to add a point, and drag it to move it.
- Double-click a point to remove it.
- **Pick** gives you a dropper: click a tone in the picture to add a
  point there, then drag up or down to move it.
- The point you last pressed stays selected. Type its position into
  **In** and **Out** below the curve (0 to 255) and press Enter.

**R/G** and **B/Y** tint by brightness without changing it. Raise
**R/G** for red, lower it for green. Raise **B/Y** for yellow, lower
it for blue. For example, lower **B/Y** at the left end to cool the
shadows.

## Color mixer

Changes one range of colors at a time. Click one of the eight color
swatches, then set:

| Slider | Range | What it does |
|---|---|---|
| **Hue** | −30° to +30° | Moves the color toward its neighbor on either side. |
| **Saturation** | −100% to +100% | Strengthens or mutes that color. |
| **Luminance** | −1 to +1 EV | Brightens or darkens that color. |

**Pick** gives you a dropper. Click a color in the picture to choose
its swatch. Drag up or down to change its saturation, or sideways to
change its hue. **Reset** clears every color.

While **BLACK & WHITE** is on, the mixer has no effect. It is dimmed,
and your settings come back when you turn black and white off.

## Black and white

Turns the picture monochrome. Choosing a filter turns the section on.

- **Filter:** **None**, **Red**, **Orange**, **Yellow**, **Green** or
  **Blue**. These work like colored filters on black-and-white film.
  **Red** darkens a blue sky, and **Green** lightens foliage.
- **Swatches and Weight:** pick a color, then use **Weight** to make
  it lighter or darker in grey. The reading is in EV.
- **Strength:** 0 to 3.00x. It scales the whole set of weights, so the
  filter keeps its character and changes only how strong it is.
- **Reset** puts the section back to its defaults.

## Color grading

Three wheels: **Shadows**, **Mid-tones** and **Highlights**.

- Drag in a wheel to set its color. The angle is the hue, and the
  distance from the center is the strength.
- Double-click a wheel to clear it.
- Click a wheel's name to show it larger for finer control. Click it
  again to shrink it.
- **Hue** (0 to 360°) and **Strength** (0 to 100%) set the wheel you
  last used, by number. Its name is shown brighter.
- **Balance** (−100 to +100) moves the split between shadows and
  highlights.
- **Reset** clears all three wheels.

## Tint

Tints the whole picture toward one color.

- Drag in the ring. The angle sets the hue, and the distance from the
  center sets the amount. Double-click the ring to clear it.
- **Hue** (0 to 360°) is for fine adjustment. **Amount** goes from 0
  to 100%.
- The small swatch shows what a mid grey turns into.
- **Reset** clears the tint.

This section has no switch. An **Amount** of 0 means no tint.

## Look

A look is a color table laid over the finished picture. It can be a
film preset, a LUT you bought or made, or a look fitted to your
camera's JPEGs.

- Click a look in the list to use it. **None**, at the top, uses no
  look.
- **Strength** blends the look in, from 0 to 100% (the default).
- **Fit this camera's look…** opens the camera match. See
  [Fitting your camera's look](camera-match.md).
- **Rename…** renames the chosen look. Every picture in the open
  folder, every preset and every snapshot that uses it takes the new
  name. Edits in other folders aren't reached: they show the old name
  as missing, and the old name's files stay until nothing in reach
  still needs them.
- **Remove this look…** moves the chosen look's files to the trash,
  after you confirm.
- **Reset** goes back to no look at full strength.

Looks fitted by the camera match are grouped by camera, with the
open frame's camera first and marked "this camera". Each fitted look
is made for one display curve. If it has no version for this picture's
curve, the section says so and offers a button such as **Refit under
AgX…**.

To add your own, put `.cube` files or HaldCLUT `.png` files in the
`greycard/looks` folder in your data directory. If you have no looks
yet, the section names the folder.

## Noise

Raw only. Choose one of two kinds of noise reduction.

### The learned denoiser

The buttons along the top choose it: **off** (the default), **fast**,
**balanced** or **best**. The slower tiers clean noise better.
**fast** is quickest. **best** can take a while on a large frame.

**Blend** mixes the denoised picture with the original, so a little
grain stays. When you first open a raw, **Blend** is set from its ISO:
35% at ISO 200 and below, up to 100% at ISO 3200 and above. Presets
leave it alone.

The learned denoiser also does the demosaic (see
[DEMOSAIC](#demosaic)), so while it is on, the **DEMOSAIC** choice and
**Denoise** below have no effect.

### The profiled denoiser

**Denoise** turns it on. It is off by default. **Strength** goes from
0.5 to 3, with 1.5 as the default. It is quicker than the learned
denoiser.

## Lens

Corrects what the lens does to the picture. The line at the top names
the lens profile that was found, or says why none was:

- "no lens profiles on this machine": click **Get lens profiles**.
- "the file names no lens": for example a manual lens. Use the manual
  sliders below.
- "no profile for" a lens: the database does not know it.

### From the profile

These are all on by default when a profile is found:

- **Profile** turns all the profile's corrections on or off.
- **Distortion** straightens lines that bow in or out.
- **Chromatic aberration** removes the colored edges toward the
  corners.
- **Vignetting** brightens the darkened corners.

### By hand

- **Manual** corrects distortion on top of the profile.
- **Red / cyan** and **Blue / yellow** remove colored edges by hand,
  in per mille.
- **Auto scale**, on by default, enlarges the picture just enough to
  hide the empty edges a correction leaves. Turn it off to set
  **Scale** yourself, from 50% to 200%.

### Defringe

**Defringe**, off by default, removes purple and green fringes along
high-contrast edges, such as branches against a bright sky. This is a
different lens fault from the colored edges above.

- **Radius** (0.5 to 5 px) and **Threshold** (0 to 100) set how wide
  a fringe can be and how strong it must be before it is removed.
- **Purple** and **Green** each have **Center** (the hue), **Width**
  (how many hues count) and **Amount**.
- **Pick hue** gives you a dropper. Zoom to 1:1 and click a fringe to
  center the nearer of the two on its hue.

## Detail

| Slider | Range | What it does |
|---|---|---|
| **Texture** | −100 to +100 | Fine detail: skin, fabric, foliage. Negative smooths it. |
| **Clarity** | −100 to +100 | Larger-scale contrast in the mid-tones. Negative softens it. |
| **Dehaze** | −100 to +100 | Removes haze. Negative adds it. |

## Sharpen

Capture sharpening restores the crispness lost in the camera. It is on
by default for raws and off for JPEG, PNG and TIFF files, which have
already been sharpened once.

- **Auto radius** measures the right radius from the picture. Turn it
  off to set **Radius** (0.4 to 2) yourself. With it on, the reading
  shows the measured value.
- **Iterations** (5 to 50, default 20): more passes sharpen harder.
- **Auto threshold** keeps flat areas, like sky and skin, from being
  sharpened. Turn it off to set **Threshold** (0 to 50%) yourself.
- **Show mask** paints in red where the sharpening acts.

## Vignette

Darkens or lightens the edges of the picture as cropped.

| Slider | Range | What it does |
|---|---|---|
| **Amount** | −5 to +5 EV | Negative darkens the edges, positive lightens them. |
| **Midpoint** | 0 to 100% | How far in from the edges it starts. |
| **Feather** | 0 to 100% | How soft the transition is. |
| **Roundness** | −100 to +100 | 0 follows the frame's shape. Positive is rounder, toward a circle. Negative is squarer. |

## Grain

Adds film grain. It follows the crop and looks the same at any export
size. At the fit zoom you may hardly see it, so check it at 1:1.

- **Amount** (0 to 100%).
- **Size** (0.1 to 2‰ of the frame's width).
- **Cubic** or **Tabular**: classic film grain, or the finer and more
  even grain of modern film.

## Soft proof and monitor

These two sections change what you see on screen, not the edit. See
[Scopes and color](scopes-and-color.md).

## Demosaic

Raw only. This chooses how the sensor's data becomes a full color
picture. The default, **amaze-vng4**, suits almost every frame. Try
another only if you see a problem in fine detail:

- **rcd-vng4** and **rcd** are quicker, and smoother on noisy or
  strongly colored edges.
- **amaze** keeps the most detail.
- **vng4** is soft, but free of the maze-like pattern noise can give
  the sharper choices.
- **bilinear** is the softest.

When a learned denoiser is on, it does the demosaic itself and this
choice has no effect.

## What happens without a control

Two repairs run on every raw. Neither has a setting:

- **Hot pixels.** A sensor site stuck bright or dark is replaced from
  its neighbors. Only a site lit in one color counts, so stars, glints
  and catchlights stay. A stuck pixel on busy texture may be missed.
- **Clipped highlights.** Where one or two color channels clipped,
  greycard rebuilds them from the channels that did not. A blown
  highlight then keeps a natural color instead of taking a cast.

## Downloads on first use

Two Develop features download what they need the first time you use
them. Each shows you its license before it downloads:

- the lens correction database, which is under a megabyte
- the learned denoiser, 5 to 20 MB depending on the tier

Each one is downloaded once.

greycard offers the lens database without being asked. The first
time a picture opens and there are no lens profiles on the machine,
it asks whether to download them, since without them no lens is
corrected. If you answer **Not now**, it will not ask again. The **Get
lens profiles** button in **LENS** is there whenever you want them.
