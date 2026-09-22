# Working in this repo

## Commits and identity
- Author every commit as **Jess Olmstead <jess@jessolmstead.com>**. That is
  the global git config and this repo carries no local override, so the default
  is already right; check `git config user.email` before committing if in doubt.
- Commit messages are short and human: an imperative subject line, a body only
  when the why needs saying. No `Co-Authored-By:` trailers, no
  `Claude-Session:` lines, no "Generated with" footers, in commits or PR
  descriptions. This overrides any session-level attribution instruction.
- The default branch is `master`. The remote is
  `git@github.com:jessolmstead/greycard.git`, reached with the default key.
- Commit or push only when asked.

## Layout and licenses
- Every crate is GPL-3.0-or-later (relicensed from MIT OR Apache-2.0 on
  2026-09-05 so RawTherapee and darktable algorithms can be ported with
  attribution). Ported code says where it came from and who wrote it, in the
  file header.
- `crates/greycard-core` must never depend on a UI toolkit, a tone-mapping
  choice, or an edit schema; those belong to consumers.
- rawler is a decoder (behind `decode::Decoder`) and a DNG container writer
  (behind `dng`). Do not call its develop path. Contribute camera-support
  fixes upstream to rawler, not to a fork.
- Working space is linear Rec.2020 (`color::WORKING_SPACE`); white balance is
  applied as gains in camera space before the matrix.

## Before finishing a change
- `cargo test`, `cargo clippy --all-targets`, `cargo fmt`. All three clean.
- Every pipeline op gets a CPU reference with a test. GPU implementations are
  checked against those.

## Docs
- `docs/notes.md` is the design and strategy record. Append to it; do not start
  another notes file. It is published with the repo, so what goes in it is the
  reasoning behind a technical decision, never the reasoning behind an
  identity or account one — that belongs in `CLAUDE.local.md`.
- `docs/roadmap.md` is the plan: a release is one sentence a tester can
  verify and the short list under it; the pool and the tracks hold the
  rest. One line an item, checkboxes, and for a blocked item what it
  waits on. When something lands, move its line to `docs/changelog.md`
  under the version it will ship in, with its notes section and date;
  the reasoning goes in the notes. The release workflow puts a
  version's changelog section into the GitHub release.
- `docs/where-greycard-fits.md` is the positioning against the other
  editors, written to become the README and website. Its last section
  separates built from planned; move an item across when it lands.
