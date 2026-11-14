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

### Moving around the picture

- A **click** on the picture zooms to 1:1 there, and another goes
  back to fit. **Space** does the same, and **Z** zooms to 3:1.
- A **middle click** on the picture zooms to 200% there, and another
  goes back to where you were.
- The **Fit** button at the panel's foot fits the picture; its chevron,
  or a right-click on it, offers Fit, 25%, 33%, 50%, 100%, 200%, and
  400%; under them, **Full screen** (**F11**).
- **Drag** to move the picture when you are zoomed in. With a tool in
  hand that draws on the picture (a repair, or a mask's shape waiting
  for its drag), hold **Space** and drag (**Shift** too, except with a
  Linear or Radial waiting, where Shift holds the shape): the pointer
  is a hand, the picture moves, and nothing is drawn. A tap of **Space**
  still zooms. A **middle drag** moves the picture whatever is in hand,
  the tool left as it was, and a middle click during any other drag
  ends that drag where the pointer is.
- A **mouse wheel** zooms a step in or out about the pointer with each
  click. A tilted wheel moves the picture sideways.
- A **two-finger scroll** on a touchpad moves the picture, in the
  direction your system scrolls.
- **Ctrl** with any scroll (⌘ on a Mac) zooms smoothly about the
  pointer, with a brush or a repair tool in hand too.
- **+** and **−** zoom a step in or out about the middle of the view.
- A **pinch** on a Mac trackpad, or on a Windows precision touchpad,
  zooms about the pointer. On Linux a touchpad pinch does nothing yet;
  use Ctrl and a scroll.
- **F** puts a mat around the fitted picture, a band of the canvas
  color on every side, to judge exposure and color against. **F**
  again takes it away. Zoomed in, the picture fills the view as
  before. The mat is remembered.

A scroll over an open sheet stays with the sheet and never zooms the
picture behind it.

### Icon buttons

A button that shows only an icon, such as undo and redo at the top of
the panel, a bin, or the arrows that put a panel away, says its name
in a word when you rest the pointer on it: "Undo", "Remove", "Hide
panel". The word goes when you click, press a key or scroll.

### Sliders

- **Drag** the handle, or click the track to jump there.
- **Double-click** a slider to put it back to its default.
- **Type a value:** click the number at the right, type it in the unit
  shown and press Enter. Escape leaves it as it was.
- **Fine steps:** click a slider, then use the left and right arrow
  keys or the mouse wheel. Each click of the wheel is one step. Shift
  with the wheel moves any slider without clicking it first. Hold Ctrl
  with the arrows or the wheel for finer steps, a tenth of one where
  the number shows it (Ctrl+Shift with the wheel on a slider not
  clicked). Hold Shift while dragging a slider and it moves a tenth as
  far as the mouse does.

### Sections

- Click a section's title to fold it away or open it again.
- Most sections have a **switch** at the right of the title. Off, the
  picture is as if the section were not there. Its settings stay, dimmed,
  for when you turn it back on. Use it to compare before and after.
- Some sections have a **Reset** button that puts the whole section
  back to its defaults.
- A section marked "raw only" does nothing on a JPEG, PNG, or TIFF.

## Look

**LOOK** is the first section, since a look is usually the first
thing you choose. A look is a color table laid over the finished
picture. It can be a film preset, a LUT you bought or made, or a look
fitted to your camera's JPEGs.

- Click a look in the list to use it. **None**, at the top, uses no
  look.
- Rest the pointer on a look in the list to see it on the picture, at
  the **Strength** set. Move off and the look you chose comes back;
  click to keep the one under the pointer. Nothing is saved until you
  click.
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

## White balance

Sets the color of the light, so that whites and greys come out
neutral.

| Control | What it does |
|---|---|
| **As shot** | On by default: the camera's own white balance. Turn it on to return to it, and off again to get back the white you had. |
| **Temperature** | 2000 to 12000 K. Higher makes the picture warmer. |
| **Tint** | −0.05 to +0.05. Positive makes it more magenta, negative more green. |
| **Auto** | Reads the white balance from the picture. On a raw, its arrow opens the presets. |
| **Neutral** | A dropper: click something in the picture that should be grey. |

On a JPEG, PNG, or TIFF there is no camera white to name, so the two
sliders move the picture from its own white instead: **Temperature**
in mired, plus warmer, and **Tint** as the same Duv difference, both
0 under **As shot**. **Auto** and **Neutral** work the same way, and
a mask's white balance adds to it.

Under **As shot**, **Temperature** and **Tint** show the camera's own
white. Move either and **As shot** turns off by itself, with the
slider where you left it; it is one step in the history, as any
slider's move is. Turn **As shot** back on to go back to the camera's
white. Turn it off again and the white you had before comes back,
while the frame stays open; with none to give back (the frame opened
at **As shot**, or the edit changed some other way since), the sliders
stay at the camera's. After a move, a double-click puts one slider back to the
camera's value and leaves the other where you set it; **As shot**
stays off. Under **As shot** a double-click changes nothing.

**Auto** reads the light from the edges in the picture, so a large
sky or sea does not throw it off. It takes most of the warmth out of
tungsten and candle light; to keep that warmth, set the white by hand.

On a raw, the arrow at the right of **Auto** (or a right-click on it)
opens the presets: **Daylight**, **Cloudy**, **Shade**, **Tungsten**,
**Fluorescent**, and **Flash**. Each sets **Temperature** and
**Tint** to that light and turns **As shot** off, one step in the
history, as **Auto** is. They are the same light on every camera:

| Preset | Temperature | Tint |
|---|---|---|
| Daylight | 5500 K | +0.003 |
| Cloudy | 6500 K | +0.003 |
| Shade | 7500 K | +0.003 |
| Tungsten | 2850 K | 0 |
| Fluorescent | 3800 K | +0.007 |
| Flash | 5500 K | 0 |

The section does not show which preset you chose; after it, the
sliders are yours to move. A JPEG, PNG, or TIFF, and a mask's white
balance, have no presets.

Press Escape to put the **Neutral** dropper down.

## Camera profile

You do not need one. The default, **Embedded**, uses the color matrix
the camera maker put in every raw file. That is what most people
develop with.

A DNG can carry more, and greycard applies some of it: the forward
matrix made for the camera, and a phone's correction for its lens's
dark corners. Not yet applied: a DNG's correction for lens
distortion, and a phone's local tone mapping and exposure baseline,
so a phone's DNG opens a little darker than the phone's own picture.

A DCP profile gives Adobe's rendering of your camera, or a profile
made for your own copy of it. Put `.dcp` files in the folder the
section names (`greycard/profiles` in your data directory), loose or in
folders of their own: Adobe's whole `CameraProfiles` folder can be
copied or linked in as it is. The list shows the profiles made for the
open frame's camera, by the name inside each file. The same profile
copied under two file names is listed once; two different profiles
listed with one name show their file names in parentheses.

**Show all cameras**, under the list, lists every profile, grouped by
the camera each was made for: the open frame's camera first, the
others folded under their names. Open a group to choose a profile
made for another camera. The switch only changes the list, and stays
as you leave it.

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

From a DCP file, greycard applies the profile's color: its matrices,
its color map, and its look table, which is where Adobe's Camera
Standard, Vivid, Portrait, and the other Camera profiles differ from
each other. (**Embedded** uses the raw file's own matrices only: a
DNG's color map and look table, when it carries them, are not
applied.) It does not apply a profile's tone curve: brightness and
contrast stay with **Light** and the display curve, so a Camera
profile brings a picture closer to the camera's own in color, not in
contrast.

A picture you developed with a DCP before greycard applied look tables
keeps its colors. To give it the look table, choose another profile
and then that one again, or lay a preset, a paste, or a sync with a
camera profile on it: anything that puts a DCP on a picture that way
always brings its look table.

## Light

| Slider | Range | What it does |
|---|---|---|
| **Exposure** | −5 to +5 EV | Brightens or darkens the whole picture. |
| **Contrast** | 0.5 to 2 | 1 is neutral. Above 1 adds contrast, below 1 takes it away. |
| **Highlights** | −2 to +2 EV | Darkens or brightens the bright areas. Pull it down to bring back a sky. |
| **Midtones** | −2 to +2 EV | Lifts or sinks the middle of the picture and leaves its black and its brightest areas where they are. |
| **Shadows** | −2 to +2 EV | Lifts or deepens the dark areas. |
| **Whites** | −2 to +2 EV | Moves the white point: where the brightest tones reach white. |
| **Blacks** | −30% to +30% | Moves the black point. |

**Auto** sets Exposure, Highlights, Shadows, Whites, and Blacks from
the picture's own histogram, as one step in the history. It leaves
Contrast and Midtones as they are. It never
clips more than the default develop does, and it keeps a night
frame's lights under white. Press it again and nothing changes.

**Highlights**, **Midtones**, and **Shadows** work on areas, not
single pixels, so the texture inside a sky or a shadow keeps its
contrast.

**Midtones** finds the middle of the frame from the frame itself, as
**Highlights** does: full half a stop under its middle tone, it fades
to nothing a stop and three quarters either side, short of where
**Highlights** is strongest. At ±2 the middle moves 0.875 of a stop.
It is not **Exposure**, which moves every tone alike,
nor **Contrast**, which moves the dark and the bright apart around mid
grey; it is closer to the curve's **Lights** and **Darks**, but works
on areas and follows the frame rather than fixed places on the curve.

**Highlights** and **Whites** read how bright the frame itself is.
**Highlights** reaches down into the upper mid-tones, as Lightroom's
does, and **Whites** works from the frame's middle tones upward rather
than from a fixed mid grey, so a face under bright light answers both.
A frame edited with either before 0.7 opens a little differently.

At **Exposure** 0, each camera body opens at a brightness of its own,
measured against that camera's JPEGs, so a frame opens about as
bright as the camera's own picture of it. A frame edited before 0.8 keeps the brightness it had; **Reset**
takes it to its body's.

### Display curve

At the foot of **LIGHT**, on a raw, the **Display curve** switch
chooses how the picture's tones and colors are fitted to the screen:

- **Per channel** is the default. Very bright, saturated colors
  shift on the way to white: blue toward magenta, red toward yellow.
- **AgX** takes bright, saturated colors to white more gradually and
  keeps their hue. Neon, stage lights and deep blue skies often look
  more natural.

Sync and paste carry it as a section of its own, **Display curve**,
checked by default beside **Light**; a preset carries it when saved
from an AgX picture. Uncheck it to leave each picture's own. A JPEG
or TIFF, which has no switch, neither takes a curve nor gives one. A
fitted look is made for one display curve; see [LOOK](#look).

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
- Drag the black point right along the bottom, or the white point
  left along the top, and every tone beyond it takes that point's
  Out. You can also type an end point's **In**. A click past a slid
  end point adds no point there.
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
change its hue. The dropper is put down when you let go, as the white
balance one is; press **Pick** again for another. **Esc** or **Pick**
puts it down without picking. **Reset** clears every color.

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

Three wheels: **Shadows**, **Midtones** and **Highlights**.

- Drag in a wheel to set its color. The angle is the hue, and the
  distance from the center is the strength. Hold **Shift** as you
  press to keep the hue and move only the strength, or **Ctrl** (⌘ on
  a Mac) to keep the strength and move only the hue.
- The wheel last pressed is in hand, its ring drawn heavier.
  Ctrl-click (⌘-click on a Mac) the picture to set its hue to the
  color there, or Ctrl+Shift-click for the opposite hue; its strength
  stays, and a wheel at zero takes one from the color's saturation.
  The color is read without the wheels and the tint, so a
  second pick on the same spot finds the same hue. Esc, or a control
  in another section, puts it down.
- Double-click a wheel to clear it.
- Click a wheel, or its name, to choose it. The chosen wheel has a
  ring around it and its name in the accent color.
- Click a wheel's name to show it larger for finer control. Click it
  again to shrink it. Either click chooses the wheel without moving
  its point.
- **Hue** (0 to 360°) and **Strength** (0 to 100%) set the chosen
  wheel by number. Choose a wheel by its name to fine-tune it from
  where it stands.
- **Balance** (−100 to +100) moves the split between shadows and
  highlights.
- **Reset** clears all three wheels.

Each change is a step in the history that names the wheel, such as
"Color grading shadows 40°".

## Tint

Tints the whole picture toward one color, as a colored filter would.
A blue tint over a warm scene cools it: colors far from the tint get
duller on the way to it rather than turning into other colors, and
greys take the tint's hue.

- Drag in the ring. The angle sets the hue, and the distance from the
  center sets the amount. Double-click the ring to clear it. Shift,
  Ctrl, and a Ctrl-click on the picture work as on the grading wheels.
- **Hue** (0 to 360°) is for fine adjustment. **Amount** goes from 0
  to 100%.
- The small swatch shows what a mid grey turns into.
- The switch at the title turns the tint off and on, keeping its hue
  and amount, like any other section.
- **Reset** clears the hue and the amount and leaves the switch as it
  is. The history calls it "Color tint reset".

## Noise

Raw only. Choose one of two kinds of noise reduction.

On a Fujifilm X-Trans raw the section says "learned: not on X-Trans
yet" and the learned tiers rest: the learned denoiser does not work on
that sensor yet. **Denoise**, the profiled one, works as on any raw. A
tier already set on an X-Trans frame (by a sync from another camera,
say) can still be turned off. See [Demosaic](#demosaic).

### The learned denoiser

The buttons along the top choose it: **off** (the default), **fast**,
**balanced** or **best**. The slower tiers clean noise better.
**fast** is quickest. **best** can take a while on a large frame. On
an Apple silicon Mac the model runs through Core ML, which is quicker
than the graphics path it used before; the first run of each tier on
a machine takes longer while Core ML prepares it.

On a graphics card short of memory (a 4 GB card, say), greycard first
lets go of what it can make again, then runs the model in smaller
pieces: the same picture, a little slower. If even the smallest piece
does not fit, the engine's own denoise stands in on screen and the
status line says so; an export of that frame is not written, since it
would not be the picture the edit asks for.

**Luma** and **Chroma** mix the denoised picture with the original,
the brightness and the color on their own. **Luma** below 100% leaves
a little grain; when you first open a raw it is set from its ISO, 35%
at ISO 200 and below, up to 100% at ISO 3200 and above. **Chroma**
starts at 100%, since color noise is rarely worth keeping. Presets
leave both alone.

The learned denoiser also does the demosaic (see
[DEMOSAIC](#demosaic)), so while it is on, the **DEMOSAIC** choice and
**Denoise** below have no effect.

### The profiled denoiser

**Denoise** turns it on. It is off by default. **Strength** goes from
0.5 to 3, with 1.5 as the default. It is quicker than the learned
denoiser.

## Lens

Corrects what the lens does to the picture. On a raw the three
corrections start on; on a JPEG, TIFF, or PNG they start off, since
the file was corrected once already by the camera or the editor that
made it, and each is a switch. **Settings > Editing > Lens corrections
on new edits** decides whether a raw's new edit starts with the
section on; a raw that already has a sidecar keeps its own. **Lens**, near the top,
picks whose profile corrects it. **From the file**, the default, is the
lens the file names. **Maker**, above it, narrows the list to one
maker's lenses, or **All**. The field beside **Maker**, **Find a
lens**, narrows it further to the lenses whose name or maker holds
every word typed, such as "24-70" or "sigma art". The line under
**Lens** names the profile found, or says why none was:

- "no lens profiles on this machine": click **Get lens profiles**.
- "the file names no lens": for example a manual or adapted lens. Pick
  it under **Lens**, or use the manual sliders below.
- "no profile for" a lens: the database does not know it. Pick it, or
  one like it, under **Lens**.

A compact's file often names no lens, or names it only as a focal
range, but its lens is part of the body: greycard finds it from the
camera. When the database has
several entries for that body, it takes the one that covers the focal
length. A raw gets the entry made from CHDK's DNGs where there is
one, and a JPEG never does, since the camera has corrected it
already. Past that it takes the plain body over a converter, and none
rather than a guess.

### Picking a lens

**Lens** lists the database's lenses that fit the body's mount, then
the Adobe lens profiles (`.lcp`) in the profile folder, by the lens
name inside each. The profile folder is the one that holds your DCP
camera profiles (see [Camera profile](#camera-profile)). greycard
reads an LCP's distortion, vignetting, and chromatic aberration,
matched to the shot's focal length, aperture, and focus distance.
A lens on another mount, on an adapter, appears when you type part of
its name in **Find a lens**, with its mounts after the name.

Where to get LCPs:

- **Adobe's own**, free with the DNG Converter or Lightroom, under the
  same license as their camera profiles, so copy yours by hand: on
  Windows from `C:\ProgramData\Adobe\CameraRaw\LensProfiles\1.0`, on
  a Mac from
  `/Library/Application Support/Adobe/CameraRaw/LensProfiles/1.0`.
  They sit in one folder per maker; copy the `.lcp` files themselves, as
  greycard reads only the profile folder's own files.
- **Your own**, made with Adobe Lens Profile Creator for a lens no
  database has.

An LCP made on another body is used when the lens database knows
yours, which gives your sensor's size; otherwise only one made on your
body is used.

The lens picked is kept in the edit, and goes with copy and paste and
with presets, as the **Profile** switch does. A lens that is not
there for this body (one the database lacks, or a profile not on this
machine) is still shown by its name, greyed, and corrects
nothing. An export without it says so.

### From the profile

These are all on by default when a profile is found:

- **Profile** turns all the profile's corrections on or off.
- **Distortion** straightens lines that bow in or out.
- **Chromatic aberration** removes the colored edges toward the
  corners. On a raw it is greyed: the aberration is measured on the
  frame itself and corrected before the profile, and the measurement
  has the last word. It is live on a JPEG, a TIFF, or a linear DNG.
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
| **Structure** | −100 to +100 | The broadest tonal shapes, gently. Negative flattens them. |
| **Dehaze** | −100 to +100 | Removes haze. Negative adds it. |

**Structure** works at a wider scale than Clarity, about a twelfth of
the picture's long edge: the lit side of a face against its shadow
side, a field against the hill behind it. It is soft by design. A
horizon or a hairline gets no halo, and its effect levels off on
strong contrasts instead of growing with them, so even at +100 it
reads as a quiet separation of tones, never as an HDR look. Like
Clarity, it leaves the brightest highlights and the deepest shadows
alone.

## Sharpen

Capture sharpening restores the crispness lost in the camera. It is on
by default for raws and off for JPEG, PNG and TIFF files, which have
already been sharpened once.

- **Auto radius** measures the right radius from the picture. Turn it
  off to set **Radius** (0.4 to 2) yourself. With it on, the reading
  shows the measured value.
- **Iterations** (5 to 50, default 20): more passes sharpen harder.
- **Auto threshold** keeps flat areas, like sky and skin, from being
  sharpened. Turn it off to set **Threshold** (0 to 200%) yourself:
  higher leaves more alone, and past about 100% only strong edges
  sharpen. Either way, what is flat is judged on the picture before
  Texture, Clarity, and Structure, so the grain they lift is not
  sharpened as detail.
- **Show mask** paints in red where the sharpening acts.

## Output sharpen

An unsharp mask on the finished picture, after the tone curve, the
curves, and the look. It crisps edges as they will be seen, where
**SHARPEN** above restores what the camera lost. It is off until
**Amount** is above zero, and it sharpens brightness only, so colors
do not fringe.

| Slider | Range | What it does |
|---|---|---|
| **Amount** | 0 to 200% | How strongly edges are sharpened. |
| **Radius** | 0.5 to 3 | The width of the edges it acts on, in pixels of the full-size picture. An export made smaller gets a radius made smaller with it. |
| **Threshold** | 0 to 10% | Differences below it are mostly left alone, so smooth skin, sky, and noise stay smooth. Raise it if grain sharpens. |

At the fit zoom you will see little of it, as with the grain; check
it at 1:1.

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

### Fujifilm X-Trans

Fujifilm's X-series cameras (other than the GFX) use an X-Trans
sensor, whose colors are laid out differently. On an X-Trans raw the
menu lists only the methods made for it:

- **markesteijn-3**, the default, makes three passes and keeps the
  most detail.
- **markesteijn-1** makes one pass and takes about half the time.
- **bilinear** is the softest.

A method made for the other kind of sensor, carried by a preset, a
sync or a paste, is left off an X-Trans frame and the status line
says so; the same goes the other way round.

Two parts of the develop don't work on X-Trans yet: the learned
denoiser, and the measured radius of **Auto radius** in **SHARPEN**,
which uses its default radius instead. The profiled denoise and the
hot pixel repair work as on any raw. The chromatic aberration
correction runs too, but takes out less of a fringe than on other
raws for now, and little of a blue one.

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
**Download all** in Settings, Models downloads the denoiser's tiers with
every other model this computer runs.

The first time a picture opens, greycard also asks once whether to
fetch every model now or as needed; that question comes first, and the
lens offer at the next picture.

greycard offers the lens database without being asked. The first
time a picture opens and there are no lens profiles on the machine,
it asks whether to download them, since without them no lens is
corrected. If you answer **Not now**, it will not ask again. The **Get
lens profiles** button in **LENS** is there whenever you want them.
