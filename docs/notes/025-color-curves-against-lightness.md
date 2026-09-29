# 25. Color curves against lightness (2026-09-07)

The first of the three color items left on the roadmap's Next up:
the Lab `a` and `b` curves of our list, here two curves in the
CURVES section beside the point curves, R/G and B/Y. Each is a curve
against lightness with the neutral through the middle: up is red or
yellow, down is green or blue, and where the curve sits on the level
line nothing happens.

**What it does.** After the point curves and before the output
matrix, each pixel goes to Oklab, `a` and `b` are shifted by what the
two curves say at its `L`, and it comes back. `L` is the curve's x,
and `L` is held, so a curve tints without lightening or darkening:
the test checks Oklab's `L` of a shaded grey to within 1e-5. Oklab
rather than CIE Lab for the same reasons as the mixer (§23): its hue
lines are straighter and the mixer's matrices are already in the
shader. Full deflection of a curve is a shift of 0.2 in `a` or `b`
(`COLOR_RANGE` is 0.4 across the picture), about the chroma of a
display primary; it is generous, and a curve at three quarters is
already a strong grade in the shadows, since a shadow has little
chroma of its own for the shift to compete with. The range is one
constant if it turns out too coarse.

**Where it sits.** Lightness after the tone curve and the point
curves, so "the shadows" are the shadows the user sees on the
histogram, not the scene's. Black and white are never moved: near
black the cube on the way out of Oklab makes any shift vanish, and a
curve's ends default to neutral. Near white a shift clips a channel
rather than darkening, which is the honest answer to tinting white.

**Baking and the shader.** `Curves::bake` now returns a struct:
the tone table as before, a color table of 256 (`Δa`, `Δb`) by `L`,
and a flag saying whether any shift is non-zero, so the viewport and
the export skip the round trip to Oklab when the curves are level.
The shader's curve uniform grew from 256 to 512 vec4s, the color
half padded, 8 KB per draw; the histogram's pass draws through the
same shader, so the bins follow the curves as they do the point
curves. A sidecar without the two new fields reads them as level
(serde's defaults; no schema bump, the meaning of nothing else
changed).

**The picture.** The same `CurveEditor`: the ground is tinted toward
what each way gives, strongest at the edges, the line that does
nothing is the level one through the middle rather than the
diagonal, and the histogram behind is of lightness: the channels'
mean resampled from the encoded axis to Oklab's `L`, which for a grey
is the cube root of its linear value, so the shadows spread out as
they do on the curve's own axis. `GREYCARD_UI_CURVE_CHANNEL=R/G`
picks the curve the dump shows.

**Checked.** With both curves in a sidecar (red and blue into the
shadows, yellow-green into the highlights) the viewport at 1:1 and
the export's crop differ by 0.05 percent RMSE, and by 8 percent from
the run without them. A lesson about the check itself: with the
window's viewport an odd number of rows high, the centered view at 1:1
samples between texel rows and no crop of the export matches it
(1.5 percent, then 0.35 with the two rows averaged); a crop in the
sidecar with an odd height puts the rows back on texel centers.

**Not yet.** Color grading (next) is the same shift with three
wheels rather than a curve: shadows, mid-tones and highlights each a
hue and a strength, blended by `L`; it composes with these curves by
adding to the same table. A luminance-vs-luminance curve with hue
held is still not built (§22).
