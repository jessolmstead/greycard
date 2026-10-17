# §326. A file's kind as a facet and a term (2026-10-16)

A tester asked for a filter for raw files only, no JPEGs, in the grid.
The bar's facets are all read from the EXIF (§160) and none of them
says what the file is, so a folder shot RAW+JPEG could be narrowed by
camera, lens or day but not to its raws. We added the file's kind as
one more facet and one more field of the filter language.

**What a kind is.** `greycard_library::filter::kind_of(path)` reads it
from the extension and nothing else: `raw` for anything
`greycard_core::decode::is_raw_path` takes for a raw, and for a
picture (`picture::is_picture_path`) its format, as `image` names it
in its MIME type, so `.jpg` and `.jpeg` are both `jpeg`, `.tif` and
`.tiff` both `tiff`, and `.png` is `png`. Nothing else is a kind, and
`kind_of` is `None` for it. The two extension lists stay the only
lists, and a raw format the decoder learns is a raw here with no
change. A picture format added to `picture::EXTENSIONS` needs its name
checked here: not every MIME subtype is a word a chip could show
(EXR's is `x-exr`). We considered telling the raws apart (`cr3`,
`nef`) and did not: the tester's question is raw or not, and
per-maker chips are what the Camera row already gives.

**Answered from the path.** The kind needs no row: every frame in the
browser has a path, and the path is the whole answer. So the kind is
answered in hand, as the keyword is from the sidecar, and not by the
index as the EXIF facets are. Its chips are matched in `shows_meta`
through `kind_of` and left out of what the index is asked, and its row
is counted from the open frames' paths. A first build asked the index,
like the camera: a frame passes the index's tests until it has a row,
so with the Raw chip on (restored from the settings, or from
`--filter kind:raw`) every JPEG the indexer had not reached yet stayed
in the grid, and with no index reader at all the chip filtered nothing
and its row was not drawn. Answered in hand, the chip holds every
frame from the moment the folder is listed, and the row is there with
or without an index. That row being up before the index has a row of
its own meant the facets' note, which said "Indexing the folder: x of
y" only while there were no rows, went missing in a mixed folder, and
it is the one place the window shows that progress. The note now
stands before the chips whenever the index is at work, opening or in
error; only "No camera, lens or date in these files' EXIF" still
waits for there to be no rows at all. What the filter bar calls
answered in hand is one predicate, `filter::in_hand`, the keyword and
the kind, and the places that used to set the keyword aside now ask it.

**One reading, every side.** Every facet is grouped and matched by the
same expression, so a chip's count and what pressing it lists cannot
disagree. For the kind that is `kind_of`, in the window and in the
library alike: the library registers it with SQLite as `filekind`, a
deterministic scalar function beside `ulower`, and groups and matches
the facet by `filekind(files.name)`, for the CLI's `list` and the EXIF
rows' counts. The typed term `kind:` is answered in SQL by that
function and on a path in hand by `Term::on_meta` through `kind_of`.
A test over a synthetic mixed library (two DNGs written by the
fixture, three TIFFs, two JPEGs, a PNG) pins the counts, each chip
pressed on the rows and on the paths, the same value typed, and every
`kind:` spelling answered both ways.

No schema change. The kind is a function of a column the index
already holds, so there was nothing to migrate and no pass to run for
it, and an index read by 0.7.0 still opens (the function is only ever
in a query, never in a view, an index or a trigger, which 0.7.0 could
not read).

**The term.** `kind:raw`, `kind=jpeg` and `kind!=raw`, with `:` and
`=` both meaning "is", as on a flag. A value may be the kind's name,
a picture's extension (`jpg`, `tif`), either with an `s` (`raws`),
any case. A raw's own extension is refused: `kind:nef` would read as
every raw, which is not what it says. Anything else is refused with
the kinds listed, raw, jpeg, png or tiff, as `flag:` and `label:`
refuse. `!=` leaves out a file of no kind, the language's rule for a
file that does not say.

**The chips.** `Facet::Kind` sits third in `Facet::ALL`, after Camera
and Lens, so it is the first row of the grid's short row and comes
after the lens in the cull panel's list. A place in `ALL` is only a
code on the window: the settings and a smart collection keep a facet
by its field, so a filter kept by 0.7.0 comes back to the same chips
(a test reads one of that shape). The row is captioned Kind and its
chips read Raw, JPEG, PNG and TIFF, most held first, as cameras are;
the status line says "kind Raw" as the chip does. A kind row with one
chip and it off is left out: in an all-raw folder it would say only
that every frame is a raw, and nothing pressing it could narrow. With
its one chip on it stays, as every chip on does, so it can be pressed
off. Every other facet keeps its rule.

**The command line.** `--filter kind:raw` turns the chip on at once,
before the folder is listed: its value is read as the term reads it
(`kind:jpg` is the `jpeg` chip), which is already the chip's own
value, so it waits for nothing, where `--filter camera:R6` waits for
the index to name the cameras. A value that is no kind is left in the
text field, where the parse refuses it and the log says why.
`kind!=raw` goes to the field as a term, as `iso!=100` does.

Not here: pairing a raw with the JPEG shot beside it, which is its own
item on the roadmap.
