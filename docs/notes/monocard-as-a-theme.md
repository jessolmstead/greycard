# Monocard as a theme

*2026-11-25*

Monocard, the fork for monochrome photographs, styles the window with an
amber accent, a darkroom safelight's. This brings the styling here as a
fourth theme, after §404's three. It is a palette table: no component
asks which theme is on.

## The accent in the tables

The accent was one blue held as constants, the same in every theme. It
now comes from `Tones`, with its hover, pressed, and muted steps, the
pair for a white ground (`accent-dark`), and the ring of a set
(`selection-ring`). Charcoal, Grey, and Light hold the old constants to
the byte.

`fill` is new: a slider's fill and a switch that is on, at rest. Every
slider and switch reads it, and under the pointer, pressed, or with the
keys takes the accent as before. In the first three themes it is the
accent, so nothing changes there.

## The table

Monocard has Charcoal's surfaces, text, and scroll bars. What differs:
the accent family (#d8a24a), the focus ring, `fill` (#a9a9a9, which is
`text-secondary`), and the chosen row's fill (white at 12%).

Contrast, by WCAG's ratio:

| | surface #1e1e1e | sheet #232323 | raised #262626 | chosen row on surface #393939 |
| --- | --- | --- | --- | --- |
| text | 14.1 | 13.3 | 12.8 | 9.8 |
| text-secondary | 7.1 | 6.7 | 6.4 | 4.9 |
| accent-text | 7.3 | 6.9 | 6.6 | 5.1 |
| text-muted | 3.5 | 3.3 | 3.2 | 2.4 |

A chosen cell on the grid's sheet is the lightest ground: text 9.0:1,
accent-text 4.7:1, text-secondary 4.6:1. `on-accent` on the accent is
8.3:1, on its hover 10.3:1, and pressed 6.6:1. `accent-dark` on white is
8.1:1. The fill at rest is 4.6:1 on a slider's track. Measured on a capture of
the strip, as §696 did, a set's ring is 4.0:1 on the strip's ground and
the current frame's 7.3:1 (Charcoal's 3.7:1 and 6.9:1). Muted words are
as on Charcoal, below 4.5:1.

The warning words and `ok-green` stay Charcoal's colors. The fork draws
them in grey, since it has no color on its panels; greycard edits color,
and a warning has to read as one.

## Not done

- The warning yellow (#dcb63c) and the yellow label are close to the
  amber (1.2:1 between them), so the two are told apart by place, not by
  color.
- The fork's figures font and its panels' layout are not part of the
  theme.
