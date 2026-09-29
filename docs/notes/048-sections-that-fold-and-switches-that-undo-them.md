# 48. Sections that fold, and switches that undo them (2026-09-15)

Asked for: each section of the panel to fold away, and to carry a
switch that takes its effect out of the picture so the effect can be
judged by its absence.

**The header.** A chevron at the left, the icon and the title, and
for a section with an effect a switch at the right. The header folds
the body away and back, the chevron turning; the body is clipped to
nothing, over 120 ms, rather than removed, so its state is not
rebuilt on every fold. The folds are kept in the settings file by
name, so the panel opens as it was left. The switch is smaller than
a Toggle's, since a header is not a row; off, the body dims to
show its settings are held and not acting.

**What a switch is.** A section's switch is a field in the edit,
`enabled`, and travels in the sidecar, so an export or a later
session sees the same picture. Curves, the mixer, the grading, the
tone curve, the sharpen and the profiled denoise had one already,
each on a toggle in its body; those toggles are now the header's
switch, and the Reset stands alone. Six parts had none and have one
now: the light (exposure and the tone curve together; the tone
curve's own toggle stays for the curve alone), the noise section as
a whole, the lens, the vignette, the grain and the retouch. White
balance, geometry and demosaic have no switch, since there is no
picture without them; the navigator and the adjustments list are not
effects. The soft proof's switch is its own on the header, with the
S in the header's hint.

**Where the test lives.** One place each. `Light::effective` is the
light as it acts, exposure zero and the curve off when the switch is
off, applied where a look is baked for the CPU finish and where the
viewport's view is made, so the two agree by construction: a sidecar
with the switch off and three stops of exposure renders pixel for
pixel as one with no exposure and the curve off. The vignette's and
the grain's `is_off`, the lens's `is_identity`, `wants_profile` and
`corrections`, and the retouch's `is_empty`, `apply_with` and
`choose_sources` answer for their switch, so every consumer, the
worker, the export, the model previews, follows without knowing. The
noise section's `tier` and `profiled_runs` are what the worker and
the settings ask now. One slip on the way: the export asked
`is_off` for the vignette and the grain, but the viewport handed the
shader the amounts as they were, so the two switches worked in the
file and not on the screen. The renderer takes the amounts through
the off-tests now, a zero being nothing to do in the shader, and a
sidecar with each switch off renders pixel for pixel as one without
the effect. The lesson is the one §21 has: a switch that lives in a
part's own off-test still needs every reader to ask that test.

**The schema.** Version 2. Version 1's `noise.enabled` was the
profiled denoiser's toggle; it becomes `noise.profiled`, and
`noise.enabled` is the section's switch, on. `migrate` moves the key
for anything under version 2, the sidecar's history included, and a
test reads a version 1 file and finds the profiled pass it asked
for, still running. Every other new field defaults to on under
`serde(default)`, so a version 1 file reads as before. Checked on a
real version 1 sidecar with fifteen steps of history: it opens, and
a screenshot run leaves the file unwritten.
