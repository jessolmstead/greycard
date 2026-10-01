# 215. The rest of the network off the window's thread, and a cap for the previews (2026-09-30)

The roadmap's line (Next 0.4.0), the leftovers of §187 and §188: the
indexer's passes wait on a root that answers the 3 s look and then hangs
mid-walk, and every root's reports stop until it answers; a folder's own
open still reads on the window's thread; `Roots::remove` on a path not in
the list and "Add this folder" on a folder no read has seen still make it
canonical there; a network mount below a local root is watched
recursively; and `network_poll_minutes` has no field in the Settings
sheet. With it, §210's open question: the previews' cap.

**The indexer's passes**

**What it was.** The indexer's one thread ran every pass itself. A tree
pass over a share that answered the launch look and then stopped
answering in the middle of a walk (a file's head read, a stat) held that
thread for as long as the share was gone. No other root was passed over,
no save's row was written, and no report came, for any root. §188's timer
made such walks happen every ten minutes.

**Now.** The passes run on a lane: a thread with a connection of its own
to the library, which the indexer's thread hands each pass to and waits
on. It waits the way §188's look waits:
- before a pass over a root begins (not when one stopped for a save is
  taken up again), the root is looked at with the roots' own look
  (`roots::answers`, the same `Checks`: 3 s, one thread a root at most).
  A root that does not answer is skipped, its rows left as they are;
- while the pass runs, every file begun, every batch written, every
  folder left and every wait on the library's lock is a word back
  (`Word::Beat`). A pass that says nothing for 3 s (`PASS_WAIT`, which is
  `ROOT_WAIT`) is set aside on its lane, and the next pass gets a fresh
  lane. A pass set aside is not abandoned: it goes on alone, and when the
  share answers it lands as any other pass, its report and all (the
  indexer looks at the passes set aside between asks, every 200 ms when
  it has nothing else). Its root is skipped, with no look, until then.

A skip is said: in the log once until the root answers again ("does not
answer; its pass is skipped and its rows kept as they are", or "has said
nothing for 3 s; set aside"), and to the window as `Told::Skipped`, whose
status line ("Archive is not answering: its frames are kept as the index
has them, and it is passed over again later") is said once too. A launch
pass skipped counts as done, so a capture waiting on the roots is let go.
The window's own folder pass goes through the same lane and guard; set
aside, its wait ends with the words in the filter's line, and its report
lands with its generation if it comes back.

The first cut abandoned a quiet pass after 30 s and dropped its thread;
the other roots still waited those 30 s once. Setting it aside instead
costs nothing when the share was only slow (a NAS's disks waking), so the
wait could come down to the look's 3 s. A wait on the library's lock is
not the share's: the lane's connection has a busy handler
(`Library::set_busy_handler`, rusqlite's 5 s in 10 ms steps) that beats
while it waits, which the locked-library test caught.

The roots each pass is looked at against are the ones the indexer has
been asked to pass over (`Ask::Roots`, `Ask::Poll`, `Ask::LeftOut`); a
watcher's change is looked at by the root it is under, a folder under no
known root not at all (a folder deleted would fail the look and miss its
missing rows). A pass is skipped while one set aside is in its way: in
the tree set aside, or, for a pass over a whole tree, with one set aside
inside it (the review's case: a share's tick set aside, then a pass over
the local root above it, which would have walked the share again). A
mount the tree's walk leaves out (below) is not in its way.

A launch pass set aside is counted done by the word that it was set
aside, and lands, or is taken up again after a save, as a pass over the
root that is not the launch's. The first cut counted it again when it
landed, and a third time if it had stopped for a save on the way, so a
capture of the view let go before the last root was passed over; the
log now says "indexed change" for such a pass rather than "indexed root".
A skipped pass reads nothing of the index again on the window's thread,
since nothing in it moved: a dead share is skipped at every tick.

**The batch's sidecars, read before the lock.** The review found the case
I had called narrow to be the common one. A batch's second phase took
the write lock and then read each file's `.gcd` and XMP (`sidecar_of`:
`Sidecar::find`'s two stats, the XMP's one or two, the reads, one more
stat), and for a file with an XMP's orientation the camera's tag, and a
new file's move looked for again on disk. A re-poll of an indexed archive
is almost all unchanged files, whose first phase is one stat, so about
four in five of the share's round trips were under the lock, and a share
that dropped mid-poll most likely hung there. Set aside holding the lock,
it then held every save's row (`index_file` waits its 5 s and is dropped
with a warning), every delete's (`forget`), and every other root's pass
on a fresh lane, for as long as the share was gone: worse than master,
where the saves at least stayed queued. Now the first phase reads the
sidecar and the XMP, sums up what the row mirrors of them (the meta, the
develop, the turns, the camera's orientation when the XMP asks for it),
and finds a new file's move, and `Looked` carries all of it; the second
phase asks the disk nothing. A save that lands between the phases has
written its own row by `index_file`, which the second phase sees under
the lock (`current`): the row's sidecar is no longer the snapshot's, so
its meta is newer than what the first phase read and is kept. A save
after the commit writes its row itself. A move the first phase found is
checked again in the index (`still_gone`: the row still at its old path
with the hash). Two probes are left under the lock, both for a new file
the first phase took for a move's other end and so did not probe, and
both only when another writer changed things between the phases: a row
it added at the file's path for a file of another size or time, or the
move's row it took, leaving the file a new one. A test counts the disk
calls a pass makes, and those under the lock, over unchanged files, a
sidecar changed, an XMP appearing, a file new and a file renamed: none
under the lock. And in the tests any disk call under the lock outside
`probe` is a panic, so one added to the second phase later fails the
suite rather than slipping past the count.

**Measured, in the editor.** A FUSE passthrough of my own speaking the
kernel's protocol directly and mounted with `fusermount3` as the user
(`target/scratch/fakenas.py`, not checked in), so `fuse.fakenas`: a
network root to the classifier. Its mode file makes it list folders and
hold every file read, then answer the held reads when cleared. Two roots,
the share first and the three sample frames' folder second, a fresh
library, the share holding its reads for 15 s from launch:
- master: both roots indexed at 15.5 and 15.7 s, after the share came
  back; the local root's three frames waited the whole hold;
- this branch: the share's pass set aside at 3.35 s, the local root
  indexed at 3.67 s (0.31 s pass), and at 15.27 s, the share back, "the
  pass over ... came back" and the share's root indexed, 3 added.

**Not done.** See the end: the rows of the window's own saves and
deletes, and the first tick after a share dies, still wait on the
indexer's thread.

**A folder's own open**

**What it was.** §207 had moved a folder's listing and its sidecars off
the window's thread already (`roots::open_listing`, the loading card, a
later open overtaking by `view_generation`); the roadmap's line was older
than that. What was left: the read listed a folder on a dead share with
no bound (the card up until the 60 s give-up), Recently opened's look at
the folder had no bound either ("opening X..." for good), Recently opened
made the folder canonical on the window's thread at every open
(`recent::folder_of`), and `index_open_folder` threw away the read's
canonical forms so `refresh_ids` made each folder canonical again there.

**Now.** The read looks at the folder first with the roots' look
(`answer_of`); no answer in 3 s is "X is not answering; is its drive or
share there? The list is as it was" (not "in 3 s": a look still out from
before answers no at once), the card taken down, the list and
the view left. Recently opened takes the same look on its thread and says
the same. Recently opened and `refresh_ids` take the canonical form the
read made (a folder opened by hand is still made canonical again by every
read of it, so §187's retargeted link holds).

**Not done.** A path the desktop hands over (`open_paths`, the Finder's
double-click) is still listed by `files::list_paths` on the window's
thread, and the launch's own folder before the window exists (§207).

**Paths no read has seen**

`Roots::remove` by a path the list does not have as written, and "Add
this folder" on a folder whose canonical form no read has kept, now ask
nothing of the disk on the window's thread. The window offers "Add this
folder" from the path as listed when it has no canonical form, and the add
makes it canonical on its thread as before; one that turns out to be under
a root says so ("already in the library, under ..."). A remove by an
unknown spelling sends the path off (`Check::Remove`), and the root goes
when the canonical form comes back and is in the list. Both looks are
bounded by the roots' 3 s: "not added: X: did not answer in 3 s".

**A network mount below a local root**

**What it was.** Only the root's own mount was classified, so a share at
`~/Pictures/NAS` under the root `~/Pictures` was watched with the rest:
notify walked it a round trip a folder, and inotify saw only this
machine's changes there.

**Now.** On Linux every mount point below a local root is classified the
same way (`mounts::remote_under`, from the same table: the type mounted at
each point, the later of two, the outermost share only). Each network one
is left out of the watch (`Watcher::start_except`: a root with one under
it is watched folder by folder, and the left-out folders are never gone
into, a new folder in them neither) and goes into `library.remote`, so the
timer passes over it as over a network root, and the loupe plans its
frames as §210 plans a share's. The root's launch pass still walks it. A
tick for it is skipped while a pass over a tree above it is queued.
macOS and Windows find none (macOS mounts shares under `/Volumes`;
Windows watches shares anyway).

**Checked in the editor.** The FUSE share mounted at `local/NAS` under
the root `local`, the inotify watches read from `/proc/<pid>/fdinfo`:
master held watches on the share's four folders (its device, inodes 1 to
4) beside the root's two; this branch the root's two alone, said "not
watching .../local/NAS, a mount below the root ...", and put it on the
timer. A file written in `local/day` was indexed 40 ms after the event.

**The walks leave it out too.** The review found the watch's leaving out
was not enough: the local root's launch pass walked into the share, and
if the share hung there, the pass was set aside with the root's key, and
every change under the local root after it was skipped and thrown away;
a local root has no timer to bring them back. Now the mounts below the
local roots are told to the indexer (`Ask::LeftOut`) before the launch
pass is asked for (the classification moved ahead of the launch's
order in `Plan::build`), and a local root's walk neither goes into them
nor marks or looks for anything in or under them
(`TreeWalk::leaving_out`). Each is passed over as its own root, by its
tick, so a share there that hangs sets aside its own pass alone. The
second review found two ways the list and a walk could part: an overtaken
build sent its roots and not its mounts, so the launch passes walked with
the old list; and a walk stopped for a save kept the list it began with.
Now the launch's roots and its mounts are one ask (`Indexer::launch`),
added to what is left out rather than put in its place (an overtaken
look knows less than a newer build's `LeftOut`, never more that is wrong
to leave out), and a walk taken up again is given the list as it is now,
the folders it had queued under a mount dropped
(`TreeWalk::leaving_out` on every take-up). A test
holds every file under the share and passes over the local root, a
change in it and its whole tree, all landing, with only the share's tick
set aside.

**An automount not mounted yet.** A systemd `x-systemd.automount` below a
root has only its `autofs` line in the table until something looks into
it, and a look is what mounts it: a watcher's walk, or a pass. The first
cut of this left such a point out of the watch and the walks and did not
poll it, and so nothing ever looked into it: it never mounted, every
later launch saw the `autofs` line alone, and an archive mounted that way
under `~/Pictures` dropped out of the library for good, where master's
walk had mounted it. Now the watcher's build mounts it on purpose, with
the roots' look into it on its own thread (`mount_on_purpose`: a listing,
3 s), reads the table again and classifies what it mounted: a share is
left out and polled as any share below a root, a local disk is watched
and walked with its root, and one that does not answer, or mounts
nothing, is left out and not polled until a later build, and said once in
the log.

**The Settings sheet's minutes**

`network_poll_minutes` has a field under NETWORK FOLDERS, below the
thumbnails: "Look every [10] minutes, 0 for never", taken on Enter or
when the sheet closes, a whole number or the field put back, as the cap
fields are. A change restarts the timer at once (`roots::restart_poll`);
`roots::poll_every` is the one place minutes become a duration, saturating.

**The previews' own cap**

**What it was.** §210 made a local preview only while the one cache was
under three quarters of the thumbnails' cap, 300 MB by default: about 480
frames. An archive of 12,000 wants 5.4 GB.

**Now.** `preview_cache_mb` in settings.json, 8192 by default, with its
field beside the thumbnails' cap ("Previews up to [8192] MB, 0 for
none"). One store with a cap a class rather than a second store: an entry
of a long edge of 2048 or more is a preview (`Thumbs::with_previews`),
counted, capped and evicted against the previews' cap, the rest against
the thumbnails'. The size is in every entry's name already, temporary
files' too, so the count on disk splits by name (`split_at`) and needs
nothing new on disk. I chose it over a second `Thumbs` because the second
would have needed a second lock, a second count at startup, a second
handle through the worker, the pool and the loupe, `Thumbs::remove` asked
of both by every delete, and the sheet adding two counts and clearing two
stores; with one store `remove`, `clear` and the count cover both as they
did, and §210's tests change only in how they build the cache. `ROOM`
now applies to the previews' cap: 6 GB of previews at the default, about
13,000 frames. A cap of 0 keeps none, and lowering it evicts the previews
alone at once. The sheet's line says both: "3 thumbnails kept, 0.0 MB of
300 MB. 3 local previews, 1.5 MB of 8192 MB."

**Checked in the editor.** The three samples as a root: with the previews'
cap at 1 MB, two previews (901 KB) and the three thumbnails; at 8192, all
three (1.5 MB). An old settings file with neither key reads 300 and 8192.

**Tests**

- The indexer: a root that does not answer its look is skipped, said
  once, its rows untouched, and the other root's report lands; one that
  hangs mid-walk (a test's hand holds every file under it) is set aside
  after the wait, the other root's report lands while it is still held,
  it is skipped with no look while out, and let go its own report lands
  and the next tick passes over it again; the window's folder pass held
  the same way is said with its generation and its report lands after;
  the window says a skip once and counts a skipped launch pass.
- A folder that does not answer: said, the list, view and card as they
  were, nothing under it read; overtaken by the next open, its words do
  not land; Recently opened says the same. A folder through a link is
  recorded as the read made it.
- A remove by an unseen path changes nothing until the look sent off
  lands, then takes the root out; one that does not answer is said; the
  list's own spelling goes at once. "Add this folder" over an unseen
  folder is offered with no look, added canonical when the look lands,
  said to be covered when it is under a root, and not offered once a
  read has made it canonical; a folder that does not answer is not added.
- The mounts below a root from a canned table (the outermost share, an
  automount, a space in the point, a local disk and the root's own mount
  left out); a left-out folder under a root is not watched, nor a folder
  made in it, while the rest and its new folders are; a share below a
  local root is not watched, is polled, and leaves the timer with its
  root; a tick waits behind the launch pass of the root above it.
- The minutes field takes whole minutes, puts back anything else, and 0
  is never; the previews' cap field the same, the cache's caps following.
- The batch: no disk call under the write lock over unchanged files, a
  changed sidecar, a new XMP, a new file and a rename; a save between the
  phases keeps its meta (the existing test, through the new path). A
  launch pass set aside is counted once over three roots. A share below
  a local root, hung: the root's passes and changes land, the share's
  tick alone is set aside; a walk leaves out the folders it is told and
  marks nothing in them, and one taken up again drops what it had queued
  under a folder left out since. A pass over a tree with one set aside
  inside it waits for it. An automount below a root is looked into by
  the build and classified by the table after (from a canned table: an
  `autofs` line that becomes `nfs4` is a share, polled; one that becomes
  `exfat` is local, watched; one that does not answer is left out).
- The cache: previews and thumbnails each capped by their own, neither
  evicting the other; a removal takes both sizes and the count follows
  each; clear and the total cover both; the count on disk splits by name
  and a seed past one cap evicts that class alone. The previews' room is
  their own cap's: a full thumbnail cap leaves it, three quarters of
  their own takes it, and a cap of 0 keeps none. An old settings file
  reads both defaults.

**Not done**

- A share that answers, but slower than 3 s on a listing, is skipped at
  every pass, as the roots' look already takes it for offline at every
  read.
- The first tick after a share dies waits on the indexer's thread for its
  look: 3 s a share root, so N × 3 s for N of them, before the window's
  next save is indexed. Every tick after is skipped at once while the
  look is out.
- The rows of the window's own saves (`index_file`) and deletes
  (`forget`) are written on the indexer's thread itself, unguarded: one
  for a frame on a hung share blocks the indexer there.
- `open_paths` (the desktop's Open With) and the launch's own folder
  still list on the window's thread.
- The mounts below a root are found on Linux only, and only when a watch
  is built: a share or an automount mounted under a root while the editor
  runs is not picked up until the roots change or the next launch.
- An XMP written by another program between a batch's phases: the second
  phase writes the first phase's older meta with its older hash, and the
  next pass over the folder heals it (the hash differs from the file's).
  The editor's own saves are not this case; they write their own row.
- `still_gone` asks the index alone whether a move's row is still the
  move's; it does not look at the disk again under the lock.
- A pass that stopped and was taken up again as a tick is not
  deduplicated against a later launch ask for the same root.
- A root added during the session is passed over before its watcher's
  build has found the mounts below it, so that one pass walks into a
  share below the new root; its later passes leave the share out.
- No real NAS: the hangs are the FUSE passthrough's and the tests'.
