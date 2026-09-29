# 173. Import from a card (2026-09-25)

The roadmap's v0.7.0 line: copy, rename, apply a preset, a backup copy.
Wave C, an opus author and an opus reviewer, three rounds.
Ctrl+Shift+I, or Import... beside Open folder in the grid's header,
opens the Import sheet; `--import SRC --to DEST` runs one with no window.

**The sheet.** The export sheet's twin, the other way: From (a folder
picker through the same `export::choose_folder` the export uses, the
portal on Linux and rfd elsewhere), a note under it saying what the
source holds ("35 frames, 36 files with the JPEGs and XMPs that go with
their raws; 1 left on the card"), To, Folders and Name patterns, a line
of the tokens, a preview ("066A3439.CR3 becomes
~/Pictures/2024-08-24/066A3439.CR3"), a Preset picker ("None" and the
presets pane's list) and an optional Backup folder with a Clear. The
button reads "Import 35 frames" and is off until there is a frame, a
destination that is there, a backup that is there if one is chosen, and
patterns that make names; a pattern that is not one says so in the
preview's place ("Not a pattern: {lens} is not a token; ..."). The
destination, the patterns, the backup and the preset are remembered in
`settings.json` under `import`, written when an import starts rather
than at the close, so a run that ends badly still remembers where its
frames went; the source is not remembered, since a card is looked for
afresh each time. With nothing remembered the destination is the
desktop's pictures folder (`~/Pictures` when there is no user-dirs
file).

**A remembered folder that is not there.** A backup drive that is not
plugged in leaves `/mnt/backup` a path with nothing behind it, and
making the folder would put the backup on the system disk under the
drive's name, which is worse than no backup. So whether the destination
and the backup are there is looked at on a thread when the sheet opens
and after each choice (a stat of a sleeping network mount can wait),
and one that is not there is said on a line of its own under Backup
("The backup folder is not there: is its drive connected? It is not
made anew here.") with Import off. The same look says when the backup
is on the destination's drive (the device id on Linux and macOS, the
drive letter on Windows): "The backup is on the same drive as the
destination: one failing drive takes both." It is a warning, not a
refusal. `--import` does make the folders its flags name, after the
check below, since a script asked for them by name.

**Finding the card.** The first time the sheet opens with no source, a
thread looks for a `DCIM` folder on each mounted volume: the folders
under `/run/media/$USER`, `/media/$USER` and `/media` on Linux,
`/Volumes` on macOS, and drives C: to Z: on Windows (A: and B: are left
alone: asking a floppy drive that is not there can wait). The first
found, in name order, is the source. The window never waits on it: the
thread posts its answer when it has one and the sheet says "looking for
a card..." meanwhile. A source picked by hand before the answer comes is
not replaced. Reading the source (listing it, pairing the files,
probing the first frame for the preview) is a second thread for the
same reason, keyed by a generation so a slow reading of an old source
says nothing. `GREYCARD_MOUNTS`, a `PATH`-style list, is looked under
instead of the platform's folders, for a test or a snapshot with
nothing mounted.

**Never on the card.** The card is the nearest folder at or above the
source that holds a `DCIM` folder, so a destination beside `DCIM`
(`EOS_DIGITAL/Imported`) is on the card as much as one under it. The
search stops at the mount point the source is on (the device id
changing on the way up; the drive root on Windows) and never looks at
the home folder or above it, since a phone-sync tool's `~/DCIM` would
otherwise make every destination under home "on the card" (the review
showed `~/Downloads/shoot` into `~/Pictures/2026` refused that way).
With no `DCIM`, only the source folder itself is kept from. An earlier
cut also took the whole mount of a source with no `DCIM` for a card,
and refused `/data/incoming` into `/data/2026` and anything on
`/Volumes/PhotoSSD` from itself; photos on a data drive or an external
SSD are common and a camera card without `DCIM` is rare, so that rule
went. The destination and the backup are refused on the card, refused
when they hold the source (`--import pics/dump --to pics` would
otherwise walk the source as part of the destination and find every
frame "already there"), and the two are refused inside each other.
Paths are compared as the file system has them: the nearest ancestor
that is there made canonical (links and `..` resolved, no Windows
`\\?\` prefix, through `dunce`) and the part not there yet put back,
so a destination that does not exist yet, or one spelled through a link
to the card, is compared like any other. The check runs before anything
is made, on the sheet and on the command line alike.

**What goes.** The source is walked whole, hidden files and folders
left out (the Mac's `._` shadows), in path order, which is the camera's
numbering: `100CANON` before `101CANON`, `IMG_0001` before `IMG_0002`.
No link is followed, to a folder or a file, in this walk or the
destination's, and a folder reached twice by its canonical path is
walked once: a link back up a card's tree would list a frame over and
over, and a link out of the destination would make the card's own
files look imported. A camera writes no links. A raw is a frame. A
picture (JPEG, PNG, TIFF) with a raw of the same stem in its folder is
that raw's camera JPEG and goes along with it; an XMP goes with the
frame it names, `IMG.xmp` or `IMG.CR3.xmp`. Two raws of one stem in one
folder (`DSCF0001.RAF` and a `DSCF0001.DNG` made from it) are two
frames, and the companions of that stem go with the first of them in
path order. A companion takes the frame's new stem and keeps what
followed the old one, so `IMG_0001.JPG` beside `IMG_0001.CR3` renamed
`2026-0001.CR3` is `2026-0001.JPG`, and `IMG_0001.CR3.xmp` is
`2026-0001.CR3.xmp`. **A lone JPEG**, one with no raw beside it, is a
frame of its own: a camera set to JPEG only writes nothing else, and the
editor opens JPEGs. Anything else (a video, the camera's own catalog
files, an XMP naming no frame) is left on the card, counted on the sheet
and listed in the log at debug level. Nothing on the card is written,
renamed or deleted.

**Naming** (`naming.rs`, pure and tested, meant for the export's
naming line as well). `{date}` 2026-09-24, `{yyyy}`, `{mm}`, `{dd}`,
`{name}` (the source stem), `{camera}` (make and model as the panel
names them, "Unknown camera" when the file says none) and `{seq}` (the
frame's place in the run from 1, four digits at least), with any
literal text between. The day is the EXIF's; a file whose EXIF has no
date (the two preview JPEGs in the sample folder) takes the local day
of its modification time, through chrono, which rawler already brings.
On a FAT card that time is stored with no zone and read through the
mount's idea of one (Linux's vfat `tz`/`sys_tz`), so a frame with no
EXIF taken near midnight can land on the day either side. An unknown
token or an unclosed brace is an error rather than literal text, since
a typo that quietly became part of every name would only be seen
afterwards. `/` or `\` in the Folders pattern's literal text makes a
level; a token's value never does, a separator in it being refused like
any other character. Each level and each name is made safe for the
strictest file system it could land on, whatever the machine: a card
imported on Linux onto an exFAT drive meets Windows' rules, and a folder
copied to Windows later meets them too. So `<>:"/\|?*` and controls are
`_`, leading spaces and trailing dots and spaces go (Windows drops them,
so two names could meet), a leading dot is `_` (a hidden file on Linux
and macOS, and `.` or `..`), and a reserved device name (CON, PRN, AUX,
NUL, COM0-9, LPT0-9, in any case and before any dot) takes a `_` in
front. A name is kept to 200 bytes, under every file system's 255 with
room for a ` (2)` and the temporary's prefix and suffix; a frame whose
companion has a longer tail than its own extension (`.CR3.xmp` against
`.CR3`) leaves room for it, so the raw and its XMP keep one stem at any
length; and the cut is
made in the stem, never inside a UTF-8 character: the extension (or a
companion's `.CR3.xmp`) is always kept whole after it. The first cut
took the whole name to 200, so a 197-letter pattern with `{seq}` lost
the last digit and the extension and two frames met; the review caught
it. An empty name pattern falls back to the source stem; an empty
Folders pattern puts the files straight into the destination.

**Collisions.** A name is never written over. Two frames of this run
that come to one name (a wrapped counter across two card folders, a
name pattern with no `{seq}`) are told apart as the export's set tells
them apart, ` (2)` before the extension, compared case-blind. A name
already on disk with other bytes (last year's `IMG_0001.CR3`) does the
same: the frame goes in as `IMG_0001 (2).CR3`, its JPEG as `IMG_0001
(2).JPG`, and the finished line says "1 renamed with (2) (the name was
another file's)". A companion whose name is another file's is not
copied and is counted on the finished line ("1 JPEG or XMP not
copied").

**Already imported.** The rule, as the sheet says it: a frame already in
the destination is known by its size and its first and last 64 KiB;
Verify against the card compares every byte, which reads the whole card
again. A frame is not copied again when (a) the destination holds a file
of the same size, the same 64 KiB head hash (`greycard_library::hash_file`,
the index's key) and the same 64 KiB tail hash (`import::hash_tail`, the
last 64 KiB and the size), under any name; or (b) the library index holds
such a file somewhere else, asked by the head hash through `by_hash`, the
row taken only when it is not missing and is not on a camera card (the
card being searched, or any folder with a `DCIM` above it: a two-slot
body's other card, browsed in the editor, is not an import). The
destination is walked once at the start and grouped by size, and each
file's head is hashed at most once, so a same-size file with another
head is never read further (a test holds it). A re-run over a card
already imported reads 128 KiB a frame off the card and 128 KiB of the
destination's copy, not either file whole; the destination's copy is
read whole only to back it up.

Why the tail and not the whole file. The first cut took the head hash's
word for it, and the review showed a card file differing only in its
last byte called "already there"; the second cut read every frame whole,
card and copy, which made a re-run read the whole card (0.46 s from this
drive's cache for 1.2 GB, about 12 minutes for a full 64 GB card at 90
MB/s). The tail catches what a re-run is for: a copy preallocated and
never finished, one cut short, and the review's flipped last byte (its
reproduction is the test). A corruption in the middle of a file that was
read back and verified whole when it was first imported is not what a
re-run looks for; the Verify against the card toggle on the sheet (off
by default, not remembered) and `--verify` on the command line compare
every byte, card and copy, for when it is, and a test holds a flipped
middle byte that the default takes for the frame and `--verify` does
not (the frame then goes in as ` (2)`). Companions already there are
judged the same way.

A frame already there under the destination still gets what it lacks:
its companions are copied if they are missing (or counted if their name
is another file's), and with a backup chosen its files are backed up
from the destination's copy, read back and checked against that copy's
own bytes as they were read (against the card's hash when Verify read
the card whole); the copy is local and the card is not read for it. So
an import without a backup followed by one with a backup leaves a full
backup, which the first cut did not. A frame found only in the library,
outside the destination, is counted and left: its companions and its
backup belong to that copy, which this import does not write beside.
No preset is laid again on a frame already there, since its sidecar may
hold edits made since.

**The copy and the verify.** Each file is written under a temporary name
of its own in its own destination folder, `.NAME.PID-N.greycard-import`
(hidden on Linux and macOS, never a camera's name, this process's id and
a counter so two imports never share one), created new, so a file or a
planted link already at that name is an error rather than something
followed and cut short. The bytes are hashed (BLAKE3 over the whole
file) as they are read off the card; a read that ends short of the
file's size is an error, not a smaller file. The temporary is synced,
then read back whole and hashed again, and only when the two agree is it
given its name. What that read-back checks differs by platform: on
Linux the file's pages are dropped from the page cache after the sync
(`posix_fadvise(POSIX_FADV_DONTNEED)`), and on macOS the read-back is
opened with `F_NOCACHE`, so there the bytes come back from the drive,
though a drive's own cache may still answer, and a file system that
ignores the advice (ZFS keeps its ARC whatever `fadvise` says) answers
from memory; on Windows nothing short of
unbuffered sector-sized reads bypasses the cache, so there the
read-back checks the file system's copy and not the medium. The sheet
says "Each copy is read back and checked against the card's bytes by a
hash before it takes its name", which is true on all three.

The name is given by a rename that never replaces a file: `std::fs::rename`
replaces on every platform, and a check before it leaves a moment in
which a file could appear and be written over. So `renameat2(...,
RENAME_NOREPLACE)` on Linux, `renamex_np(..., RENAME_EXCL)` on macOS,
and `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` on Windows. Linux's
vfat, exFAT and CIFS take `RENAME_NOREPLACE`. Where the file system
refuses the flag (a FUSE mount without rename2, a macOS volume that
refuses `RENAME_EXCL`), a hard link then an unlink, which cannot replace
either; and where it has no hard links too, the check and the rename,
the moment and all. The log says once a run which of them named the
files ("import: files are named by renameat2(RENAME_NOREPLACE)" here).
The temporary is in the destination folder and not a temp directory
because a rename across drives is a copy, and on Windows an error. The
copy keeps the card's modification time, put on it after it has its
name.

Then the backup: each file is copied from the destination (not the card,
which is the slow disk and may be the dying one) to the same folders and
name under the backup, through the same temporary and rename, and for a
frame copied in this run checked against the card's hash, not merely its
own. A backup name that already holds these bytes (judged as a frame
already there is: size, head and tail, or every byte with Verify) is
done; one holding other bytes moves the frame's backup on to ` (2)`,
` (3)`, stopping at the first that is free or holds these bytes, so a
re-run finds the copy it made rather than making another (the review
saw `(2)`, `(3)`, `(4)` over three runs). The companions follow the
frame's backup name, `X (2).CR3.xmp` beside `X (2).CR3`. Any error (a
full disk, a read error, a file gone from the card between the reading
and the copy, a copy that does not read back) removes the temporary and
stops the import, the line naming the file; nothing half-written is left
under a real name. If a file of the destination had been hashed whole
against the card file and the copy's hash differs from that reading,
the card gave two different answers and the import stops.

A process killed mid-file leaves its temporary. Before the first file
lands in a folder, the run removes this program's temporaries there that
were last written more than a day ago by a process that is no longer
running (the name carries the process id; Unix asks with `kill(pid, 0)`,
and on Windows the age alone decides). The card's time used to go on
the temporary before its read-back, which made a file being written look
days old to another import's sweep, and the review had one import remove
another's temporary mid-run; the time goes on the file after it has its
name now. The destination walk skips hidden files, so a temporary is
never taken for an import.

**The preset.** After a frame lands, with a preset chosen, its sidecar
gets the preset as one history step, "Preset: Warm Negative", through
the same `preset_at_start` the `--preset` flag uses (which records with
`record_as` and seeds a raw's learned-denoiser blend from its ISO first,
as a first open would). The sidecar goes where the Settings sheet's
sidecar setting puts it (§153), beside the raw or in the hidden
`.greycard` folder, and `--import` reads the same setting or
`--sidecar-folder`; a run with sidecars off (`--no-sidecars`) lays no
preset. The first cut always wrote to the hidden folder. A preset naming
a camera profile goes through §162's fit check on the import's thread
(`panel::sync::preset_fitted`, the state-free core of
`preset_for_body`, fed the make and model the frame's probe already
read): a profile made for another body is left off that frame, the rest
of the preset laid, and the finished line says "the preset's camera
profile left off 1 frame (made for another camera)". No sidecar is
written when no preset is chosen.

**Progress, stop and the end.** The import runs on its own thread and
posts each frame to the status line, "importing 30 of 35:
DSCF0599.RAF". The grid covers the status plate, so the grid's header
says the same in place of the selection while it runs, and the Import
button becomes Stop import (Stopping... once pressed). The button always
stops it, after the frame in hand (its companions, backup and sidecar
included). Escape stops it only when nothing else takes Escape: in the
grid Escape leaves the grid, in culling it leaves culling, with a set it
lets the set go, and the first cut stopping a long import on the way to
any of those was a card's worth of waiting lost. Closing the window
mid-import stops it the same way: the window stays up saying "finishing
IMG_x.CR3... the window closes once it has landed" and closes itself
once that frame lands; if it has not landed after 60 s (a read hung on a
dying card), the window closes anyway and the log says which frame was
left. When the event loop ends with the import still running (a batch
run's capture), the wait for the frame in hand is bounded the same way,
so no process outlives its window silently. Measured through `--sheet
imported --snapshot` on the fake card with a backup: the frame in hand
landed 20 ms after the window closed (38 ms in the review's run), leaving 27 whole files in each of
the destination and the backup and no temporary. At the end the line
says what landed, how much, how fast, and what did not, as in these from
real runs: "imported 35 frames to ~/m-photos (1196 MB at 648 MB/s), 36
backed up" and, for the run with a backup after one without, "imported
0 frames to ~/b-photos, 36 backed up, 35 already there". When frames
landed, the folder that took the most of them opens in the browser,
where the library's indexer picks it up as it does any folder opened;
the other folders a `{date}` pattern made are indexed when they are
opened. Nothing opens when nothing landed, and when the browser shows
another folder than when the import began (the user moved on during a
long import), it is not taken away from it: the line ends "; open
~/Pictures/2026-09-24 to see them" instead. The grid's header keeps the
finished line for 20 s. The log gets one line for the run (source,
destination, counts, bytes, seconds, MB/s, the backup, the preset, the
profile left off, what was left, the error) at info, or at warning when
anything was stopped, failed, renamed or not copied, so the command line
says it without -v; and one line for each frame found already there,
naming where.

**The command line.** `--import SRC --to DEST [--preset NAME] [--backup
DIR] [--name PATTERN] [--subfolder PATTERN] [--verify]` runs one with no window
and no GPU, prints the finished line on stderr and exits 1 on an error.
The check runs before any folder is made: the first cut made
`DCIM/out` on the card and then refused it. With it, `--preset` means
the preset laid over every frame imported (the store is seeded first,
as the window's first run seeds it, so the film presets are there on a
fresh machine). `--name` defaults to `{name}` and `--subfolder` to none,
so a script gets exactly what it asks for rather than the sheet's
remembered patterns. `--library` names the index it asks about imported
files, the user's by default. `--sheet import` opens the sheet for a
snapshot, with `--import`, `--to`, `--backup`, `--name` and
`--subfolder` filling it rather than running anything; `--to`,
`--backup`, `--name` and `--subfolder` are taken without `--import` too,
so the sheet can be shown with a found card and a chosen destination.
`--sheet imported` presses Import once the source is read, for a
snapshot of the run or of the folder it opens.

**Measured**, release build, on a copy of the sample folder's 33 raws and
2 JPEGs as a fake card (`target/work/card/DCIM/100CANON`, 1.2 GB, plus
an XMP beside one raw and a stand-in video), `--import` with
`--backup`, `--subfolder '{yyyy}/{date}'`, `--name '{date}-{seq}'` and
`--preset 'Warm Negative'`, into fresh folders, no library. The machine
was shared with other agents' builds (load average 10 to 22).

Before the review's changes:

| run | the import's own time | process wall | MB/s |
|---|---|---|---|
| 1 | 2.40 s | not taken | 499 |
| 2 | 4.19 s | 4.22 s | 285 |
| 3 | 1.83 s | 1.84 s | 652 |
| 4 | 2.37 s | 2.38 s | 505 |

After them (the read-back from the drive on Linux, the head hash first,
the rename that never replaces), load average 12:

| run | process wall | MB/s |
|---|---|---|
| 1 | 3.66 s | 328 |
| 2 | 2.42 s | 495 |
| 3 | 1.85 s | 648 |
| re-run into the same folders (whole-file rule) | 0.46 s | 35 already there |

And after the last round (the destination's copy no longer read whole
for a frame already there, the backup names found again), load average
10: fresh runs 2.14 s (562 MB/s), 1.92 s (626 MB/s) and 1.91 s (628
MB/s); a re-run 0.06 s, 35 already there, the backup still 36 files;
with `--verify` 0.55 s.

And with the head-and-tail rule, load average 8: fresh runs 2.76 s (435
MB/s), 2.41 s (497 MB/s) and 2.09 s (575 MB/s); a re-run 0.34 s, 35
already there, having read 35 x 128 KiB (4.5 MB) of the card; the same
re-run with `--verify` 0.50 s, having read all 1.2 GB of it. From this
drive's cache the two differ little; from a card at 90 MB/s the first
is a fraction of a second and the second about 13 s for this card.

MB/s is the card's bytes over the import's own time. 35 frames, 36
files and 36 backup files each run, and 35 sidecars each with the one
labeled step (beside the raws, as this run's settings put them). Every
byte checked outside the editor: the card's SHA-1s before and after are
identical, and the sorted SHA-1s of the destination's 36 files and the
backup's 36 equal the card's. The card here is a file on the same NVMe
drive and was just read, so its reads came from the page cache; each
byte is read, hashed, written, synced and read back twice (destination,
then backup). A real card reads at 90 to 300 MB/s depending on the card
and the reader, and the import will go at the card's speed. The review
measured 670 to 720 MB/s from cache and a 0.72 s re-run on a loaded
machine. A run with no backup followed by one with a backup (the
review's case): the second took 1.7 s and left 36 files in the backup
whose SHA-1s equal the card's. The sheet's time to open with a card: the
card search took under a millisecond over two volumes (a fake mount
folder through `GREYCARD_MOUNTS`; this machine has nothing under
`/run/media`), and the source was listed and its first frame probed 0.00
to 0.05 s after the sheet was asked for. A walk of `~/Pictures` (863
files) for the already-there check took 9 ms. `--to card/DCIM/out` and
`--to card/Imported` each exit 1 with "the destination ... is on the
card (.../card)" and make nothing.

**Tests.** `naming.rs`: every token, literal text between, `{seq}`'s
width a floor; a pattern with no token (and an empty one falling back
to the stem); unknown and unclosed tokens refused; a collision of
`{name}` made unique by `{seq}`; every refused character and the
controls, a separator in a token's value, Windows' device names in any
case and before a dot, trailing dots and spaces, a leading dot, `..`
levels dropped, a long name cut on a character's edge; the review's
197-letter pattern keeping `.CR3` within 200 bytes, a 190-letter one
keeping its `{seq}`, a multibyte stem cut on a character's edge, and a
companion's `.CR3.xmp` kept whole; a day read from either EXIF spelling
and refused when it is not one. `import.rs`, all on small synthetic
files in the temporary directory:
- the card read into frames and companions;
- copy, verify and backup, the card byte for byte unchanged, then a
  second run under another pattern that copies nothing;
- (review 3) a run with no backup, a companion deleted, then a run with
  a backup: the companion put back, all three files backed up, a third
  run doing nothing;
- (review 10) a name that is another file's going in as ` (2)` with its
  JPEG following, the line saying so, and a companion whose name is
  taken counted;
- two frames of one name in one run both landing;
- a cancel landing the frame in hand and nothing after, no temporary;
- a reader that fails partway and a read short of the file's size each
  leaving neither file nor temporary, and a file gone mid-run stopping
  the run with its name;
- the head hash equal and the whole hash different for a pair differing
  in the last byte, and a backup refused against a hash it does not
  have;
- (review 4) a library indexing a copy whose card file differs in its last byte past its
  head, and a row on another card's `DCIM`: both imported, not "already
  there", and the same bytes off any card found;
- (review 5, Unix) a `loop -> ..` in the card listing one frame once, and
  a link from the destination to the whole tree not making the card's
  files "already there";
- the preset as one labeled step beside the raw and in the hidden
  folder as the placement says, and none with sidecars off;
- (review 9) a preset naming a profile made for a Nikon Z 9 laid on an
  EOS R5 frame without its camera profile, the rest applied;
- (review 1, 2) the destination and the backup refused under `DCIM`,
  beside it, as the card itself, through `..` and not there yet, through
  a link to the card, around a source folder, and inside each other;
  `card_root` from a folder under `DCIM`; `resolve` through `..`;
- (review 6) the rename refusing a file that is there and leaving both,
  a stale temporary swept and a fresh one kept, two temporaries for one
  name being two names;
- (review 15) a same-size file with another head not read whole, the
  card file not read whole either;
- the head-and-tail rule: a copy differing only in a middle byte taken
  for the frame by default and not with `verify` (which imports it as
  ` (2)`), and one differing in its last byte never taken;
- a card as a volume with a `DCIM` folder, and two folders on one drive;
- a `DCIM` in a fake home folder not making `Downloads/shoot` a card,
  one on a volume found from under it, and a data drive's `incoming`
  imported into a folder beside it, never into itself or its parent;
- a backup name holding other bytes giving ` (2)` once, with the XMP as
  `IMG_0002 (2).CR3.xmp`, and two re-runs backing up nothing more;
- a 196-letter name with `{seq}` keeping a raw and its `.CR3.xmp` on one
  stem within 200 bytes;
- the sweep removing an old temporary of a process that is gone and
  keeping a fresh one and an old one of a running process, and the card's
  time on the file after its name;
- two frames of one name in one run not counted as "renamed".

`panel/import.rs`: the preview's path, its not-ready cases and a bad
pattern's words; the note's counts; Ctrl+Shift+I opening the sheet on a
window with nothing open, a card found filling the source, a late
reading ignored, a hand-picked source kept, Import off until the
destination has been looked at and is there, Escape closing it;
`--import --to --name` run with no window twice, the second copying
nothing, and `--to` alone taken; (review 1) `--import` refusing
`DCIM/out`, `EOS_DIGITAL/Imported` and a backup on the card with none of
them made, nor the good destination beside them; (review 14) a backup
folder that is not there said on its own line and Import off, then the
same-drive warning; (review 12) Escape leaving the grid with the import
running and stopping it only on the second press; (review 13) a close
with an import running kept up with "finishing IMG_0042.CR3", and the
after-loop wait giving up after its limit on a thread that sleeps past
it.

**Snapshots** (on a headless mutter): the sheet over the grid with
`--import` (`import-sheet.png`), the sheet finding the fake card through
`GREYCARD_MOUNTS` with the destination defaulted to `~/Pictures`
(`import-card-found.png`), a remembered backup that is not there
(`import-backup-missing.png`), the grid mid-run with Stop import and
"importing 30 of 35: DSCF0599.RAF" in its header (`import-running.png`),
and the grid after a three-frame run opened on the destination with its
line (`import-done.png`).

**Left.** A start number for `{seq}`: a second card imported into the
same folder on the same day with `{date}-{seq}` meets the first card's
names, and those frames go in as ` (2)` (said on the line) rather than
continuing from 0036. Ejecting the card afterwards. Choosing which
frames to import (the whole source goes; the already-there check makes
a re-run cost 128 KiB a frame of the card). Thumbnails of the card on the
sheet. The destination walk reads every file's size under the
destination, 9 ms for 863 files and seconds for a hundred thousand, on
the import's thread before the first copy. Card detection is by a
`DCIM` folder, so a card formatted without one, or a phone over MTP
(not a mount point on Linux), is not found and is chosen by hand; such a
card is only kept from as the source folder, so a destination beside
that folder on it is taken. On the Windows and macOS paths (the
drive letters, `/Volumes`, `MoveFileExW`, `renamex_np`, `F_NOCACHE`)
the code compiles only on those machines' CI; it was not run there, and
a cross-check from Linux could not build (no MinGW C compiler for the
bundled SQLite).

**The review.** The first read said not yet, with five blocking faults
reproduced on a fake card: the headless run made the destination
folder on the card before checking it; the check protected the source
folder and not the card, so a destination beside DCIM took 272 MB and
a destination containing the source found every frame "already there"
by walking the card as part of itself; a frame already there skipped
its backup and companions, so a second run with a backup made an empty
one and said success; the library shortcut took the head hash's word
and a file with one byte flipped at its tail was skipped; and both
walks followed symlinks, a loop in the source listing one raw 41
times. Beneath those: a truncating `File::create` and a replacing
rename that two editors could corrupt a verified file through, a
read-back reading the page cache, the sidecar ignoring the placement
setting, Escape anywhere stopping a running import, an unbounded
close-wait, a remembered backup drive recreated on the system disk
when unmounted, and a re-run reading the whole card. The second read
found every fix holding and no new blocking bug, and three more: backup
names multiplying on each re-run, a temporary carrying the card's
mtime so another import's sweep took it for stale, and the
already-there branch still reading the destination copy whole; it also
argued the mount-by-device rule the wrong way round, which is why it
went. Two policy calls were the parent's: the head-and-tail rule for a
re-run with Verify for every byte, and the card as what sits under a
DCIM folder and nothing else. Three rounds, each blocking item's
reproduction now a test.
