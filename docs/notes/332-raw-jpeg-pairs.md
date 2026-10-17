# §332. RAW+JPEG pairs (2026-10-16)

A camera set to RAW+JPEG writes `IMG_1234.CR3` and `IMG_1234.JPG` for
one press, and the browser listed both: every frame twice in the grid
and the strip, two ratings to keep in step, and a rejects move that
left the camera's JPEG behind. The roadmap item asked for the JPEG to
be the raw's companion, as Lightroom treats it by default, with a
setting to list it apart as Lightroom's "treat JPEG files next to raw
files as separate photos" does. The kind filter (§326) sits beside
this: it narrows a mixed folder to its raws; this stops the pair being
two frames at all.

**The rule.** One function writes it, in `greycard_library::pairs`: a
JPEG (`.jpg` or `.jpeg`, any case) is a companion when a raw
(`decode::is_raw_path`) of the same stem is in the same folder. The
stems are compared without regard to case, as the import's scan and
the rejects move's short-XMP check already compared them, and as the
disks of two of the three platforms match names. A JPEG with no raw
beside it is a frame of its own, as today. Every raw is a frame: two
raws of one stem, a `.dng` made from the `.cr3` beside it, are two
frames. The JPEG goes to one of them, never both: a JPEG paired with
each would be taken by a move of either and left without the other.
We give it to the camera's own format over a DNG, then the order of
`RAW_EXTENSIONS`, then the name (`pairs::raw_rank`). A DNG beside a
camera's raw of its name was made from it later, by a converter or our
own writer, while the JPEG was written with the camera's raw at the
press; a DNG alone beside a JPEG is a camera that writes DNG, and is
the raw. The import's scan took the first raw by path, which put a
`DSCF0001.JPG` with the `DSCF0001.DNG` before the `.RAF`; it asks the
same rank now, so a card imports its pairs as the browser lists them.

The rule lives in the library rather than in core: the index answers
it in SQL, and every consumer that lists a folder (the browser, the
CLI, the headless export) already has the library. `pairs_in` pairs
any list of paths by folder; `pair` splits a listing into frames and
each raw's companions; `companions_on_disk` and `companions_of` read a
raw's folder now, each folder once; `raw_on_disk` finds a JPEG's raw.

**The frame is the raw.** `files::list` lists a folder through `pair`,
so the companion is not a frame in the grid, the strip, the lightbox,
culling, collections or the export queue, all of which walk the
window's files. A JPEG named on its own, on the command line or from
the desktop, opens its raw, as a `.gcd` named on its own does; two
paths of one pair given at once are its one frame. The window keeps
each listed raw's companions in `State::companions`, by the raw's
path, from the same read that lists the frames: a folder's from its
listing, files given from their folders read once each, and a view of
the index from the index's own rows of those folders
(`Library::companions_in`), so a folder under an offline root still
marks its pairs and none is read for them. Every read of the list
replaces the map, so a JPEG copied in after its raw marks the raw on
the next merge and stays out of the list; `State::move_file`, the one
place a listed path is written, carries a raw's entry to its new path.

The companion has no sidecar of its own: the raw's `.gcd` is the
pair's, and so its rating, flag, label and keywords. We checked what
is written beside a JPEG today: only what the window writes for a
frame it holds, its `.gcd` and with the setting on its XMP. A
companion is never a frame, so nothing is ever written beside it. The
raw's XMP takes the long name, `IMG_1234.CR3.xmp`, because the short
`IMG_1234.xmp` is the raw's only when no other picture shares its
stem, which was already the rule; darktable reads the long name.

**The index.** Both files keep their rows: the index is a cache of what
is on disk (§72), and a JPEG's row is what a listing apart reads and
what an archive copy is found by. Whether a row is a companion is
derived, not stored. The library registers `pairkey`, `pairs::pair_key`
as a deterministic SQLite function beside `filekind`: a file's folder
bytes, a zero byte and its case-folded stem, for a file of the kind
asked, NULL otherwise. `pairs::COMPANION_SQL` says "a JPEG whose key is
among the keys of the raws not missing", one scalar `IN` over a
subquery SQLite gathers once per statement into a list it looks each
JPEG up in. No schema change, nothing to migrate, and an index written
by this build still opens in 0.7.0, since the function is only ever in
a query. A raw found missing frees its JPEG, as its absence from the
folder would.

The first cut compared a row value, `(folder, pairstem(name)) IN
(SELECT folder, pairstem(name) ...)`, and the review timed it on a
synthetic library of 100,000 rows (1,000 folders of 100: half raws,
four in ten companions, one in ten a JPEG alone): every whole-library
listing took 40 to 47 s in a debug build, the CLI's `list` the 47; we
timed the same test in release at 7.7 to 8.7 s, and one folder's
`rows_in_canonical` at 29 ms. The plan gathers the list once, so the
time is in the lookups: SQLite does not look a row value up in it as
it looks up one value. A correlated `EXISTS` against the folder's own
rows, through the folder index, was the review's suggestion, measured
by the review at 0.33 s whole-library in release; but it tests each
JPEG against every row of its folder, the square of a folder's size,
and a card's folder of 2,000 is common.
The one-key list is neither: on the same 100,000 rows in release,
`rows_under_canonical` 100 ms (77 ms with the JPEGs apart, which runs
no test), `query` 98 ms, `folder_counts_under_canonical` 63 ms,
`count_under_canonical` 58 ms (5 ms apart), the Kind facet over the
whole library 76 ms, and one folder's `rows_in_canonical` 22 ms, since
the raws' list is gathered from the whole table each time. The same
100,000 as 50 folders of 2,000: 102, 56 and 100 ms for the rows, the
count and the query, the same as with small folders. The roots pane
counts each root on the window's thread (`roots::recount`), one
`count_under_canonical` a root, so 58 ms a root of 100,000.

`Library::set_jpegs_apart` holds the setting on the connection. Off,
the default, every listing and count leaves companions out: `query`
and `count` (the CLI's `list`), `paths_under`, `rows_under_canonical`
and `rows_in_canonical` (the views of the roots), `count_under`,
`count_in_canonical` and `folder_counts_under_canonical` (the roots'
counts and the tree), and `facet_counts` over the whole library. So
`greycard library list kind:jpeg` lists only the JPEGs that are frames.
A question about rows already chosen answers for those rows:
`ids_passing` and `facet_counts` within ids. The filter bar asks only
about the frames the window lists, which are frames already, and the
Kind row is counted from those paths in hand (§326), so neither needed
a word. A test over a synthetic folder (two fixture DNGs, a raw the
decoder refuses, four JPEGs and a PNG) pins every listing and count
both ways, the companions, the rule in SQL against the rule in hand,
and a raw deleted freeing its JPEG.

**The mark.** A raw with its JPEG beside it carries "+JPEG" with the
marks in the picture's top right corner (edited, exported, in a
collection), on the grid and the strip, in the marks' quiet color: the
pair is a fact about the file, like those, not a judgment like the
badges. It is on the file's own cell only; a version is a look of the
raw and has no JPEG. Under the frame's name in the panel's head a line
says "With the camera's JPEG, IMG_1234.JPG", a property of its own so
the name the export sheet reads stays the file's. A JPEG that is a
frame of the list itself (a collection it was put in while the JPEGs
were listed apart, holding its raw too) is no companion in that list:
the landing of each read leaves it out of the map, so its raw carries
no mark, takes no picture from it, and a move or a delete of the raw
leaves it, the delete and the move both excluding every file the list
holds.

**Moves and deletes.** Whatever moves or deletes the raw takes the
JPEG, asked of the disk at the moment it acts (`delete::companions_of`)
rather than of the map, so a JPEG that arrived since the listing goes
too. The plan of a delete and a move read each folder once for all its
raws, the companions through `pairs::companions_of` and the names the
shared XMP is judged by through `delete::Folders`: a raw at a time,
1,000 raws in a folder of 2,000 files read that folder 1,000 times,
1.4 s on the window's thread against 14 ms once, both in a debug
build. The JPEG's own
sidecars go with it, and so does the short XMP the pair shares
(`delete::shared_xmp`), which is neither file's alone while they are
together and would be nobody's if left. A JPEG of its name already at
the destination keeps the pair whole where it is and names the file,
as a sidecar there does. The delete sheet counts the JPEGs ("3 frames,
2 camera JPEGs and 4 sidecars"), and so do the rejects move, Move back
and the status lines. A JPEG that goes from the disk with its raw goes
from the index with it: `Deleted` carries the companions' paths and
the window's delete forgets their rows beside the raws', where the
first cut forgot only the raws' and the JPEG's row stood as a frame of
every listing of the index until a pass found the file gone. Half a
pair, a JPEG that would not go when its raw went (a permanent delete
refused on it, a rename in a move refused), is said on the status line
as a refusal of the trash is, not only logged.

A JPEG that has a `.gcd` of its own today, rated while the JPEGs were
listed apart or by a build before this one, leaves the list under the
default with that sidecar unread: the raw's `.gcd` is the pair's. A
delete of the raw takes the JPEG's sidecar with it, counted among the
sidecars the sheet names before anything goes. The archive's backup of
chosen frames copies each raw's JPEG with it (a folder's backup copies
every file already). The archive's removal of rejects takes each file
of the local rejects folder on its own, the JPEG among them, since the
local move put both there and each copy on the archive is found by its
own content. No path renames a file but the import, whose companions
already take the raw's new name.

**Culling.** The companion is usually a better picture to cull from
than the raw's embedded preview: the camera's finished picture, full
size, where some raws carry a small one. The loupe's wants and the
develop view's stand-in name it (`cull::Want::picture`), keyed still by
the raw's frame so the delivery lands on its cell; the local preview of
a frame on a network mount is made from what is shown. It is shown
only when it is no smaller than the raw's own (`cull::decode_paired`):
a JPEG within a few percent of the frame's own size (read from the
raw's description, no pixel decoded) is taken at once, since no
preview inside the raw is larger; otherwise the raw's preview is
decoded and the larger of the two shown, so a camera set to a small
JPEG beside a full-size preview culls, and checks focus at 1:1, on the
full one. A JPEG that will not read, gone since the list was read
(moved by hand, or left at an old path when the frame on screen was
followed to its archive copy) or cut off, gives way to the raw's own
preview rather than leaving the loupe empty. Cut off is told by the
file's end: a JPEG still being written from a card reads its header
and decodes, its missing rows grey, and would be shown and kept as
the frame's preview, so it is taken only when it ends with its
end-of-image marker, padding after it allowed (`cull::jpeg_whole`).
The status line says "the camera's JPEG beside the raw, 6000 × 4000",
and "a small camera JPEG beside the raw" when the one shown is under a
full frame, where a raw's own says "the camera JPEG" and a small one
"a small camera preview".

**The setting.** "Treat a JPEG beside its raw as a separate frame" on
the Settings sheet, in a RAW+JPEG section of its own, off by default,
kept in `settings.json` as `jpegs_apart`. Flipping it sets the index
reader's connection, counts the roots and the tree again, and reads
the open list again through the ordinary merge, so the JPEGs come in
or go out as any file does and the frame on screen stays. Apart, a
JPEG is a frame in every way: its own sidecar, its own rating, and a
move of the raw leaves it. The headless `--export` and the CLI's
`library list` read the same key; the CLI reads the editor's switches
through one `editor_setting(key)`, `sidecars_in_folder` for its
Lightroom import and `jpegs_apart` here, from `settings.json` under
`$XDG_CONFIG_HOME` as the editor finds it, each off without one.
Naming a JPEG beside its raw, to open, to `--cull` or to `--export`,
acts on its raw unless apart.

**Export.** The raw is the frame and is what exports; the JPEG is
never a frame, so nothing exports it or touches it, unless apart.

**Tests.** The rule (any case, lone JPEGs, two raws of one stem, a DNG
alone, both JPEG spellings, which files have a stem, the index's key),
the folder read now, a folder listed with pairs and apart, a JPEG
named on its own, the index's listings and counts both ways, the
import's choice, the delete with the shared XMP and apart and its
half-pair words, a delete in the window forgetting the JPEG's row with
the raw's, the rejects move out and back with a name taken and apart,
the loupe's wants naming the JPEG, its decode with its status words,
a smaller or unreadable JPEG giving way, a window opening a folder of
pairs with its marks and name line and the setting flipped both ways,
a JPEG arriving after the open, a list holding a raw and its JPEG both
as frames, and the CLI's reading of the switches. By hand, under weston
on hard links of three raws with a JPEG written beside one and a lone
JPEG: the grid lists four frames, the raw marked +JPEG and the Kind
row Raw 3, JPEG 1; the loupe on the pair, whose written JPEG is 1,200
pixels across beside an R5's full-size preview, shows the raw's own
preview, and the name's line names the JPEG.

**Not here.** Stacks and bursts, the next items. The lightbox's tiles
still show the raw's own preview: a tile is a few hundred pixels, which
any preview inside a raw covers, and the tile's picture is the edit's
wherever the frame has one, so the JPEG beside would be a second
file read a tile for nothing seen. A collection holds what was put in
it: a JPEG added while listed apart stays in its collection after the
setting is turned off. The Lightroom catalog import and the pp3 import
can write a sidecar beside a companion, a catalog listing the camera's
JPEG as a photo of its own, which the window then never reads; we left
the imports as they are, and the follow-up is for an import to map a
companion's row onto its raw's frame. The one-folder queries
(`rows_in_canonical`, `count_in_canonical`) still gather the raws'
keys from the whole table, about 22 ms in release for a folder's view
of a 100,000-row library; the subquery limited to the folder asked
(`pr.folder = ?`) would take that away.
