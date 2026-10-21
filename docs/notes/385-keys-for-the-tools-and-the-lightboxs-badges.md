# §385. Keys for the tools, and the lightbox's badges (2026-10-21)

The second half of the roadmap's keys line, asked by a Mac tester by
mail and in issue 31: keys for the Develop view's tools, and the stars
back on the lightbox on request.

## The tools

- **R** the crop, in and out, as the panel's Crop and Done. R is the
  crop in Lightroom, Capture One, and darktable alike, and nothing
  bound it.
- **W** the white balance's pipette, again to put it down. Lightroom's
  W, free here.
- **M** a new mask with a Linear in hand, **Shift+M** one with a
  Radial, the next drag placing it. Lightroom's own pair for its
  gradient and radial tools, both free.

Each key brings its tab, as its button is only on that tab: R the Crop
tab, M the Masks tab, and W the Develop tab unless the Masks tab shows
a mask's look, where the pipette works under the mask as its button
there does. Each goes through what its button calls: the Crop button's
body moved to a window function (`crop-in-or-out`) that the button and
R share; the tab bar's choice moved to `tab-chosen`, so a key leaving
the Crop tab shows the frame whole as a click does; W calls
`pick-started("White")` and does nothing on a frame the button is
disabled for; M calls New mask's own path (`mask-new`) and then the
shape's `add-shape`. At the most masks New mask refuses and says so,
and the key then adds no shape to the mask already chosen. M or
Shift+M pressed again before the drag, while the mask the key made is
still empty, changes the shape in hand in that mask, or puts it down
when it is the same shape, as the shape's button pressed again does;
otherwise a second press would leave an empty mask behind each time.
A held key is one press, and with no frame open the four do nothing.

R and M put down whatever else is in hand first (W does already, by
the pipette's own path): without that, W then M left the pipette and
a Linear in hand together, and the next drag sampled white under the
new mask. R in the middle of a crop's drag used to drop the drag's
state, so the release found nothing and the crop changed with no undo
step, no save, and no render; leaving crop mode mid-drag now makes the
edit the release would have, for the button too.

The four are the Develop view's alone: in the grid, the lightbox, and
culling they do nothing. They work with the panel hidden. A focused
slider keeps the arrows after W, since W reaches the window through the
slider's pass-through like any other key; R and M change the tab,
which lets go of a focused slider as a click on the tab does.

## The lightbox's badges

**J** in the lightbox draws the grid's badges (rating, flag, and label)
on each tile, the same component at the grid's size, on the corner of
the tile's picture; again, off. J is Lightroom's key for the grid's
cell extras, and in the grid and lightbox J did nothing (the clipping
warnings are the loupe's, and J in the lightbox's loupe is still
those). Only the badges come back: the name, the marks, and the label's
tint stay off, as the lightbox has them. Off at each start, kept across
leaving and entering the lightbox, and never written to settings.json:
the lightbox is the pictures alone unless asked, each session.

The keys sheet has a row for each, the tools' under Editing and J's
under Views, beside L.

## Tests

In `keys.rs`: R in and out of the crop, the level put down, and the
Done button leaving what R entered; W arming the pipette, again
putting it down, and from the crop leaving for the Develop tab; M and
Shift+M each making a mask with the shape in hand, placed by a drag,
the other key before the drag changing the shape in the same mask, and
the same key again putting it down; R mid-drag making one edit and the
release none more; W then M, and W then R, leaving no pipette in hand;
and all four doing nothing in the grid, the lightbox, culling, and with
no frame open. The J row for the clipping warnings now says "in the
Loupe", since J has two rows. In `entry.rs`: W with the exposure
focused, then the arrows still nudge it. In `lightbox.rs`: the badges
off by default, on all six tiles after J ("3 stars, pick, yellow") with
the marks still off, kept across L out and in, and off again. The keys
table's handler test holds the new chords to the handler.

## Not done

The user guide's Keys chapter needs the five rows, and its J row "in the Loupe". We did not run the
editor; the badges' place on a tile is checked by the accessible
labels, not by eye. Escape does not leave the crop, as before.
