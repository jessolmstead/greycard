# 203. The Masks tab laid out to be read at a glance (2026-09-29)

Roadmap line: "The Masks tab laid out to be read at a glance: today it
has an Invert for the mask and an Invert shape for each shape, and the
shape buttons twice (new mask, then Next shape); one place for each
control, and the mask, its shapes and their settings in an order that
says which is which."

**What it was.** The ADJUSTMENTS section put the mask list first, then
three rows of shape-kind buttons that started a *new* adjustment
(Linear, Radial, Brush; Subject, Sky, Object; Luminance, Color), then,
once a mask was chosen, its name and its Invert, a card for whichever
shape was selected (its own "Invert shape" toggle among its settings),
and a *second* copy of the same eight buttons under "Next shape" to
add another shape to the chosen mask. Two ways to reach Sky, two to
reach Luminance, and so on; a shape's own invert lived only in the
card of whichever row happened to be selected, so reading the mask at
a glance meant clicking through every row in turn.

**What it is now, and why.** One block for the mask: the list, then
the chosen mask's name, Show mask and Invert. Under it, its shapes as
a list, each row its kind, its own invert (a half-filled circle, which
reads as invert rather than the flip-horizontal glyph's mirror; a
tooltip names it, and it is reachable from the keyboard, not only a
click — see below) and a delete, chosen by clicking the row. Directly
under that list, the chosen shape's own settings, when it has any: a
Radial's feather, a Luminance or Color window, or the brush's own tool
options while a brush is chosen. A shape whose only per-shape setting
was already the invert now on the row (Linear, Subject, Background,
Sky, Object) shows no card at all, rather than one with nothing in it
but the kind's name repeated from the row above. Under that, one "Add
a shape" heading over the whole group that puts another shape into the
chosen mask — the Add/Subtract/Intersect row and the eight kind
buttons together, not two labels over what is really one control. The
order reads: the mask, its shapes, the chosen shape's own settings,
then how to add another — each thing once, next to what it is about.

Starting a *new* mask is a single "New mask" action with no kind of
its own: it makes an empty adjustment, named "Mask N" until a shape
gives it a better one, and selects it; the first shape then comes from
the same "Add a shape" row everything else does. For a made-at-once
shape (Subject, Sky, a range) that is one click more than before — New
mask, then the kind, where one click used to make a whole mask — spent
on never showing the same button twice and on a mask's first shape no
longer depending on the Add/Subtract/Intersect row being reset before
it is asked for: it is always Add now, whatever the row was left on.
That row itself is hidden until the mask has a shape, since a first
shape has nothing to join, take from or intersect with and the
question does not apply yet. A placed shape (Linear, Radial, Brush,
Object) is the same drag it always was, just asked for from one place.

Seven loose ends the redesign opened, all closed here — the first two
found on a second pass, after the first round of fixes already landed,
and the last two on a third:

- A shape's own row was named for its kind (and the mask along with
  it, on its first shape) at the *press* that armed a drag, before the
  drag was known to have moved at all — so an abandoned drag (a press
  with no movement, which places nothing) still renamed the mask it
  never gave a shape to, and, if the mask was kept for some other
  reason, left it wrongly named for a shape it did not have. Naming
  now happens only at the *release* that finds the shape drawn (or, for
  a brush stroke, an object pick or a sky pick, which are never
  abandoned once pressed): an abandoned first attempt leaves the mask
  exactly as nameless as "New mask" left it, so a later shape can still
  name it correctly.
- The cleanup that takes an abandoned mask back with its shape checked
  too little: a mask "New mask" left with nothing in it, renamed by
  hand or given its own Invert before a shape was ever tried, was still
  taken back along with the abandoned shape, losing the rename or the
  Invert with it. "Exactly as `New mask` left it" now means no shape
  landed there yet (nor ever did, `fresh_mask` cleared the moment one
  does, in this mask or another), a `Look::default()`, the placeholder
  name still on it, and its own Invert still off — any one of those is
  deliberate work on the mask, and it is kept, empty, exactly as
  switching off its last shape already leaves one.
- A shape's mode used to come from whatever the Add/Subtract/Intersect
  row was last left on, including a mask's *first* shape, which has
  nothing before it to join, take from or intersect with — read as
  anything but Add it evaluates to nothing everywhere the mask is not
  inverted, so the adjustment silently did nothing. A mask's first
  shape is now always Add, whichever shape starts it and whatever the
  row says; the row itself is hidden until there is a second shape to
  ask the question about, and unchanged from there.
- A mask "New mask" just made has no shape yet, so `show_names` (which
  reads `Adjustment.name` with no fallback of its own) showed a blank
  row in the list until the first shape named it. A fresh mask is now
  named "Mask N" (N its id) from the moment "New mask" makes it, and
  is renamed for its first shape exactly as a made-at-once shape from
  the old `add_adjustment` always was ("Subject 3", "Linear 1") — but
  only while it still carries that placeholder, so a mask already
  renamed by hand keeps the name given it.
- The invert icon was a click-only control: a `TouchArea` with an
  `accessible-role` for a screen reader but no keyboard path, where the
  toggle it replaced (a `Toggle`, in the settings card) had one. It now
  has its own `FocusScope`, Space and Enter beside the click, and the
  same tooltip on focus as on hover. Fixing it surfaced an unrelated
  bug: toggling it used to call the panel's general `show_edit`, which
  rebuilds the shapes list's names as a fresh model on every call —
  tearing down and rebuilding the list's rows, focus included, a
  moment after a click had just set it. The toggle now updates only the
  one list it changes (`shape-invert`), leaving the rows themselves
  alone.
- Naming a mask for a drawn shape (the fix two bullets up) renamed
  `st.edit` at the release but refreshed nothing on the panel: the name
  field, the list and the heading still showed "Mask N", and the next
  `read_edit` — any later panel action, an invert toggle among them —
  copied that stale panel text straight back over the name just given.
  The release now refreshes the panel too (`show_edit`, or, for the
  brush/object/sky path, the same) whenever it actually renames
  something, so the name sticks through whatever comes next.
- A brush, an object or a sky left in hand kept taking strokes and
  picks at `placing.index` through a `target-changed` or a
  `delete-adjustment` that had moved on without it — a delete shifting
  a later mask down into the slot the deleted one left, so a stroke
  taken afterward would land in, and rename, whatever mask happened to
  slide into that spot. Both now put the tool down first, as leaving
  the Masks tab already did.

While removing the duplicate buttons, the top-level `add_adjustment`
callback that used to make a mask from a kind directly — dead in the
UI once its buttons were gone, but still driving four tests — was
removed along with the `Placing::fresh` machinery only it ever set;
the tests now drive `new_mask` and `add_shape`, the way the UI does.

**Checked.** A `--snapshot` of the Masks tab, before and after, on
`3G0A4650.CR3` with a preset mask of two shapes (a Radial and an
intersecting, inverted Luminance), at the window's usual 1500×950
(logical, 2250×1425 physical) size: before shows the buttons twice
(the top three rows for a new adjustment, the same eight again under
"Next shape" once scrolled to it) and "Invert shape" as a separate
toggle in the settings card; after shows the row-level invert icon
lit for the inverted shape and dim for the other, the RADIAL card
(Feather only, no repeated heading-only card for a kind with nothing
else) sitting directly under the shapes list, and "Add a shape" — one
heading, the mode row and the kind buttons together — directly under
that. Above the fold at that size, with no scroll: the mask card, the
shapes list, the chosen shape's settings, the "Add a shape" heading,
the mode row, and the Linear/Radial/Brush row. Below it, needing the
scroll `masks-after-scrolled.png` shows: Subject/Background, Sky/
Object, Luminance/Color, and the look sections under them. Master's
three short rows (Linear/Radial/Brush; Subject/Sky/Object; Luminance/
Color) sat right under the mask list, before a mask even existed to
have settings, and so were all reachable without a scroll; the same
rows here sit under whatever the chosen shape's own card takes up
first, so a made-at-once shape (Subject, Sky, a range) is not always
above the fold the way it once was — a real cost of the reordering,
not hidden here. Kept under `target/scratch/`:
`masks-before.png`, `masks-before-scrolled.png`, `masks-after.png`,
`masks-after-scrolled.png`.

**Tests.** Every existing headless test for the Masks tab still
passes, the four rerouted through `new_mask`/`add_shape` included.
New: a mask started by "New mask" is empty, named "Mask N", and takes
its first shape (and a real name) from "Add a shape"; a mask's first
shape is Add whatever the row says, made-at-once or dragged; an
abandoned first shape does not rename the mask it never landed in,
and a mask kept nameless by that is still named correctly by its next
real shape; a drawn shape's name (and a brush's first stroke's) still
reads correctly after a later `read_edit` (an invert toggle stands in
for "any later panel action" in both tests); an abandoned first shape
takes an untouched fresh mask with it but never one whose look was
touched, whose name or Invert was set by hand, or that once had a
shape (five tests over the two rounds: the drop, and four
must-not-drop cases); the mode row is hidden until the mask has a
shape; a shape's invert is a switch on its row, does not need the row
chosen first, and is reachable from the keyboard (a click, then Space,
then Enter, each toggling it); deleting a mask with a brush still in
hand puts the tool down rather than leaving it armed for whatever
slides into the deleted mask's slot; and, reading the layout itself
rather than the callbacks, that "New mask"
and each of the nine "Add a shape" buttons appear exactly once — the
literal check for the double-button bug this line is about. That last
test found a real quirk in the Slint testing backend worth recording
for the next one who hits it: `find_by_accessible_label` on a control
an `if` has just turned true, queried on a window that was ever asked
about a label *before* that change, comes back empty even for a
control that is there — asking only *after* the change avoids it,
which is what the test (and the new `testing::count_labeled` helper)
does.

**Not done.** The invert tooltip pops upward from the icon so it
paints over what is already on screen above it rather than pushing
anything out of the way; for the first row in a long shapes list this
can overlap the "Shapes" heading above the list. Cosmetic, not
functional, and not fixed here. The made-at-once buttons (Subject,
Background, Sky, Object, Luminance, Color) are below the fold at
1500×950 for a mask with a settings card in front of them (a Radial or
a range chosen); making the rows themselves more compact, so more of
them fit without a scroll, is a follow-up, not done here.
