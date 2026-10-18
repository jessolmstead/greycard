# §349. The color under the pointer on the scopes (2026-10-18)

The roadmap's idea was to show the R, G, B and W points on the
histogram while the pointer is over the photo. Lightroom shows the
pixel's RGB percentages under its histogram, and a hand from there
looks for them. The vectorscope's dot is the more useful half, and
one Lightroom cannot offer, having no vectorscope: rest the pointer
on a cheek and see where that spot sits against the skin line. So
the readout went in with marks on every scope, not only the
histogram.

## Where the color comes from

The scopes bin the analysis picture (§38): the frame drawn 512 wide
through the viewport's shader, read back for the navigator since
§44. `Renderer::analyzed_at` keeps that read back's pixels and reads
one at a point of the frame, as fractions of it. The hover maps the
pointer to those fractions with `view_to_frame`, the arithmetic
`view_on_picture` already did (now `view_in_frame`, shared), and the
analysis picture covers exactly that frame, crop and crop mode
included, so no geometry is undone. The hover measures that frame
as the frame's notifier draws and analyzes it: on the picture
`drawn_picture` gives, turned as it is drawn while a turn's develop
is on its way, under the peeked edit or the panel's. While the
picture on the GPU is not the open frame's own develop (one held
under the last frame's edit while the next develops, or another
frame's), there is no readout, since the scopes describe a frame
the panel no longer shows. The droppers keep their own path.

We did not take the droppers' path. `sample_view` copies from the
developed texture and blocks on the map, which is fine for a click
and not for every pointer move; and `finish::pick` repeats the
finish on the CPU without the masks, the vignette or the grain, so
in a masked area its color is not the one the scopes binned and a
mark could fall off the trace. Reading the same bytes the compute
pass binned keeps the marks on the traces. The cost is resolution:
one analysis pixel is about a dozen source pixels across on a
6000-pixel frame, so at 1:1 the readout is a small area's color, not
the pixel's. For a mark on a 256-level scope that is what we want; an
exact pixel would need its own one-texel read back from the
viewport's target, asynchronous, and we left it.

## Calm under a moving pointer

The first build read the one analysis pixel under the pointer, and
over grass or foliage, where neighbors differ by half the range, the
marks leapt across the scope at an ordinary hand's speed. Two things
calm them. The color is read from the analysis pixels within three
screen pixels of the pointer (`HOVER_REACH`), so the marks follow the
picture's tones and not each blade in it; at the fit that box is a
few analysis pixels across (the reach is about 3 x 512 / the view's
width), and at 1:1 it is under one analysis pixel, so the one under
the pointer. The box's mean alone would calm the marks, but a mean
can sit between the colors the box holds, on a cell nothing lit: an
edge between two hues put the vectorscope's ring on an empty cell
between their two clouds. So the color is the box's own pixel
nearest the mean (`scope::under_pointer`), and every mark is on a
lit cell of the trace; the waveform and the parade place it by that
pixel's column, not the pointer's. And the marks glide, 120 ms eased
out, to each new place: the rows are updated in place when their
count holds, since a model replaced on every move makes the marks
anew and nothing animates. A move that reads the same pixel as the
last, or the second of a diagonal move's two reports, leaves them be
rather than restart the glide.

## With Selection on

§189's Selection weighs every bin by the chosen mask's coverage,
which the analysis picture carries in its alpha. The readout keeps
that alpha (the navigator's copy has it set opaque), weighs its mean
by it as the bins were weighed, and takes the nearest of the covered
pixels only, so on a soft edge the marks are the selected part's
color. Where nothing under the pointer is selected, the pixel is
counted on no trace; the readout then gives the plain mean's nearest
pixel, and the marks show at 40% where it would sit, with
the numbers dimmed. That is a use of its own: with a skin mask
selected, rest the pointer on the background and see where it falls
against the skin's cloud.

## The marks and the numbers

`scope::marks` is the reference: for a color and how far across the
frame it is, where it lands on each scope, as fractions of the
scope's picture. Each mark sits in the middle of the cell the binning
puts it in and the drawing draws that cell at: on the histogram a
line through each channel's level and one through the luma's; on the
waveform three dots at the pixel's column; on the parade the same in
each channel's panel, through the panel's own mapping of 256 columns
onto 84 pixels; on the vectorscope one ring at the wheel cell's
center. A color off the wheel's grid bins to no cell and has no
ring. The tests bin a picture with the reference, draw it, and
check each mark is on a lit pixel of its channel's trace and another
color's mark is not, and that a box straddling two colors reads one
of them; a GPU test checks that the color `analyzed_at` returns is
the read-back picture's at that point, or over a reach one of the
box's own pixels, and is in the compute pass's bins at the histogram
level, that pixel's waveform column and the wheel cell that `marks`
uses; that with a gradient selection one end of the frame is in it
and in the weighed bins and the other out; and that a new image
reads nothing until its own analysis is back. It passes on NVIDIA
and lavapipe.

W is the luma, Rec.709 on the encoded values, the Y the vectorscope's
Cb and Cr are already taken against, and is labeled L in the
readout: `R 182  G 140  B 120  L 151`, levels 0 to 255 as the scopes
bin them. Its mark is white on the histogram; the waveform and the
parade draw no luma trace, so it has no dot there. The numbers sit in
a pill at the top of the scope while the pointer is on the picture,
over the scope rather than in a line under it, so the panel below
does not move as the pointer crosses the picture's edge; the cost is
the top few pixels of the trace while hovering, under the marks,
which are drawn over the pill.

The `.slint` side reports the pointer through the viewport's touch
area, its `mouse-x` and `mouse-y` changes and its `has-hover`, in
the viewport's own pixels as the droppers' presses are. A new
analysis picture under a resting pointer, from an edit or a scope
switched, reads the color again, and so does a frame drawn anew
under it: a zoom or a pan from the keys, the fit or 1:1, or a
resize of the window or a pane puts another spot of the picture
under the pointer. Off the picture, in culling, before the first
analysis, or while the camera's JPEG stands in for a develop, there
is no readout; a new image drops the last analysis's pixels, so the
first move after its develop lands never reads the last frame's at
the new frame's shape. A re-develop of the open frame under a
resting pointer, as after an edit from the keys, leaves what is
shown until its analysis is back, rather than have the readout blink
off and return without the glide; a step to another frame clears it.
A leave reported while the state is borrowed is tried again 16 ms
later rather than dropped, a delay so that a nested event loop
holding the borrow does not spin the retry.

## Found on the way: the scopes are under the monitor's table

§42 says the histogram, the scopes and the clipping marks are of the
export space's encoded values, and the user guide says the same. They
are not when a monitor profile is in use: the analysis picture is
drawn through the display table, as §44 says of the navigator. On
the PA279CRV's profile the same point of a frame read `R 125  G 114
B 94` with the profile and `R 131  G 114  B 91` with
`--no-display-profile`, the whole histogram moved, and the shadow
clipping lamp lit only without the profile. The readout reads what
the scopes bin, so it inherits this until the analysis is drawn
without the table. On the roadmap's bugs.
