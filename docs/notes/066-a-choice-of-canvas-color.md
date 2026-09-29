# 66. A choice of canvas color (2026-09-18)

The viewport's margin around a fitted picture was two hardcoded
greys in the shader (0.063 outside the frame, 0.03 outside the
source but inside it), and neither matched Theme.canvas's #0f0f0f
exactly: close enough that nobody saw the seam. Only the outer one
is the panel's business; the inner one is the gap a turned picture
leaves inside its own frame.

MONITOR gained a Canvas row of four swatches (Black, Dark grey, Mid
grey at #2e2e2e, about 18 percent as displayed, and Light grey),
the `Swatches` component the mixer's bands use. The choice is a
uniform now (`Params.canvas`), fed from the same four encoded sRGB
constants the panel's Rectangle paints with, so the shader and the
panel agree exactly rather than by coincidence. It skips the display
table: the shader's early return consumes it before the LUT sample,
where the hardcoded colors sat. Kept in the settings by name, not
index, so reordering the swatches cannot repoint a saved choice.

Checked headlessly: two settings files differing only in the canvas,
a `--snapshot` of each, compared with ImageMagick. The margin moves
from (15,15,15) to (46,46,46) exactly; the picture's own pixels are
byte-identical between the two.
