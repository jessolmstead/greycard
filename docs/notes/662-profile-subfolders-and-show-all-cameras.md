# §662. Profile subfolders, and Show all cameras (2026-11-12)

## What changed

The camera profile folder is read with its subfolders, so Adobe's
`CameraProfiles` tree (Camera/<model>/..., Adobe Standard/...) can be
copied or linked in whole. A switch at the foot of the CAMERA PROFILE
list, **Show all cameras**, lists every profile grouped by the camera
its header names: the open frame's camera first (under each name it is
sold by, so aliases still count), open and marked "this camera"; the
others by name, folded with their counts; profiles naming no camera
last, under no heading. Off, the list is as before.

## The walk and its bounds

Breadth first, folders in name order, so a cap cuts the deepest and
latest first and the cut is the same on every run. Bounds: 8 folders
deep (Adobe's tree is 2), 50,000 files and folders looked at, 20,000
profiles kept; past a cap the walk stops, logs it, and lists what it
has. Links are followed, since linking Adobe's folder in is the common
way to bring it, but every folder is checked by its canonical path, so
a link back up or two links to one place are walked once. Hidden
folders are skipped (a trash folder holds deleted profiles). A folder
that will not read, an entry that will not read, and a file name an
edit could not hold (not UTF-8, or a `\`) are logged and passed over.

Each file costs a header read, and only once while its length and
clock stand: `Dcp::info` now reads the first 4 KiB, the directory, and
the four values it answers with, never the tables (a sparse buffer the
file's length, so every bounds check sees the real length and the
answer is the whole file's, errors included). The headers are kept by
path, so a listing of a tree already read is the walk and a stat per
file. The listing still runs where it did, on the window's thread at
each open and as the section opens; a thread at start reads the
headers ahead so the first open of a large tree does not read them.

Timings, 3,000 profiles in 300 folders, each with a 90x16x16 look
table (830 MB), NVMe, files out of the page cache: release build,
first listing 568 ms against 981 ms reading each file whole; every
listing after 18 ms. Debug build: first 395 ms cold, 42 ms warm; after
20 ms. 800 cameras with every camera shown: rows built and drawn in 26
ms.

## How a profile is named now

An edit named a profile by its file name without the extension. It
now names it by its path under the folder, folders joined by `/` on
every system: "Camera/Canon EOS R5/Adobe Standard". A file at the top
is still its bare file name, so every sidecar and preset written
before reads as it did. Two files of one name in two folders are two
names. `path_in` takes a name of plain parts only (no empty part, no
`.` or `..`, no `\`, no root, no drive). The develop key reads the
named file through the same path. The command line's
`--camera-profile` takes a path when it ends in `.dcp` or is a file
there, else a name in the folder; a value that is neither says so
("no such file", or no profile of that name in the folder).

Profile rows are named to a screen reader by the label they show. Each
listing logs, at info, how many profiles the camera filter hid, so an
alias mismatch shows in a default log; each one's names are at debug.

An edit naming a bare file that has since moved into a subfolder does
not follow it: it shows as missing, as any profile not there does.

A profile file whose extension is not lowercase (`Real.DCP`) is listed
as `Real` and found by that name: when no `.dcp` file of the name is
there, the lookup takes, of the regular files of that stem whose
extension is `.dcp` in any case, the first by file name, which is the
one the listing shows first. A folder named like a profile is never
taken. That lookup is not cached: a profile named and missing reads its
folder at each develop, which we accept for a case no one keeps. The
regular-file check and the open are two steps, so a file swapped for a
pipe between them is not caught; we leave that race too. Only regular files are profiles, after any
link: a link to a device or a pipe named `.dcp` is passed over in the walk and refused by
both readers before they open it, and each read is bounded by the
file's length.

A profile beyond the walk's caps is not listed, so an edit naming it
shows it "(missing)" on the panel, yet the develop, which reads the
named file directly, still applies it. We leave that edge: a folder
past the caps is not one anyone has.

## The switch

A view of the list, kept in the settings file as
`profiles_all_cameras` the way the filter bar's switch is: written as
it flips, read at start. Not in the edit or the sidecar; flipping it
changes no edit and asks for no develop. A camera's group folds and
opens for the session, as the look list's do; the group holding the
chosen profile is always open. With the switch off, a profile the edit
names is listed and chosen whatever camera it was made for, and the
existing "Made for ..." line says so. Choosing another camera's profile
applies it as any profile is, and is logged.

## Length

The app has no virtualized list. The profile list keeps to twelve
rows' height and scrolls inside, as the folder tree does, and other
cameras' groups start folded, so an Adobe folder is some hundreds of
heading rows, not thousands of profiles.

## Left

The Adobe lens profiles (`.lcp`) beside the camera profiles are still
read from the folder's top only. The keys the index was made under
hash the folder's top-level files only; a profile replaced in a
subfolder still changes the key of every edit that names it.
