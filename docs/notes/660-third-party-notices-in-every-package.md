# §660. Third-party notices in every package (2026-11-12)

## How the file is made

- Every package now carries `THIRD-PARTY-NOTICES` beside `LICENSE`. The Mac app
  also has both files in `Contents/Resources`, since the disk image carries the
  app alone.
- `scripts/notices.sh` builds the file from cargo-about's output plus hand-kept
  parts. It runs cargo-about 0.9.3 offline, after a `cargo fetch`, over every
  triple a release builds.
- We chose this so that one file covers every platform and a given commit always
  produces the same file.
- The file is generated when a package is made, not committed, so it always
  matches the tree it ships with.
- CI runs the script on every pull request. A dependency under a license that
  `about.toml` does not accept therefore fails the pull request, not the release.

## Guards

- The script fails on any cargo-about warning, not only on its errors. A stale
  checksum in a clarification is only a warning to cargo-about. After it,
  cargo-about falls back to scanning the crate's sources and emits C headers as
  license texts.
- As a second guard, the script fails if the assembled file has a line starting
  `#include` or `#ifndef`.

## License choices

- The order of `accepted` sets the preference wherever a crate offers a choice,
  so MIT is taken before Apache-2.0. Per-crate choices are named in
  `about.toml`.
- Slint is taken under GPL-3.0-only. Its royalty-free license forbids embedded
  use and exposing Slint's APIs. A GPL program cannot pass on restrictions like
  those, so that license cannot serve one.
- The consequence: greycard's source stays GPL-3.0-or-later, but the binary as a
  whole is conveyed under GPLv3 exactly.
- rawler is LGPL-2.1, linked statically into a GPL-3 binary. LGPL-2.1 section 3
  allows putting a copy under a later GPL. Separately, section 6(a) is met
  because we ship greycard's complete source, which lets a user relink.

## The crates template

- The template lists each crate by name and version only, under its license
  text. Each license's text carries its own notices: the crate's copyright lines
  as it ships them.

## Where cargo-about misses a crate's own text

- Some crates ship their license under a file name cargo-about does not find.
  It then falls back to the license's standard text, with unfilled
  placeholders. We name each such file in a clarification in `about.toml`,
  with its sha256, so the shipped text and its copyright line are what the
  notices carry:
  - the Windows crates' `license-mit`;
  - brotli-decompressor's BSD text;
  - libm's and dpi's MIT texts;
  - md5's MIT section;
  - adler32's zlib text.
- The placeholders left belong to crates that ship no license file, or whose
  own file is the unfilled template.
- Some crates keep further notices beside their licenses, in files cargo-about
  never reads: copyright lines for code they took from Go, the Rust project,
  Tokio, or mio, or for their own authors when the license text has none.
  `packaging/notices/crate-notices` lists each of these files with its sha256,
  one line per crate version, so a second locked version takes a one-line
  addition.
- `notices.sh` finds each version's source directory through the
  `manifest_path` that `cargo metadata` resolves. It gets that path from a
  second cargo-about run over the same tree, using a small template. That way
  we need no JSON parsing in sh and no Python on the runners, and registry,
  git and vendored sources all work.
- The script fails in four cases, each with its own fix:
  - a listed crate is no longer in the tree: delete its lines;
  - a listed version is no longer built: delete that line;
  - a version is built that the list does not name: the message prints the
    line to add, with its sha256, after reading the file;
  - a file's sum no longer matches: read it and update the sum.
- It also fails when a `[<name>.clarify]` block in `about.toml` names a crate
  outside the tree. cargo-about ignores such a block silently, and the crate it
  was meant for would lose its text. The header check accepts every way TOML
  can spell the header: spaces around the parts, a quoted name, or a trailing
  comment.
- What the list check cannot catch: if all of a crate's lines are deleted, its
  notice is dropped without a word. A crate newly in the tree that keeps such a
  file is found only by someone reading it.
- Windows checkouts may convert line ends to CRLF. We guard against that two
  ways:
  - `.gitattributes` pins LF on everything the notices are built from: the shell
    scripts, `about.toml`, `packaging/notices/*`, and the icons' LICENSE. Every
    platform then assembles the same bytes.
  - `notices.sh` strips carriage returns wherever it parses: the crate list it
    has cargo-about write, `crate-notices`, and `about.toml`'s headers. A CRLF
    checkout therefore reads the same even without `.gitattributes`.
- We chose this over clarifications because, in cargo-about, a clarified file
  appears only under one of the licenses the crate is taken under. Some of these
  files hold other licenses (crossbeam-channel's carries CC BY 3.0 and Go's
  BSD), and cargo-about would have shown them under the wrong heading.

## Hand-kept parts and when to refresh them

- ONNX Runtime's third-party notices: when ort-dylib's runtime version moves.
- DXC's license and third-party notices: when pyke's build changes.
- `ported.txt`: when new code is ported.
- The ring clarification: when ring's license files change.
- What greycard downloads on first use is named in the file, but it is not
  shipped.

## Installing cargo-about in CI and release builds

- CI and every release or package job build cargo-about from source, pinned to
  0.9.3 and `--locked`.
- The binary is cached under a key made of its version and the runner's OS and
  architecture, so it is built once per platform rather than once per run.
- A tag's run can restore caches only from its own ref and from master, never
  from a pull request.
- The CI job that writes the notices runs on the release's Ubuntu, so the binary
  it caches also runs on the release's Linux leg.
