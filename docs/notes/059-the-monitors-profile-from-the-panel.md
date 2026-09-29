# 59. The monitor's profile, from the panel (2026-09-18)

An export looked nothing like the viewport: flatter and paler on
screen, deeper and more saturated as a file. The develop and the
finish were not at fault; `--no-display-profile --screenshot` matched
the export's pixels to within a count, and the OS screenshot matched
`--screenshot` with the profile. The difference was §17's table alone.
colord's primary here is the PA279CRV, and its profile is the one
colord derives from the EDID: the panel's native gamut, red at
x 0.688, gamma 2.2, up to 144 of 255 from sRGB. The viewport was
compensated for that; the JPEG, opened in a viewer that does no
color management, was sent to a wide-gamut screen as it was. The
monitor is in its Adobe RGB mode, so the viewport was the truer of
the two, and the export the viewer's fault; but the EDID profile was
not quite the right one either, since the mode emulates Adobe RGB
(red at x 0.648), not the native panel. An Adobe RGB red through the
EDID profile shows as 255,72,43. Skin moves two or three counts.

So the profile is now the panel's to choose, a MONITOR section after
SOFT PROOF: System (colord's, and when colord knows more than one
monitor with a profile, which of them by model), sRGB (no table),
Adobe RGB, Display P3, or a file through the desktop's chooser. The
standards are `MonitorProfile` in `display.rs`, built in Little CMS
from their primaries and curves (Adobe RGB's gamma is 563/256;
Display P3 has the sRGB curve), for a monitor whose own menu emulates
one, which is what the EDID cannot say. A line under the choice says
what the table does: "PA279CRV's profile from colord: up to 144 of 255
from sRGB", or "sRGB to the screen as it is", or that a file could
not be read, in which case the table is the identity. The choice and
the monitor are kept in the settings (`display_profile`,
`display_monitor`); `--display-profile` and `--no-display-profile`
set what the panel opens with, as `--proof` does, and the panel takes
over from there. A first cut had the flags override the panel for
the whole run, which read as a section whose buttons did nothing; a
flag that cannot be clicked away is a trap, and a headless run never
writes the settings anyway (§20). The table is rebuilt when the
choice changes, keyed on the output, the proof and the monitor
together.

Tests: sRGB as the monitor is the identity; Adobe RGB and Display P3
keep white and grey and show sRGB's red and green short of their own
primaries (Adobe RGB's red is sRGB's chromaticity exactly, so there it
is only dimmer, 0.86); the keys round trip. Checked on the machine:
the viewport under Adobe RGB and under colord's AdobeRGB1998.icc as a
file agree, and the section reads as meant.

What it is not: the window's own monitor, still the primary by
guess until the compositor says; nothing in the section can tell
that the monitor is in a mode its EDID does not describe, which is
why the standards are offered by name.
