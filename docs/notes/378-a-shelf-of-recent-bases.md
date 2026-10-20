# §378. A shelf of recent bases (2026-10-19)

A tester (issue 30) with an eight-thread CPU, an RTX 3080 Ti, and
45 MP R5 files found switching between frames in Develop slow, and
read the cause right: the worker made the base again on every open,
going from A to B and back to A included. His log shows each switch
as a decode of 0.25 s and a develop of 2.5 s, "base made" every
time; the 0.7.0 log shows the same, so nothing had regressed. The
worker held one base and `discard_base` threw it away on every open.
The develop line named only the CA, the fills, the denoiser, and the
sharpen by time, so no log said where a base's 2.2 s went.

## The shelf

On an open of another file, the base in hand goes on a shelf
(`worker/shelf.rs`) instead of away. Opening a file whose base is
on the shelf takes it back, and the develop is then the upload and
the sharpen: on this machine 0.22 s against 2.7 s for the return to
a frame, measured on two 45 MP files through a headless window.

The key is the file (its path, its length and modification time,
and on Unix its change time and inode), what its decode says of the
frame (make, model, sensor size, and orientation, so an EXIF tag
edited in place under the same size and time is seen), the edit's
base (`Edit::same_base`: white balance, noise, demosaic, camera, and
lens), and the frame's turn. A file keeps one base per edit, so two
versions of a file each keep theirs. The order on an open is shelve
the base in hand, take the coming one, then bring the shelf under
budget, so the open frame's base is never counted or evicted. As in
the develop itself, a base whose CA ran on the GPU is not taken back
without one.

The budget is a quarter of physical memory past the first 4 GB,
capped at 4 GB, least recently left first: an 8 GB machine keeps one
45 MP base (515 MB as the shelf counts it), 16 GB keeps five, and
32 GB or more keeps seven. A base over the whole budget on its own is
not kept, and leaves the others where they are. The memory is read
by `sysconf` on Unix and `GlobalMemoryStatusEx` on Windows
(`ram.rs`, one more feature of the windows-sys the crate already
has); if it cannot be read, the shelf keeps one base.
`GREYCARD_SHELF_MB` overrides the budget for measuring, read once
when the first open makes the shelf, 0 turning it off and a value
that does not read falling back to the computed one. A cgroup limit
is not seen: a container capped below its host's memory gets the
host's budget.

What goes with a base: the learned denoiser's pair, only when the
base was blended from it (an unused pair would be a gigabyte of
waste), and the retouch's patched picture, since the open clears the
fill cache and every fill would otherwise run its model again;
pictures shared between these are counted once. What is let go: the
GPU's copies (the picture before the sharpen, the patched upload, the
dehaze's reduced copy), released as before and uploaded again on
take-back at the same 0.07 to 0.10 s a fresh upload costs, and the
masks' detail bands, rebuilt beside the develop and as large as the
base. Never shelved: a base standing in for a missing or failed
learned denoiser, since the model may be there by the next open, and
a picture's (TIFF or JPEG) base, which is its decode.

Of the places that discarded the base, the lens database change
empties the whole shelf, since the lens is baked in; an open shelves;
the two panic paths drop the base in hand and leave the shelf, whose
bases were finished before they were shelved and are touched by no
job. The shelf is made by the first open, so a command-line export,
which never opens a frame, has none and logs nothing of it. The
shelf lives on the worker's thread alone; a shelved picture is only
read, never written in place.

## The stages in the log

The develop line now names each stage of a fresh base, `name 0.00 s`:
`prepare` (less the CA, which has its own part), `demosaic` (with the
engine's denoise when on), `finish`, `lens`, `guide`, `retouch`, and
`upload` on the GPU path or `halves` on the CPU's. The worker calls
`prepare_with`, `demosaic_prepared`, and `finish` itself, which is
what `develop_with` did; core's `develop/timing.rs` is an ignored
measurement, not something the app runs. A taken-back base reads
"base from the shelf" with its upload. Nothing in the window says
any of this: the user's rule from today is that diagnostics go to
the log.

A fresh 45 MP base here: CA on the GPU 0.10 s, prepare 0.19 s,
demosaic 1.14 to 1.19 s, finish 0.07 s, lens 0.69 to 0.77 s, guide
0.02 s, upload 0.07 to 0.10 s, 2.4 to 2.5 s in all, the tester's
number. The demosaic and the lens correction are most of it, which
is what a next step against the first open itself would take on.
With `--cpu-ops` a fresh develop is 3.2 to 3.4 s and the return
0.9 s, almost all the CPU sharpen.

## Verified

- Unit tests: shelved and taken back for the same edit base and
  turn, including when only post-base settings changed; not taken
  back when the base, the turn, the file, its stat, its change time
  or inode, or the decoded orientation differs, nor a GPU-CA base
  without a GPU; one base per version, the newer duplicate winning;
  LRU eviction, the one-base fallback, the oversize newcomer leaving
  the others; the open frame's base never evicted; a lens database
  change emptying the shelf and leaving an unmade one unmade; the
  learned pair kept only when used and counted once, stand-in and
  picture bases never shelved, the patched picture kept; the budget
  formula and the variable's readings; the log line's text for a
  made, kept, and shelved base.
- End to end through `develop_job` on the CPU: a taken-back base and
  its picture equal a base made fresh for the same file and edit, bit
  for bit, and say "from the shelf" once. On the GPU: a GPU-CA base
  comes back with its device copies gone, its develop uploads again,
  and the viewport texture read back equals a fresh GPU develop's;
  passes on NVIDIA and on lavapipe under §221's recipe.
- Release build, headless weston, two 45 MP files, A then B then A:
  the key matched on the real files and the return read "base from
  the shelf, upload 0.10 s, sharpen on the GPU, total 0.22 s".
- A fresh Opus review found the logic sound, and its round fixed the
  oversize newcomer, the key's blindness to an in-place tag edit, the
  export's needless shelf line, and the missing tests above.

## Not verified

- The Windows and Mac memory reads were not run on those machines;
  the Windows path compiles and passes clippy for the GNU target.
- Peak resident memory of a session with a full shelf.
- The learned path's stage times and its pair on the shelf, with a
  real model.
- A DCP or lens profile the edit refers to changing on disk under a
  shelved base; only the raw's own identity is checked.
- Reopening the same file in another version whose base alone is
  over the budget (a learned-denoiser base on an 8 GB machine, or a
  frame near 100 MP) rebuilds it, since the shelf refuses such a base
  and the base in hand goes through the shelf. No worse than before,
  and the hand-back without the shelf is a small later change.
- The prefetch of the neighbors onto the shelf, which is the next
  step and the roadmap's.
