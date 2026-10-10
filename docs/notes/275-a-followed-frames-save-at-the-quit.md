# 275. A followed frame's save at the quit (2026-10-10)

The first of the two gaps §261 left open. A frame followed onto its
archive copy while its own root is away (§244) lost its last edit at
quit.

**The cause.** A followed frame has no sidecar of its own here: its
root is the one that went, and its path in the window is the copy's.
So a save of it only advances the sidecar in memory and queues the
write to the copy, and that write needs the index reader to find the
frame's hash. The quit's own save of the open frame comes after the
reader is closed (§261), so `after_save` returned at once and nothing
was queued, sent or noted. §261's net for the other frames (each
unsent write noted waiting in the index as "saved as the window
closed", for the next window's catch-up to write from the file) cannot
catch it: the catch-up writes from the frame's file, and this frame
has none but the copy itself. A write queued behind one still out for
the frame was dropped the same way.

**Written at the quit, bounded.** The quit's saves are one function
now, `startup::save_at_quit`, called where they were: the open frame's
save, then the followed frames' writes, then §261's notes for the
rest. Every frame the window follows whose latest revision the index
does not record on its copy is written there from memory by
`sync::write_followed_at_quit`. The write is the job's own
(`write_one`): the copy checked against what we last wrote there,
recorded when it lands, the row cleared. It runs on a thread of its
own and the window waits for it at most `QUIT_WRITE_WAIT`, 5 s, the
worker's wait at the quit: room for the roots' look at the archive
(three seconds) and a write. A copy that comes back not behind
(another machine saved it since) is joined with the window's sidecar
on that thread and the join written, as the window's settle would at a
landing that will not come; the second write takes the archive's
answer of a moment ago and the row the first made, so it neither looks
again nor counts a try. One that holds all of it already is left, its
row for the next catch-up. A frame whose latest revision is recorded
on its copy already is not written again, so a quit after the last
save had landed costs nothing.

**Never beside a job still out.** A save of a followed frame made
while the window lived sent a job, and at the quit it may still be
running: the window hears a job's end only at its landing, which a
closing window never gets. Two writers of one copy end on whichever
renames last, and the older one's record would then clear the newer's
row. So the window now counts each write job from its sending to its
end on its own thread (`sync::Running`, ended by a guard that also
fires for a job never sent), and the quit's write waits for the
frame's job to end before it starts; it then finds that save recorded
and stops, or writes over it, or joins, as any write does. The wait
for the job is inside the same 5 s.

**When the wait runs out.** A write that has not ended in the wait,
or is still behind a job that has not, is noted waiting in the index
as "saved as the window closed", keyed by the frame as the window
followed it (the copy), and the log says the save is lost unless the
write lands before the editor ends (and, when a job was still out,
that an older save's write is still ahead of it). It goes on until the
process ends: if it lands, it records itself and clears the row;
otherwise the next window says the edit waits, and its catch-up
settles the row against what stands on the copy, which is whole either
way, every write going through a temporary and a rename. The edit
itself, in that case, is lost, as it is when the archive does not
answer at all (the write then ends inside the wait with the row
"offline"): its only places are the copy, which did not take it, and
the local file, whose root is away. That is §244's accepted loss for a
frame whose two places are both away; nothing here carries an edit
that lives in memory alone past the process.

**Why not close the reader after the last save.** Moving the quit's
save before the reader closes would let the followed frame's save
build its write, but the write would still go as a job whose landing
never comes and which the process ends under, with no bound the quit
controls; every other frame's save would then send a job too, changing
what §261 settled for them (noted, not sent); and the save would tell
the indexer to read the frame again while it is being stopped. A
direct write the quit waits for is the one way the edit is known to be
on the copy before the process goes.

**The test.** `sync::tests::a_followed_frames_save_at_the_quit_reaches_its_copy`
drives the follow for real (the local root renamed away, the frame on
screen following its copy), closes the reader as `startup::main` does,
and runs the quit's saves through `startup::save_at_quit`, the
function `main` calls; taking the followed write out of it fails the
test. The test does not drive `main` itself, so a `main` that stopped
calling `save_at_quit` would not fail it. What it shows, on the files
and the index: after the quit's save the copy's sidecar on disk is the
window's, the index records its latest revision there, no row waits,
and no job was sent; a second quit with nothing new writes nothing.
With a save's job still out (sent, not yet run), the quit's write does
not go beside it: the wait runs out with the copy byte for byte as it
was and the row noted; when the job then runs, the quit's write
follows it, and the copy ends on the newer save, with the older in its
history, recorded, no row left (taking out the wait for the job fails
this). With the copy saved by another machine first, the copy holds
both edits and the join is recorded. With a write that cannot end in
the wait, the index holds one row, "saved as the window closed", for
the copy; the next window says "1 edit waiting for Archive", and its
catch-up clears the row with the copy's sidecar byte for byte as it
stood.

**Open.** A job still out at the quit whose write lands in the last
milliseconds of the process clears the row the quit noted for the
newer save; if the process ends before the quit's write behind it
has made its own row, the copy holds the older save and no row says
otherwise. That is §261's narrow window, now for followed frames too.
