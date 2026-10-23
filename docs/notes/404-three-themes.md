# §404. Three themes

*2026-10-23*

The window was one near-black palette, every color a constant on
the `Theme` global. A tester asked for a lighter grey (pixls,
2026-10-22), and a roadmap line had wanted the same. The Settings
sheet's Interface category gets a Theme row: Charcoal, today's look
and the default; Grey, a mid step; and Light.

## One global, three tables

The global keeps its name and every token, so no component changed
its reading; the tokens now come from one of three `Tones` tables
chosen by a name the Rust side sets from settings.json at start and
on the row's change, with an unknown name reading as Charcoal.
Charcoal's table is today's constants to the byte, and the review
diffed it. The fluent widgets we use (a line edit, a combo box, a
scroll view) follow Slint's `Palette.color-scheme`, which the
`fluent-dark` style compiles to a constant, so setting it dark
changes nothing and setting it light on Light makes them follow.

The accent stays one blue in all three; as words on Light it has an
`accent-text` dark enough to read. The label colors split in two:
the swatches (a badge's square, a chip's dot, a cell's tint, the
strip's band) stay the one bright set in every theme, since they sit
on fixed dark plates where a darkened color vanished; the words
(the keys conflict line, the delete sheet, the match sheet) read
`warn-red`, `warn-yellow`, and `ok-green`, the same colors on
Charcoal and Grey and darkened on Light to hold 4.5:1.

## What the theme must not touch

The picture's surround is its own setting (§397's mat and the
canvas color) and stays; the lightbox's surround is a fixed
near-black. Text drawn on the dark plates over the picture, on the
thumbnails' badges, on the navigator's rectangle, and on the scope's
ground reads fixed tokens, since those grounds never lighten; on the
first pass the status plate's words went black on Light, and the
reviewer found the navigator and the lightbox's "Developing" after
it. Selection fills and the focus ring moved into the tables too,
since a 15% blue veil that reads on charcoal is invisible on a
light grey, and several rows show their choice by that fill alone.

## Not done

- Grey's contrast was judged by eye against Charcoal's; Light's
  was measured. A fourth tone or an accent choice is a palette
  table away.
