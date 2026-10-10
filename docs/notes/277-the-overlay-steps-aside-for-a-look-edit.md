# 277. The overlay steps aside for a look edit (2026-10-10)

A mask picked shows its overlay so the user can see what it chose. Once
the user moves a slider in the mask's look (exposure, the light sliders,
color, tint and the rest of `Look`), the overlay turns off, so the edit
is seen without a trip to the Show mask toggle.

## The rule

On every panel change (`view-changed`; the look sections a mask shows
all raise it) we compare the look the panel holds with the look it last
showed (`mask::look_edited`). If it differs and the overlay is on, the
overlay goes off. Feather, range and luminance limits, invert, the brush,
adding or removing a shape and the toggle itself leave the look as it
was, so they never hide it; they need the overlay to be seen. Choosing
another mask, making a new one and putting a tool in hand keep their
rules.

Comparing looks, instead of wiring a hide into each slider, keeps one
rule for every look control, including ones added later, and the shared
sections (Light, Color) need no knowledge of whether they edit a mask.
The baseline is the look as the panel last showed it, read back from the
panel where the panel is written (`show_edit`, which every target switch,
undo, history click, preset, paste and sync goes through) and where it is
changed. It is never taken from the stored edit: the panel's read-back
is not bit-identical to it (a mask's local white is f32 in the panel, a
curve of fewer than two points reads as identity), and a restore that
changes only the look would otherwise be counted as an edit.

## Choices

- Every look change hides it, not only the first. A toggle turned on by
  hand is for looking at the shape; the next slider drag is for looking
  at the picture, and it hides again.
- A tool in hand forces the overlay on and keeps the toggle's old value
  in `show_mask_kept` to restore once the tool is down. A look change in
  that state turns the overlay off and drops the kept value, so the tool
  going down leaves it off. The alternative restored the old value, which
  could bring the overlay back over the edit by surprise. The tool stays
  in hand with the overlay off, which is the user's last word.
- The user's own toggle works both ways as before.

## Tests

In `panel/mask.rs`: a look change hides the overlay, a feather and an
invert change do not, turning it on by hand and moving a slider again
hides it again; with a tool in hand a look change hides it and leaves
`show_mask_kept` empty; choosing another mask leaves the overlay alone;
after an undo, and after a history click, that restored only the look,
showing the overlay and then dragging feather leaves it on; a mask from a
foreign sidecar (a local white of 5123.4567 K and an empty red curve)
does not hide at the first toggle.
