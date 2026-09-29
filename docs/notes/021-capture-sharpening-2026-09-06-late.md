# 21. Capture sharpening (2026-09-06, late)

The first item off our list, and the one the resize made
urgent. Ported from RawTherapee's capture sharpening (Ingo Weyrich,
2019, GPL): Richardson–Lucy deconvolution of the luminance under a
Gaussian point spread, blended in by local contrast so that flat
areas keep their noise, with the point spread's width read off the
mosaic. `crates/greycard-core/src/develop/sharpen.rs`, the last op
of the develop; `--sharpen` on the CLI; a DETAIL section on the
panel, on by default in the edit (§16) and off by default in the
engine, which takes no taste (§5).

**Why deconvolution and not an unsharp mask.** An unsharp mask adds
a scaled high-pass and halos in proportion. Richardson–Lucy assumes
what is true of a lens and a filter stack: linear light through a
blur, and iterates toward the picture that, blurred, gives what was
recorded. It works in linear light, which is where our working image
already is; RawTherapee runs it on its own linear luminance too. The
sharpen is a gain on all three channels from the luminance's change,
so color ratios hold exactly (a test checks). Twenty iterations by
default, RawTherapee's; more is sharper and, past a point, ringed,
and each tile stops on its own when any pixel falls under half its
blended start, RawTherapee's guard against dark halos.

**Ported as written, with these differences.** The truncated square
Gaussian is applied as two one-dimensional passes, which it is
exactly (the kernel is an outer product), for a fifth of the work.
Tiles at the picture's edge clamp their reads so the outer pixels
are sharpened too; RawTherapee leaves a border. The clip mask is
taken on the working image against the develop's ceiling (§13), with
the same two-pixel widening. Units are the working space's (luminance
to one, L* to a hundred) rather than RawTherapee's scaled integers,
with the tile statistics' limits converted; the conversion matters
(see below). The contrast measure, the sigmoid (half at the
threshold, `x = 16 (c/t − 1)`), the two-pass search for the flattest
patch and the one-percent rule for the automatic threshold are as
in `rt_algo.cc`.

**The radius from the mosaic.** RawTherapee reads the point spread
off the sharpest pair of diagonal green neighbors: taken as a point
source under a Gaussian, their ratio gives the standard deviation as
`sqrt(1 / ln ratio)`. Two things learned porting it. A pure Gaussian
star with a faint pedestal fools it: for a wide star the largest
ratio is between two photosites out on the flank, not at the center,
so a test on stars only holds for sharp ones, and the test now uses
an edge, whose ratios are bounded and rise monotonically with the
blur, which is what the estimator is for. And a hot photosite wins
the contest outright, setting the radius for the whole picture: on
our six test files the raw estimate was 0.40 to 0.78, and three of
them were spikes (0.40 → 0.45, 0.55 → 0.58, 0.47 → 0.60) once a
sample more than four times brighter than every same-color
neighbor two away is refused as an edge. RawTherapee's own hot pixel
filter runs before its estimate; ours is opt-in (§13), so the guard
lives in the estimator. Clamped to 0.4 to 2.0 either way.

**Measured.** On the six test files the radius is 0.45 to 0.78, the
automatic threshold 8 to 13 percent at ISO 100 to 250, and the blend
covers 29 to 66 percent of the picture; the ISO 1000 DNG gets a 26
percent threshold and 1 percent of the picture, which is the rule
working as meant: the flattest patch is noisy, so almost nothing
clears it. A denoise first lowers the threshold, which is the order
the pipeline has. Cost: 0.46 s on 24 MP, 1.3 s on 45 MP, on top of
1.0 and 2.3 s for the develop. In the editor a sharpen change costs
only the sharpen: the worker keeps the develop before it (the edit's
`same_base`) and applies the sharpen on a copy, the first cached stage
and the pattern for every op after the demosaic; `develop()` with
the sharpen in its settings is the reference the worker is held to.

**A bug worth recording.** The first run reported 62.52 percent of
every 45 MP picture clipped and 30.09 percent of every 24 MP one:
identical across pictures, which is never content. A single-precision
sum of ones stops at 2^24, 16.8 million, and both fractions are
`1 − 2^24 / pixels`. Means over a picture are now summed in double;
so should any other whole-picture total be, and §13's bench sums are
already f64.

**Not yet.** Output sharpening after a resize (§15). The corner
radius offset RawTherapee has for lenses soft at the edges, which
needs a per-lens notion the engine lacks. And the panel shows what
the automatics measured beside their sliders, but not the mask; a
mask view is a viewport mode for later.
