# §710. A reset on each section's header (2026-11-17)

Discussion 59 asked for a way to put one section back without Reset
all. Every section of the Develop tab, the Crop tab's three, and a mask's
sections on the Masks tab carry a reset arrow (rotate-ccw, as the
in-section Reset buttons) in the header, left of the switch. A click
puts that section back to what a new edit has for it, as one step; the
fold and the section's place stay. Tooltip and accessible label: the
existing `@tr("Reset")` (no context, the same msgid as the Look
section's button), so no new string; its accessible description is
the section's title.

"What a new edit has" is the frame's Original (`reset::original_of`),
the state Reset to Original returns to, so a header reset and Reset to
Original agree section by section (a raw's learned-denoise blend from
its ISO, a picture's `Edit::for_picture`, the lens as the new-edit
setting had it).

| Section | Reset puts back |
|---|---|
| Look | `look_lut` (no look, full strength) |
| White balance | As shot (the Original's white) |
| Camera profile | `camera` whole: the file's own profile, tone curve off, look table as new |
| Light | `light` whole (switch, exposure, the six tone sliders, and the pre-0.8 baseline, which goes to the body's as Reset to Original takes it); on a raw, the display curve too (its choice is in the section); on a picture, where the section does not show it, the display curve is left as it is |
| Color | `color` (switch, saturation, vibrance) |
| Curves | `curves` whole: every channel's points, the parametric curve, the switch |
| Color mixer | `mixer` (switch, all bands) |
| Black & white | `bw` (switch, weights, strength) |
| Color grading | `grading` (switch, three wheels, balance) |
| Tint | `tint` (switch, hue, amount) |
| Detail | `detail` |
| Noise | `noise` whole, the learned tier and its blends too |
| Lens | `lens` whole |
| Sharpen | `sharpen` |
| Output sharpen | `output_sharpen` |
| Vignette | `vignette` |
| Grain | `grain` |
| Demosaic | `demosaic` |
| Crop (Crop tab) | the crop, its aspect, Portrait |
| Rotate | quarter turns and mirror taken back by the very moves that made them (`Geometry::turned(-n)`, `flipped`), so the crop and the keystone turn back with the picture and keep their meaning; then the straighten to zero, and a kept crop refitted to the plane as the slider would |
| Perspective | vertical and horizontal keystone to zero (zero reads the same whichever way the picture is turned); a kept crop refitted |
| Mask: White balance | `LocalWhite::Off`: switch off, no light kept, Follow global off |
| Mask: Light, Color, Curves, Mixer, Grading, Tint, Detail | that section of a new mask's look (`Look::default()`) |

The step has no label of its own: the history names it by what moved,
as the in-section Reset buttons' steps always were ("Light", "White
balance as shot", "White balance reset" for a mask, "Color tint
reset").

Exactly one step, the rest of the edit bit for bit: the reset goes
over the line's current state, not over the panel read back. The panel
holds a white balance as f32 where the edit keeps f64, so recording the
panel first made a phantom "White balance" step and rewrote an
untouched section to the f32 values. The panel is recorded first only
when a move is still waiting for the save timer (it is running), as a
delete does it (`delete.rs`); the timer is stopped then, so it cannot
record the panel again after the reset. Made current through
`take_current` (saved, developed when the develop's part moved). Undo
restores.

## Without the icon

- Retouch: its reset would delete every stroke and its fills at one
  click of a small icon. Reset to Original does take them; we left the
  icon off.
- The Masks tab's mask section: its body is the mask (shapes, invert,
  opacity, ranges); a reset there would be the mask's removal.
- Proof, Monitor, Metadata, Culling, and the left pane's lists: no part
  of the edit.

## Sections that keep an in-section Reset

Left in place; the header's does the whole section and its switch:

- Look: Reset (no look, full strength) does exactly what the header's
  does.
- Curves: Reset puts back the channel on screen only (or the
  parametric curve).
- Color mixer: Reset zeroes the bands, keeps the switch.
- Black & white: Reset zeroes the weights and strength, keeps the
  switch.
- Color grading: Reset clears the wheels and balance, keeps the switch.
- Tint: Reset clears hue and amount, keeps the switch.
- Perspective: Reset all, the whole Crop tab (crop, rotate,
  perspective).

We kept them: Curves' and Reset all do something the header's does
not, and the rest cost nothing.

## Choices

- Hidden at the defaults, its room kept: dimmed was indistinguishable
  from lit in the Light and Grey themes. Its width stays reserved, so
  nothing in the header moves as it comes and goes; a click on the
  empty room is the header's (it folds, as the rest of the header).
  Known per section by `section_reset::resets_for`, the panel's edit
  against the Original, filled into a `SectionResets` struct on every
  panel move (`show_panel_resettable`), every `show_edit`, and every
  `show_history`. A reset arriving at a section at its defaults is a
  no-op in Rust as well.
- Hidden with a dead switch: a superseded section (a monochrome
  frame's white balance, the global mixer under black and white). The
  guard is in Rust too (`superseded`), so a click arriving anyway is
  refused. The queue's read-only view, no frame, culling, and a panel
  standing in for an unread sidecar refuse in Rust, as Reset to
  Original does.
- Narrow panels: the title keeps a few letters (56 px) and is cut
  before anything else; an owner (a mask's name) is cut before the
  title. At the panel's narrowest, 280 px, the header holds chevron,
  icon, those letters, the reset, and the switch, so the reset is never
  dropped.
- The empty room after a reset is the header's: its touch area is
  switched off and the row under it takes the press, as on a header
  whose reset was never shown. A click at a place sampled before a
  reset or a fold can miss the header when the panel is scrolled: the
  change of the panel's height holds the scroll to the new end, which
  carries the header from under the pointer. That is any change of
  height's, not
  the reset's.
- The tip: its TipArea is made once and only switched off, and a
  TipArea now takes its tip down when its words go, so a tip up as
  the section reaches its defaults (an undo by a key) does not stay.
  A tip stuck after a tab switch unmade its TipArea is the old
  TipArea behavior, any panel TipArea's; left.
- The icon has no focus scope, so the keys' focus walk is as it was;
  its TouchArea takes the press, so a click or a drag from it never
  folds or moves the section.
