# Bound the Add to collection menu

With 392 collections at 150% display scale, the submenu in
[issue #203](https://github.com/jessolmstead/greycard/issues/203)
asked for an 18,941-pixel-tall texture. The GPU's limit was 16,384
pixels, and opening the submenu crashed the editor.

Keep up to ten destinations in the submenu, most recently used
first. The settings keep their collection IDs, so renaming a
collection keeps its place. Collections without a recorded use fill
the remaining places in the library's usual set and name order.
**New collection...** stays in the submenu.

A library with more than ten collections also gets **All collections...**
It opens a sheet whose list scrolls inside a bounded area of the
editor window. Search matches the full set path and name without
regard to case. Opening the sheet and updating the recent destinations
wait until the submenu has closed.

The frame-menu tests cover adding directly from a ten-collection
library, moving that destination to the front, and scrolling and
searching a 392-collection library at 150% scale.
