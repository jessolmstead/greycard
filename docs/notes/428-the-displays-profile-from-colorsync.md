# §428. The display's profile from ColorSync

*2026-10-25*

"System" in the MONITOR section meant colord's profile on Linux and
sRGB everywhere else. On a Mac's Display P3 panel that left the
viewport's sRGB values sent to a P3 screen as they were, so the
viewport read more saturated than the same export in Preview; picking
Display P3 by hand matched. Every Mac on the default saw it.

On macOS "System" now reads ColorSync. `display::system_monitors` is
the one function the startup calls; on Linux it is colord as before,
on macOS it lists CoreGraphics' active displays
(`CGGetActiveDisplayList`), marks `CGMainDisplayID` primary and puts
it first, and takes each display's color space
(`CGDisplayCopyColorSpace`) and that space's ICC data
(`CGColorSpaceCopyICCData`). The bytes are written to
`<cache>/greycard/monitors/<display id>.icc`, rewritten only when they
change and through a rename, so `Monitor.profile` stays a path and
the table is built from it exactly as from colord's file. The model is
NSScreen's `localizedName`, matched by `NSScreenNumber`, else
"Display <id>". We declare `CGDisplayCopyColorSpace` ourselves: the
crate's binding panics on a null return, and a display without a
color space should be listed without a profile, not end the run. The
bindings are objc2-core-graphics, objc2-core-foundation, and
objc2-app-kit, all already in the tree through winit; no build step.
The panel's note names the source ("Color LCD's profile from
ColorSync"). Windows is unchanged.

Tests: the cache file is written once, left alone for the same bytes,
and rewritten for new ones, with no temporary left; the main display
comes first, a display without a name is "Display <id>", one without
ICC data or a cache has no profile. The macOS code is checked with
`cargo check` and `clippy --all-targets` for aarch64-apple-darwin
(C build scripts stubbed, since there is no SDK here), not run.

To check on the Mac: start the editor from a terminal with the MONITOR
section on System. The log has `ColorSync: <display name> (primary):
<cache>/greycard/monitors/<id>.icc` and then `display profile <that
path> (<display name>'s profile from ColorSync): at most N of 255
from sRGB`, N well above zero on a P3 panel. Open a saturated frame,
export it, and open the export in Preview beside the viewport: the
two should match, as they did with Display P3 picked by hand, and
switching between System and Display P3 should change the viewport
by little or nothing.

## Review

A fresh review found no defects. It checked the ownership of the
Copy functions' results, that `CGDisplayCopyColorSpace` is the
display's current profile and not a generic space, that NSScreen is
touched on the main thread only, and that nothing is written on
Linux. Three nits were amended: comments that still said colord, a
doc comment above the wrong function, and the temporary file left
behind by a failed rename. The first run on a Mac is the user's.
