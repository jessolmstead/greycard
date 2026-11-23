# §700. The build's commit in its version (2026-11-15)

A tester's report said only `0.11.0`, which covers every build from the
release to master's tip. A build now knows its commit, so the report
names the one they ran.

**The label.** `greycard_version::LABEL` is `0.11.0` for a release,
`0.11.0 (79527a9)` for any other build from a git checkout, and
`0.11.0 (79527a9+)` when tracked files had uncommitted changes at the
first build after HEAD last moved. With no git (a source tarball, git
not installed, or a tarball unpacked inside some other repository) it
is the plain version. It lives in a crate of its own,
`greycard-version`, whose build script asks git for the short hash,
`status --porcelain --untracked-files=no`, and `describe --exact-match
--tags --match v<version>`. Every git call that fails gives no commit;
the build never fails on it, in a worktree, on a detached HEAD, or in
a shallow clone. The formatting rules are pure functions in
`src/stamp.rs`, shared by the build script and its tests.

**Where it shows.** The log's first line, `--version` on both
binaries, the Report a problem issue's version field, and Settings'
About. Same wording; the value carries the commit. The window's
library takes the label from the binary rather than naming the crate.

**Where it does not.** Anything compared or stored keeps the plain
version: the update check and its User-Agent (a build at `0.11.0
(abc)` is offered what `0.11.0` is), the guide's link to its tag,
EXIF Software in exports and DNGs, the model cache and runtime record
keys, the match comparison's build key (already a binary hash),
sidecars, and the index. A label in a cache key would throw every
cache away at each commit.

**Release builds.** HEAD exactly at `v<version>` with a clean tree is a
release. A tag build in CI may not see its tag, so release.yml sets
`GREYCARD_RELEASE=1` for every job; the other workflows (CI,
windows-package.yml, the ARM and Intel packages) are unchanged and
carry the commit. A packager sets `GREYCARD_COMMIT` to name a commit,
or to empty for the plain version, and `GREYCARD_RELEASE=1` for a
patched tag checkout (docs/building.md). The Windows installer and
package.sh read the version from cargo metadata, and the setup check's
`--version` match still finds the plain version inside the label.

**What reruns the stamp.** Only this worktree's HEAD moving: the script
watches its `HEAD` and its reflog `logs/HEAD`, both per worktree,
appended at every commit, checkout, reset, merge, and rebase. No
source file, no index, and nothing shared, so an edit, a `git status`,
or another worktree's commit or branch deletion leaves the build
fresh; we run many worktrees at once. Without a reflog (reflogs off,
some CI clones, whose builds are fresh anyway) it watches the branch's
ref and packed-refs instead. A moved HEAD recompiles the version crate
and the editor's and the command line's crates; never Slint, which is
greycard-slint's build, nor the engine crates. The editor's library is
recompiled too, though it no longer names the crate: cargo gives a
package's library and binary the same dependencies.

**Limits.** The `+` is the tree at the first build after HEAD moved; a
later uncommitted edit does not add it, nor does reverting one remove
it. That suits its use: testers and CI build clean checkouts. Tags are
not watched either, so a build made before HEAD was tagged keeps its
hash until HEAD next moves.

**The commits since the release (2026-11-22).** A tester who built
master and went back to the release could not see which was newer
(issue 199). In a full clone with the tag `v<version>` among HEAD's
ancestors, the label now counts the commits past it:
`0.12.0+135 (8bab17c)`. A release, a packager's commit, a shallow
clone, or no tag gives the label as above. The count is shown and
never compared or written: the off-card record's key is the label
without it (`KEY`). The stamp now also reruns when the tags move
(`refs/tags`, `packed-refs`, and `shallow` are watched), so tagging
HEAD gives the plain version at once, and a repack rebuilds the stamp
once.
