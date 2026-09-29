# 87. The contrast filters against the real glass (2026-09-19)

Asked, looking at §77's presets: they do not read like a red or a
yellow filter does in Lightroom, in RawTherapee or on film. They do
not, and the strength control the roadmap now asks for is the
smallest of four reasons. Worth writing all four down before any one
of them is touched, because they pull in different directions and
only one is plainly just a number.

**The base grey is not a luma mix.** §77 took Oklab's lightness with
a and b at zero, so the unfiltered conversion is the pixel's
perceptual lightness and a mid blue and a mid yellow come out at the
same grey. Everywhere else the baseline is a weighted sum of the
channels, blue near 0.07 and green near 0.72, which puts a sky well
down and foliage well up before a filter is chosen at all. So the
filters here do not only move less, they move from somewhere else:
half of what a Lightroom user reads as the red filter darkening the
sky was the luma mix, already there at every slider zero. This is the
largest of the four by eye and the least obviously wrong. A
perceptual base is the more defensible conversion and it is what
makes neutral in, neutral out exact; changing it changes what None
means and every mono edit already written.

**One stop is the whole range.** `RANGE` is 1.0, so the Red filter's
deepest cut is the one stop at Blue and nothing can be dialed past
it. A Wratten 25 red drops a blue sky about three stops below the
panchromatic rendering, a 15 orange about two, an 8 yellow about one.
Greycard's Red is doing roughly what a real yellow does and its
Yellow about half of that: the set is compressed into a third of the
range it is named for. This one is just a number.

**The response is flat above a chroma of 0.03.** `mixer::confidence`
smoothsteps to one at `CHROMA_FULL` and clamps, so a pale horizon at
0.04 takes a band's weight as fully as a deep zenith at 0.15. §77's
"Not." owns this as a deliberate match to the mixer's own sliders,
and for the mixer it is right. For a filter it is the thing that
changes how a sky *reads* rather than how far it moves: real
absorption is proportional to how much of the blocked light is
there, which is what gives a filtered sky its gradient, near-black
overhead and open at the horizon. Flat weighting hands back a
uniformly darker band instead.

**A band reaches its neighbors and stops.** `Mixer::weights` splits a
hue linearly between the two centers it lies between, so a weight on
Green is exactly nothing at Blue. A filter's transmission is broad
and overlapping: the red glass pulls cyan, green and blue down
together with a smooth falloff. The centers are unevenly spaced too
(25, 60, 100, 140, 195, 265, 300, 335), so that falloff runs over 35
degrees between red and orange and 70 between aqua and blue.

**And two in the tables.** Red cuts deepest at Blue (-1.0) rather
than at Aqua (-0.8), where a red filter against panchromatic
sensitivity actually costs most, and it lifts Magenta (+0.2), where
the real glass passes magenta's red and blocks its blue and the band
should sit near neutral or a little down. Green's Blue at -0.3 is
mild against a 58 green, which darkens a sky nearly as a yellow does.

**What is right.** The gain is a true linear-light gain. Oklab's L
goes as the cube root of luminance, so `2^(stops/3)` on L is exactly
`2^stops` on the light, which is what a stop means. None of the above
is a complaint about `BlackWhite::light`.

**The strength control.** A `strength` on the section, zero to three
or so, as a scalar on the summed gain in `BlackWhite::stops`: the
presets keep their shape and keep today's look at one, and the range
above one is what reaches the real glass without the tables being
rewritten. A separate field rather than baked into the weights, since
`serde(default)` of 1.0 is what every older sidecar meant and §16's
rule then needs no version bump. The alternative, raising `RANGE` and
dividing the tables to match, buys the same reach but changes what a
stored weight means, so it costs a `VERSION` bump and a `migrate`
case for nothing. `BlackWhite::filter` keeps its exact match on the
shape, so the panel still says Red while the strength says how much
of it, and a hand-moved band is still visibly custom.

**What the strength does not fix.** The second of the four and
nothing else. A sky under Red at three will be very dark and still
flat across its gradient, and None will still start from perceptual
lightness. Those two are their own decisions and judging either is a
question of how it looks, not what it measures, so both wait on
§85's reference frames.
