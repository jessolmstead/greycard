# 46. Custom export resolution (2026-09-15)

The editor list's smallest item. The sheet's long edge was four
names, Full and three powers of two; a gallery that wants 1600 or a
print lab that wants 3000 had no way to say so.

**What it is.** A fifth choice, Custom, beside the four; picking it
opens a row with a field for the long edge in pixels and, beside the
field, the size that edge gives: the frame after the crop scaled to
it, rounded as the resize rounds, or the frame's own size "as it is"
when the number is no smaller than the picture, since an export never
enlarges (§20). The number is whole pixels, sixteen or more; anything
else (blank, a word, a decimal, a negative) is no size, and the export
is the picture's own, which the row says. The Sharpen label greys by
the same test, so it now goes grey for a custom edge the picture is
already under, not only for Full. The typed text is kept in the
settings as typed, 1600 to begin with, beside the size's name.

**How.** `export::long_edge` turns the sheet's name and the typed
text into the `Option<u32>` the settings carry, and `parse_edge` is
the reading of the text; the sheet asks the same function through a
pure callback so the size it shows is the size that comes out, rather
than a second parse in Slint that would take "1.5e3" or "16abc"
differently. The frame's size reaches the sheet from the render
closure, where the geometry is already worked out for the view, as
two properties. A test pins the parse: the names, the bounds, and
the slips.

**Beside it.** `--long-edge N` with `--export PATH`: the flag's size
over the sheet's remembered one for that run, and not remembered
after, as `--export` runs leave the settings alone (§43). Checked on
the R6 II frame: 1500 gives 1500 × 1000, 90000 gives the 6000 × 4000
it is.
