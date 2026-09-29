# 140. The Windows runner in CI, and the types the platforms are told (2026-09-22)

**The third runner.** `ci.yml` gains `windows-2025`, so a change that
breaks the Windows build is caught on a push rather than at a tag, the
gap §137 left. The job's shell is Git Bash on every runner
(`defaults: run: shell: bash`), which changes nothing on Linux or macOS
and lets Windows print its MSVC toolsets before the build as
`release.yml` does, since the failure that guards against, unresolved
`__std_*` symbols against a toolset older than 14.51.36231, is at link
time and `cargo test` links as much as a release build does. The disk
cleanup and the apt install stay Linux-only.

**The `.orf` in the desktop entry.** `assets/greycard.desktop` was the
one list missing `image/x-olympus-orf`. The browser's
`raw_extensions`, the desktop entry and the Windows registration
scripts now name the same eight. Two lists outside the shipped ones
differ and were left: an ignored preview-timing test in
`decode/mod.rs` that skips `.orf` and `.rw2`, and a bench tool that
also takes `.pef`.

**The `.gcd` on the Mac.** `Info.plist` exports `org.greycard.gcd`,
conforming to `public.data` and `public.content`, the same
non-committal shape §120 gave it on Linux, and claims it as Editor and
Owner, so Finder offers greycard first for a sidecar. The raw types
join as Viewer and Alternate, offered and not taken, the shape
`register.cmd` gives them on Windows: `public.camera-raw-image` as the
floor, plus the maker's UTI for each of the eight, from
`com.canon.cr3-raw-image` to `com.adobe.raw-image` for DNG.
`package.sh` folds the sidecar's SVG into its own `.icns` by the same
`sips` and `iconutil` steps as the app icon, so the document gets the
two-cards mark rather than a blank page.

That makes Finder offer greycard and launch it; it does not make a
double-click open the file. Finder delivers the file as an Apple Event,
not on the command line, and nothing in the tree turns one into an
open, which §110 already said of Open with. Declaring the types is
still worth doing alone, since it is what puts greycard in Get Info and
the Dock's menu; wiring the event is its own line on the roadmap. The
plist parses and `package.sh` passes `sh -n`, but none of this has run
on a Mac: the UTI spellings and the `.icns` step are for the first tag
to check, with `public.camera-raw-image` there so a misspelled maker
UTI leaves no raw type unreached.
