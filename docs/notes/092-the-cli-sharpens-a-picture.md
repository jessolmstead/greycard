# 92. The CLI sharpens a picture (2026-09-19)

§88's own line on the list is closed: `--sharpen` on a JPEG, PNG or
TIFF did nothing, because `run_develop_picture` took no develop
settings at all, where a raw's `run_develop` had already been fixed
to call `correct_and_sharpen`, which runs `correct_lens` and then
`sharpen::sharpen`, after it develops. `run_develop_picture` now
takes the same `Option<sharpen::SharpenOptions>` `--sharpen` and its
`--sharpen-radius`, `--sharpen-iterations` and `--sharpen-contrast`
flags already built (they were parsed into `DevelopSettings::sharpen`
all along; only the picture path never read the field) and calls
`correct_and_sharpen` in place of the bare `correct_lens` it used to
call, so the lens correction and the sharpen run in the one order on
both paths through the one function.

The two arguments that function needs beyond the options are where
a picture differs from a raw, and both come from the worker's own
`Input::Picture` arm in `develop_job` (`crates/greycard-ui/src/
worker.rs`), read as the reference: `radius` is always `None`, since
a picture carries no mosaic for `measure_radius` to read a point
spread from (`Radius::Auto` falls back to `sharpen::DEFAULT_RADIUS`,
0.75, the same as a raw whose mosaic could not be measured); and
`clip_level` is `Picture::clip_level()`, the same constant
(`develop::CLIP_FRACTION`, 0.98) the worker reads off its `Picture`
for this input, rather than a raw's own headroom-derived level. Nothing
else about a picture's develop changes: no demosaic, no denoise, no
mosaic-measured anything, so the sharpen is the only settings field
that was ever missing from this path.

Two more flags were silently dropped on the same path and are fixed
alongside it. `--preview` on a picture always rendered at as-shot
exposure, `srgb_preview(&p.image, 0.0)`, ignoring `--exposure`; it now
takes the same `exposure: f64` the raw path's `--preview` target
does. And `--ai-denoise` on a picture was never looked at, since
`run_develop` returns to `run_develop_picture` before reaching the
raw-only code that reads it; the learned denoiser needs a Bayer
mosaic to denoise, which a picture does not have, so `run_develop`
now prints one line to stderr saying the flag is ignored on a
picture, rather than accepting it and doing nothing.

A test pins it the way the two raw-path tests in §88's original
addendum do, but on `run_develop_picture` itself rather than on
`correct_and_sharpen` directly, since the wiring from CLI flags into
that shared function is exactly what was missing: it writes a
synthetic checkerboard PNG, develops it twice (with a manual lens
distortion that moves pixels, once with `--sharpen`-equivalent
options and once without), and reads back both TIFFs to compare
against `sharpen(lens(x))` and `lens(x)` computed independently from
the same decode; a third, independently computed `lens(sharpen(x))`
is checked to differ from `sharpen(lens(x))` by more than a rounding
error, the same guard the raw-path test uses, so the test proves the
order and not only the equality. The two real outputs differ, and the
sharpened one matches the independently-computed reference to the
pixel. Run by hand on `4Z4A3525_greycard.jpg` from
`~/Pictures/Test/export` (an 8-bit sRGB JPEG, 4098x5464): `--sharpen`
now reports "sharpened: radius 0.75 (no mosaic to measure), 20
iterations, contrast threshold 28%, 38% of the picture, 0.00%
clipped" and the TIFF it writes differs from the one without
`--sharpen`, where before the fix the two were byte-identical since
the flag was silently ignored on this path.
