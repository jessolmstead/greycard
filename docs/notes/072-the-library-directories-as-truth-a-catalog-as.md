# 72. The library: directories as truth, a catalog as cache (2026-09-18)

The roadmap asked why people swear by Lightroom's catalog and how
to match it. The answer, taken apart, is five things, and they split
cleanly between what must be true on disk and what can be rebuilt:

1. Speed at scale: browsing and filtering a hundred thousand frames
   with instant thumbnails. A cache problem.
2. Metadata at scale: stars, pick and reject flags, color labels,
   keywords, applied to five hundred selected frames in one keystroke.
   Per-image truth, plus bulk operations.
3. Filtering with facets: rating, flag, label, camera, lens, ISO,
   focal length, date, keyword, free text, with counts beside each
   value. An index problem.
4. Virtual structure: collections, smart collections (saved queries),
   stacks, virtual copies. Not derivable from disk, so it needs a
   home that is not the cache.
5. Culling offline: rating a shoot with the drive unplugged, off the
   previews. Falls out of a persistent thumbnail cache keyed by
   content rather than path.

Import from card, maps and faces are secondary. Tethering is its own
item.

What people hate about Lightroom is not the cache under it (a SQLite
file beside a real folder tree, as darktable's `library.db` and
digiKam's are) but that the cache became the only truth: a catalog
file that must be backed up, the exclamation mark on a photo moved
outside the program, one catalog open at a time. The design here
keeps directories as the truth and makes the catalog disposable.

**Three layers.**

*Truth on disk.* The raw and its `.gcd` sidecar (§16). Rating, flag,
label, keywords, title and caption go into the sidecar as a `meta`
section separate from the edit and its history, so an undo never
un-stars a frame. In the sidecar rather than only the database
because it moves with the folder, survives an rsync, and keeps
`--no-sidecars` honest. Keywords are flat strings with slash paths
(`Places/Scotland/Skye`); the hierarchy is derived, never stored.
XMP read and write is a mapping onto this section, for interop with
other tools, and comes later.

*Virtual structure, also truth.* Collections and smart collections
cannot be rebuilt from disk, so they live in one small file of their
own under `~/.local/share/greycard/`, not in the database. A
collection is a list of references; a smart collection is a saved
query. A reference is by content identity, not path, or every move
breaks it: the file's hash, with its last known path as a hint. This
is the rule that keeps the design coherent: the database is
rebuildable from files, sidecars and this one file, and nothing else.

*The index.* SQLite, one row per file: path, size, mtime, content
hash, the EXIF worth filtering on (camera, lens, ISO, aperture,
shutter, focal length, date taken, dimensions, orientation), and the
sidecar's `meta` mirrored in with the sidecar's mtime, so a newer
sidecar always wins and the database is never authoritative.
Thumbnails stay in the cache but keyed by content hash instead of
path, so a moved shoot does not re-render. The database goes under
`~/.local/share`, not `~/.cache`: rebuildable in principle, but at
40 to 110 ms a thumbnail (§35) a full rebuild of a hundred thousand
raws is hours, and cache cleaners wipe `~/.cache`.

**The hard parts, named now.**

- *Move and rename detection.* Hash the first 64 KB plus the file
  size, not the whole file. Unique enough in practice, and what the
  other tools do; whole-file hashing on a large library is a
  non-starter. Lightroom's exclamation mark becomes a "found it here"
  resolution, since the hash matches the moved file.
- *What the library is.* A set of roots the user has added, like
  Lightroom's folder panel. Ctrl+O on a folder (§65) indexes it and
  makes it a root that can later be forgotten. An all-roots view is
  where collections and global search live; the current folder view
  stays as it is.
- *Staying in sync.* Lightroom's "synchronize folder" is a chore
  because it is manual. An inotify watcher on open roots, plus an
  mtime comparison on launch, makes it automatic.
- *Virtual copies* are a list of named versions in the same sidecar,
  each with its own edit and history; a collection entry references
  hash plus version. Stacks are cross-file and belong in the
  collections file. Lower priority.
- *Where the code goes.* A `greycard-library` crate depending on
  `greycard-edit` for sidecar reading and never on the UI, so core
  stays clean under the layout rules. The CLI gets a filterable
  listing, which is the testable surface for the query language
  before the filter bar exists.

**A fork, decided for now.** One global index, rather than a per-root
`.greycard/` directory in the Capture One session style where a
shoot folder carries its own index and collections and is
self-contained when moved to another drive. Per-root is attractive
for portability but makes global search open every root and splits
collections by where they happen to live. Global first; the
hash-based move detection does the portability work. Revisit if a
shoot-as-a-unit workflow turns out to matter.

**The order**, each step useful alone: the `meta` section and rating
keys in the sidecar, with no database at all; then the index and a
filter bar for the current folder, with the thumbnail cache rekeyed
by hash; then roots, the all-library view and move detection; then
collections and smart collections; XMP interop and virtual copies
after that; import from card last. The roadmap carries the pieces
and what each waits on.
