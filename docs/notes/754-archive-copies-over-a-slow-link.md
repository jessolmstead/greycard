# §754. Archive copies over a slow link (2026-11-26)

Over a slow link to an archive share, slow and not hung (about 19
Mbit/s up and 7 down, a single megabyte read taking up to 4.8 s),
Back up copied at most one frame, said the copy had timed out, and
stopped. Bring back failed the same way. On a home network none of it
showed. This amends §217's and §218's figures: 1 MB chunks, and 3 s
of quiet for every job.

The copies ran under the quiet limit meant for asking whether a root
answers, and their heartbeat was not honest:

- **Writes.** The copy beat at each megabyte written, but writes to
  a share land in the machine's own cache at once. The whole upload
  then happened inside the one `sync_all` at the end, which beat
  nothing: 41 s for 100 MB.
- **Reads.** The read back, and Bring back's read of the share, beat
  once a megabyte.

**Syncs in steps, toward a share.** Where the destination is on a
network mount, the copy syncs its data as it goes and beats after
each sync, so no call covers the file. The step follows the link: it
starts at 1 MB, halves after a sync that took over 3 s, down to
256 KB, and doubles after one under 1 s, up to 2 MB, so a link that
slows fifteenfold in the middle of a file still gets a step through.
The last `sync_all`, the read back, and the name come in the order
they did; a file is on the disk and proven before it is called
landed. A local disk still syncs once: stepping cost two to four
times there (0.06 s against 0.23 s for 100 MB on an SSD). The
destination alone decides, found through its links, so a Bring back
writes as a local copy does.

**Smaller reads.** The beating readers take 256 KB at a time,
everywhere. On a local SSD the difference is inside the noise.

**Two limits.** A job that reads or writes files' contents on the
archive has 30 s of quiet (`COPY_WAIT`): the copies, Remove rejects'
run and its look, and the Back up sheet's planning look, which can
read frames whole. A look that only lists and stats keeps 3 s. A
share that has truly stopped is still set aside and reported, after
30 s where it was 3; one that does not answer at all is still caught
in 3 by the look inside the job.

**The rest of a copy beats too.** The destination's sweep and the
sidecars' carry beat at each file, where they beat once a folder.
Indexing the landed copy reads it from the share, its pages just
dropped for the read back, and where the short read fails it reads
the file whole: 29 s for a 25 MB raw at 7 Mbit/s, and a run whose
copies had all landed would have said it timed out. That read beats
through a beat set for the thread (`decode::beating_reads`), which
the whole reads of raws and pictures consult; with none set they are
`std::fs::read` as before. A read handed to another thread does not
beat, and none on this path is.

**Not measured.** A real share on any system, the smaller reads on a
Mac's local disk, and Windows under a virus scanner. Below about half
a megabit a second a 256 KB step still outruns the limit.
