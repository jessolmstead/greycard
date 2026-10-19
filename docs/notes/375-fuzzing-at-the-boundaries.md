# §375. Fuzzing at the boundaries (2026-10-19)

The outside review that found the Lightroom mapper writing a NaN into
a sidecar (§373) found it by reading the code. That class, a reader
that accepts something the editor cannot write back, is what a fuzzer
finds without anyone having to think of the input, so the roadmap's
line for it is now built: libFuzzer targets through cargo-fuzz, in
`fuzz/` at the repository's root, run weekly in CI and on demand
locally. `fuzz/README.md` says how.

## What is fuzzed

Eight targets, one for each reader of a file greycard did not write:

- `sidecar_json`: an edit sidecar, `Sidecar::from_json`.
- `edit_json`: one edit, `Edit::from_json`, migration and all.
- `preset_json`: a `.gcp` preset, which people pass around, and the
  edit it makes laid on a fresh one.
- `lightroom_xmp`: a Lightroom preset or XMP sidecar through
  `lightroom::read` and the mapper.
- `lightroom_table`: a catalog's develop settings as Lua table text,
  through `parse_table`, `read_table` with no body and with a measured
  one, the flat curves, and the crop placed on five frames (as it is,
  turned in Lightroom and here, measured with a border, mirrored, and
  a catalog that says its file has no size) as the catalog import
  places it.
- `pp3`: a RawTherapee profile, for a frame of unknown size and one of
  known size, carrying what it sets and carrying every section.
- `xmp_meta`: the rating, flag, label, keywords, title, caption, and
  time shift read from an XMP sidecar.
- `filter`: the filter language.

Each asserts more than "never panics". What a JSON reader accepts
writes and reads back equal (a sidecar, which has no equality, writes
the same JSON twice). What an import makes, the preset and the edit it
lays on a fresh one, does the same, which is the §373 property. The
meta from an XMP goes into a sidecar and reads back with it, and
written into a fresh packet, or back into the packet it came from, it
reads back the same. A filter that parses prints as text that parses
to the same filter, and its SQL has one parameter for each `?`.

The filter has no printer of its own; the fuzz crate carries one,
and leaves out of the round trip a filter whose value holds a double
quote, since the language has no escape for one: `a"b` reads as a
word but cannot always be written back. That is a limit of the
language, not a crash, and we did not add an escape for it.

## What is not

The catalog's SQLite reader needs a database file, and fuzzing one
is fuzzing SQLite. The import's report and its sheet (`sheet.rs`)
parse nothing. A smart collection's rules go through `parse_table`,
which is fuzzed, but their translation to a filter is private to the
catalog reader and is not; it checks its own output with
`Filter::parse` and refuses what does not read. The raw decoder is
rawler's, and its fuzzing is upstream's. The window's filter module
in `greycard-ui` wraps the library's parser and is left to it.
Settings, collections, the roots list, and the export queue are files
greycard writes for itself, not ones people hand it.

## How

cargo-fuzz in a workspace of its own (`fuzz/Cargo.toml` has its own
`[workspace]`), so the main build, its tests, and clippy never see
it. It carries the root's `[patch]` of rawler, since a patch applies
only from the workspace it is in, and its `Cargo.lock` started as a
copy of the root's so the readers are built against the same
versions. The seeds in `fuzz/corpus/<target>/seed-*` are taken from the crates'
own tests: the sidecar fixture, the shipped presets, the XMP
and table samples from the Lightroom tests, the RawTherapee profile
from the pp3 tests, the XMP packets from the meta tests, and the
filter language's documented examples. What a run adds to a corpus is
ignored by git.

The targets build without the address sanitizer (`-s none`). The code
under test is safe Rust, so the sanitizer finds little a panic does
not, it slows every run, and it reserves terabytes of address space,
which a `ulimit -v` refuses. Debug assertions and overflow checks
stay on, as cargo-fuzz builds by default.

CI's `fuzz.yml` is a workflow of its own, on a weekly schedule and by
hand, since `ci.yml` runs on every push with its jobs gated on the
event. It builds once on a nightly pinned by date, with cargo-fuzz
pinned too, runs every target for 60 seconds (by hand, for 60, 300,
or 600, a choice so the longest run fits the job's timeout and still
uploads), runs them all even after one fails, uploads what libFuzzer
saved for a week, and fails at the end. About a quarter of an hour a
week on one Linux runner: eight minutes of fuzzing, the rest
installing cargo-fuzz and the build. The corpus a run grows is thrown
away with the runner, so each run starts from the seeds and costs the
same. So that a change to a reader's API does not break the targets
unseen until Sunday, `ci.yml` also checks the fuzz crate on stable on
every push, on Linux alone, which needs no cargo-fuzz.

## What the first runs found

Every target but the filter failed inside its first minute. Four
causes:

- **A number past an `f32`'s range.** `"texture": 1e71` in a sidecar's
  history, or `-2.2e53` in a preset's mixer, read as infinity, which
  serde_json writes as `null`, and the next read refuses that: the
  NaN of §373 again, from the sidecar's own reader. The migration that
  every stored edit passes through now refuses an edit that holds a
  number no `f32` can, before any step, as it refuses any other value
  of the wrong kind, and says where (`detail.texture is 1e71`) and
  what was being read, a sidecar or a preset; the largest `f32` still
  reads. The tint's loose reading, a hue wrapped round the circle and
  an amount held to its ends, is for numbers in range: one past it is
  refused with the edit, which is the one rule for every field.
- **A float a unit in the last place off.** A white balance's tint, an
  `f64`, came back from a sidecar one bit different from the one
  written: serde_json's default parse is not correctly rounded for
  every seventeen-digit number. Its `float_roundtrip` feature is, at
  about twice the cost of parsing a float, which a sidecar does not
  notice; it is on for the workspace. It also makes serde_json refuse
  a number past an `f32`'s range read straight from text into an
  `f32`, in every file greycard reads that way, rather than read it as
  infinity. The Lightroom, catalog, and RawTherapee targets all failed
  on this one.
- **An edit written as a list.** serde reads a struct from a list of
  its fields in order as well as from an object, so `[0]` read as a
  version 0 edit that no migration step had touched, which wrote back
  as one the next read migrated. An edit, a snapshot, a version's line,
  and a preset that are not objects are refused; a version entry with
  no id is still left out, as the versions' reader leaves it. In the
  same place, an edit's schema version that is there and is not a
  whole number a `u32` holds is refused, where `7.0`, `-1`, and `"9"`
  read as version 1 and `4294967297` wrapped round to it; a missing
  one is still version 1.
- **A description inside a property.** The XMP reader took every
  `rdf:Description` in the packet for one of the frame's, so a
  description nested in a property's value (a struct, in RDF) was
  read as top-level, and the writer then spliced the property twice,
  one splice inside the other, dropping the outer: the caption read
  back changed. Only the descriptions inside no other are read and
  written now. No real sidecar we know of nests one of our properties
  inside another, but Lightroom does nest descriptions, which the
  review below found mattered.

Each has a unit test in `greycard-edit` named `fuzz_*`, with the
minimized input inline. With the fixes, every target ran a minute
clean, single process, on this machine: the sidecar 1.9 million
inputs, the edit 12.4 million, the preset 11.6 million, the Lightroom
XMP 6.2 million, the catalog table 1.8 million, the RawTherapee
profile 0.24 million, the XMP meta 3.9 million, and the filter 3.6
million. The filter found nothing in either round.

After the review's fixes below, each ran 30 seconds clean again
(the sidecar 1.2 million inputs, the edit 6.4 million, the preset 4.5
million, the Lightroom XMP 2.8 million, the catalog table 0.6
million, the RawTherapee profile 0.14 million, the XMP meta 1.9
million, and the filter 1.9 million).

## What the review of the fuzzing found

The review of this work found what the fuzzer could not, since it
fails no round trip. The Lightroom importer read its settings from
every `rdf:Description` in an XMP, nested ones included, a later
one's keys taking the place of an earlier one's. Lightroom writes a
profile's look (`crs:Look`) into the sidecar, for its default profile
too, and the look keeps the profile's own settings in a description
inside its own, `crs:Parameters`, with a tone curve and a
`CameraProfile` among them. Coming later in the file, those were read
over the picture's: a three-point curve the user drew came across as
the look's seven points, and the note that the picture's own profile
(say Camera Faithful) has no place here was lost to the look's Adobe
one. The reach is the one path that reads develop settings from
XMP text: a Lightroom preset file brought in through Import, which
carries a look whenever the preset saved its profile. The catalog
import reads its settings from the database's top-level keys alone,
so it and the mapper's measurements against Lightroom's exports
(§333, §342) were never affected, and a preset brought in from a
file disagreed with the same settings read from the catalog. The
importer now reads the top-level descriptions only, through the same
helper as the meta's reader, so a preset file carries the user's
curve and names the profile it cannot carry, as the catalog import
does.
