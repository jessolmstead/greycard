# §381. Hidden files are no frames (2026-10-20)

macOS leaves a `._NAME` AppleDouble file beside each raw on a share or
a USB drive, under the raw's own extension. The browser listed them,
the indexer made rows of them, and both showed them as unreadable
pictures.

We skip every name that begins with a dot, not only `._`. The rest of
the code already treated a dot as hidden: the index's walk, the roots'
folder listing, and the folder tree pass over dot folders, the
watcher over any path with a dot in it, and the card import over dot
files. The browser and the index were the gap. The dot rule also
catches `.DS_Store`. Nothing of ours is a dot file: a sidecar is
`NAME.CR3.gcd`, and the sidecar folder is a dot folder that was
already skipped. The rule goes by the name alone, so it holds on every
platform, as the files travel with the folder.

`greycard_core::decode::is_hidden` says whether a file's name begins
with a dot. The format predicates, `is_raw_path` and
`is_picture_path`, stay pure: they pick a decoder, and a dot-named
file the user names by hand still opens. The listing predicate,
`is_indexed_path`, refuses a hidden name, and the listing and walk
sites use it: the browser's folder listing, the index's folder pass
and its one-file update, the pairs' read of a folder, the RawTherapee
importer's folders, and the camera match survey. The SQL clause the
companion rule's raws are found by says the same, so the two agree.
The camera match's own `is_apple_double` went, as `is_hidden` covers
it.

An index made before this holds rows for the `._` files. The folder
pass drops them rather than marking them missing, there or not,
missing already or not, since they were never frames; nobody has to
use Forget missing for them. A `._NAME.xmp` was never taken as NAME's
XMP, since the names an XMP is looked for under come from the frame's
own name; a test now holds that.

Verified: unit tests that a folder holding `IMG.CR3`, `._IMG.CR3`,
`._IMG.xmp`, `._IMG.JPG`, and `.DS_Store` lists one frame and no
companion in the browser, makes one row in the index with no companion
by pairs, and gives `IMG.CR3` no XMP and the short XMP name to write;
that old hidden rows go on the next pass; that a hidden file named by
hand still lists; and that `is_hidden` ignores a dot inside a name and
a hidden folder above it. Not verified on a real Mac share or in the
running editor.
