# §657. Read-only workflow tokens and SHA256SUMS (2026-11-12)

An outside review (2026-11-11) found `contents: write` given to the
whole release workflow and no checksums beside a release's files.

**Tokens.** Every workflow defaults to `contents: read`, and only the
release job asks for `contents: write`. Build steps, the scripts they
fetch, and the tools they install never hold a token that can change
the repository. The release job checks out with
`persist-credentials: false`: it reads only the changelog, so the
write token need not sit in `.git`.

**SHA256SUMS.** The release uploads `SHA256SUMS` in sha256sum's own
format, over exactly the files it uploads, matched by the same globs.
A whole download is checked with `sha256sum -c` (`shasum -a 256 -c` on
a Mac), one file with `--ignore-missing`. The step runs with
`failglob`, so a file type with no match fails it as
`fail_on_unmatched_files` fails the upload. A failed tentative leg
(Windows on ARM, the Intel Mac) does not trip it, since the other
leg's files still match each glob.

**Unproven until a release runs.** The write permission on the one
job under a read default, and SHA256SUMS appearing on the release:
both were read and simulated (ten stand-in files, both tools), not
run. Split symbols for each release, the review's third point, stay on
the roadmap.
