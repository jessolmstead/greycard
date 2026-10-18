# Presets, history and several frames

This chapter covers the left panel's **NAVIGATOR**, **PRESETS**,
**SNAPSHOTS** and **HISTORY** sections, versions of a frame, and how
to give one edit to many frames at once.

## The navigator

**NAVIGATOR** shows the whole frame. When you are zoomed in, a
rectangle marks the part in view. Click or drag in it to move the view.
The section's heading shows the zoom.

## Presets

A preset is a named part of an edit: some sections of it, such as
**Light** and **Grain**, or single sliders of a section, such as
**Dehaze**, with your settings. Applying one replaces those sections
and sliders of the frame's edit and leaves the rest alone.

### Applying a preset

Click a preset's name in **PRESETS**. It lands as one step in the
history, so Ctrl+Z takes it back whole. With several frames selected,
it lands on each of them (see
[Several frames at once](#several-frames-at-once)).

greycard comes with three film presets:

| Preset | What it does |
|---|---|
| Muted Slide | A slide film look: less saturation, hard highlights, crushed blacks, cyan shadows |
| Red-Filter Mono | Black and white through a red filter: dark sky, light skin, coarse grain |
| Warm Negative | A warm color negative: soft contrast, matte blacks, fine grain |

They are built from the ordinary sliders, so you can see and change
everything they set.

### Saving your own

1. Edit a frame until you like it.
2. Press **Save...** under the list.
3. Give the preset a name.
4. Check what it should carry. The sections are listed in the
   Develop tab's order, each a heading with its parts under it
   (Detail: Texture, Clarity, Dehaze), each part with its own check.
   The heading's check takes or leaves the whole section, and shows a
   dash when only some of it is checked. They start checked where
   your edit differs from the defaults, so the preset holds what you
   did and nothing else. **Check all** and **Uncheck all** set every
   check at once.
5. Press **Save**.

Saving under a name that is already in the list replaces that preset.

The sheet works from the keyboard too: **Tab** moves from check to
check, **Space** or **Return** turns the one with the focus ring, and
**Esc** closes the sheet without saving.

The parts are each section's sliders, or the few settings that make
one control: a curve's channel, one band of the color mixer, one
grading wheel. **White balance**, **Tint**, **Look**, **Noise**,
**Sharpen**, **Demosaic** and **Camera profile** have no parts; each
goes whole.

Some sections are never checked for you, because they belong to one
picture more than to a look: **White balance**, **Lens**,
**Demosaic**, **Camera profile** and **Masks**. Check them yourself if
you want them. Under **Masks** each of the frame's masks is listed by
name, so a preset can carry one mask alone, and **Replace masks** or
**Add to masks** says whether the preset's masks take the place of a
frame's own or join them. A mask a frame already has is not added
twice.

A slider carried alone leaves the rest of its section, and the
section's switch, as the frame has them. A preset never carries the
crop, the straighten or the retouch, and never the **Display curve**
switch.

A preset's **Noise** carries the noise reduction but not the learned
denoiser, which each frame sets from its own ISO.

### A preset with a camera profile

A camera profile is made for one camera. A preset that names one lays
it only on frames from that camera, the frame on screen included. The
rest of the preset still applies, and the status line names the frames
the profile was left off. If you want the profile anyway, pick it in
**CAMERA PROFILE**, which warns "Made for X, not Y." but lets you.

### A preset with a look

A preset can carry the **Look** section. It names the look, so the
look's file has to be on the machine where you use the preset.

### Importing presets, including Lightroom's

Press **Import...** and choose one file or several:

- a greycard preset (`.gcp`), from another machine or another user;
- a Lightroom or Camera Raw preset (`.xmp`).

A Lightroom preset comes across as a starting point, not an exact
match, because Lightroom's sliders work on a different tone pipeline.
These come across: exposure, contrast, highlights, shadows, whites,
blacks, white balance, the tone curves (point and parametric), the HSL
mixer, vibrance and saturation, black and white with its mix, split
toning and color grading, texture, clarity and dehaze, sharpening (on
or off only), luminance noise reduction, the post-crop vignette, grain
and the lens profile switch.

These do not: color noise reduction, defringe, the global color grade,
calibration, a camera profile that is not Adobe's, and masks. Once the
import is done, greycard names any of them the file set, so you can do
them by hand: on the status line for one short preset, or on a small
sheet when you chose several or the list is long.

A file is left out, and the sheet says why, when it is not a preset,
when it cannot be read, or when the list already has a preset of that
name. To bring in a new copy of a preset you have, remove the old one
first.

### Removing a preset

Click the bin at the end of its row, then **Remove**. To get back a
film preset you removed, open **Settings...** and press **Restore**
under **PRESETS**. It puts back only the missing built-in presets and
leaves yours alone.

Presets are kept as `.gcp` files in a `greycard/presets` folder under
your configuration folder (`~/.config` on Linux, `~/Library/Application
Support` on a Mac, `%APPDATA%` on Windows). Copy that folder to take
them to another machine.

A preset that carries single sliders, or whose masks are added to a
frame's, needs greycard 0.6 or later. An older version reads it as its
whole sections only: it leaves out the single sliders, and its masks
replace a frame's own instead of joining them.

## Snapshots

A snapshot keeps the whole edit of a frame under a name, so you can try
something else and come back.

- **Take** keeps the current edit as "Snapshot 1", "Snapshot 2" and so
  on, and opens the name for typing. Type a name and press Enter, or
  click anywhere else to keep what you typed. Esc keeps the old name.
  While you type, moving the pointer over the picture doesn't take the
  keys away from the name.
- **Hover** over a snapshot to see it in the picture without changing
  anything.
- Each row has three icons: the arrow restores the snapshot, the
  pencil renames it, and the bin removes it. Rest the pointer on one
  and the status line says what a click does.
- Restoring is a new step in the history, named "Snapshot: " and the
  name, so Ctrl+Z undoes it.
- A **click** on the row restores it too, after a moment's pause, so
  that a **double-click** can rename it without restoring it first.

Snapshots are kept in the frame's `.gcd` with its edit, and they are
never dropped as the history grows.

## History

**HISTORY** lists every step of the frame's edit, newest at the top.
The bottom row, "Original", is the edit the frame started with. On a
version, the section's head names the version, and the history is
that version's own.

Each step is named for what it did:

| A step from | Reads like |
|---|---|
| One slider | "Exposure +0.50" |
| A section's switch | "Color tint off" |
| A section's Reset | "Color tint reset" |
| One grading wheel | "Color grading shadows 40°" |
| Several sections | "Light, Curves off, Grain" |
| A preset | "Preset: Warm Negative" |
| A preset on a selection | "Preset ×3: Warm Negative" |
| A sync | "Sync from IMG_0001.CR3" |
| A paste | "Paste from IMG_0001.CR3" |
| A snapshot | "Snapshot: Evening" |

A step in a mask starts with the mask's name, such as "Lamp: White
balance on".

- **Click** a row to go back to that state. The steps after it stay,
  muted, until you change something, as with undo. Click a muted row
  to go forward to it again.
- **Hover** over a row to see that state in the picture without
  changing anything.
- **Ctrl+Z** undoes a step, **Ctrl+Shift+Z** or **Ctrl+Y** redoes it.

The history keeps the last 50 steps, plus "Original". Use a snapshot
for a state you want to keep for good.

### Exports in the history

Each export adds a row on the state it was made from, in italics with
an export mark: the file name, the export preset if you used one, and
the time ("IMG_0001.jpg · Web 14:32"). Hover over it for the full path
and date. Click it to go back to the edit that was sent out. An
export row goes away with its state if that state leaves the history.
The exported mark on the frame's thumbnail stays (see
[the library](library.md#edited-and-exported-marks)).

## Versions of a frame

A **version** is a second edit of the same raw, kept beside the
first: a warm one and a cool one, a color one and a black and white,
a wide crop and a tight one. Where a snapshot is a state you can go
back to inside one edit, a version is a picture of its own. It has its
own cell in the filmstrip and the grid, its own thumbnail, its own
history and snapshots, and its own export. The raw is not copied; the
versions live in the frame's `.gcd`.

### Making one

Right-click a frame and choose **New version**, or press **Ctrl+'**
(⌘' on a Mac). The new version starts as a copy of the edit you were
on, opens at once, and asks for a name with "Version 2" filled in:
type a name and press Enter, or press Esc to keep "Version 2". Its
history starts with one row, "From the original" (or "From" the
version it was copied from).

**New version from the original** starts from greycard's defaults
instead, for an edit that has nothing to do with the first.

The version's cell sits right after the frame's, with its name on a
tag at the top left. In the grid, the name under the cell is the
version's.

### What a version shares

Everything in the edit is the version's own: the crop, the white
balance, every slider, every mask. The rating, the flag, the label,
the keywords and the quarter turn belong to the file, so the frame
and all its versions share them. Choose between versions by deleting
the ones you don't want, or by swapping the one you like into the
frame's own place, not by rating them.

### The version's menu

Right-click a version's cell for:

- **Rename version...**
- **Swap with the original** trades the two edits whole, each with
  its history and snapshots. The version keeps its name, and now holds
  what the frame held.
- **Delete version...** asks first ("Delete the version Warm of
  IMG_0001.CR3?"), then removes that version alone. It can't be
  undone. **Delete** over a selection of versions does the same; over
  a frame, it deletes the file and its versions with it.
- **Compare versions** puts the frame and its versions side by side
  in culling (see [Comparing versions](culling.md#comparing-versions)).

### Versions and everything else

- A paste, a sync or a preset acts on the version you chose, and
  leaves the frame's own edit alone.
- An export of a version is named with the version's name, as
  `IMG_0001-Warm.jpg`, and is recorded in that version's history. See
  [Exporting](export.md#a-version-of-a-frame).
- A version can be put in a collection on its own (see
  [Collections](library.md#versions-in-a-collection)).
- `has:versions` in the filter finds the frames that have one.

## Several frames at once

### Selecting frames

| To | Do |
|---|---|
| Add a frame, or take it out | **Ctrl+click** it |
| Select every frame from the open one to another | **Shift+click** it |
| Add that run to the selection | **Ctrl+Shift+click** |
| Move one frame along and add it | **Shift** with an arrow key, in the filmstrip or the grid |
| Select every frame from the open one to the first or last | **Shift+Home** or **Shift+End** |
| Go back to one frame | A plain click, a plain arrow, or **Esc** |

Selected frames are highlighted in the filmstrip and the grid. The
frame on screen has the brighter outline.

### What acts on the selection

- ratings, flags, labels and the [ ] turns;
- a preset click, one history step on each frame;
- **Sync...**, copy and paste settings;
- **Export...**, which writes every selected frame (see
  [Export](export.md));
- the right-click menu.

### Sync settings

Sync copies the open frame's edit onto the other selected frames.

1. Edit one frame.
2. Ctrl+click or Shift+click the others.
3. Press **Sync 4 frames to this one...** at the top of the develop
   panel (it says how many), **Sync...** under PRESETS, or
   **Ctrl+Shift+S**.
4. Check the sections to copy, and press **Apply**.

Every section starts checked except **Masks**, because masks are drawn
around one picture's subject. The sheet lists what a preset's does: a
heading a section, its parts under it, the open frame's masks by
name, **Check all** and **Uncheck all**, and the same keys. Uncheck a
slider and only the rest of its section goes; check a single mask and
only that mask goes. **Replace masks** (the default) takes each
frame's own masks away; **Add to masks** keeps them and puts the
copied ones after. A Subject, Background, Sky or People mask carries
what it asks for and is made again for each frame, from that frame's
own picture.

- The crop, the straighten and the retouch are never copied.
- A camera profile goes only to frames from the same camera. The status
  line names any frame it skipped.
- **Noise** brings the learned denoiser and its blend along, unlike a
  preset.
- Each frame gets one history step, "Sync from" the open frame, so
  Ctrl+Z on that frame undoes it.

While the sheet is open, the arrow keys and undo do nothing, so the
frames it names are the frames it syncs.

#### Set or shift

**Temperature**, **Tint** and **Exposure** each have **Set** and
**Shift** at the top of the sheet:

- **Set** (the default) gives every frame the open frame's value, as
  above.
- **Shift** moves each frame from its own value by the open frame's
  difference from where it started: Temperature and Tint from its
  as-shot white balance, Exposure from 0 EV. This is Lightroom's Quick
  Develop: if you warmed the open frame from as shot, every frame gets
  as much warmer from where it was, rather than all taking the open
  frame's value.

Temperature shifts by the same step in mired (a million over the
kelvin), which is how far a change looks alike at any light. A step is
fewer kelvin on a warm frame than on a cool one: the open frame warmed
300 K from 4800 K moves a frame at 3000 K about 115 K and one at
6500 K about 560 K. The sheet says the step and what it comes to at a
few lights.

A frame at **As shot** that is shifted becomes a custom white balance
at its own as-shot light, moved. If the open frame is at As shot, it
has moved nowhere, so with either of Temperature and Tint on Shift
every frame's white balance stays as it is. If the open frame isn't a
raw, or its raw doesn't record its white balance, Temperature and Tint
are set instead, and the sheet says so. The choices are greyed while
their section is unchecked, and greycard remembers them. A paste has
no shift: it sets.

### Copy and paste settings

- **Ctrl+C** copies the open frame's whole edit.
- **Ctrl+V** opens the same sheet, titled **Paste settings from** the
  copied frame, over the selected frames. Check what to paste and
  press **Paste**.

The checks start as your last sync or paste left them, and masks
checked by name check the copied frame's masks of the same name. A
paste
follows the sync's rules: crop, straighten and retouch stay each
frame's own, and each frame gets one step, "Paste from" the frame you
copied. The frame you copied from is left as it is.

The copy stays inside greycard until you quit. It does not use the
system clipboard, so Ctrl+C in a text field still copies text.

### The right-click menu

Right-click a frame in the filmstrip, the grid or the picture for a
menu. On a Mac, Control+click does the same.

- **Copy settings of** the frame you right-clicked
- **Paste settings from** the copied frame **onto** the selected
  frames (greyed until you have copied something)
- **Sync 4 frames to** the frame on screen, with more than one frame
  selected
- **New version** and **New version from the original**, and on a
  version **Rename version...**, **Swap with the original** and
  **Delete version...**; **Compare versions** on a frame that has
  versions (see [Versions of a frame](#versions-of-a-frame))
- **Lightbox**, or **Leave lightbox** (see
  [the lightbox](culling.md#the-lightbox))
- **Rating**, **Pick**, **Reject**, **Unflag**, **Label**
- **Add to collection**, and **Remove from** the collection that is
  open (see [Collections](library.md#collections))
- **Reveal in file manager** (on Linux it opens the folder without
  selecting the file)
- **Export...**, or **Export N frames...** over a selection
- **Move back to** a folder, for frames in a rejects folder (see
  [Culling](culling.md))
- **Back up to** or **Bring back to**, for library roots with an
  archive (see [Archives](archives.md))

On a selected frame, the menu acts on the whole selection. On a frame
that is not selected, that frame is opened first and the menu acts on
it alone.
