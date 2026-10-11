# §291. The Portrait switch follows the frame, and a toggle keeps the crop (2026-10-10)

The user's three reports against the Crop tab, on a frame shot on end (EXIF
Orientation 8: a 6000x4000 sensor developed upright as 4000x6000,
no quarter turns in the edit, the aspect free, no crop): the Portrait
switch should start the way the photo is; picking an aspect gave a
landscape crop even with the switch on, as if the switch were
backwards on that photo; and every toggle of the switch left a
smaller crop.

## What was not wrong

We checked the three ways the picture and the crop could disagree
first, and none of them does.

- The size the crop is fitted in is the oriented one. The worker hands
  the engine the camera's tag composed with the frame's own turn
  (`Orientation::turned`), the engine's one orientation step stands the
  picture up, and `source_size` is set from the develop that lands,
  4000x6000 for this frame. Nothing hands the geometry the sensor's
  6000x4000, so this is not a bug every portrait frame carries.
- The engine's orientation and the geometry's `turns` do not overlap:
  the orientation is applied before the geometry, which only ever sees
  the upright picture, and `turns` is the edit's own.
- The overlay and the panel read the same geometry on the same size:
  the render step draws the outline from `drawn_picture`, which is
  `source_size` once the frame's develop has landed.

The test harness, with a 4000x6000 source, shows the switch turning a
named ratio the right way every time.

## What was

The flag meant two things. A named ratio is written landscape and the
flag turns it, so off is wide and on is tall. `Original` (since the
version 4 schema) is the plane's own shape and the flag turns that, so
on a tall frame off is tall and on is wide. The switch started false,
so on a portrait photo the first ratio picked came out wide, and
anything that took the aspect to "Original" with the switch on came
out wide too, the switch reading "Portrait" over a landscape crop.
That is the switch working backwards on this photo, and only on photos
shot on end, where the two meanings disagree. On a landscape photo they
agree, which is why it went unseen.

The shrinking was `refit_crop`'s rule for a crop that changes:
"no bigger than it was" capped each side at the old crop's. Turning a
wide crop tall kept its height as the new height, and turning it back
kept that as the new width, so each toggle cut its area down by the
ratio squared.

## What we changed

- `Geometry::with_aspect` holds the pick rule. A named ratio picked
  where none was held (the aspect free or "Original", or no crop)
  takes the plane's own way up: the switch goes on for a tall plane.
  A ratio picked over another leaves the switch as it is, so whatever
  the user flipped it to stays. "Original" picked leaves the crop the
  frame's way up (the stored flag off, which is the frame's own shape
  for it); left on from a tall ratio it would have cut a tall frame
  wide. Free leaves it. Reset still puts the switch off.
- The switch means one thing everywhere: the crop is tall. The
  stored flag keeps its meaning, so nothing is migrated; the window
  maps it for the switch, which shows the flag the other way under
  "Original" on a tall plane (`portrait-shown`, from `plane-tall`,
  which is the picture's shape as the viewport draws it turned by the
  edit's quarters). A click flips the stored flag, and so what is
  shown, under either reading. A tall frame under "Original" reads
  on with a tall crop; clicked, it reads off with a wide one and the
  flag on. An edit saved under "Original" with the flag on opens as
  it rendered, wide, with the switch off. The Toggle control gained
  `follows`, for a switch whose state is its owner's binding.
- The custom box asks on every key, and "16:" reads as no ratio. A
  half-typed ratio is not taken for a pick of Free, or the "16:9" a
  key later would be a ratio picked over Free and would set aside a
  switch set by hand; the aspect before it stands until the text
  reads again.
- The panel remembers the aspect the crop was last shown or fitted
  under (`aspect-fitted`, written by `show_geometry`), so the
  geometry-changed callback can tell a pick from a toggle of the switch
  or a move of the angle.
- `refit_crop` measures "no bigger" two ways. The same shape that no
  longer fits (an angle, a keystone) is still never wider or taller
  than it was. A new shape keeps the crop's area. We chose area over
  the longer edge so a square picked from 65:24 keeps as much of the
  picture as the band did, rather than growing to the band's length;
  between the two ways up of one ratio the two measures are the same.
- The area a change of shape was asked to keep is held on the panel
  (`crop-kept-area`) until anything else moves the crop (`set_crop`
  clears it). A shape the plane cannot hold at that area is fitted
  smaller, and the next toggle aims at the kept area again rather than
  at the smaller crop, so toggling twice gives back the crop exactly,
  even for a full-frame wide crop whose tall turn has to be smaller.
- The overlay's rectangle moved out of the render step into
  `viewport::show_crop_on_view`, unchanged, so a test can ask where the
  outline stands on the screen.

## What did not need changing

`Aspect::ratio` and the stored flag's meaning under `Original` stay
as version 4 left them, so no schema change and no migration: the
switch's single meaning is the panel's reading of the flag, not a new
flag. `Geometry::turned` already flips the flag for a named ratio
across a quarter turn and leaves `Original` alone, since its plane
turns with it, and `migrate_with_frame` is untouched; both are still
right, and under a turn the switch's reading under "Original" follows
the plane, as the crop does.

## Tests

- `a_ratio_picked_takes_the_planes_way_up_once` (greycard-edit): the
  rule on tall, wide and turned planes, a ratio over a ratio, no crop,
  "Original", Free, and a source of no size.
- `a_ratio_picked_on_a_tall_frame_is_tall_with_the_switch_on`: 3:2 on
  a 4000x6000 source is the whole tall frame with the switch on; 4:3
  over it stays tall; a hand-flipped switch survives 16:9; "Original"
  is the frame's shape and reads tall; reset puts it off.
- `toggling_the_switch_twice_gives_back_the_crop`: three rounds of two
  toggles on a tall frame, a wide one (where the tall turn is capped)
  and a small off-center crop, each back to the start within 1e-4 and
  never larger in between; a fitting turn keeps the area exactly; an
  angle change and its undoing never grow the crop.
- `a_tall_develop_shows_a_tall_crop_outline_on_screen`: a develop
  lands at the size the engine's own orientation step gives a
  6000x4000 sensor tagged 8, and the overlay's rectangle for 3:2 is
  tall on the screen, and wide once the switch is clicked.
- `the_switch_reads_tall_under_original_as_under_a_ratio`: "Original"
  on a tall frame reads on with a tall crop and the flag off; a click
  reads off, flag on, crop wide; a ratio reads on with the flag on;
  an edit saved under "Original" with the flag on opens unchanged,
  wide, reading off.
- `a_custom_ratio_typed_key_by_key_keeps_a_switch_set_by_hand`: "1",
  "16", "16:", "16:9" typed after the switch was set wide by hand.
