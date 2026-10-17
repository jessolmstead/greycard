# §341. The CLI's main.rs, one file a subcommand (2026-10-17)

The first of the three passes on the window's structure. The CLI's
`main.rs` was 4,326 lines: every subcommand, its helpers and two test
modules. It is now eleven files beside `log.rs`, and nothing a user
can see changed.

## What went where

- `main.rs` (275 lines): the clap `Cli`, the `Command` enum with every
  variant's doc comment where clap reads it, `main`, and `run`, which
  only dispatches.
- `info.rs`, `lenses.rs`, `models.rs`, `presets.rs`, `rawtherapee.rs`,
  `register.rs`, `stack.rs`: each subcommand and the helpers only it
  uses (the lens words with `lenses`, `StackFormat` with `stack`,
  `FetchReport` with `models`, the sidecar and section helpers with
  `presets`).
- `library.rs`: `LibraryAction` and its three actions, the editor's
  settings reader, `--root`'s parser, `SORT_KEYS`, `listed` and
  `stacks_over`. The Lightroom import is 25 lines of options and a
  call into the library crate, so it stays here.
- `develop.rs` (1,699 lines): the flags and settings, the learned
  denoiser, the lens-then-sharpen order, the picture path, outputs.
- `common.rs`: what more than one subcommand calls: `load`,
  `megabytes`, `denoiser_tiers`, `on_exists_named`, `SOFTWARE`,
  `write_linear_tiff`, `srgb_preview`.

The one change of shape is `develop`. Its 45 flags were a struct
variant, and its arm built the engine's settings in 125 lines of
`run`. The fields are now `develop::DevelopArgs`, a clap `Args`
struct; the variant is `Develop(develop::DevelopArgs)` and keeps the
doc comment, the struct carries none, so the about comes from the
same place; the arm's body is `develop::run`, unchanged under a `let
DevelopArgs { .. } = args`. The other variants' fields are few and
their arms one call, so they stay in the enum.

Tests went with what they test: nineteen before and after, none
edited past `super::Cli` becoming `crate::Cli` in the listing's.

## The proof

A script cut the new files from the old one's line ranges; a count
of every stripped line, old against new, leaves only imports, module
headers, `pub(crate)`s and qualified calls. Master's binary and the
split's, both named `greycard` so clap's usage lines agree, were run
on `--help`, `-h` and `help <sub>` for the top level, every
subcommand and every library action, and on fourteen of clap's own
refusals (`--on-exists`, `--sort`, `--reference`, `--root`, a
missing `requires`): byte-identical, exit codes too. Then on a run of
each subcommand that needs no network, in a scratch XDG: `info` on
six raws and a PNG; `lenses` with a copy of the fetched database;
`develop` on a raw (bilinear, to DNG, TIFF and PNG), on a picture
with sharpen, detail, dehaze map, defringe and a lens profile, and
five refusals; `register`; `stack` to DNG and TIFF with a frame
dropped; `library index` and `list` five ways; `presets` import,
list and apply; `rawtherapee` dry runs. Stdout, stderr and exit
codes match with paths and timings masked (the info and listing runs
again unmasked), images' pixel signatures and the DNGs' hashes
match, and the applied sidecar differs only in its write stamp.
