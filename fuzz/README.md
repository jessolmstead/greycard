# Fuzzing

libFuzzer targets, through cargo-fuzz, over the text (and two binary
formats) greycard reads from files it did not write. Each target asserts
more than "never panics": whatever a reader accepts writes as JSON (or
XMP, or filter text) and reads back the same, so a value the editor
can read but not write again, such as a float that is not finite, is a
crash.

| Target               | Reads                                                        |
|----------------------|--------------------------------------------------------------|
| `sidecar_json`       | an edit sidecar, `.gcd`                                      |
| `edit_json`          | one edit, migrated from its schema                           |
| `preset_json`        | a preset, `.gcp`, and the edit it makes                      |
| `lightroom_xmp`      | a Lightroom preset or XMP sidecar, through the mapper        |
| `lightroom_template` | an older Lightroom preset, `.lrtemplate`, through the mapper |
| `lightroom_table`    | a catalog's develop settings table, its curves, and crop     |
| `pp3`                | a RawTherapee profile                                        |
| `xmp_meta`           | the rating, flag, label, keywords, and text of an XMP        |
| `filter`             | the filter language, printed back and parsed again           |
| `dng_opcodes`        | a DNG's opcode list, its gain maps applied to a buffer       |
| `dcp`                | a DCP camera profile, its map and look table on pixels       |
| `focus_point`        | a raw's head to its focus point, every maker read            |
| `canon_modes`        | a CR3's maker note to the capture modes its decode reads     |

The seeds in `corpus/<target>/seed-*` are taken from the crates' own
tests and fixtures (`canon_modes`'s notes by the core's ignored
`write_mode_seeds`, with `GREYCARD_MODES_SEEDS` naming the folder).
What a run adds to a corpus is ignored by git.
`dcp` has none: a camera profile is not ours to commit, so the target
lays each input over a small profile it builds itself
(`greycard_core::testing::dcp`). Without cargo-fuzz, the same checks
run over mutated copies of that profile as an ignored test:

```sh
GREYCARD_DCP_FUZZ_SECS=600 cargo test -p greycard-core --lib mutated_profiles -- --ignored
```

`GREYCARD_DCP_CORPUS` adds a folder of profiles to start from, and
`GREYCARD_DCP_ARTIFACTS` is where a failing input is written.

## Running

cargo-fuzz needs a nightly toolchain. CI pins one by date, and
cargo-fuzz's version, in `.github/workflows/fuzz.yml`; use the same:

```sh
rustup toolchain install nightly-2026-10-01 --profile minimal
cargo install cargo-fuzz --locked --version 0.13.2
```

From the repository's root, a target for a minute:

```sh
cargo +nightly-2026-10-01 fuzz run -s none filter -- -max_total_time=60
```

`-s none` builds without the address sanitizer. The code under test
is safe Rust, so the sanitizer would find little a panic does not,
it slows every run, and it reserves terabytes of address space, which
`ulimit -v` refuses. Debug assertions and overflow checks stay on.
`cargo fuzz list` names the targets.

## A crash

libFuzzer stops at the first failure and writes the input to
`fuzz/artifacts/<target>/crash-<hash>`. Run it alone to see the
panic, and make it as small as it goes:

```sh
cargo +nightly-2026-10-01 fuzz run -s none <target> fuzz/artifacts/<target>/crash-<hash>
cargo +nightly-2026-10-01 fuzz tmin -s none <target> fuzz/artifacts/<target>/crash-<hash>
```

Then put the minimized input, inline, into a unit test in the crate
whose reader took it, named for what it found, beside the reader's
other tests; fix the reader; and run the target over the artifact
again to see it pass. A small input worth keeping as a seed goes in
the corpus as `found-<what>`. The artifacts themselves are never
committed.
