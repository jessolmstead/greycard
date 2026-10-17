# §329. Importing a Lightroom catalog, part 1: the command (2026-10-16)

§79 designed the import as the capstone of the library plan: a command
first, with a dry run that reports before anything is written, then a
sheet in the window. This is the command, `greycard library
import-lightroom <catalog> [--root A=B ...] [--write] [--force] [--set
NAME]`, built in `greycard-library` (`lightroom_catalog.rs`) on the
pieces that now exist: the meta section, versions (§312), the
collections file (§273, §304) and the roots file. The sheet is part 2.

## The schema as we found it

We read four real catalogs, copies of a working photographer's 2026
shoots: one of about 350 images with no virtual copies, one of about
560 with 2 copies, one of about 155 with 1, one of about 280 with 13.
None had a keyword or a collection of its own; each had Lightroom's
six stock smart collections in the "Smart Collections" set, two had a
quick collection, and all had stacks only where a copy was stacked on
its master. The import reads:

- `Adobe_variablesTable`, `Adobe_DBVersion`: the schema's number. Two
  were seen. 1500000 in catalogs whose develop settings say Camera Raw
  18.2 and 18.3 (Lightroom Classic 15.2 and 15.3); 1504001 in catalogs
  at Camera Raw 18.4 and 18.5 (Lightroom Classic 15.4 and 15.5). The
  number moves with the schema, not with every release, so a schema
  covers a run of versions. `KNOWN` holds these two; any other is
  refused with a message naming them.
- `AgLibraryRootFolder` (`absolutePath`, `name`), `AgLibraryFolder`
  (`rootFolder`, `pathFromRoot`, which ends in a slash or is empty),
  `AgLibraryFile` (`folder`, `baseName`, `extension`). A root's path is
  the machine's own: `A:/Photos/...` from a Windows catalog,
  `/Users/<name>/Pictures/...` from a Mac one, forward slashes both.
- `Adobe_images`: `rootFile`, `masterImage` and `copyName` for a
  virtual copy (which shares its master's file), `rating` (a real),
  `pick` (1, 0, -1), `colorLabels` (the label's text).
- `AgLibraryKeyword` (a tree by `parent`, its root a row with no name)
  and `AgLibraryKeywordImage`.
- `AgLibraryIPTC.caption`. The title has no column in these schemas;
  it is `dc:title` in the XMP packet `Adobe_AdditionalMetadata.xmp`
  keeps for each image, four bytes of length and the packet deflated.
  We take only the title from the packet: its `xmp:Rating` was stale
  against the column in the catalogs we read.
- `Adobe_imageDevelopSettings`: `text`, the develop settings as a Lua
  table of the Camera Raw keys (`s = { Exposure2012 = 1.45, ... }`,
  nested tables for the curves, the masks, the healed spots, the
  Enhance filters); `processVersion`; and `hasDevelopAdjustmentsEx`,
  1 for an image developed and -1 for one not, which agreed with the
  history in all but five of some 1,340 images (edits undone). In
  these catalogs the XMP packet carries no `crs:` keys at all, so the
  table is the only form, and §79's small parser is a Lua table
  reader (`lightroom::parse_table`) feeding the mapper's same keys
  (`lightroom::read_table`).
- `Adobe_libraryImageDevelopHistoryStep`, counted per image, for when
  `hasDevelopAdjustmentsEx` is empty: an image with a step past the
  one it arrived with is developed.
- `AgLibraryCollection` (`creationId` says a collection, a smart
  collection, a set, or a book, print, slideshow or web creation;
  `parent` the set; `systemOnly` the quick collection),
  `AgLibraryCollectionImage` in `positionInCollection` order, and
  `AgLibraryCollectionContent` with the smart collection's rules, the
  same Lua form: a list of `{ criteria, operation, value }` and a
  `combine`.
- `AgLibraryFolderStack`, counted.

Faces, publish services, the cloud tables and snapshots are not read.
One with Lightroom's `.lock` beside it is refused, since Lightroom has
it open.

**Write-ahead mode.** All four catalogs are in SQLite's write-ahead
mode: the newest changes sit in `<catalog>-wal` until Lightroom quits.
A plain read-only open of such a database makes `-shm` and `-wal`
beside it, so our first dry runs were not free of side effects, and in
a folder that cannot be written the open fails. So a catalog with no
log, or an empty one, is opened through an `immutable=1` URI, which
takes no lock and makes nothing; one whose log holds changes is opened
read-only with it, since only that reads the log. A catalog in
write-ahead mode with no log beside it is whole if Lightroom quit
before it was copied, and missing the newest changes if it was copied
while Lightroom ran, so the report says to copy the `-wal` with it or
quit Lightroom first, as `--help` does. Any error opening it other
than "not a database" or "no such table" is said as it is, rather than
as "not a Lightroom catalog".

## The mapping

- **Meta, exactly.** Rating, pick and reject, the label by its color's
  name, the keywords as slash paths from the top of the tree
  (`Places/Scotland/Skye`, §72's form), the title and the caption. A
  label whose text is no color (a renamed label set) is left off and
  counted by its text.
- **The develop, approximately.** Through §52's mapper, as one state
  over the frame's starting edit, labeled "Imported from Lightroom", for the
  user to see and redo. What the mapper has no place for is counted by
  name across the catalog. We taught the mapper four names a
  catalog's images carry and a preset's file seldom does: the crop
  (`HasCrop` in an XMP; in the table the edges are there only when
  cropped), the Upright keys, the healed spots (`RetouchAreas`) and
  the Enhance filters (`FilterList`), beside the masks it already
  named.
- **Old processes.** An edit whose `processVersion` is below 6.7 (the
  2003 and 2010 processes) is counted as further off still; the
  mapper reads their older keys as it always did. No separate mapping.
  None of the catalogs had one.
- **Virtual copies.** Each is a version of its file named for the copy,
  started from the defaults and given its own settings as its line's
  "Imported from Lightroom" state. The meta is the file's, so a copy rated,
  flagged or labeled apart from its master keeps the master's, and is
  counted.
- **Collections.** Under a set named for the catalog's file (`--set`
  for another, `--set ""` for none), the catalog's sets below it as
  the slash path: a set "Clients" holding "Cover" in a catalog
  `Shoot.lrcat` is `Shoot/Clients/Cover`. A slash in a set's or a
  collection's own name becomes a dash, since a slash would make a
  set of it. A reference is the file's content hash and its path, and
  a copy's is its version. A member whose file is not found has no
  hash and is left out, counted. A non-empty quick collection (a
  plain collection Lightroom marks as its own) comes as "Quick
  Collection"; a book, print, slideshow or web creation comes as its
  frames, its layout said not carried. A collection of the same name,
  there already or made earlier in the same run, is joined, frames
  added at its end, and not rewritten when nothing is new.
- **Smart collections.** Translated when every rule has a term and the
  rules are all asked together (a filter's terms are): rating with
  any comparison, pick and label by `==` and `!=`, keywords `all`, or
  `any` of one word, the capture date by comparison or a range. The
  filter goes in `Saved.text`, as the bar's text field holds it. The
  scope is the one root when the catalog has one, canonical as the
  roots file keeps it (a scope as typed would not match the editor's
  view of the root, and the collection would quietly cover every
  root), and every root otherwise. Anything else is skipped and named
  with why: a union of several rules, a time counted back from today,
  the edit time, the file format, an empty keyword list, and the
  camera and the lens, which Lightroom names as the file's EXIF does
  ("Canon EOS R5m2") where the index keeps the decoder's cleaned name
  ("EOS R5 Mark II"), so a term on either would match nothing.
  Lightroom's six stock smart collections, by their names and their
  one rule as every new catalog has them, are left out and counted:
  every catalog has them, and a library of many per-shoot catalogs
  would hold six of them for each. Changed ones are the user's and are
  translated or skipped like any other.
- **Roots.** A `--root` names a root by its path, a folder above it
  (Windows paths compared with the case folded), or its name; the
  longest match wins. With none, a root is looked for where the
  catalog says. A mapping is taken when any of the first five files
  under it is there, and the report gives found and missing per root.
  With `--write` the roots taken go into the roots file through
  `Roots::edit`; the editor indexes them at its next launch.

**Where a new sidecar goes.** Where the frame's sidecar is, when it has
one. Else under the folder's `.greycard` when the folder has one, so a
shoot whose sidecars live there keeps them together; else as the
editor's `sidecars_in_folder` setting says, which the command reads
from the editor's settings file; else beside the file. The report says
which, and how many went under `.greycard`.

**One writer each.** Sidecars through `Sidecar::save_in`. The
collections file's types and writer moved from `greycard-ui` to
`greycard_library::collections` (the reference, the two kinds, the
file shapes, `read_folder`, `Collection::write` and `Smart::write`),
and `filter::Saved` with them, since a smart collection's file holds
it; the editor's store keeps its join, heal and forget and writes
through the same functions, and its `Saved::of` and `.filter()` are a
trait, `SavedBar`. The roots through `Roots::edit`, and only when one
of them is not a root already, so a second import leaves the roots
file as it was.

**The report** describes the write, not only the catalog: each count
of what would be carried says how many of its files were found, so a
dry run with a root not mapped reads "297, 0 of them found" rather
than promising 297 states.

**Left alone.** A frame whose sidecar already has history, versions or
meta is not written unless `--force`, and counted in the dry run. A
second import of the same catalog changes nothing; under `--force` the
copies' versions are found again by their names rather than made
twice.

## What the real catalogs said

The dry runs found every file under the mappings given (1,327 of 1,327),
six files already carrying greycard sidecars (three in each of two
shoots, left alone), and 442 developed images, of which the mapper could not carry:
the color noise reduction Lightroom applies by default (405), the
crop (152), masks (160), healed spots (103), the Enhance denoise (37),
the camera-matching profiles by name, and the defringe amounts. Every
smart collection in them was one of Lightroom's stock six, untouched,
so none came across. A write into copies of seven of one shoot's raws,
with a scratch library, wrote six sidecars under the folder's
`.greycard` (the setting on this machine) with the meta, the import
state and two copies as versions, left the seventh (a sidecar already
there) byte for byte, wrote the quick collection and added the root;
run again it wrote nothing, the roots file included. The dry runs,
with the immutable open, left nothing beside the catalog copies.

## Open

- The crop is the edit these catalogs hold most after the light and
  color sliders, and it is not carried. Lightroom's crop edges are in
  the unrotated frame and its angle turns the crop, where ours is a
  rectangle of the leveled plane after the turns; a mapping is a small
  item of its own, with a test against Lightroom's export.
- A smart collection on a camera or a lens is skipped. Carrying it
  wants Lightroom's EXIF names put through the decoder's cleaning
  (`camera_name`) for each camera the library holds, or a facet on the
  raw EXIF name beside the cleaned one.
- The stock six are known by their English names; a catalog made by a
  Lightroom in another language keeps its six, translated or skipped.
- The set named for the catalog is our choice for many catalogs into
  one library; one large catalog wants `--set ""`. The sheet should
  ask.
- A copy's own rating, flag or label is lost to its master's while the
  meta is the file's.
- Only the two schemas seen are known. An older catalog (Lightroom 6,
  Classic 7 to 14) is refused until one is read and its tables checked.
- `colorLabels` holds the label's text in the catalog's language and
  label set; only the English color names are read as colors.
- The quick collection and a creation's frames come as collections;
  whether a creation should is the sheet's question too.
