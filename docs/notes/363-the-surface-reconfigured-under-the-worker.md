# §363. The surface reconfigured under the worker (2026-10-18)

A tester on a laptop with an integrated Radeon, under COSMIC with
tiling on, had the window die on a custom white balance or a
demosaic change, every time, with wgpu's "Failed to wait for GPU to
come idle before reconfiguring the Surface" (issue #27). Our first
reading, a GPU reset, did not fit the text: a lost device says so.
A stand-in program found the cause, and the tester confirmed it
with `--cpu-ops`, which keeps the develop off the window's device.

## The race

The worker runs its GPU ops on Slint's own wgpu device and queue.
When Slint reconfigures the window's surface, on a resize or when
the compositor marks the surface outdated, wgpu-core's configure
waits for everything submitted and then requires the queue empty;
if another thread submitted during the wait, the result is reported
as a validation error, by wgpu's own comment a case that "can only
be non-empty if another thread is submitting". A custom white
balance and a demosaic change both remake the base, the worker's
longest run of submits, and a tiling compositor resizes the window
on its own, right after it maps and whenever the layout changes, so
the first develop after launch collided with it and the crash
repeated at every start. Nobody had set an uncaptured-error handler
on the device, neither Slint 1.18.1, our femtovg fork nor we, so
wgpu's default handler panicked. In a stand-in with the real ops on
one thread and configure's exact wait on another, one wait in six
failed on an NVIDIA card and one in four on a Radeon with the
tester's Mesa. Memory across remakes was flat; nothing leaked.

## The handler

We set our own handler on the shared device at the window's
rendering setup, classifying by type and not by text. A configure
failing with the wait timeout is logged and a reconfigure
scheduled. That alone was not enough: a failed resize leaves Slint
holding the new size while the surface keeps the old, so every
frame after it fails validation with mismatched attachment sizes,
and the editor still died sixteen milliseconds later. Those frames
are let through while a retry is pending, on the window's thread
only; they reach the screen cleared, since wgpu clears a surface
texture that was never rendered before presenting it. Every other
validation error, every other configure failure, out of memory
(with its chain of causes, since wgpu's text for it is bare) and
internal errors still panic with wgpu's full text, as before: those
are bugs, and the object the call made is invalid from then on.

The handler runs inside Slint's configure with its cells borrowed,
so it posts the retry to the event loop, which dispatches a resize
event at the window's current size, the same event and conversion
Slint's winit backend sends, reaching the renderer's resize and so
the surface's configure; a redraw alone is not enough on Wayland,
where nothing reports the old swapchain outdated. The first retry
runs at once; a race inside the retry's own configure schedules the
next fifty milliseconds out, and any number can follow in a
session. wgpu-core returns from the failed configure before it
takes the old presentation, so the old swapchain stays usable until
then. Under a resize storm during develops on the Radeon, the
unpatched editor died with the reporter's exact text in the first
develop; with the handler, three runs of 150 s took 18 raced
configures and no crash, and the empty flash after a race went from
up to 1.5 s with the timer to under 80 ms with the immediate retry,
two cleared frames remaining that winit draws before our closure
runs.

## What the crash hid

A failed configure drops the callbacks wgpu's wait had collected,
unfired, and the scopes' read-back submits in one frame and collects
in the next, treating a closed channel as not ready; after a
recovered race during an analysis the histogram, navigator and
readout would have frozen for the session and the window redrawn
at sixty frames a second forever. A closed channel is now a lost
read-back: the staging buffer is unmapped inside an error scope,
since a map that had failed leaves it unmapped, and the analysis is
sent again in the same call. The other read-backs map and wait in
one call on the window's thread, or a band at a time on the
worker's, so none can lose a callback this way. Not reproduced live
in two long runs, since the worker's own wait usually fires the
window's finished maps first; the test builds the state directly.

## Left

A sheet first drawn in a cleared frame may keep a blank shadow
until it is rebuilt, since femtovg draws its shadow cache in the
same command buffer; untested and unfixed, cosmetic. The retry can
alternate between immediate and delayed while the worker keeps
submitting, bounded by the worker's activity. Upstream, wgpu's
configure should wait for the submission it read and not require
an empty queue, and should fire the closures it collected on the
error path; a note with the stand-in is kept for filing.

## Tests

The configure race is recoverable, with or without a retry pending;
the other configure failures, out of memory, internal and the same
error from any other call are fatal, out of memory carrying its
chain; a mismatched frame is let through only while a retry is
pending and on the window's thread; failures ask for one retry at a
time, a retry that races again schedules another at fifty
milliseconds and a later race is immediate again; the retry sends a
resize at the same logical size and then a redraw, a minimized
window only the redraw; a lost scopes read-back is made again and
the scopes go idle, on lavapipe.
