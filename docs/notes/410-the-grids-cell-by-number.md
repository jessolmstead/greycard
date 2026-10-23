# §410. The grid's cell by number, the wheel switchable

*2026-10-23*

Three of a tester's asks about the grid (pixls, 2026-10-22): the
cells too far apart, the cell size set by the wheel only, and the
wheel resize firing when they meant to scroll.

The gap between cells was 8 px and is 4, the theme's smallest step;
the rings are drawn inside the cell and the picture is inset 4 px,
so neighbors still read as two, and the 2 px drop bar still fits
the gap. It is a small change to the eye: at 176 px cells the pitch
moves 2 percent. What makes the grid look loose is the square cell
itself, which fits a 3:2 picture with 55 px of plate above and
below it, so rows sit 80 px apart with a 4 px gap. If the tester
still finds it loose, the answer is a cell shaped to the picture,
not a smaller gap; the roadmap holds that.

Two rows in the Settings sheet's Library category: Cell size as a
number in logical pixels, bound to the same setting the wheel
changes and clamped to the grid's 96 to 512, taken on Enter or when
the sheet closes; and Resize with the wheel, on by default, which
off makes Ctrl+wheel scroll like the plain wheel while plus and
minus still resize. The switch governs the lightbox's Ctrl+wheel as
well, since it is the same function over the same sheet. A step
from a typed size goes to the next fixed size in the direction
pressed, not from the nearest, so 174 and plus is 176.
