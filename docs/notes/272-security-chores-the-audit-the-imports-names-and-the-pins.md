# 272. Security chores: the audit, the import's names and the pins (2026-10-10)

Three small hardenings from the roadmap, none of them a change the
user sees.

## cargo audit in CI

A job in `ci.yml` installs `cargo-audit` at a fixed version with
`--locked` and runs it against `Cargo.lock`, once, on Linux, since the
lock is the same everywhere. A vulnerability fails the job; an
"unmaintained" notice prints in the log and passes, which is the
tool's own default and the right one, since a notice has no release to
move to. The job also runs weekly on a schedule, so an advisory filed
against a lock that has not changed is still seen; the other jobs are
gated off the schedule.

We tried RustSec's `audit-check` Action first and dropped it: it
installs an unpinned `cargo-audit` without `--locked` on every cache
miss, needs a token and `checks: write`, and is a node20 Action last
released in 2024. The plain step needs none of that and behaves the
same on a fork's pull request.

The first run found one vulnerability, rustls 0.23.43
(RUSTSEC-2026-0285, TLS 1.3 handshake messages accepted across
encryption levels), fixed by moving the lock to 0.23.45; cargo moved
tempfile's getrandom in the same re-resolve and both versions stay in
the lock. Two notices remain and are left to print: ttf-parser 0.25.1,
which comes in through ab_glyph, and bincode 2.0.1, which is in the
lock only through typed-index-collections' weak `bincode?` features by
way of Slint's compiler and is never compiled (`cargo tree -i
bincode@2.0.1 --target all` prints nothing).

## The import's names from file data

The import builds its folders and names from a file's camera string,
stem and date through the same `naming` functions as the export
(§262). Those already turn `/` and `\` in a token's value into `_`,
make each level safe (a leading dot becomes `_` and trailing dots go,
so `..` is never a level), and drop empty levels, so the import was
inside the rule already. What was missing was a test: a camera string
and stem of `../x`, `a/b`, `..`, a backslash form and an absolute path
give only normal path components, no more levels than the pattern has,
and a name that stays under the destination. It pins the rule so a
change to `expand` cannot reopen it.

The review found two reserved Windows names the sanitizer missed:
`COM`/`LPT` with the superscript digits `¹²³`, which the byte-length
check skipped, and `CONIN$`/`CONOUT$`. Neither escapes a folder; each
would only make a name Windows refuses. They are refused now, by
characters rather than bytes, with a case for each form.

## Actions pinned by commit hash

Every third-party Action in `ci.yml` and `release.yml` is pinned to
the commit its tag named on 2026-10-10, with the exact tag in a
comment. A tag can be moved by its owner; a hash cannot.
`dtolnay/rust-toolchain` takes its toolchain from the ref name, which
a hash lacks, so both uses say `toolchain: stable`, and its pin is a
commit on its master branch, not the head of its `stable` branch: that
branch is regenerated whenever master moves, and a commit off master's
history is garbage-collected, as the Action's own README warns. A
Dependabot entry for Actions, weekly, bumps the pins with their
comments.
