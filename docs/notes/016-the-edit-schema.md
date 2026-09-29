# 16. The edit schema (2026-09-06)

`crates/greycard-edit`: the edit as a typed struct in physical units,
with a schema version and a sidecar. The engine takes no schema (§5
rule 1 and the layout rules); this crate depends on the engine and
turns an `Edit` into `DevelopSettings`, and the viewport reads the
rest (exposure in stops, the tone curve) from the same struct. What is
in it today: `light` (exposure, the tone curve's switch and a contrast
that the viewport does not yet honor), `white_balance` (as shot, or
kelvin and Duv), `noise` (on, strength), `demosaic` by the engine's
names. Every field has a default and every struct is `serde(default)`,
so an older sidecar loads whole and a newer build's extra fields are
ignored on the way back; a field that changes meaning bumps
`VERSION` and gets a case in `migrate`; a sidecar from a newer version
than the build knows is refused, not half read. Names are the
serialized form, so a sidecar reads as prose: `"mode": "custom",
"temperature": 3200.0`.

The sidecar (`FILE.gcd` beside the file, JSON inside, written whole
through a temporary name) keeps the current edit and up to fifty
earlier states, each a whole edit, oldest first: §5 rule 9, history
rather than overwrite, in its simplest form; an undo stack and named
versions can be built on it without changing the file. The UI writes
it 800 ms after the panel rests and when a file is left, and reads it
when a directory opens. Checked: a run that developed at 3200 K and
half a stop, quit, and a second run with no flags produced the same
viewport to the byte. `--no-sidecars` leaves the disk alone.

**Contrast (2026-09-06, late).** The first tone edit beyond exposure:
`light.tone.contrast`, a power about mid grey in the working space
before the shoulder, so the mid-tone slope changes and the top still
rolls off. In the shader, previewed at once, saved in the sidecar.
Checked by sidecar-driven screenshots at 0.6 and 1.6: flat and lifted
against deep and saturated, as a power in RGB does (it moves
saturation with slope, which is the usual behavior of a contrast
slider and can be revisited when the curve gets its own design).

**The histogram (2026-09-06, late).** The whole image drawn 512 wide
through the viewport shader into an analysis texture, a compute pass
binning the encoded output into 256 bins per channel through atomics
in a storage buffer, and three kilobytes read back a frame later
(the read back is asked for, the frame is asked for again, and the
bins are there next time). A new analysis only when the image or the
edit changes, since an analysis that asked for a frame that asked for
an analysis ran the window at sixty frames a second doing nothing,
which the frame counter caught. Drawn as a 256 by 80 picture, three
channels additive on a square-root scale, the end bins (everything
clipped) left out of the scale so a spike does not flatten the rest.
Two things learned: Slint's `WGPUSettings` default asks the device for
downlevel limits, which allow no storage buffers at all, so the app
asks for desktop limits; and the histogram is of what is on the
screen, encoded, which is what editors show and what the user's eye
can be matched to.

**Undo, redo, a visible wait, and keys (2026-09-06, late).** The
sidecar's history is now walkable: undo and redo buttons beside the
file name, `Ctrl+Z` and `Ctrl+Shift+Z` (or `Ctrl+Y`), each step a
whole earlier edit, the redo stack kept for the session and not
written. Whatever the panel holds when undo is pressed is recorded
first, so nothing is lost to the 800 ms rest. The left and right
arrows step along the strip when no slider holds them (Slint passes
an unhandled key up through the focus scopes, so the sliders' arrows
and the window's coexist). And while a develop is on its way the
status plate carries a running bar, so the denoise slider, the one
control that cannot preview, reads as working rather than stuck.
