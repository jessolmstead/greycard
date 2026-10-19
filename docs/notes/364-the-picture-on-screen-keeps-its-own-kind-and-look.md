# §364. The picture on screen keeps its own kind and look (2026-10-18)

A tester moving between frames saw each one come up far too
contrasty and saturated, then correct once the develop landed, with
JPEGs too but only after the first load, which made a raw hard to
compare with its JPEG. We read it as a placeholder drawn through the
wrong color transform, and measured it in captures under headless
weston before touching anything. The cached pictures are stored and
drawn right, with and without a monitor profile. The flash has four
causes, all in the window's own bookkeeping, and all in 0.7.0.

## The picture on screen through the next frame's kind

On a frame switch the last develop stays on the GPU, under its own
edit, until the new one lands. The moment the new frame opened, the
viewport took the new frame's kind of source, so a JPEG's finished
picture went through the raw path and gained the body's baseline
and shoulder, measured at +1.35 stops at the midtones with a fifth
more saturation, and a raw's picture through the picture path fell
1.2 stops. The white balance already allowed for the held picture;
the kind did not. The kind now travels with the picture: the
worker's develop carries its source's kind, the one place that puts
a picture on the GPU records it beside the upload, and the view
reads the drawn kind, as do the Curve and Mixer droppers. The open
frame's kind was left with no reader and was removed.

## A frame come back to

A frame whose camera picture was already held never had its wait
marked arrived, so nothing stood in and the last frame's develop
sat on screen under a status line claiming the camera preview, for
the whole develop; a first visit decodes and shows the picture, so
only a revisit showed it, hence "only after the first loading". And
the wait was built with the previous wait's picture, so another
frame's camera picture could stand under this frame's line. A held
picture now stands in as itself from the start, by construction.
This one dates from the first commit.

## The picture on screen under the next frame's look

Two more, found in review. Selecting A and then B before A's
develop landed set the held edit to A's whenever a picture had ever
been shown, so the picture still on the GPU, an earlier frame's, was
drawn under A's edit until B's landed, whenever B had nothing to
stand in; measured at +0.63 stops for forty seconds. The held edit
is now set only when none is held, since a held edit means the GPU
has an earlier frame's picture. And entering Cull cleared the held
edit while leaving it never set one, so leaving Cull onto a frame
whose camera picture was not there drew the last develop under the
new frame's look; leaving now holds the developed frame's saved
edit when it is not the frame left onto. The one case left is a
failed develop, after which nothing is held although the picture is
an earlier frame's, stated where the rule is.

## The droppers

Under a held picture the view's source size is zero, so every
Curve, Mixer, Range, Defringe and White dropper click read the
top-left texel of the old picture through a division by zero, and
between a develop's delivery and the frame that uploads it the
texture was still the last one. The droppers now wait for the
frame's own develop to be up with nothing standing in, in the
Neutral dropper's own words.

## Measured

Under weston, three frames (a raw with no edit, a raw with an edit,
a JPEG) stepped through twice: raw to JPEG between its open and its
develop went from −1.21 stops against the frame's own develop to
+0.00; a revisited held frame from the other frame's develop on
screen to its own picture at once, within a fifth of a level of its
develop; the picture under the next frame's look from +0.63 stops
to +0.00. The landing code the view runs is one function the tests
call, so a regression on any of the four fails a test, each shown
to fail with its fix disabled.

## Tests

The picture on screen drawn as its own kind until the next lands,
raw to picture and back; a frame come back to standing in with its
kept camera picture; a kept frame chosen while another stands in
showing its own; a frame left before its develop keeping the last
picture under its look; leaving Cull onto another frame keeping the
last picture under its look; a dropper waiting for the open frame's
develop, delivered and not yet up, up, and with a camera picture
standing in; a frame kept from before standing in at once as itself.
