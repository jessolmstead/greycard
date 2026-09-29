# 45. Manual chromatic aberration (2026-09-15)

The last of the editor list's short items, and the one §39 left
under "not here". Two sliders in the LENS section, Red / cyan and
Blue / yellow, each the fraction by which that channel's radius is
scaled beyond the green's, plus or minus a percent, shown in tenths
of a percent; `--lens-ca-red` and `--lens-ca-blue` on the CLI's
develop.

**One resample.** The manual distortion of §39 over a profile's is a
second correction and a second cubic resample, because two `poly3`
distortions do not compose into one. A scale on top of the database's
CA model does: `m (v + c r + b r²)` is the same polynomial with every
coefficient times `m`. So `ChromaticAberration::scaled` folds the
manual correction into the profile's and the picture is resampled
once, with each channel read at its own place as before. With the
profile's CA off, or no profile, the manual model stands alone; with
a manual distortion making a second correction, the aberration rides
with the first. Tests check the fold against the scale applied after
the model, and each placement.

**Sign.** Positive pulls a channel that landed too far out back in,
which is the common case for red on the long end of a zoom. The
range is a percent of the radius, thirty pixels at the long edge of
a 24 MP frame, several times what any lens the database knows shows;
the step is a hundredth of that, a third of a pixel there.
