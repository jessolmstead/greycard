# §442. The disk image's window

*2026-10-28*

The 0.9.0 image (§440) held the app and a link to Applications and
nothing else, so Finder opened it as a plain window with the two
icons wherever it put them, and nothing said to drag one onto the
other. It now opens as the usual install window: 640 by 400, no
toolbar or status bar, the app on the left and Applications on the
right at 128 points, and an arrow between them over a line saying
what to do.

## Made once, copied at release

A disk image's window is a `.DS_Store` at its root, which Finder writes
only when told through AppleScript to arrange an open window. A CI
runner cannot be counted on to let a script drive Finder (Automation
permission, a logged-in session), so the release does not make the
layout; it copies one. `packaging/dmg/layout.sh`, run by hand on a Mac
when the window changes, builds a throwaway writable image with the
same names in it (an empty `greycard.app` serves, since the layout
keeps names and positions, not contents), has Finder lay it out, and
keeps the `.DS_Store` as `packaging/dmg/DS_Store`, without the dot so a
global gitignore does not drop it. `package.sh` copies it and the
background into the image's folder before `hdiutil create`. The image
was never byte for byte reproducible (§440), so a committed binary
costs nothing there.

The `.DS_Store` finds the background by path through the volume's
name, `/Volumes/greycard/.background/background.tiff`, so the volume
must stay `greycard` in both scripts; each says so. The same holds at
the other end: an image opened while another `greycard` is mounted
comes up as `greycard 1`, and shows the plain window. Every disk image
laid out this way does that. The layout also carries the throwaway
image's path and the volume UUID of the Mac that made it, which are
harmless but come along in every release.

The release workflow, which mounts the image to check it, now fails
when `.DS_Store` or the background is missing from it. `*.tiff` is
ignored repo-wide, and the background has an exception in
`.gitignore`; without it the file stays on the Mac that made it, and
the release's copy fails.

## The background

`packaging/dmg/background.swift` draws it with AppKit at 1x and 2x,
and `tiffutil -cathidpicheck` folds the two into one `.tiff`, which
Finder shows sharp on a Retina screen. Drawn rather than painted, so a
change is an edit and a rerun. The arrow's ends follow the icon
positions `layout.sh` sets; change one and change the other.

The ground is Light's base grey, `#c4c4c4`, with the words in near
black and the arrow in the selection's blue. Finder draws the icons'
labels itself, black in a light appearance and white in a dark one,
over whatever the background is; the grey keeps both readable, which
was checked by eye on the M5 in Dark mode.

## A slip on the way

The first `layout.sh` refused to run with a `greycard` volume already
mounted, then its exit trap detached `/Volumes/greycard` anyway, which
was the 0.9.0 image open in Finder. It now checks before setting the
trap and detaches only the device it attached itself.
