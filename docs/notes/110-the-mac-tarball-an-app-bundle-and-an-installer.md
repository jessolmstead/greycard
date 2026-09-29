# 110. The Mac tarball: an app bundle, and an installer that clears the gate (2026-09-20)

The first tester is on a Mac, so the path from the releases page to
a running editor was walked here before a tag makes it real. Three
things were in the way: `package.sh` did not finish on macOS, what it
would have rolled was a Linux layout with a `.desktop` file in it,
and a tester who got past both would have met Gatekeeper.

**package.sh stopped at tar.** The reproducibility flags — `--sort`,
`--owner`, `--group`, `--mtime` — are GNU tar's, and macOS ships
bsdtar, which has other spellings for some and none for the rest. The
script now does the parts both can do the same way: the entries come
from a sorted list through `-T` with recursion off, rather than a
directory walk, and the commit's date goes onto the files with
`touch` rather than to tar; owner is `--owner=0` on one and `--uid 0`
on the other. `ustar` is the format both write without extended
headers, which matters on bsdtar since a pax header would carry the
very uid and mtime being fixed. And on macOS extended attributes
would come along as `._` AppleDouble entries, so the stage is
stripped with `xattr -cr` and bsdtar told `--no-xattrs
--no-mac-metadata` besides. Rolled twice, the tarball hashes the
same.

**A bundle, because that is what a Mac opens.** The macOS tarball
holds `greycard.app` and nothing else but the scripts, the license
and the README. Both binaries sit in `Contents/MacOS` beside the one
copy of Dawn, since each finds it through `@executable_path`; the
icon is an `.icns` that `sips` rasterizes from the SVG at each size
and `iconutil` folds, both of them in the base system, and the
render keeps the gradient and the blurred shadow. `Info.plist` is a
template in `packaging/` with the version filled in. Its minimum
system is Dawn's: the Rust binaries are built for 11.0 but the
prebuilt library ONNX Runtime brings says 13.4, so 13.4 is what the
plist says, and Launch Services refuses the app on an older machine
with a sentence instead of a crash. The bundle is ad-hoc signed as a
whole with `codesign -s -`, which the linker had already done to each
binary (Apple silicon insists) and which seals the plist and the
resources with them.

**Gatekeeper, and what install.sh does about it.** An ad-hoc
signature is not a Developer ID: a bundle downloaded by a browser
carries the quarantine mark, Archive Utility passes it on to what it
unpacks, and Gatekeeper refuses to open the app until the user finds
Open Anyway under Privacy & Security, which on macOS 15 is the only
way (right-click Open no longer suffices). Notarization is the real
answer and costs a developer account and CI secrets; it can wait for
the repo being public and testers beyond the first. Meanwhile
`install.sh` copies the app to `~/Applications`, takes the mark off
with `xattr -dr`, and links `greycard` and `greycard-ui` into
`~/.local/bin`, links rather than copies so each still finds Dawn
beside its real self — dyld resolves `@executable_path` through the
link, checked. It says how to put `~/.local/bin` on the PATH in
`~/.zprofile`, since macOS does not. The README says `sh install.sh`
rather than `./install.sh`, because the script is quarantined too
and `sh` is not. `uninstall.sh` removes the bundle and the links.
Tested end to end from the tarball with a planted quarantine mark,
launched through Launch Services with `open -W`, and taken out
again.

**The workflow.** The release workflow is a version job, a build
matrix (ubuntu-24.04 and macos-15, which is Apple silicon), each
uploading its tarball as an artifact, and one release job that
downloads them all and publishes once, so two builds do not race to
create the same release. CI now runs on a Mac too, since the file
chooser and the monitor profile have a Linux side and an
everywhere-else side behind `cfg` and a change to one is not seen to
compile from the other; on a private repo a Mac minute costs ten, and
the matrix line is easy to shorten if that is too dear.

**Not done.** Notarization, above. Intel Macs: ort has no WebGPU
bundle for x86_64-apple-darwin, so there is no build and the README
says so. "Open with" from the Finder: the plist declares no document
types because winit's open-file event is not wired to the command
line's path argument, and advertising it would be a lie. A `.dmg`
would be nicer than a tarball with a script in it and is not
different in kind.
