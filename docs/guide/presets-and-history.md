# Presets, history and several frames

This chapter covers the left panel's **NAVIGATOR**, **PRESETS**,
**SNAPSHOTS** and **HISTORY** sections, and how to give one edit to
many frames at once.

## The navigator

**NAVIGATOR** shows the whole frame. When you are zoomed in, a
rectangle marks the part in view. Click or drag in it to move the view.
The section's heading shows the zoom.

## Presets

A preset is a named part of an edit: some sections of it, such as
**Light** and **Grain**, with your settings. Applying one replaces
those sections of the frame's edit and leaves the rest alone.

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
4. Tick the sections it should carry. They start ticked where your edit
   differs from the defaults, so the preset holds what you did and
   nothing else.
5. Press **Save**.

Saving under a name that is already in the list replaces that preset.

Some sections are never ticked for you, because they belong to one
picture more than to a look: **White balance**, **Lens**,
**Demosaic**, **Camera profile** and **Adjustments** (the masks). Tick
them yourself if you want them. A preset never carries the crop, the
straighten or the retouch, and never the **Display curve** switch.

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

Press **Import...** and choose a file:

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
calibration, a camera profile that is not Adobe's, and masks. The
status line names any of them the file set, so you can do them by
hand.

### Removing a preset

Click the bin at the end of its row, then **Remove**. To get back a
film preset you removed, open **Settings...** and press **Restore**
under **PRESETS**. It puts back only the missing built-in presets and
leaves yours alone.

Presets are kept as `.gcp` files in a `greycard/presets` folder under
your configuration folder (`~/.config` on Linux, `~/Library/Application
Support` on a Mac, `%APPDATA%` on Windows). Copy that folder to take
them to another machine.

## Snapshots

A snapshot keeps the whole edit of a frame under a name, so you can try
something else and come back.

- **Take** keeps the current edit as "Snapshot 1", "Snapshot 2" and so
  on, and opens the name for typing. Type a name and press Enter.
- **Click** a snapshot to go back to it. This is a new step in the
  history, named "Snapshot: " and the name, so Ctrl+Z undoes it.
- **Hover** over a snapshot to see it in the picture without changing
  anything.
- **Double-click** to rename it, and click the bin to remove it.

Snapshots are kept in the frame's `.gcd` with its edit, and they are
never dropped as the history grows.

## History

**HISTORY** lists every step of the frame's edit, newest at the top.
The bottom row, "Original", is the edit the frame started with.

Each step is named for what it did:

| A step from | Reads like |
|---|---|
| One slider | "Exposure +0.50" |
| Several sections | "Light, Curves off, Grain" |
| A preset | "Preset: Warm Negative" |
| A preset on a selection | "Preset ×3: Warm Negative" |
| A sync | "Sync from IMG_0001.CR3" |
| A paste | "Paste from IMG_0001.CR3" |
| A snapshot | "Snapshot: Evening" |

- **Click** a row to go back to that state. The steps after it stay,
  muted, until you change something, as with undo.
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

## Several frames at once

### Selecting frames

| To | Do |
|---|---|
| Add a frame, or take it out | **Ctrl+click** it |
| Select every frame from the open one to another | **Shift+click** it |
| Add that run to the selection | **Ctrl+Shift+click** |
| Move one frame along and add it | **Shift** with an arrow key, in the filmstrip or the grid |
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
3. Press **Sync...** under PRESETS, or **Ctrl+Shift+S**.
4. Tick the sections to copy, and press **Apply**.

Every section starts ticked except **Adjustments (replaces masks)**,
because masks are drawn around one picture's subject. Ticking it
replaces each frame's own masks with the open frame's.

- The crop, the straighten and the retouch are never copied.
- A camera profile goes only to frames from the same camera. The status
  line names any frame it skipped.
- **Noise** brings the learned denoiser and its blend along, unlike a
  preset.
- Each frame gets one history step, "Sync from" the open frame, so
  Ctrl+Z on that frame undoes it.

While the sheet is open, the arrow keys and undo do nothing, so the
frames it names are the frames it syncs.

### Copy and paste settings

- **Ctrl+C** copies the open frame's whole edit.
- **Ctrl+V** opens the same sheet, titled **Paste settings from** the
  copied frame, over the selected frames. Tick the sections and press
  **Paste**.

The sections start as your last sync or paste left them. A paste
follows the sync's rules: crop, straighten and retouch stay each
frame's own, and each frame gets one step, "Paste from" the frame you
copied. The frame you copied from is left as it is.

The copy stays inside greycard until you quit. It does not use the
system clipboard, so Ctrl+C in a text field still copies text.

### The right-click menu

Right-click a frame in the filmstrip, the grid or the picture for a
menu. On a Mac, Control+click does the same.

- **Copy settings of** the frame you right-clicked
- **Paste settings** (greyed until you have copied something)
- **Rating**, **Pick**, **Reject**, **Unflag**, **Label**
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
