# 41. Clipping: the marks on the histogram and the warnings over the picture (2026-09-15)

Asked for before the next of §40's list: know where an edit has run
out of range, without reading the histogram's ends.

**What clips.** The histogram of §38 is of the encoded output, and
its first and last bins already held everything the output clips to
black and to white; they set no scale. `scope::Clipping` reads them:
a channel clips at an end when any pixel of the 512-wide analysis
image lands in that bin, as `scope.wgsl` rounds it (level 0 below
half a step, 255 above 254.5 steps), which is one pixel in a few
hundred thousand, the same sensitivity as the histogram's own bar.
Under the tone curve's shoulder (the ACES fit in `tone`) a highlight
reaches 255 only well past white in linear light, so the mark says
what the export will say, not what the sensor did; the sensor's own
clipping is the highlight reconstruction's business (§13o).

**The marks.** Two small squares at the histogram's top corners,
shadows left and highlights right, lit in the color of the channels
that clip: red, green or blue for one, yellow, magenta or cyan for
two, white for all three, as Lightroom's triangles are. Outlined
faint when nothing clips, in the mark's color when something does,
in white while that end's warning is painted over the picture. Each
is a button for its own warning; J toggles both together, on unless
either is on. The pair is kept in the settings file (§38) as the
scope is, and `--clipping` turns both on for a screenshot.

**The warnings.** `Warn` on the renderer's `View`, a vec4 in the
shader's params after `cubic` so the vec4s that follow stay aligned:
bit 1 the shadows, bit 2 the highlights. After the display table and
the mask overlay, the encoded output `e` (after the grain, before the
display table, which is the export's value) is rounded as the scope
shader rounds it; a pixel with any channel at 255 is painted red, else
one with any channel at 0 blue, solid, so a clipped region reads as a
shape and not a tint. The analysis draw passes no warnings, or the
histogram would bin its own paint. Only the histogram is post-table
and the warnings pre-table: under a monitor profile that pulls a
primary in, a saturated channel may bin at 254 while the warning says
255. Nothing in this is an export op, so there is no CPU reference
beyond `Clipping`'s tests; the overlay was checked by screenshot at
-4 and +6 stops.

**Beside it.** `--exposure` had never held: the panel took the file's
edit when it arrived and dropped the value set before. It goes into
the first file's edit now, as `--develop-temperature` does.
