# §705. Lightroom's .lrtemplate presets (2026-11-16)

A user's presets would not import: they were `.lrtemplate`, the develop
presets Lightroom wrote before Classic 7.3 (2018). We now read them
wherever `.xmp` presets are read: the window's Import (its chooser
filter), a folder or pack import, and `greycard presets --import`.

**The format as read.**

A Lua source file: `s = { ... }` (the `s =` optional), an optional UTF-8
byte-order mark, `--` line comments and `--[[ ]]` / `--[==[ ]==]` long
comments. The fields used: `type` (must be the string `"Develop"`),
`title` (the name), and `value.settings`, the Camera Raw keys without the
`crs:` prefix. `id`, `internalName`, `uuid`, and `version` are read and
ignored. A key written twice is its last, as Lua takes it.

We did not write a second parser. The catalog import already had a
reader for this same Lua subset (`lightroom::parse_table`, the
`Adobe_imageDevelopSettings.text` column). It gained a strict mode
(`parse_lua(text, true)`), used for templates, since a template comes
from anywhere:

- values: tables, strings (`"..."`, `'...'`, `[[...]]`, `[=[...]=]`),
  numbers (decimal with fraction and exponent, hexadecimal, a leading
  minus), `true`, `false`, `nil`; any other bare word is an error;
- one call: `ZSTR` before a string, for the top-level `title` and
  `internalName` only (Adobe's own presets and some vendors' write titles
  so); any other call (`LOC "..."`, `ZSTR(...)`, ZSTR elsewhere) is an error;
- keys: names or `["quoted"]`; `[1] =` and other expression keys are errors;
- string escapes: Lua 5.1's (`\n \r \t \a \b \f \v \\ \" \'`,
  backslash-newline, `\ddd` up to 255); any other escape, a `\ddd` past
  255, or a raw line end inside a quoted string is an error;
- `Key = nil` is no entry (Lua's meaning); a `nil` in a list keeps its
  place, so a curve's later pairs stay paired;
- trailing `,` or `;` separators are fine; text after the table is an error.

**The name.**

The title, Adobe's `$$$/path/Key=Name` giving the text after the last
`=` (as an XMP's `crs:Name` does), then shown as a list or a terminal may
show it (`greycard_edit::preset::shown_name`): control characters (C0,
DEL, C1) go, tab, LF, VT, FF, CR, and NEL each becoming an ASCII space
and the rest dropped. Only a run of ASCII spaces the change made or
touched (holding such a space, or where a control was dropped) becomes
one space, or none at either end; every other character stays byte for
byte, other Unicode spaces (U+00A0, U+3000, ...) and untouched runs of
spaces among them (`"東京\u{3000}夜\t"` is `"東京\u{3000}夜"`). Nothing
left, or no title, is the file's stem. The cleaning is where a name is
made, `Preset::from_choice` and the preset JSON's reading, so an XMP's
name and a `.gcp`'s get it too; a name without control characters is
untouched. Names looked up compare through it on both sides:
`Store::find` (so `--preset` and `--apply` with the old spelling still
find a preset) and the import sheet's remembered preset, shown in the
list's spelling. A title such as
`"a\0b\n\27[31mred"` came through whole before and would have reached
the list and the command line's output, a terminal escape among it.

**Bounds.**

All errors, never panics:

- A file is read through `File::open(..).take(most + 1)` and refused past
  `most`, whatever its size says: a pipe fed without end, a device that
  says it is empty (`/dev/zero`), a file growing after it was looked at.
  Nothing past the most is held. `.lrtemplate`: 2 MB (`TEMPLATE_MOST`;
  real ones are tens of KB, a few hundred with brush strokes). `.xmp`
  presets: 16 MB (`XMP_MOST`; real ones are tens of KB, a megabyte or two
  with brush strokes or mask points), which they had no bound at all
  before. `.gcp`, and a file of any other name taken as ours (`presets
  --import /dev/stdin`): 64 MB (`GCP_MOST`); its JSON holds no image and
  no look table (a look is named), the most of it brush strokes at some
  tens of bytes a point, so some two million points. `read_template`
  checks the length again on its text, for callers that hand it one.
- The XMP beside a raw, as the take reads it (`beside.rs`): 64 MB, past
  which it is not read, a warning logged, and nothing taken. The
  open's own adoption of that file (`xmp::adopt_own`), the index's
  read, and the culling tool's shared XMP go through the same read
  since 2026-11-19 (`xmp::bytes_of`); the save's reread of an XMP it
  is about to rewrite (`xmp::stage_dated`) is still whole.
- Tables nested past 64; a long bracket with more than 16 `=`; more than
  250,000 entries in all tables together (strict mode only).
- A number too large for a float is a number, read as absent, as an
  XMP's `NaN` is.

Memory: a table holds many times its text, so the bounds are close. The
worst inputs we could build within them, measured by the process's peak
resident size (`the_worst_template_within_the_bound_timed`, debug build):
250,000 one-entry tables (`{1},`, refused at the entry bound) about 40 MB
above the process's baseline; 250,000 numbers about 26 MB; 250,000 empty
tables or strings about 18 MB; 2 MB of long strings about 21 MB. Before
(16 MB, no entry bound, the settings cloned) a template of `{},` peaked
near 1 GB.

**Mapping.**

The settings become the same `keys`/`curves` maps the XMP reader builds
(`table_settings`, shared with the catalog import) and go through the one
`map`: what an XMP preset carries, a template carries the same way.
Curves are flat lists, x then y on 0..255. A table that is not a curve
(`GradientBasedCorrections`, `PaintBasedCorrections`, `RetouchAreas`, ...)
is kept as its key alone, so it is named among what did not come across
("masks", "spots"), as an XMP's structures are. A hand-written template
and the equivalent hand-written XMP give equal imports.

A template has no group of its own. Imported from a folder it takes the
folder's group, as an XMP without `crs:Group` does.

**Old process versions.**

Templates are old by nature: many are process 2010 (PV 5.x) or 2003, some
2012. One rule serves every format, in the shared `map`:

- `lightroom::PROCESS_2012 = 6.7`, process 2012's version, is the one
  threshold, now shared by the mapper, the catalog import's report, and
  the Lightroom measurement. 6.6 was Camera Raw 6.6, of Lightroom 3.6,
  still the 2010 process.
- A settings set is of the old process when its `ProcessVersion` is below
  that, or, with none, when no key or curve names the 2012 process (a
  `2012` in the key: `Exposure2012`, `ToneCurvePV2012Red`).
- `Exposure` (2010) is still read as stops where `Exposure2012` is absent:
  same unit, same meaning near enough. `Clarity` and `ToneCurve*` likewise.
- `Contrast` of an old-process set is not read onto our Contrast: its
  scale is its own and +25 is its default, so before this an untouched
  old preset came in at +25 (a contrast of 1.125). In a 2012 set without
  `Contrast2012` (Adobe always writes it there; another writer may not),
  `Contrast` is read as before.
- In an old-process set, `Brightness` (default 50), `Contrast` (25),
  `HighlightRecovery` (0), `FillLight` (0), and `Shadows` (5, the 2010
  blacks) are named in the "not carried" list when off their defaults,
  by Lightroom's key names as the other uncarried settings are (no new
  translated text). In a 2012 set, where Lightroom may carry them inert,
  they are not named. A preset of nothing else is refused as carrying
  nothing, its names given.
- No mapping onto the 2012 sliders: their meanings differ.

`lightroom/beside.rs` listed `("Blacks", 5.0)` among the values Lightroom
starts a frame at; no Adobe file has `Blacks`, the 2010 key is `Shadows`.
It is `("Shadows", 5.0)` now, so a 2010 sidecar whose old blacks moved
no longer counts as Lightroom's start. A take whose mapped edit is the
frame's starting edit (no slider, crop, or turn carried) now takes
nothing: no sidecar written, not counted in an import's report, a debug
line logged, and the next open finds it the same. Before, such a take
wrote a `.gcd` holding no edit on every open, for an old file whose only
change was Brightness, Contrast, Fill Light, Recovery, or (now) Shadows,
and for a 2012 file with a stray old key off its default. The packet's
rating, label, and keywords still come as an open adopts them from any
XMP. A take that carries anything is as before. The mapper still reads a
`Blacks` key as old blacks (`either("Blacks2012", "Blacks")`), a branch
Adobe's files never reach; left.

**Refusals.**

A template whose `type` is not `"Develop"` (export, metadata, local
adjustment presets), or with no `value.settings`, or with settings that
carry nothing, is refused with the existing "no Camera Raw settings in
the file". Parse errors use the reader's own words ("... at byte N"); new
plain reasons: "a line ends inside a string, at byte N", "an escape past
255, at byte N", "an escape 'q' at byte N", "more than 250000 entries",
and "larger than N MB".

**The lenient reader's changes.**

The catalog's lenient reading shares the tokenizer and so changed at four
edges, none of which Lightroom's serializer writes: a bare word starting
`--` is now a comment, not a word; a long string's leading CRLF is
dropped as its leading LF was; `\a \b \f \v` are their control
characters, not the letters; a backslash before CRLF is one line's end,
not CR then LF. Bare words are still taken as written there, and the
entry bound does not apply.

**Limits.**

- Files must be UTF-8 (the XMP path's rule too).
- `--preset FILE` and `presets --apply FILE` take only `.gcp` paths
  (`Store::find` → `Preset::load`); neither takes `.xmp` today, so
  neither takes `.lrtemplate`.
- A body with an offset of its own (the GFX 100S II's -0.9 EV) still
  takes a sidecar that carries nothing else: the offset is an edit.
- Two presets whose names clean to one are both listed, and a lookup
  by name takes the first in list order.
- No `.lrtemplate` written by Lightroom was at hand: the format is as
  the community describes it, and the fuzz target builds but was not
  run under a fuzzer.
