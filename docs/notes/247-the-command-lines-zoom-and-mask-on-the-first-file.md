# 247. The command line's zoom and mask on the first file (2026-10-04)

Two flags meant for screenshots lost their effect on the way into the
first file's develop; both were found during the white balance in a
mask work, and the captures there ran on a build that held both, not
committed. Both come down to one question the code had no answer
for: is this open the session's first?

**`--zoom 1` opened fitted.** The culling entry already promised that
at the start the command line's zoom stands. The develop view broke
that promise one step earlier: the camera's JPEG that stands in for
the develop (`cull::placeholder_arrived`) set the zoom to fit, every
time. The rule it was written for is right for every other open — a
file chosen opens fitted, and a magnified screen-size copy in between
would be a blur that jumps twice — so it stays, with one exception:
the session's first open, where the zoom is the command line's and
now stands through the camera's picture into the develop.

A first version keyed the exception on there being no picture held
from the frame before, reasoning that leaving a frame always fits the
view one way or the other. It does not: when the frame on screen is
deleted or moved out (`cull::drop_files`, or `roots::merge` finding it
gone from disk), the current frame is cleared and the nearest row is
opened with nothing to leave, so nothing is held and nothing fits;
only the placeholder's fit was catching it, and the user's zoom on the
deleted frame carried to the next. So the exception is a one-shot
flag instead: `State::opened` is set by the first `open_frame`
whichever way it goes, and that open alone sets `zoom_stands`, which
the placeholder reads.

The status line under the camera's picture said "fitted" in all
cases. It now says the zoom when the view is not fitted, in the
culling loupe's own words (`cull::zoom_words`, shared by both): "100%"
for the full-size copy, and "100%, screen-size copy until the full one
decodes" while the view-size copy is what is on screen.

**`--show-mask` was dropped.** The mask asked for was taken in
`open_frame` only when no frame was current. A first file the library
index stands in from its row is opened in two steps: the pick
(`pick_pending`) makes it current at once and asks for its sidecar,
and `open_frame` runs when the read lands. By then the frame was
current, so the open no longer counted as the first, and the target
stayed unset; the viewport's `show_mask` is the panel's toggle and the
target together, so nothing was painted. This is the "second open":
not a second develop, but the open proper after the pick. A run with
`--no-sidecars` has nothing to read and opened in one step, which is
why it showed only with a sidecar. The same flag answers it: the mask
and the patch are taken by the session's first open. A first open
that cannot take them — a run started in culling, or a first file
under an offline root — now drops them, rather than leaving them for
whatever frame is opened next.

**Checked.** Under a headless weston, on a hard link of `5M0A3976.CR3`
with a sidecar holding one radial mask: `--zoom 1 --no-sidecars
--screenshot` captured the fitted frame before and the 1:1 crop after;
`--show-mask 1 --screenshot` captured no overlay before and the mask
painted after; both flags together give the 1:1 crop with the mask
painted. Four tests drive the paths:
`the_command_lines_zoom_stands_through_the_first_placeholder` (a zoom
of 1 kept under the first file's camera picture and its develop, the
next frame opened from it fitted);
`a_frame_after_one_deleted_while_zoomed_opens_fitted` (the frame on
screen at 2x dropped from the list, the nearest one fitted under its
camera picture and its develop; it fails on the first version);
`the_command_lines_mask_reaches_a_first_file_read_after_its_pick` (a
first file standing in under a root, picked and then read, has the
mask asked for as its target and the panel's toggle on); and
`a_first_open_in_culling_leaves_no_mask_for_a_later_frame`. The first
and third fail on the old lines.

**Open.** A first file whose sidecar read stalls opens as its stand-in,
which counts as offline: the mask and patch are dropped and the zoom
does not stand, and when the read lands the reopen develops it fitted
and without the mask. A capture on a stalled network root therefore
still comes out fitted; the mask behaved the same before.
