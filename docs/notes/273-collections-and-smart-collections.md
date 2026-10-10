# 273. Collections and smart collections (2026-10-10)

§72 settled the shape: collections and smart collections cannot be
rebuilt from disk, so they live in one small file of their own under
`~/.local/share/greycard/`, a collection is a list of references, a
smart collection is a saved query, and a reference is by content
identity with the last known path as a hint. The roadmap held three
calls to settle before it is built; they are settled here.

## 1. Two identical files sharing a hash

The hash is the first 64 KB and the size (§72), so a shoot and its
selects folder hold the same hash twice, and the index already lives
with that: the archive tables key on hash plus copy plus frame for
exactly this case (§243).

**A collection holds a picture, not a path.** Membership is
by hash. Every local frame of that hash shows the collection's badge,
because it is the same picture. When the collection is opened, each
hash shows once: the frame at the hinted path if it is in reach, else
the first local frame of the hash the index knows. Adding the selects
copy of a frame already in the collection adds nothing and moves the
hint to it. This is what "survives a move" means in practice: the
hint is advice, the hash is the reference.

## 2. A file whose hash changes when another tool rewrites it

A metadata write by another tool (exiftool, a camera maker's utility)
changes the head and usually the size, so the hash changes and the
index sees a new file at a known path. The sidecar beside it does
not change, so the edit survives already; the collection entry would
not.

**Healed by the hint.** When a reference's hash is not in the
index but a file stands at the hinted path, the entry takes that
file's hash and keeps the path. The index already notices the case
(same path, new size and mtime, a sidecar beside it, hash unknown);
when it does, it rewrites every reference to the old hash in the
collections file in the same pass, so the heal happens even before the
collection is opened. A reference whose hash is gone and whose hint
has nothing at it is kept and shown as missing, as a moved shoot is
until it is found, never dropped. This is Lightroom's behavior too,
and the alternative, a stable id written into the sidecar, would make a
sidecar for every frame added to a collection and still break when a
tool rewrites the sidecar.

## 3. Whether a collection keeps an order of its own

**Yes, and the smart ones do not.** A collection's list is
its order; a frame is added at the end, and dragging in the grid
reorders the list. The lightbox is for judging a set whole, and a set
laid out by hand (the cover first, the pairs beside each other) is what
that judging needs; capture order is one of the grid's sorts, applied
as a view without rewriting the list. A smart collection is a query
and has no list, so it sorts as the grid sorts.

## What they are

A collection is a list filled by hand: the frames going to a client,
the cover candidates, a portfolio gathered from six shoots. It spans
folders, a frame can be in several, and taking a frame out of it
touches no file. A smart collection is a saved search: the filter
bar's query (§133, §168) given a name and kept in the sidebar, so
rating a frame to four stars puts it in and lowering it takes it out.
The lightbox judges a set whole, and the set is usually a hand-picked
one, so collections come first; smart collections are nearly free once
the file exists.

## The files

One directory, one file a collection, under
`~/.local/share/greycard/collections/`:

- `<id>.json`, a collection: its name, the path of sets it sits in
  (`Clients/2026`), its ordered references (hash, hint path, and the
  virtual copy's version once those exist) and the time it was made.
- `<id>.smart.json`, a smart collection: its name, its sets, the
  filter in the filter bar's own text form (`Filter::parse` reads it
  back) and a root scope, this root or all.

Sizes are not the reason for the split; a reference is a hash and a
hint, some 200 bytes, so a library of five hundred collections and a
hundred thousand references is 20 MB that serde reads in a fraction of
a second. Write amplification is: one file for everything would be
rewritten, through the temporary name and rename every file of ours
goes through, on every add to any collection, and that is a 20 MB
write a keypress once a library is large. One file a collection makes
an add a small write, keeps each collection readable and diffable, lets
a backup tool copy it, and gives the later carry across machines (on
the roadmap) one file to move, as the sidecars move. The id is made at
random when the collection is created, so a rename never changes the
file name and two machines never collide on one. Deleting a collection
deletes its file. Loading is a directory read at launch.

A single enormous collection, two hundred thousand frames in one, is
40 MB rewritten an add. We do not design for it now; if it ever shows
up, that one file can become append-only, a line a change replayed at
load, without touching the others. SQLite would carry it too, but §72
keeps collections out of the database on purpose: the index is a
rebuildable cache that can be deleted, and a collection is truth that
cannot be rebuilt from disk.

The file is per machine. §79's Lightroom import writes into this
directory.
