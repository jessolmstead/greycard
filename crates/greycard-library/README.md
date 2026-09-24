# greycard-library

The library index: directories stay the truth, this is a rebuildable
cache of them (design record: `docs/notes.md` §72).

One SQLite file, one row a raw or picture: path (its exact bytes, so
a name that is not UTF-8 is still its own name), size, mtime, a
content hash (BLAKE3 of the first 64 KB and the size), the EXIF a
filter asks about (camera, lens, ISO, focal length, aperture,
shutter, date taken) and the sidecar's meta (rating, flag, label,
keywords) with a hash of the sidecar's bytes, so a changed sidecar
always wins. The database lives at `greycard/library.sqlite` under
the platform's local data directory (`$XDG_DATA_HOME`,
`~/.local/share`, `~/Library/Application Support`, `%LOCALAPPDATA%`);
`Library::open` takes any path, `open_read_only` is for a listing
beside a running index, and `open_in_memory` is for tests. The file
carries an application id and a schema version: a SQLite file that
is not a library is refused, an older library is rebuilt, a newer
one is refused.

Indexing a folder is incremental: a file whose size and mtime the
index holds is not read; a sidecar whose bytes changed refreshes the
meta and nothing else; a file under a new path whose hash the index
knows, gone from a folder that is still there, is a move and keeps
its row. Files gone from a folder, and folders gone from under a
tree, are marked missing and kept until `prune_missing`, so a move
is found from either end. A folder found empty on disk with rows
under it is a drive not mounted, its mount point left behind, as
likely as a shoot deleted: nothing under it is marked missing or
claimed as a move, and the report names it as unavailable. The
trade-off is deliberate: a folder emptied on purpose keeps its files
listed as present until the folder itself is removed, and a move
that leaves its folder empty shows both paths, with nothing flagging
the stale one, until then. A pass that wrote a row's meta after a
running pass read the sidecar wins: the running pass re-reads the
sidecar under the lock rather than writing back what it read. A folder
that is not there and that the index knows nothing of is an error,
a name mistyped. A tree pass does not go into hidden folders or
follow links to folders, and names the links it skipped. The pass
works in batches of fifty files: the files are stat'd, hashed and
probed with no transaction open, then the batch's rows are written
in one short write transaction, so a listing never waits and a
second writer — another pass, or the editor's `index_file` after a
save — gets in between batches. A pass and a concurrent `index_file`
on the same file, two passes on two folders on a library that is not
there yet, and four openers of a fresh library at once all finish
with every row and no error; each is a test with its own connections
on its own threads.

The filter language (`filter` module) is terms separated by spaces,
all of which must pass:

```text
camera:"R6" lens:RF iso>=3200 focal<=35 aperture:f/1.8 shutter<1/60
date:2026-09 rating>=3 flag:pick label!=red keyword:wedding
name:IMG_00 folder:/shoots missing:yes harbor
```

A bare word is searched in the file's name and its keywords, case
folded with Unicode's rules. `:` on a text field is contains, `=`
the whole value, `!=` not it; numbers take `< <= > >=` and may carry
their units, ISO and rating being whole numbers; a date is a prefix
and every operator compares that much of the file's date; `!=` on
any field leaves out a file that does not say. The parser refuses
what it has no answer for — an unknown field, a rating of two and a
half, a lone quote — rather than matching nothing or everything.

```rust
let mut lib = greycard_library::Library::open_user()?;
let report = lib.index_folder(Path::new("/shoots/skye"), &mut |p| {
    eprintln!("{} of {}", p.done, p.total);
})?;
let picks = lib.query(&greycard_library::Filter::parse("flag:pick rating>=3")?)?;
let same = lib.by_hash(&picks[0].hash)?;
let one = lib.by_path(Path::new("/shoots/skye/IMG_0001.CR3"))?;
```

The CLI is the thin surface over it. Each `list` argument is one
term, so a term with a space or a shell character goes in the
shell's quotes:

```sh
greycard library index ~/shoots --tree
greycard library list camera:R6 'iso>=3200' 'rating>=3' flag:pick keyword:wedding date:2026-09
greycard library list 'lens:RF 24' --paths
```

The `thumbs` module is the browser's thumbnail cache, keyed by the
same content hash as the index's `hash` column, the long edge and the
caller's recipe, one file an entry (`<hash>-<size>-r<recipe>.thumb`)
under `greycard/thumbs/` in the platform's cache directory: a small
header (format, recipe, the caller's stamp, the size, a checksum) and
a JPEG. A damaged entry, or one under another stamp, is a miss and is
removed; past its cap, or when the cap is lowered, the least recently
used entries go, down to nine tenths of it. What it holds is counted
once and kept up after (`known_usage`); `usage_at` does the count
without the cache's lock, for a thread that is not drawing a window.

```rust
let mut thumbs = greycard_library::Thumbs::user(300 << 20)?;
let key = greycard_library::hash_file(&path)?;
let tag = greycard_library::thumbs::Tag { recipe: 1, stamp: 0 };
let thumb = match thumbs.get(&key, 176, tag) {
    Some(t) => t,
    None => { let t = make(&path); thumbs.put(&key, 176, tag, &t)?; t }
};
```

This crate depends on `greycard-edit` for the sidecar and
`greycard-core` for the probe, and never on a UI.

GPL-3.0-or-later, as the rest of greycard.
