# 205. A color label tints the grid's plate, and bands the strip's foot (2026-09-29)

Roadmap line (Tweaks): "Color-labeled frames should be more obvious
than a small color icon. Maybe the background around the frame should
go that color?"

**What it was.** A frame's color label showed only in the small pill
`ThumbBadges` draws over the picture's corner — a few pixels of color
next to the stars and the flag, easy to miss across a folder's worth
of tiles. The grid cell's plate (its background, behind and around the
picture) was `Theme.selection`, `Theme.hover` or transparent, never
touched by the label; the strip cell the same, with a name row instead
of the label row.

**What it is now, and why.** In the grid, a labeled cell's plate is
tinted a quarter of the way from the sheet's own color toward the
label (`Theme.plate-tint-mix`, 0.25; `Theme.plate-tint`), so the five
colors are tellable apart across a folder without turning it into a
quilt. The selection's and the hover's ground are a second, sibling
rectangle laid directly over the tint — the same `in-set`/`has-hover`
reading that already sat over the plain plate's `transparent`, but
`Theme.hover` itself replaced with `Theme.hover-veil`: the same grey
as a translucent wash (`#ffffff0d`, about 5% white) rather than a flat
fill, so it still composites to `hover`'s own (46,46,46) over the
plain sheet but lets a labeled plate's tint show through it rather
than hiding it the way an opaque fill would. A chosen tile keeps
exactly the cue an unlabeled chosen tile has (the accent ring,
`Theme.selection`'s own translucent blue, now with the tint showing
faintly through it too). The mix and the veil both live on `Theme`
(`label-color`, `plate-tint`, `hover-veil`) so the cell names no color
of its own;
`Theme.sheet` stands in for "transparent" only when there is a label
to mix toward, so an unlabeled cell renders exactly as it did.

In the strip, the plate is left alone on purpose — it sits beside the
developed picture, and a tinted surround there would pull the eye's
color judgment while looking at the frame. Instead a labeled cell
gets a 3px band, inset by the cell's own corner radius so it does not
run past the rounded corners. The band sits between the picture and
the name, not along the cell's outer foot: the picture is shorter by
the band's height and a small gap above it (`96px - 5px`), reserved
whether or not the row carries a label, so the name's own position
never moves and the band never draws over the border or the name.

Both flow through `Thumb.label`, which was already carried by the
windowed cells (`cells.rs`, §202): a row's label reaches whichever
slot its cell is in, the same as `rating` and `flag` already did, so
re-pointing a slot on a scroll costs nothing extra here.

Four things a review caught, over two passes at this:

- `Color.mix(other, factor)` weighs its *own* color by `factor`, not
  `other`'s — the opposite of what "mix a quarter of the way toward"
  reads as. `base.mix(label, 0.25)` is 75% label and 25% base, not the
  reverse; a first cut wrote it that way and every plate came out
  saturated almost to the label's own color. Fixed by mixing at
  `1.0 - the wanted share` (`Theme.plate-tint`'s own comment says so).
- The first cut's band sat at the strip cell's very bottom edge,
  which is also where the 2px accent ring and the 1px chosen outline
  end and where the name's descenders sit: the band replaced the
  border across its width and clipped text under it, the two never
  checked against each other since one is drawn well after the other
  in the file. Moved to just under the picture instead (above), which
  also answered a pre-existing question the review found on the way:
  the row's own arithmetic (padding, the picture, the spacing, the
  name) already ran a little past the strip cell's height before this
  line touched it, name and picture both squeezed by a few pixels;
  shortening the picture on purpose for the band's own room does not
  make that gap worse.
- The first cut also gave the selected and hovered plates a *stronger*
  mix (0.4) of their own, mixed straight into the selection or hover
  color as the tint's base, reasoning that a ground already a step up
  from the plain plate needed a bigger tint to still read over it. A
  chosen red tile measured (122,70,72): a warm, saturated red with
  only a faint blue edge to say it was chosen at all, the ring aside.
  The layered fix above replaced it: the tint and the selection's or
  hover's ground are painted separately now rather than mixed
  together, so a chosen red tile measures (86,68,80) — the accent
  blue reading clearly over a faint red.
- That first layered pass still put flat, opaque `Theme.hover` in the
  overlay for the hover case, which fixed "chosen and hovered read the
  same" but overcorrected it: a hovered labeled tile composited to a
  flat (46,46,46), the same as an unlabeled hovered tile, with the
  tint hidden underneath rather than showing through — only the badge
  icon still said which label it was. `Theme.hover-veil` (`#ffffff0d`,
  white at about 5%) replaces `Theme.hover` in the overlay: over the
  plain sheet it composites to the same (46,46,46) `Theme.hover` gave
  outright, so an unlabeled hovered tile is unchanged, but over a
  tinted plate the wash lets the tint read through it. A hovered red
  tile now computes to (91,62,60) (worked by hand from the veil's
  alpha and the tint's own (82,51,49), not screenshotted — the CLI has
  no way to plant the pointer over a cell for a snapshot).
- A failed thumbnail's "Unreadable" box had no ground of its own,
  so it read the cell's ground under it — `text-muted` and
  `border-strong` are chosen against the plain sheet, and dropped to
  roughly 2.2:1 on a labeled plate and 1.6:1 on a chosen labeled one,
  the border past reading distinguishable from the fill at all. Given
  its own `background: Theme.sheet`, inset within whatever plate is
  under it, so the box and its text always read at the ratio they
  were chosen for (about 3.3:1 against the sheet; `border-strong` is
  not held to a text ratio and was never meant to be, but is now
  visible again against a fixed, known ground rather than a plate that
  could be lighter than it).

**Checked.** Six hard-linked frames under `target/scratch/snap-labels`
(read-only originals in `~/Pictures/Test`, never touched, `--grid`
opened directly on the folder rather than through a sidecar-reading
path that would need `--no-sidecars`), five with a hand-written `.gcd`
sidecar setting `meta.label` to each of the five colors and one with
none; the first (red) is the frame a plain open selects, so its tile
shows the selected-and-tinted case. Purple has no key in the browser
yet (the meta module's own note: Lightroom gives it none either), so
its sidecar was written directly rather than through a keypress — the
only reason to hand-write sidecars instead of using the culling keys
for the other four. A second, two-frame folder
(`target/scratch/snap-failed`) for the failed-thumbnail check: one
real hard-linked frame (selected, unlabeled, so the app has something
it can actually open — a truly unreadable *current* frame fails the
whole process at startup, not just its thumbnail) and one file of
random bytes named `.CR3`, labeled blue, which the decoder correctly
refuses and marks failed.

`--grid --snapshot` before and after, and a plain `--snapshot` (the
loupe, its strip along the foot) after, all through
`target/scratch/run-snapshot.sh` (an isolated `XDG_*` set under
`target/scratch/xdg`) and the plain `target/debug/greycard-ui`;
before/after by reverting the changed files with `git checkout --`,
building, then reapplying the diff with `git apply` rather than
touching the shared stash. Sampled actual pixels with ImageMagick
(`magick ... -format "%[pixel:p{x,y}]" info:`) rather than trusting
the screenshot's look alone, since a dark, muted tint next to
near-black chrome reads more vividly than its numbers: the yellow
plate came out `rgb(81,72,41)` against a `rgb(30,30,30)`-ish sheet —
the quarter-mix, not the three-quarters the first bug produced; the
chosen red tile `rgb(86,68,80)`, matching the layered math by hand;
the unlabeled tile's plate matched the sheet exactly; the failed box's
own ground sampled as `rgb(35,35,35)` (`Theme.sheet` exactly) on the
labeled tile behind it, where before this fix it would have read the
tint instead. The strip's band sampled as the five label colors
between the picture and the name on five tiles, absent on the sixth,
clear of the corner badges, the border ring, and the name's own text
in every case — checked at the pixel level on a zoomed crop of the
selected, labeled tile, where the ring, the band and the full name
("a-red.CR3") are all visible together. No light theme exists yet to
check against (`ui/theme.slint` has one `Theme`, no variant, and
`greycard-ui`'s CLI has no `--theme`), so this is dark only; the mix
runs through `Theme` colors rather than literal ones so a light
`Theme` would pick it up without touching the cells. Kept under
`target/scratch/`: `grid-before.png`, `grid-after.png`,
`loupe-before.png`, `loupe-after.png`, `grid-failed.png` (the
unreadable box, one plain and one labeled tile) and `strip-zoom.png`
(a crop of the selected, labeled strip tile showing the ring, the
band and the full name together).

**Tests.** The harness cannot read a cell's painted background, so
what is tested is that the label (and `chosen`, the model's "selected"
flag) reach a cell's data after a scroll re-points its slot, and that
a plain row's cell carries neither: `cells.rs`'s
`a_labeled_rows_color_and_chosen_flag_reach_its_cell_after_a_repoint`,
built on the existing repoint test right above it (a window at row 10,
a label set on row 19 while it has no cell, a scroll to row 12 that
repoints row 19 into a slot that held row 11, the label and `chosen`
read back off that slot; a kept slot's row checked to carry neither).

**Not done.** No light theme to check the tint against (above). Hover
was reasoned through and hand-computed, not screenshotted: the CLI has
no way to plant the pointer over a specific cell for a snapshot, so
the hovered red plate's (91,62,60) is worked by hand from
`Theme.hover-veil`'s alpha and the tint's own value, not seen directly
on screen; the plain sheet's case (46,46,46), matching `Theme.hover`
exactly, is the one part of this checked by eye before, since it is
also what an unlabeled hovered tile has always shown. `plate-tint-mix`
is a flat constant, not sampled against a longer shelf of frames at
once for whether a quarter is the right amount by eye over more than
six tiles; the roadmap line's
"maybe" is answered with a specific number, not tried against
alternatives.
