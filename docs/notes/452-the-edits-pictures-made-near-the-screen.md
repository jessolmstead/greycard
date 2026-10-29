# §452. The edit's pictures made near the screen

*2026-10-29.* A tester left the Library open over a 12515-file root
and watched the CPU sit at 50 percent and the memory triple, with
nothing in the window to say why (discussion 51). Reproduced here:
every camera thumbnail the pool delivered queued that frame's reduced
develop if the frame carried an edit, and the Library asks a thumbnail
for every frame under the root, so every edited frame of the root was
developed on the CPU, two at a time, 1 to 22 s each, until the list
was done. The list the window notes as edited is not the drain; that
map also says whether a cell shows the edit or the camera's picture,
so it has to be whole.

The pool now starts an edit's picture only for a frame near the
screen. The window works out the rows on view plus as many again
above and below (`near_rows`), maps them to cells through the shown
list, so the frames a filter, a collection, or a stack hides never
count, and hands the pool that set; it is sent again when the list is
rebuilt or renumbered. Far frames stay queued, a key and nothing
more, and start when a scroll brings them near. A save or a change
on disk still remakes its frame's picture as before. On a 210-frame
root with 168 edited, 167 reduced develops and 553 CPU seconds became
14 and 32; the work scales with the screen, not the root.

The work shows. A pass begins at the first picture really made, cache
hits counting for nothing, and the log says so at its start and, at
its end, how many thumbnails, previews, and edit's pictures it made
in how long. The status line reads "making 12 pictures..." for a
count known to be left (previews waiting, near edits, lookups that
missed, jobs in hand), "making pictures..." while thumbnails still
wait to be looked up, and clears when the pass is over; it writes
only over an empty line or its own last text, and every word of it
is sent under the pool's lock so a count never lands after the end.

Found on the way: in the real window the pool put a reduced develop's
"pictures kept" into the thumbnail outbox, which drops anything that
is not a thumbnail, so the window never heard it. Only thumbnails go
through the outbox now.

Left for the roadmap: the camera thumbnails and the previews behind
them are still made for the whole root, about a quarter of a CPU
second a cold frame, an hour of CPU on that tester's root and more
for raws with no embedded JPEG, where each preview is a develop. They
fill the cache that makes scrolling instant, so they stay, visible
now, until the previews take the same near-screen rule. The base
shelf already takes a quarter of the machine's memory above 4 GB,
capped at 4 GB; the tester's 1.1 GB was within it.
