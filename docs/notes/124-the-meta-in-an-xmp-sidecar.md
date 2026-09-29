# 124. The meta in an XMP sidecar (2026-09-20)

The meta in an XMP, for the tools that read one (2026-09-20)

§72 put XMP interop on the list and §117 wrote the meta section it
would map onto. `greycard-edit`'s `xmp` module is that mapping:
`xmp:Rating`, `xmp:Label`, `dc:subject`, `dc:title` and
`dc:description`, the five fields every other cataloguer reads, plus
`greycard:Flag` under the namespace an export's packet already uses
(§51). A folder culled here opens in Lightroom, Bridge, darktable or
digiKam with its stars, its label and its keywords on, and a folder
culled there opens here the same way.

**The pick has no standard field.** Adobe never gave one a property;
Lightroom's pick and reject live in the catalog and it does not
export them. So the flag goes under greycard's namespace, which
every other tool ignores and this one reads back. The reject does
have a convention, and it is honored: Bridge, darktable and exiftool
spell it `xmp:Rating` of -1, so a frame with no stars and a reject
flag writes -1, and a -1 read back is a reject whoever wrote it. A
frame that is both rejected and rated keeps its stars in the rating —
they are the field's own meaning — and its reject in
`greycard:Flag`, so every combination round-trips without a second
field for the number.

What that costs, reasoned rather than observed, since there is no
Lightroom on this desk to check it against: Bridge and darktable
read -1 as a reject and will show a greycard reject as one.
Lightroom Classic writes -1 into an XMP but its catalog's reject
flag is a separate column, so it most likely shows such a frame as
unrated rather than rejected — the reject does not travel to
Lightroom, only away from it. A frame that is rejected *and* rated
shows its stars everywhere and its reject only here. And darktable
rewrites the XMPs it touches from its own model, so foreign
properties, `greycard:Flag` among them, probably do not survive a
darktable write. None of that is worth a second scheme; it is worth
saying plainly, which the positioning doc now does.

**The packet is spliced, not rebuilt.** This is the part that had to
be right. Lightroom writes its whole `crs:` develop block, its
`xmpMM:` chain and its tone curves into the same file, and a rating
pressed here must not cost any of it. So `write` parses with
roxmltree (already a workspace dependency for the preset reader,
§52), finds this module's six properties wherever they sit, and
edits the source bytes around them: a scalar already in the file
keeps its form and only its value moves, a property that already
says what the meta says is not touched at all, and anything new goes
in after the children that are there. Changing a rating on a
Lightroom sidecar is a one-character diff; everything else in the
file is the same bytes it was. The test builds a realistic Lightroom
packet and asserts the whole file equals the input with
`xmp:Rating="2"` replaced by `"5"`. Checked against a real
implementation too: exiv2 reads everything the module writes,
including the flag, and a packet exiv2 has rewritten reads back here
whole.

One property is deliberately *removed* rather than kept.
`lr:hierarchicalSubject` is Lightroom's parallel copy of
`dc:subject` with the keyword paths spelled out, and when the flat
list changes here a stale copy of it beside the fresh list is a
contradiction — and the copy is the one Lightroom believes. So it
goes when the keywords move, and stays untouched when they do not;
Lightroom builds it again from `dc:subject` on the next read. Not
written from this end: §72 keeps keywords as flat strings with slash
paths and derives the hierarchy, so there is nothing here that knows
which separator Lightroom wants.

**A file that will not parse is left alone.** A packet this build
cannot read is one it cannot safely rewrite, so `read` and `write`
both refuse and the caller logs it, and such a file is not marked as
having been read, so the complaint comes back if it is never fixed.
The XMP is what a malformed file costs; the `.gcd` is never touched,
is written after it, and holds the meta regardless. Same instinct as
§117's loose reading, one level up: the interop file is expendable,
the truth is not.

**Two names, and which of them a frame owns.** Lightroom writes
`IMG.xmp` beside `IMG.CR3`; darktable writes `IMG.CR3.xmp` (and
reads either). The long name is always the frame's own. The short
one is only the frame's when nothing else in the folder answers to
`IMG` — a raw and a JPEG of one shot both map to `IMG.xmp`, and a
sidecar shared between two frames is one that each writes over. So
when the stem is shared the short name is not read and not written,
either; whoever made it keeps it. One function decides the file for
reading and for writing both, which is the only way the two do not
drift apart: the name that is already there, the newer when both are
there and both are ours, and otherwise the name the frame owns. The
folder is only read to answer the stem question when the answer can
matter, which is when `IMG.xmp` exists or a file is about to be made.

**When an XMP is believed.** Not when it is newer — that was the
first design and it was wrong three ways. A rating cleared here
leaves the XMP untouched and still saying three stars, and a `.gcd`
saved a moment later is newer only by a moment, so the three came
back on the next open. A `.gcd` written for an exposure hid a rating
Lightroom had written before it. And a folder copied without
preserving times had neither file's real age. Times are the wrong
instrument: they compare two files that are about different things.

So the sidecar records what it last took — the XMP's name, its
length and a hash of its bytes — and the XMP is read again only when
that file has changed. A hash rather than a time because it compares
the XMP to *itself*: a copy, an rsync, a clock that jumps, none of
them make a file say something new. It is an FNV-1a in hex, not a
cryptographic question. The mark lives beside the meta on the
`Sidecar` rather than inside `Meta`, so that `Meta` keeps meaning
what a person said about a picture and `is_empty` keeps deciding
whether there is anything to write.

**Silence is not a statement.** `read` reports which of the six
properties the packet actually carried, and taking its word moves
only those: a tool that adds an `exif:LensModel` to a sidecar has
not thereby cleared the keywords, the title and the label it never
mentioned. A property that is *there* and empty — an empty
`rdf:Bag` — does clear, because that is a tool saying there are no
keywords rather than saying nothing at all.

**Opening a folder writes nothing into it.** The meta taken from an
XMP and the mark for it ride on the sidecar in memory; they reach
the disk with that frame's next real save. Browsing five hundred
frames of somebody else's Lightroom shoot must not deposit five
hundred `.gcd` files in it, and now does not. The write order is the
other half of that: the XMP goes first and the `.gcd` second, so the
mark the `.gcd` carries is for what the XMP now holds rather than
what it held a moment ago. And a write whose bytes would not change
the file is skipped, mark and all, so taking a snapshot or renaming
one does not bump every XMP's modification time.

**The setting is off by default, and only the writing is behind it.**
Reading an XMP that is already there is not a choice; it is believing
what a file says, it costs nothing when there is no such file, and it
is what gets a Lightroom user's twenty years of stars onto the screen
the first time they open a folder here — and now it costs them
nothing on disk either. Writing one is a decision about somebody
else's folder, and §117 argued the case against it already: one file
beside the frame, not two, because two is two things to keep in step
and two ways for a folder copied somewhere to arrive half itself.
For the user who never runs another tool, a second file per frame
doubles exactly the clutter §86 was written about and duplicates
what the `.gcd` already holds. For the user sharing folders with
Lightroom, one switch turns it on. The harm of the wrong default
runs one way only: a missed switch is found and flipped, a folder of
paired files is permanent. So `xmp_sidecars` starts off, in
`settings.json` and behind `--xmp-sidecars` for a run.
`--no-sidecars` still turns off both, an XMP being a sidecar.

**Left out.** No panel control for the setting yet — it is in
`settings.json` and on the command line, and it belongs in a
preferences sheet that does not exist. No CLI command to export a
whole folder's existing meta to XMP; until there is one, the switch
takes effect on the next frame whose meta is written. An XMP whose
properties are all cleared is left as an empty packet rather than
deleted, since deleting a file somebody else's tool made is not this
module's business. A keyword list that *changed* is removed and put
back after the children that are already there, so it moves within
the description; everything else in the file is byte-identical, and
more machinery than that was not worth it. And the edit stays in the
`.gcd`, as §86 said it would: darktable and Lightroom rewrite the
XMPs they touch, and a develop crossing that boundary is the catalog
import's job (§79), not a sidecar's.

One thing that followed. A rejected frame takes its XMPs to the
rejects folder with its raw and its `.gcd` — both names when a
folder has been through both tools — and an XMP whose name is
already taken there keeps the frame where it is, as a taken raw or
`.gcd` name already did. An interop file left pointing at a frame
that is not there any more is worse than none, and a move that
silently writes over one is worse than both.
