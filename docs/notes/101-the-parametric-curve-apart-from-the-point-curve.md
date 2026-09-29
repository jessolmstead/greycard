# 101. The parametric curve, apart from the point curve (2026-09-19)

The roadmap's "separate parametric curve from the usual flexible
curve". §22 called the Light sliders a parametric curve in all but
name; they are not one, they act in linear light in stops, and a
user coming from Lightroom looks for the other thing: four region
amounts, three split points, a smooth curve drawn from them on the
same encoded axis the point curve lives on. `Parametric` in
`crates/greycard-edit/src/curve.rs` is that; the CURVES section
shows one curve or the other, and both apply.

**The edit.** A `parametric` value inside `Curves`, so it rides in
the same section, the same preset section and the same sidecar
field as the point curves; `#[serde(default)]` on both structs
means a sidecar without it reads as the identity and an old sidecar
loads unchanged. Four amounts, `highlights`, `lights`, `darks`,
`shadows`, in −1 to 1 (the panel shows ±100), and `splits`, the
shadow, midtone and highlight split points as fractions of the
encoded axis, defaults 0.25, 0.50, 0.75. The splits are kept in
order with `SPLIT_GAP` (0.05) between neighbors and from the ends:
`ordered_splits` sanitizes whatever a sidecar says, `set_split`
moves one within its neighbors. `is_identity` is true only when the
amounts are all zero; the splits alone change nothing.

**The curve.** Six nodes: (0, 0), one at the middle of each of the
four regions the splits cut the axis into, and (1, 1). Region *i*
of width *w* and middle *m* has its node at (*m*, *m* + *a* ·
REACH · *w*), *a* its amount and REACH 0.3. Between two nodes the
curve is the diagonal plus the nodes' offsets blended with a
smoothstep, *y* = *x* + *o*ₖ (1 − *s*) + *o*ₖ₊₁ *s*, *s* = *t*²(3 −
2*t*), *t* the fraction of the way across; the offset at both ends
is zero. The first draft put a Fritsch–Carlson cubic through the
same nodes, which is smoother in the second derivative but couples
the tangents: a full dip in the shadows lifted the darks by 0.7
percent above the diagonal, an effect of the wrong sign in a region
the slider does not own. The smoothstep blend is a convex
combination of two neighboring offsets, so an amount reaches its
neighbors' middles and no further, never changes sign on the way,
and every x outside those two intervals is untouched exactly. Its
slope is 1 + 1.5 · (*o*ₖ₊₁ − *o*ₖ) / *h* at the worst point, and
with |*o*ₖ₊₁ − *o*ₖ| ≤ REACH · (*w*ₖ + *w*ₖ₊₁) = 2 · REACH · *h*
that is at least 1 − 3 · REACH = 0.1: monotone for every amount and
every split, with a tenth of the slope left at the extreme of two
neighbors pulled fully apart. At the default splits a full amount
moves its node by 0.075, about 19 of 255.

**Where it acts.** Baked into the existing `CurveLut` as the first
stage: each entry's x goes through the parametric curve, then the
channel's own point curve, then the master; the fourth place holds
the parametric and the master. The shader and `finish.rs` read the
same table as before, so the viewport, the export and the local
adjustments (a mask's look has its own `Curves`) all pick it up
with no new lookup, and the CLI's `apply` writes it into a sidecar
like any other field. The order (parametric first) means a point
curve drawn afterwards sees the parametric's output as its input,
which is how Lightroom composes the two.

**The panel.** A mode Segmented, Parametric | Point, above the
editor; the point mode is as it was, with the channel Segmented and
the draggable points. The parametric mode shows the parametric
curve over the RGB histogram, a dim line at each split with a
triangle handle at its foot, and four EditSliders, Highlights,
Lights, Darks, Shadows, ±100 in steps of 1. The split handles use
the editor's `curve-press`, `curve-move` and `curve-release`
callbacks as the points do: a press within `CURVE_HIT` of a split's
x takes it, a drag moves it within its neighbors' gap, the release
records one history entry, a double-click puts that split back at
its default, and Reset puts the whole parametric curve back. The
Pick dropper is a point-curve thing and hides in the parametric
mode. A slider redraws the picture at once through
`parametric-changed` rather than waiting for the next histogram.
The mode is remembered in `settings.json` as `curve_mode`, beside
the scope. `draw_curve` in `main.rs` draws the picture as before,
with the parametric branch added.

**History and Lightroom.** `describe` names the moved control with
its value: "Curve lights −35", "Highlight split 80%", "Curves" when
a point moved or several controls did, and with an adjustment's
name before it as for the other sections. The Lightroom reader maps
`ParametricShadows/Darks/Lights/Highlights` (±100 → ±1) and
`ParametricShadowSplit/MidtoneSplit/HighlightSplit` (0..100 →
fraction), which it used to list as unmapped; the sample preset in
the tests carries them now.

**Checked.** Unit tests: the identity at zero (with the splits
moved too, and off whatever the amounts say); monotone at every
combination of five amount levels over four amounts and five split
sets, 3125 curves, with the nodes in order and the ends at 0 and 1;
each amount's peak lies in its own region, moves nothing beyond the
neighbors' middles and never the wrong way; the splits sanitize,
clamp and move the region; the bake composes parametric, channel
and master in that order; the sidecar round-trips. With a sidecar
carrying highlights −60, lights +35, darks −20, shadows +50 and
splits 0.2, 0.5, 0.8, the viewport at 1:1 and the export's matching
crop differ by 0.28 percent RMSE, the display profile off on both
(the §18 recipe), against 0.26 percent for the same raw with no
sidecar at all, which is the method's own residual; the curve
itself moves the export by 1.6 percent. A wrinkle worth keeping:
the frame was even on both sides (6000 by 4000) but the window the
compositor gave left a viewport 1410 by 1203, and the odd height
puts the 1:1 view half a row off any integer crop of the export,
1.4 percent RMSE as §18's memory says. Shifting the export's crop
half a row with a bilinear resample (`magick -filter point
-interpolate bilinear -distort SRT "0,0 1 0 0,-0.5"`) and
comparing again gives the figures above; the view's own height,
not only the frame's, has to be even for the plain crop to line
up. A snapshot of the CURVES section in the
parametric mode with that sidecar is the picture the roadmap asked
for. A `--panel-scroll PX` flag beside `--snapshot` scrolls the
panel down that many logical pixels so the snapshot can show a
section below the fold. The first draft set the scroll from an
environment variable before `app.run()`, and the review found it
did nothing three times out of three: the ScrollView puts its
`content-y` back to the top when it first lays out, while the
conditional sections have no height yet, and the one snapshot that
worked was luck. The flag applies the scroll where the snapshot
fires, once the first develop is on screen, and captures a further
moment later; two runs in a row land on the same section. Also
found in review: switching to the parametric mode with the curve
dropper in hand left it in hand with its button gone, so the next
press on the picture edited the point curve behind the parametric
one; the mode change puts the dropper down, and its press does
nothing in the parametric mode besides.

**Not yet.** The point curve is not drawn faintly behind the
parametric one, nor the other way round; the two are shown one at
a time. Lightroom shades the four regions behind the curve as
bands; the split lines and handles are enough to see them here. A
Pick dropper for the parametric curve (choose the region under the
pointer) is not built.
