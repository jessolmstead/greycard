# 151. lcms2 is built in on macOS and Windows (2026-09-22)

v0.1.0's Mac package would not start on a Mac without Homebrew's
little-cms2: dyld stopped at launch on
`/opt/homebrew/opt/little-cms2/lib/liblcms2.2.dylib`. lcms2-sys asks
pkg-config for a system lcms2 first and builds its vendored copy only
when that finds nothing. The release workflow assumed a Mac runner has
no pkg-config; the macos-15 image has Homebrew's, and little-cms2 with
it. A local build never showed it, because this Mac has no pkg-config.

The release workflow now sets `LCMS2_STATIC=1` on macOS and Windows,
which skips the probe. It goes into `$GITHUB_ENV` in its own step
rather than as a conditional `env:` value, because lcms2-sys reads it
as set-or-not and an empty string counts as set. Linux keeps the
distribution's lcms2: it is in every desktop's base set, and the
package already depends on the system for glibc and the rest.

`package.sh` now fails a Mac package whose binaries or Dawn link
anything outside `/usr/lib`, `/System/Library`, `@rpath` and
`@executable_path`. A Mac has no package manager to fill in a missing
library, so a stray path is always a bug, and the runner is exactly the
machine that will have whatever it points at.
