# greycard-library

The library index: directories stay the truth, this is a rebuildable
cache of them (design record: `docs/notes.md` §72).

One SQLite file, one row a raw or picture: path, size, mtime, a
content hash (BLAKE3 of the first 64 KB and the size), the EXIF a
filter asks about (camera, lens, ISO, focal length, aperture,
shutter, date taken) and the sidecar's meta (rating, flag, label,
keywords) with the sidecar's own size and mtime, so a newer sidecar
always wins. The database lives at `greycard/library.sqlite` under
the platform's data directory (`$XDG_DATA_HOME`, `~/.local/share`,
`~/Library/Application Support`, `%APPDATA%`); `Library::open` takes
any path, and `open_in_memory` is for tests.

Indexing a folder is incremental: a file whose size and mtime the
index holds is not read; a sidecar that changed refreshes the meta
and nothing else; a file under a new path whose hash the index knows,
and whose old path is gone, is a move and keeps its row. Files gone
from a folder are marked missing and kept until `prune_missing`, so a
move is found from either end.

The filter language (`filter` module) is terms separated by spaces,
all of which must pass:

```text
camera:"R6" lens:RF iso>=3200 focal<=35 aperture:f/1.8 shutter<1/60
date:2026-09 rating>=3 flag:pick label!=red keyword:wedding
name:IMG_00 folder:/shoots missing:yes harbor
```

A bare word is searched in the file's name and its keywords. `:` on
a text field is contains, `=` the whole value, `!=` not it; numbers
take `< <= > >=` and may carry their units; a date is a prefix and
every operator compares that much of the file's date. The parser
refuses what it cannot answer rather than matching nothing.

```rust
let mut lib = greycard_library::Library::open_user()?;
let report = lib.index_folder(Path::new("/shoots/skye"), &mut |p| {
    eprintln!("{} of {}", p.done, p.total);
})?;
let picks = lib.query(&greycard_library::Filter::parse("flag:pick rating>=3")?)?;
let same = lib.by_hash(&picks[0].hash)?;
let one = lib.by_path(Path::new("/shoots/skye/IMG_0001.CR3"))?;
```

The CLI's `greycard library index DIR` and `greycard library list
FILTER...` are the thin surface over it. This crate depends on
`greycard-edit` for the sidecar and `greycard-core` for the probe,
and never on a UI.

GPL-3.0-or-later, as the rest of greycard.
