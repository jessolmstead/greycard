# 190. Delete from disk, to the trash or for good (2026-09-27)

Until now, Move rejects was the only way to take
a frame out of the browser, and nothing was ever deleted. Two deletes
now sit beside Move rejects in the CULLING section:

- **Delete selection…** deletes the chosen frames: the set, or the
  frame on screen when nothing else is chosen. It uses
  `chosen_frames`, the same frames a rating key reaches, so a frame
  the filter hides is never deleted.
- **Delete rejects folder…** deletes the frames in the `rejects`
  folder beside the open folder. It then removes the folder and its
  `.greycard` if they are empty. It works in the folder view only;
  the all-roots view has no single folder to have a rejects folder
  beside it. The folder is the one beside the first file's folder.
  In a folder view opened from several paths, that is whichever came
  first. This is accepted because the sheet names the folder before
  anything goes. A rejects folder that is itself a symlink is refused.

The Delete key opens the first sheet in the loupe, the grid and while
culling (the loupe added the same day; see §196).
On macOS, Backspace does too (`Platform.os`), because the key marked
delete sends Backspace. A key only ever opens the sheet. Nothing is
deleted until the sheet is confirmed.

**What goes.** Each frame goes with:
- the frame file itself;
- its `.gcd`, wherever it is: beside it, under `.greycard`, or both;
- the XMPs that `xmp::paths_of` gives it, which is the same set Move
  rejects carries.

The short `IMG.xmp` counts only when no other picture shares the
stem, so deleting a raw leaves the XMP it shares with its JPEG.
Nothing else in the folder is touched.

`crate::delete::plan` checks every path before anything is deleted,
and `delete` checks each one again just before it goes:
- The file's folder, canonicalized, must be an allowed folder or that
  folder's `.greycard`. For the selection, the allowed folders are
  the chosen frames' own folders and the rejects folders beside them.
  Only those are canonicalized, not every folder in the list: in the
  all-roots view that would be thousands, and a hung mount would
  block. For the rejects delete, the rejects folder alone.
- The path must be a regular file, not a link. If a frame, a sidecar
  or the hidden folder is a symlink, the whole frame is refused. A raw
  never goes without its sidecars, and a sidecar never goes without
  its raw.
- If a frame was deleted by another hand between the sheet and the
  confirm, it counts as gone. Any sidecars it left still go, and its
  tile leaves the list.
- If a folder was swapped for a link after the sheet opened, its
  frames are refused at the confirm. The allowed folders were
  canonicalized when the sheet opened, and the link now resolves
  elsewhere. Refusals made at the confirm are reported in the status
  line with the rest.

**Trash or permanent.** The sheet offers what `delete::offer` says:
- **Move to the trash** alone, on a platform with a trash. All three
  we build for have one through the `trash` crate. It is the default,
  and Enter presses it.
- **Delete permanently** alone, on a platform with no trash. The sheet
  then says the delete cannot be undone.
- Both, when the trash has refused files from this folder earlier in
  the session (`State::trash_refused`). The trash is still the
  default. The permanent delete is a choice the user makes.

The permanent button is red and is never the default. It has no
FocusScope, so a click is the only way to press it. Tab cannot reach
it, and Enter never presses it.

**When the trash refuses.** On Linux, 5.2.9 does not refuse a mount
just because it cannot make `.Trash-$uid` there. It falls back to the
home trash and copies the file. A refusal is rarer than that: a home
trash that cannot be written, a permission error, or a file that
cannot be read to copy.

The trash takes each frame's files in one call, the raw first.
`trash::delete_all` stops at its first error, so a refusal can leave
part of a frame behind: the raw in the trash and a sidecar still
beside where it was. After a refusal, what went is read back from the
disk, and the status line says exactly that. It says "the trash took
A.CR3 but refused A.CR3.gcd (left in .greycard)", not that A.CR3 was
refused, and it says where each sidecar was left. A raw that was
already gone before the confirm is said as "A.CR3 was gone already,
and the trash refused ...", never as taken by the trash. A frame whose
raw went counts as gone.

After a refusal:
- The delete stops there. The frames after it are not tried.
- The frames before it are in the trash. The refused frame, unless
  its raw went, and the untried frames stay where they were.
- The folder is remembered, and the next sheet for it offers the
  permanent delete as well.
- Nothing falls back to a permanent delete. The user decides.

An answer the sheet did not offer counts as a no. That check is made
in Rust against the offer, so a stray callback cannot delete anything
permanently.

**Off the window's thread.** The confirm plans the delete again on the
window's thread; that is only stat calls on the chosen frames. It
then hands the plan to a thread of its own (`greycard delete`) and
says "deleting N frames...". The thread reads each frame's thumbnail
key (a hash of the first 64 KB) and then deletes. The result comes
back through `invoke_from_event_loop`. Only then are the rows
forgotten, the cache entries dropped and the tiles taken out of the
list.

Running it off the window's thread matters because the trash can be
slow: a copy into the home trash from another disk, or on macOS an
`osascript` a call. A second delete is refused while one is out.

The thread's work is wrapped in `catch_unwind`. The trash crate
panics when `CoInitializeEx` fails on Windows. A panic still lands a
result, so the window never stays refusing deletes for the session.
The status line says the delete failed and nothing more was deleted,
the panic message goes to the log, and the frames the disk shows
gone before the panic leave the list.

While a delete is out, `State::deleting` holds the planned paths.
Until it lands:
- Move rejects and a second delete are refused.
- No sidecar of a planned frame is written (`panel::delete::held`,
  checked in `write_sidecar`, `save_edit`, `take_sources` and the
  history's `take_current`). Otherwise a rating pressed during the
  delete would leave an orphan `.gcd` beside a trashed frame. The
  change stays in memory. Other frames write as usual.

It is a fresh thread, not a pool's. On Windows the crate initializes
COM, apartment-threaded, on the first thread that calls it, and it
must not be called from a thread that set COM up the other way.

In a test build, `delete::system_trash` is a panic, and the job is
queued for the test to run and land. No test can reach the user's
trash.

**Afterwards.**
- **Index rows.** A new `Library::forget(paths)` removes the rows
  outright. A removal the watcher notices only marks the row missing,
  so that a move can find it again. After a delete there is no move
  to find. The forget runs on the indexer's thread: the window sends
  `Ask::Forget`, and the thread answers `Told::Forgotten`. A pass in
  progress gives way to it, as it does to a save. It runs after any
  save already queued, so a sidecar written just before the delete
  cannot bring a row back.
- **Cached thumbnails.** `Thumbs::remove(hash, stamp)` removes every
  size and recipe cached for that hash under that file's mtime stamp.
  The hash and stamp are read before the delete. Entries under
  another stamp belong to another copy of the file, and stay.
- **Tiles and pictures.** `panel::cull::drop_files` drops the grid's
  tiles and the culling pictures. It was split out of `move_rejects`,
  and both now use it. If the frame on screen was deleted, the
  nearest frame left takes its place.
- **An unsaved edit.** If the frame on screen is being deleted and
  its save is still pending, the save is written before the delete
  is planned. The edit then goes to the trash with the frame, or
  stays with it if the trash refuses it.

**Runs nobody is at.** `State::deletes_allowed` is false for any run
with `--snapshot`, `--screenshot`, `--export` or a `--time-*` flag. It
is also false in a test's state until the test sets it. Such a run can
open the sheet (`--sheet delete` does, for a snapshot), but it never
deletes, even when the sheet is answered.

**The crate.** `trash` 5.2 (5.2.9 at landing), from
github.com/Byron/trash-rs (crates.io still lists the older
ArturKovacs/trash URL), MIT licensed. Its default features are the
ones it needs: `chrono` writes the DeletionDate that the freedesktop
spec requires, and `coinit_apartmentthreaded` sets up Windows' COM
apartment. What each platform pulls in:
- **Linux and the BSDs:** its own freedesktop implementation, which
  adds `libc`, `once_cell`, `scopeguard` and `urlencoding`. `chrono`
  is already in the tree.
- **macOS:** `objc2` and `objc2-foundation` (for NSFileManager), both
  already in the tree through winit, plus `percent-encoding`.
- **Windows:** `windows` 0.62, the version already in the tree.

When 5.2.9 moves a file into the home trash from another device, it
copies the file and then removes it. The spec allows this ("trash into
the home trash"), so the file still ends up in the trash.

**Tests.** 22, the last eight from two review rounds:
- in `delete.rs`: what the sheet offers; the permanent path; paths
  outside the folder, `..`, and links; a frame gone before the
  confirm; a trash stand-in that refuses a frame; one that takes the
  raw and refuses the sidecar; a raw gone before the confirm whose
  sidecar the trash refuses; a hand-made plan with a path outside
  the folder, and a folder swapped for a link after planning; the
  test build's trash panics; an emptied rejects folder removed;
- in `panel/delete.rs`: a full run with a real indexer; an answer the
  sheet did not offer; a run nobody is at; a folder swapped for a
  link between the sheet and the confirm; a second delete while one
  is out; a thread that panics still landing; a planned frame's
  sidecar not written, and Move rejects waiting, while a delete is
  out; a frame gone before the confirm; the rejects folder; the
  keys;
- in greycard-library: `forget` and `Thumbs::remove`.

**Not done.**
- **The Windows Recycle Bin for a file too large for the bin, or on a
  drive with none.** The crate passes `FOF_WANTNUKEWARNING`, so the
  shell may show its own permanent-delete prompt. Check this on the
  Windows machine.
- **Quitting while a delete runs.** The delete's thread is detached,
  and quitting kills it where it stands. That can split one frame (the
  raw in the trash, its sidecar left), and nothing lands, so the index
  rows of what went wait for the next pass over the folder, which
  marks them missing.

**Left out.**
- **A Delete item in the right-click frame menu.** The menu is shared
  with other branches, and the Delete key already covers it.
- **Disabling Delete rejects folder when there is no rejects folder.**
  The button stays enabled and says so in the status line instead.
  Otherwise the window would have to stat the folder on every rebuild.
- **Asking the trash whether a mount has one.** The crate cannot
  answer that without trying. Support is decided at compile time, and
  a refusal is remembered per folder for the session.
- **An undo inside greycard.** The system trash is the undo.
