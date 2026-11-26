# §750. Lambda, a fourth theme (2026-11-26)

Monocard, a fork for monochrome photographs, styles its window in
grey with a warm accent, and its author sent that styling here (PR
213). It is a fourth theme after §404's three, named Lambda, and a
palette table only: no component asks which theme is on.

## The accent in the tables

The accent was one blue held as constants, the same in every theme.
It now comes from `Tones`, with its hover, pressed, and muted steps,
the pair for a white ground (`accent-dark`), and the ring of a set
(`selection-ring`). Charcoal, Grey, and Light hold the old constants
to the byte.

`fill` is new: a slider's fill and a switch that is on, at rest. Under
the pointer, pressed, or with the keys they take the accent as before.
In the first three themes `fill` is the accent, so nothing changes
there. In Lambda it is grey (#a9a9a9, 4.6:1 on a slider's track), so
the orange is kept for what is chosen and what is under the hand.

`overlay` is new too, the blue of marks drawn on the picture: the
compare's frame, a guessed face's box, a mask's center handle, the
level and guide lines. It is #6ea8ff in every theme. An orange mark is
lost on a warm picture, and the picture is not the theme's to color.
The curve's chosen point, a control and not a mark, follows the
accent.

## The table

Lambda has Charcoal's surfaces, text, and scroll bars. What differs:

- The accent, #fb7e14, an orange and not the amber it came with: the
  amber sat 1.2:1 from the warning yellow, and the two were told apart
  only by place. It is 6.4:1 on the surface; `on-accent` on it is
  7.3:1, and 5.9:1 pressed; `accent-dark` (#853800) is 8.2:1 on white.
- The warning yellow, #eac94a, a step lighter than Charcoal's, 1.6:1
  from the accent. The accent and the red are 1.3:1 apart, told by
  hue.
- The chosen row, the accent at 25%: 1.25:1 from a row under the
  pointer and 1.5:1 from a plain one, where white at 12% gave 1.18:1.
  On it, text is 9.2:1 and secondary text 4.6:1.

## Not done

- The fork's figures font and its panels' layout are not part of the
  theme.
- A switch at rest and one under the pointer differ by hue more than
  by lightness (1.1:1).
