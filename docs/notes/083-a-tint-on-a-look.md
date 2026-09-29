# 83. A tint on a look (2026-09-19)

The roadmap's "add a color (HSV?) to the mask area", which we
settled as a tint: Lightroom's local Color, a hue and a strength that
pull the masked area toward one color. A TINT section after COLOR
GRADING with two sliders, Hue and Amount, off at an amount of
nothing. `crates/greycard-edit/src/tint.rs` holds the edit and the
one function `finish.rs` and the shader both hold to,
`Tint::applied`.

**On the look, not the edit.** §77 kept the black and white off a
look on purpose — half a mono picture is not a mono picture. A tint
is the opposite case: it is exactly the thing a mask wants, and it
blends. So `Tint { hue, amount }` sits on `Look` beside the mixer and
the grading, which means the global picture carries one too and the
section shows on the Develop tab as well as the Masks tab, at no
cost: `read_look` and `show_edit` already move a whole look between
the panel and the edit. A new field with `serde(default)`, the amount
at nothing, which is what every older sidecar meant, so no version
bump — §16's rule is that a field with a default is added freely and
only a field that *changes meaning* bumps `VERSION`.
`Section::Tint` for presets, serialized `tint`.

**The maths.** In Oklab, at the pixel's own lightness. A floor of
chroma follows the lightness, `CHROMA * max(0, 1 - ((L - Lm)/Lm)²)`
with `CHROMA` 0.1 and `Lm` 0.5646, mid grey's lightness (`0.18^⅓`):
a bell peaking at mid grey, nothing at black and nothing again at
twice that lightness, where the scene is white, so the tint is a
mid-tone thing, as Lightroom's is. The chroma goes to `chroma +
amount * (max(chroma, floor) - chroma)`: it rises to the floor and
never falls, so the tint colors and never washes out, and a
saturated patch keeps the saturation it had.

**What the bell gates, and what it does not.** It gates the lift and
not the turn, and that is the whole of what "a black stays black and
a white stays white" means here. A *neutral* outside the bell is
given no chroma, so it comes out the neutral it went in; a pixel that
already has color there keeps every bit of that chroma and still has
its hue turned, at the top of the scale as anywhere. So a saturated
pixel above an Oklab lightness of 1.1292 — about 1.44 linear, above
scene white, which is where a rebuilt or pushed highlight lives —
lands on the tint's hue at a full amount with its chroma intact. That
is the behavior wanted: it is how a bright sky is hand-colored, and
a tint that gave up above scene white would leave the brightest part
of a mask behind while the rest of it moved. The hue turns toward the tint's by the share of
the chroma the tint answers for, `amount * target` against `(1 -
amount) * chroma`, the short way round. That share is what makes the
control behave at both ends without a confidence fade of the mixer's
kind (§63): a neutral has no chroma of its own, so its share is one
and it takes the tint's hue outright rather than a hue read off its
own noise; a color as saturated as the floor meets the tint halfway
at half the amount; a full amount replaces the hue rather than adding
to it. An amount of nothing is a return, not a round trip, so it is
the identity bit for bit.

**Where in the pass, and the mono question.** Last in the Oklab pass
of `mix_with`: after the mixer, after the global saturation and
vibrance, and after the black and white's chroma drop. After, not
before, and deliberately — Lightroom lets a local Color tint a black
and white, which is how a picture is hand-colored, and that is worth
more than a tidy rule. §77's guarantee is unharmed: it was about
controls that *scale* chroma, and nothing that scales chroma runs
after the conversion still. The tint adds, as the point curves and
the grading wheels add, which is what toning and coloring are. A
mask with a tint over a mono picture is a hand-colored patch, and
the rest of the frame stays exactly neutral.

**The blend, as a vector.** A hue does not average, so a tint blends
into a look as `amount` in its hue's direction: the global look's
vector plus each local's times its mask weight, then the length back
to an amount (capped at one) and the direction back to a hue. Two
masks at half a hue each are one mask at the hue between them, which
is how the three grading wheels already sum into one shift (§26). The
shader holds it the same way, a `vec2` in the uniform and one per
local, so the blend is the same arithmetic on both paths.

**What a file may say.** A hue and an amount are brought into range
as they are read, in the one place a sidecar, a preset and a
Lightroom import all pass through: the hue round the circle, the
amount to its ends, and anything that is not a number at all to
nothing. Two paths read a tint — the finish blends every look's as a
vector and reads one back off the sum, the dropper takes one straight
— and a hand-edited hue of 1e30 would have read differently on the
two. `pick` now takes the same vector round trip as well, so the two
cannot part company at all.

**The panel.** A Hue slider on a track painted with the Oklab hue
circle at the grading wheels' lightness and chroma, a stop every
thirty degrees, so which color each way goes to reads without
dragging — the mixer's hue slider's trick (§38, §60). An Amount
slider, and beside Reset a swatch of what a mid grey becomes under
the tint as it stands, which is the whole control in one square. No
switch on the section: an amount of nothing is off, and a switch
would be a second way to say the same thing. History says "Color
tint 210°", "Color tint 50%" or "Color tint off", with the mask's
name before it for a local — "Color tint" and not "Tint", since the
white balance's Duv already answers to that name. "Off" is said only
when it was on: picking a hue with the amount still at nothing is the
ordinary way round the control, and turns nothing off.

**Checked.** `5M0A3976.CR3` cropped to 3600x2400, a global tint at
250° and a quarter, a radial mask tinted 40° at 80 percent and a
linear one 200° at 60: the viewport at 1:1 and the export's crop
differ by 0.40 percent RMSE and 0.11 percent mean, against 0.23 and
0.094 for the same edit with every amount at zero. The extra is
misregistration, not the tint — the half-pixel row blend at an odd
viewport height meeting a hue change at an edge. Blur both by two
pixels to take the misregistration out and the two read 0.201 and
0.196 percent; over a flat 200-pixel patch inside the radial mask,
where the tint moves the picture by 7.4 percent, the tinted and
untinted agreements are 0.1928 and 0.1926 percent. The tint moves the
whole export by 5.5 percent RMSE, so it is plainly doing something.
Hand-coloring: the same two masks over a Red-filter black and white,
viewport against export 0.24 percent, the untinted baseline; the mono
export's mean HSL saturation is 1.3e-7 and the hand-colored one's
0.33, 4.6 percent RMSE apart, and the frame away from both masks is
exactly neutral. Tests: a mid grey at a full amount lands on the hue
with the floor's chroma, at every hue and at half the amount for half
the chroma; a neutral at either end of the scale and beyond it
untouched; the bell symmetric about mid grey; a saturated pixel past
the bell, at a lightness of 1.5, landing on the tint's hue at a full
amount and halfway at half, its chroma kept both times; a saturated
red turning the short way and keeping its chroma, halfway at half; a
pale color giving way sooner than a saturated one; the vector round
trip, its sum and its cap; a sidecar's hue brought round the circle
and its amount held to its ends as they are read, with a hue of 1e30
and one of 1e300 among them, and a normalized tint reading the same
taken straight as through the vector round trip; an amount of nothing
the identity through `mix_with` and through the whole finish; the tint coloring a mono picture with the tint's
own hue while the untinted mono keeps no chroma at all; a mask
blending it by weight and two masks at half a hue each equalling one
at the hue between; and the history rows, global and per mask.

**Not.** No Lightroom import: `LocalTint`/`LocalHue` live in that
file's local corrections and the importer still reads global sections
only (§52), so a Lightroom Color mask does not come across. No
saturation of its own beyond the amount, and no luminance: the tint
holds the lightness, and the Light sliders in the same look are the
answer to a patch that also wants to be brighter. No dropper to pick
a hue off the picture; the swatch and the painted track were judged
enough for now.
