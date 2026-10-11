# §294. The tint has a switch (2026-10-10)

Trying the tint after the blend (§286) went in, we wanted the one
thing every other section has: a switch. Light, Detail, Color grading,
the mixer, a mask's white balance, each carries one in its header, and
off, the sliders stay where they were and the picture is as if the
section were not there, so a comparison is one click. The tint had
none, on the global look or on a mask's; the only way to see the
picture without it was to drag the amount to nothing and lose it.

**The schema.** `Tint` gains `enabled`, on by default. Off, the tint is
nothing on both paths: `Tint::applied` returns its input bit for bit,
`is_identity` says so (which is what keeps the Oklab pass from running
for a switched-off tint alone), and `vector` is zero. The vector is the
one thing the shader is handed, the global's as `tint` in the
parameters and each mask's in its `Local`, and it is what the finish
sums over the looks before reading a tint back off the sum, so a mask's
switched-off tint adds nothing to the sum and changes nothing else. The
shader needed no change: a zero vector is already the identity there.
`from_vector` gives a tint switched on, as it is the sum of tints that
act.

**What a sidecar says.** The other sections' switches are written
always, `"enabled":true` and all; they were there from the start. The
tint's is written only when off. The develop key hashes the edit as
serde writes it, and every edit has a global tint, so writing the
switch always would have changed the key of every frame in every
library and made every kept thumbnail and preview again, for a field
that changes no picture. Left out at its default, a sidecar written
with the tint on is byte for byte what it was, the key is the key it
was, and a sidecar from before the switch reads as on. A build from
before the switch reads `"enabled":false` as a field it does not know,
and applies the tint; the picture it shows is the one the user had
before switching off, and it drops the field on its next save. This is
the
display curve's rule (left out at its default, so a picture nobody
switched writes what it always wrote), and `LocalWhite`'s. Off, it is
written first, as the other sections' switches are:
`{"enabled":false,"hue":210.0,"amount":0.4}`.

**The panel.** The Tint section carries the section switch, bound to
`tint-enabled` and through it to the global's tint or the chosen
mask's, as the section already switched its sliders by target. Off,
the ring and the two sliders dim and keep their values, as the
grading's wheels do; the swatch beside Reset keeps showing what a mid
grey becomes under the tint as it is set, dimmed with the ring, rather
than going grey, so the switched-off section still says what it holds.
Reset is as it was: the hue and the amount back to nothing, the switch
left alone. A flip is a `view-changed`, so it is a step in the history
and, on a mask, a look control under the overlay rule: with the
overlay showing, switching the tint either way hides it.

**The history.** The switch reads "Color tint on" and "Color tint off",
a mask's with its name before it, as every section's switch does. That
took the words from Reset, which read "Color tint off" when the amount
went to nothing; with a real switch beside it the two would have read
alike for two different things. Reset, the hue and the amount both back
to nothing, now reads "Color tint reset", as a mask's white balance's
does. An amount dragged to nothing on its slider reads "Color tint 0%",
the amount's own row.

**The copy sheet.** Tint has no parts, so the switch goes with the
section: `Section::copy` carries the whole tint and `same` compares it,
switch and all. A preset made before the switch reads as on, so laid on
a picture whose tint is off it turns the tint on, as laying any whole
section does.

**Checked.** The shader and the CPU both take a tint through
`vector()`, so the parity test cannot see the switch itself; what
covers off is the unit and panel tests. The parity test's random look
now draws the switch off 15% of the time for the global and every mask,
so switched-off edits round-trip through it, and it passes on lavapipe
at the default seed.

**Tests.** `tint.rs`: the default on; off, `applied` the input bit for
bit at three points where the same tint on moves it, the vector zero
and read back as nothing, the hue and the amount kept; a sidecar
without the field reads on, a tint on writes exactly what it wrote
before, and off writes the switch and reads back off. `history.rs`: the
global's and a mask's switch rows, Reset's row, an amount dragged to
nothing. `preset.rs`: the switch carried and compared with the section.
`panel/color.rs`: the global's and a mask's tint switched off through
the panel, the values kept on the sliders and in the edit, the vector
zero, the other target untouched, the swatch unchanged, through
`show_edit` and read back the same, on again the same tint, each flip
its history row, and on a mask a flip either way hiding the overlay.
`render.rs`: the parity test's random look draws the switch.
