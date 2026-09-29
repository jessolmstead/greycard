# 47. The sharpen's mask in the viewport (2026-09-15)

What §21 left: the panel showed the radius and the threshold the
automatics measured, and the status line how much of the picture the
blend covered, but not where. A threshold is set by looking at where
it falls.

**What it is.** Show mask in the DETAIL section, greyed with the
sharpen off: the blend mask (the clip mask times the sigmoid of local
contrast, blurred, §21) painted red over the picture where the
sharpen acts, the same red and the same strength as a local
adjustment's mask, so the two views read as one. The clipped candle
cores on the chapel frame come out unpainted with a red rim, which is
the mask's two-pixel widening of the clip seen from the outside; the
bokeh stays dark; the stonework is solid. `--sharpen-mask` paints it
for a screenshot.

**How.** The sharpen hands its blend back: `sharpen_with_mask` in
the core returns the stats and the plane, and `sharpen` is that with
the plane dropped, so the develop and the export are as they were. A
test checks the plane is the sharpen's own (its mean is the stats'
blend mean, the edge near one, the flat under a hundredth, the
picture identical to a plain sharpen's). The worker already uploads
the developed picture as four halves a pixel with the alpha at one
and unread; the blend goes in the alpha instead, zero when the
sharpen is off, so the mask reaches the viewport with no texture, no
binding and no copy of its own, and is sampled through the same
bilinear or cubic fetch as the picture, at the same source position.
The shader's cubic fetch returns four channels now. The flag rides in
the params' spare `cubic.y`. The scopes, the navigator and the
export read the RGB and never the alpha, checked.

**Not.** The CLI writes no mask file; the mask is a thing to look at
against the picture, and the viewport is where that happens.
