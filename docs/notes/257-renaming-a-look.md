# 257. Renaming a look (2026-10-07)

A look is named by its file in the looks folder, and that name is what
a sidecar, a preset and a snapshot store. Renaming the file by hand
left every edit using it showing "(missing)". **Rename…** in the Look
section renames the look's file and every `<name>.<curve>.cube` of it
(§235) together, and rewrites the name everywhere greycard can reach.
It took four review rounds. Nearly everything they found was about
what else is running while a rename runs, so most of this section is
about that.

### The sheet

The sheet asks for the new name. It refuses:

- a name a file can't have on any of the three systems, including
  Windows' reserved names and invisible characters such as U+200B;
- a name already in the looks folder, ignoring case;
- a name too long for the temporary `.channels.cube.part` name.

It warns, in yellow, when pictures in the folder or presets already
name the new name for a look that isn't there, since they will pick up
this look. Edits in other folders aren't reached and will show the old
name as missing; the sheet says so.

### Ids: a renamed state is a new state

A sidecar's states are chained, each id taken from its content and its
parent's id (§243). The rename rewrites every state of a sidecar in
place, outside the history: history, current, the redo stack and the
snapshots. From the first state that names the look onward, each id is
rebuilt from its new content.

Keeping the old ids looked simpler, and it loses work. A copy of the
sidecar elsewhere (an archive's, §244) that went on after the rename
would have states with the same ids and the old content. The join
would take those as the same states and drop the renamed ones. A test
shows it. With new ids, a join keeps both branches, nothing lost.

The cost: a state after the first renamed one that doesn't name the
look still gets a new id, because its parent's changed. A renamed
history joined with an unrenamed copy that went on can then show that
state twice, once under each id. Ids are deterministic, so renaming
the same chain on two machines, or renaming back, gives the same ids.

### What runs where

The window thread does only what's in memory or small:

- making the new names, linking or copying the look's files to them;
- the open frame and frames followed from an archive;
- presets, the copy/paste clipboard and the export queue.

Every other sidecar is read again from disk, not taken from the
window's copy, since an archive sync or another machine may have moved
it. It is renamed and written back where it was found, beside the raw
or in `.greycard` (§243). This runs on a thread under `supervise` with
§244's `WRITE_WAIT`, and the sheet's Cancel becomes **Stop**.

Landing 10,000 written sidecars takes 0.02 s on the window thread.
400 pictures take 0.29 s in all.

The thread's result for a frame is taken only if the window's copy
hasn't changed since it was sent, by its latest revision hash. If a
sync landed meanwhile, the window's current copy is renamed and saved
instead, so another machine's states are kept.

### Reads that land late

A sidecar read that is out when the rename starts can land after it,
carrying the old name. Only those reads, recorded at the start, are
caught up as they land, and never with a write on the window thread:
the frame takes the thread's written copy, or the new name in memory.
The rename holds, and the old files stay, until they are in.

A first version kept every rename for the session and renamed any
read that named an old name. That would quietly relink a new look that
reused the old name: rename Neon to Glow, make a new Neon, apply it in
another folder, browse back to it, and it named Glow. Nothing outside
the reads that were out is ever touched now.

### When the old files stay

The old name's files are removed only when every sidecar that names
the look has been rewritten. They stay, and the status line says why,
for:

- a sidecar that couldn't be read or written, or wasn't reached
  before a stall or Stop;
- a sidecar out of reach: a frame on an offline root, a followed frame
  not yet read, or every frame when sidecars are off;
- a frame whose edit goes to an archive's copy, written later;
- a preset that couldn't be saved.

So nothing on disk ever names a look that isn't there. At worst, both
names stay on disk.

A rename is refused while an archive copy or move, an import, a
delete or another rename is running. A panic is caught per frame:
outcomes already reached land, and only the frame that panicked counts
as stale.

### Left as is

- A late write for a frame that was sent with no copy replaces
  whatever the window loaded since. That needs the thread stuck inside
  a save on a share that comes back while the user has that frame
  loaded and edited.
- A read thread that panics never clears its in-flight entry, so later
  renames that session wait `WRITE_WAIT` and keep the old files. That
  is safe, and the read queue is already stuck in that case.

The TITLE line inside a `.cube` is a label and isn't changed. It can be
edited by hand without breaking anything.
