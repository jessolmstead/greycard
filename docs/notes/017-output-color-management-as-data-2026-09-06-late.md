# 17. Output color management, as data (2026-09-06, late)

§5 rule 8 asked for the display transform to take a 3D table from
the first day so that a monitor profile is a data change. It does now:
the viewport shader's last step samples a 33-point 3D texture over
encoded sRGB, identity by default, and `--display-profile FILE.icc`
builds the table through Little CMS (`lcms2`, MIT) as sRGB to the
profile, relative colorimetric, every grid point transformed once at
start. Tests: the identity is the grid; Little CMS's own sRGB written
out and read back as a file is within a level of the identity.

On this machine: colord's sRGB profile moves nothing (1.4 of 255 at
most in the table, a different curve fit; the screenshot is the same
to five decimal places); the monitor's colord-generated EDID profile
moves up to 8.6 of 255, a real if modest correction; AdobeRGB, as a
wrong profile on purpose, changes the picture plainly. So the path is
right end to end and the profile is the only variable.

What is not done, and is on the roadmap: choosing the profile without
a flag. colord knows both monitors here and their profiles (D-Bus,
`org.freedesktop.ColorManager`); the right answer is the Wayland
color-management protocol once Slint's backend carries it, and colord
by D-Bus until then, with the window's monitor decided by where it
sits. Also not done: a table per monitor when the window moves, and
HDR, which is the surface format's business, not this table's.

**The profile without a flag (2026-09-06, late).** colord over D-Bus
(`zbus`, blocking API, no runtime): `GetDevicesByKind("display")`, each
device's `Model`, `Metadata` (its `OutputPriority` says which is the
primary) and `Profiles`, the first profile's `Filename`. The primary
display's profile is the default; `--display-profile` overrides,
`--no-display-profile` gives plain sRGB, and the choice is printed at
start so a window on the other monitor can be told. On this machine
the primary is a wide-gamut ProArt whose EDID profile sits up to 144
of 255 from sRGB: without the table, every sRGB-encoded value would be
shown a good deal more saturated than meant, which is the case the
whole path exists for. GNOME's compositor applies no ICC transform to
a window's content on Wayland (only the calibration curves), so this
is the application's job until the color-management protocol lands,
and then it is the compositor's and the table becomes the identity.
