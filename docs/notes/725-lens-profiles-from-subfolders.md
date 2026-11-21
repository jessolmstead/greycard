# §725. Lens profiles from subfolders, their listing held (2026-11-21)

Adobe's lens profiles sit in one folder a maker, and we read only the
top of the profile folder, so the guide told users to copy files out one
by one. Camera profiles have been read from subfolders since §662.

**One walk.** The walker moved from `greycard-edit-io` to
`greycard_core::profile_folder`, since `greycard-lens` depends only on
core: breadth first in name order, 8 folders deep, 50,000 looked at,
20,000 kept, links followed and each folder walked once, hidden folders
passed over. A nested lens profile is named `Maker/Lens.lcp`; a
top-level one keeps its bare name, so old edits read. The develop key
resolves a nested name as the develop does.

**Why the listing is held.** The first build walked and parsed the whole
tree on every open: 12.8 s for 2,000 profiles, before the picture
showed. So the lens listing now gets §708's treatment. It reads each
file's first 16 KiB for the lens, make, and model, keeps them by length
and time, and is held between opens. A look for changes runs on its own
thread, one at a time, and walks only when a folder's clock has moved,
or when the LENS section opens, which forces it. An open lists from what
is held and never waits on the walk; a later list reaches the window as
`Outcome::LensOptions`, shown only for the frame still open, and held
back while Find a lens has focus. A whole file is parsed only for the
chosen row, off the window's thread, with at most 16 kept. On the
2,000-file tree a later open takes about 30 ms.

**The develop does not use the listing.** It reads the chosen file by
its path, so a develop is the same whatever the listing holds.

**Which profiles are for this mount.** With a whole tree linked in,
listing every maker's lenses buries the user's own. A profile is listed
when it was made on this body, or on a body whose mount this body takes
by the lens database (Micro Four Thirds across its makers, L-mount,
Pentax under either name, RF with EF). Where the database cannot place a
body, the same maker decides. The rest are found by typing in Find a
lens, as the database's own off-mount lenses are. Between two profiles
of one lens: the chosen one, then this mount, then the kind of picture,
then this body.

**What changed for a user.** A file whose head reads but whose samples
do not parse is now listed, and choosing it corrects nothing, with the
line an export already says. Sony A profiles on an E body need typing,
as the database does not call the mounts compatible. On the command
line, `--lcp` takes a path, then a name in the profile folder.

**Left.** The picker's list can change under an open popup, since the
ComboBox does not say when it is open. `keys_made_under_in` still hashes
only the top of the profile folder, as it has since §662. Real Adobe
trees were not at hand: the timings are from synthetic files of their
size.
