# 188. The roots' watcher off the window's thread, and not over a network mount (2026-09-27)

Adding `/mnt/archive/Photos`, a share on the NAS, froze the editor for
18 s. The log had "library: added /mnt/archive/Photos" at 13.874 s and
"library: watching 1 root(s), set up in 18084 ms" at 31.959 s, both on
the window's thread. This is the first of §187's leftovers, and it took a review round to
get the remove and the timer right; both are below as they ended up.

**What it was.** `watch`, called by `add`, `remove` and `start`, set
the watcher up on the window's thread. A recursive inotify watch is
not one call. notify walks every folder under the root and registers
each on its own event loop's thread, while the caller waits for the
answer. The shim counted four calls a folder (a `read_dir`, two stats,
an `inotify_add_watch`), 8,167 for a tree of 2,041 folders. Over a
network each is a round trip. `start` and `watch` also looked at every
root with a `read_dir` first, so a root that did not answer held the
window for as long as it did not.

On Linux and macOS the watch was also no use there. inotify, and
FSEvents on a network volume, fire for what this machine does through
the mount, never for what the server or another machine does. On the
share it saw only the editor's own writes, which index their frames
themselves. Windows is different, below.

**Counted.** A tree of 2,041 folders on the local disk, with the shim
(`LD_PRELOAD`) putting 2 ms before every call under it, as a round
trip; a fresh library; the root in `roots.json` and `--all-roots`.
Debug build.
- Before: "watching 1 root(s), set up in 16947 ms" on `main`. The first
  frame was on screen at 17.6 s.
- After: "set up in 16945 ms" on `greycard roots watch`. The launch
  pass was asked for at 0.28 s and the first frame was on screen at
  1.3 s. The shim counted no call under the tree on the window's
  thread over the whole run, against 3 before (`start` and `watch`
  each opening the root, and `remember_last_file`).
- A root that does not answer (the shim holding every call under it)
  beside one that does: before, the window's thread was stuck in
  `start`'s `read_dir` until the hang was let go at 15 s, and nothing
  was drawn. After, the first frame was on screen at 1.2 s, the root
  taken for offline at 3.2 s, the other root passed over and merged by
  3.7 s, and the only thread stuck was the one `greycard root check`.

The first run again after the review's fixes below: "set up in
16940 ms" on `greycard roots watch`, first frame at 1.2 s, no call on
the window's thread.

An add was not measured through a click. It takes the same path as the
launch (`watch` asks for a build and returns), plus the folder's own
look on a thread of its own.

**Now.** `watch` takes a number (`watch_token`), and sends a `Plan`
(the roots as they are, and an `Asker`) to a thread of its own. The
thread:
- looks at each root through §187's `Checks`, so a root that has not
  answered in 3 s is offline and costs no further wait;
- at launch, hands the order of the pass back at once (`Launch`), the
  open folder's root first, made canonical there. The first cut handed
  it back with the watcher, and the launch pass waited 17 s for it;
- sets the roots on a network mount aside (on Linux and macOS), and
  builds the watcher over the rest;
- lands the watcher on the window's thread (`take_built`) through
  `invoke_from_event_loop`.

What a build saw is kept only when its number is still the last asked
for. A watcher built for roots that have changed since is dropped when
it lands, and the newer build puts its own in place. The launch's look
carries the number too: a late one still asks for the launch pass over
the roots still in the list, but does not put its offline roots over a
newer build's.

The watcher already there is kept until the new one lands, on an add
and on a remove alike, so the roots that stay are watched throughout.
The first cut dropped it at once on a remove, so a change in any other
root while the new watcher was built (up to 17 s on the tree above)
was missed, and no pass came after to find it. Keeping it means the
root just taken out is still watched until then. A change there asks
for a pass over a folder no longer under a root; its rows are kept as
§174 keeps them, and the view does not list them. The timer is
restarted at once without the root, which asks the disk nothing.

Dropping a watcher asks notify's thread to stop and does not wait. The
build's thread is not joined on the way out: the watcher and the timer
are dropped before the indexer is stopped, as before, and a build still
out lands on nothing. If no thread can be had, the launch's `starting`
is not left set.

A root new to the list is not watched until its build lands. The pass
the add asks for walks it meanwhile. A change in a folder that pass has
not reached yet is found by it; a change in one it has already passed,
before the watcher lands, waits for the next pass over that folder
(the next launch, or a later change there). At launch the launch pass
covers every root the same way.

`add` makes the folder canonical and looks at it on a thread of its
own too (`Roots::checked`), and adds it on the window's thread
(`Roots::add_checked`), asking the disk nothing more. `Roots::remove`
takes a root out by the path the list has, without making it canonical,
so a root on a share that has stopped answering can still be taken
out. The chips always pass that path. A path that is not in the list
as it is written is still made canonical, on the window's thread.

What still asks the disk on the window's thread here: that fallback in
`Roots::remove`, and `open_folder_to_add` (the roots' row, "Add this
folder"), which makes the open folder canonical when no read has kept
its canonical form yet (§187).

**Which mounts are network mounts** (`mounts.rs`). On Linux, the mount
table: the longest mount point the root is under, by component, and
of two at the same point the later. An automount lists `autofs` first
and the share mounted over it after, which is what `/mnt/archive` is
on this machine (`autofs`, then `cifs`). Taken for a network mount:
- `nfs`, `nfs4`, `cifs`, `smb`, `smb2`, `smb3`, `smbfs`, `ncp`,
  `ncpfs`, `afs`, `coda`, `afpfs`, `webdav`, `davfs`, `ftp`, `sshfs`;
- the cluster filesystems: `ceph`, `glusterfs`, `lustre`, `gpfs`,
  `beegfs`, `orangefs`, `pvfs2`, `moosefs`, `ocfs2`, `gfs2`;
- a virtual machine's view of its host: `9p`, `virtiofs`, `vboxsf`,
  `vmhgfs`, `prl_fs`;
- every `fuse.*` (sshfs, rclone, gvfs, s3fs), plain `fuse`, `macfuse`
  and `osxfuse`. Not `fuseblk`, which is a local disk through ntfs-3g
  or exfat-fuse, nor `fuse.portal`, the desktop's document portal.

A mount point's escapes (`\040` for a space) are read as the kernel
writes them; a `\4xx`, which is not a byte, is left as it is.

A FUSE filesystem that is local (gocryptfs, mergerfs) is taken for
remote as well. It loses its watch and gets the timer instead. That
costs a pass every ten minutes; taking a network mount for local
costs every change made on the server.

On macOS, `statfs`: remote when the mount is not `MNT_LOCAL`, or when
its type is one of the above. Such a root is not watched and is polled,
as on Linux.

On Windows, a UNC path is remote, and a drive letter is remote when
`GetDriveTypeW` says `DRIVE_REMOTE`, which covers a mapped network
drive. There a network root is watched all the same, off the window's
thread like the others, and not polled (`WATCH_REMOTE`).
ReadDirectoryChangesW over SMB is one call a root, not one a folder,
and the server sends its own changes back (SMB's CHANGE_NOTIFY). The
detection is only for the log: "on a network drive; watched, since
Windows forwards the server's changes".

Neither platform's code was compiled here (no Mac toolchain, and the
Windows cross-build stops at SQLite's C compiler). The functions only
Linux uses (`mount_of`, `unescape`), and `network` on Windows, carry
`allow(dead_code)` off their platform, so clippy's `-D warnings` holds
there. CI on the two platforms is the check.

**The timer.** On Linux and macOS a root on a network mount gets a tree
pass every `network_poll_minutes` (settings.json, 10 by default, 0 for
never; the minutes are multiplied saturating), on a thread of its own
(`Poll`). This is the roadmap's polling fallback. At each tick the
roots are looked at through `Checks`, and one that does not answer in
3 s is left for the next time. The rest go to the indexer as
`Ask::Poll`, which queues a `Background::Poll(root)` tree pass:
- at the back, behind the launch pass's roots, so a tick never holds
  up a local root's launch pass. The watcher's changes still go ahead
  of it;
- not at all when a pass over the whole root is already queued: the
  launch pass's, one stopped partway (a stopped pass goes back into the
  queue, and the queue is only read between passes, so one under way
  is in it), or the last tick's.

The first cut sent each tick as a watcher's `Change::Tree(root)`, which
goes ahead of every launch pass and was not checked against the
launch's own. A tick during the first walk of a large share put a
second walk at the front. That walk took up the launch walk's stopped
state and finished it, and then the launch job walked the share again
from the start. Every later tick also went ahead of the local roots'
launch passes.

It is an mtime pass, so a file whose size and mtime the index holds is
not read, but every folder is listed and every file statted. On a
share of 200 GB that is the launch pass's walk again every ten
minutes, which is why it can be set. A local root is never passed over
on a timer.

The timer starts when a build lands with some root on a network mount,
and not otherwise. On a remove it is started again at once without the
root taken out. After an add, the timer already there keeps running
over the roots it had until the new build lands and starts one over
the new set. It is dropped on the way out before the indexer. Each
network root is said in the log once a session: "not watching ...: it
is on cifs, a network filesystem, where a watch sees only this
machine's changes; it is passed over at launch and every 10 min".

**Tests.** The classification over a canned mount table: the longest
point by component, the automount, `\040` in a mount point, `\477` left
alone, `fuseblk`, and this machine's own table read. A watcher built
for roots that changed since is dropped, landing after the newer one. A
root taken out during a build: the old watcher stays until the new one
lands, the stale build is dropped, and the new one watches the rest. A
late launch look asks for the pass over the roots still there and
leaves a newer build's offline set alone. A root added is watched when
its build lands, and a folder that is not there is refused with no
build. A root said to be on `nfs4` is not watched and is polled (on
Windows, watched and not polled); taking it out stops its timer at once;
with the timer off there is none, and a local root is never polled. The
indexer's queue puts a tick behind the launch passes and skips one
while the root's launch pass or last tick is queued. The timer's body
runs on a counter for a clock and passes over only the roots that
answer; its thread asks, and stops when dropped.

Every root in these tests is said to be local or remote by the test, so
a temporary directory on NFS cannot change what they see. `send_build`
and `send_check` are swapped out under `cfg(test)`: the builds are
queued and taken in place, and an add's look runs in place. So the
thread and the `invoke_from_event_loop` landing are exercised only by
running the editor, as in the runs above.

**Not done.**
- A network mount below a local root (the root `~/Pictures`, a share
  mounted at `~/Pictures/NAS`) is still watched: notify walks it
  recursively with the rest, a round trip a folder, and inotify sees
  only this machine's changes there.
- The timer has no field in the Settings sheet; settings.json only.
- The indexer's own thread still waits on a root that answers the 3 s
  look and then hangs in the middle of a walk (§187). The timer makes
  such walks happen every ten minutes rather than once a launch.
- A folder's own open still reads its sidecars on the window's thread
  (§187).
- The macOS and Windows code is written and not compiled here.
