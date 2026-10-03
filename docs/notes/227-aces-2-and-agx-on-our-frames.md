# 227. ACES 2.0 and AgX on our frames, by their output (2026-10-03)

The first look at §226's switch on a real frame, a row of torii gates
with the sun on them, was bad: the sunlit gates washed from orange to
salmon across the bright half of the picture, the lanterns' collapse
(§226) on a whole scene. We asked what the other transforms do with
the same frame, and `tools/compare-transforms.py` answers that the
cheap way, by their output rather than by a port:
greycard's own scene-linear Rec.2020 develop (`greycard develop
--output`) run through OpenColorIO's built-in ACES 2.0 studio config
and Sobotka's AgX config, each given the exposure that puts its median
luminance at greycard's per-channel export's, so only the transform
differs. Six frames: the gates, the lanterns at night, the headland
with the sun, the jets, a skin tone in shade and a backlit couple.

**The gates.** Per channel, the sunlit gates stay saturated orange and
drift toward yellow at the top, which is the picture we know.
Hold hue to white washes them to salmon: the worst of the five. ACES
2.0 keeps them saturated all the way up, redder and deeper than per
channel, the hue held rather than sliding to yellow, with more
contrast in the shadows; the best render of the gates, and what holding
hue is supposed to look like, the chroma kept into the highlights and
let go only at the very top. AgX Punchy is close to per channel, a
touch more saturated and warmer, the gates going yellow-orange at the
top by its chosen skew. AgX base is flat and pale, a log base meant to
be graded, and not a bad place to start from either.

**The lanterns.** The same order. ACES 2.0 renders a lit lamp:
orange-salmon at the core, warm, the hue held and the chroma kept until
just under white. Per channel is a saturated orange-yellow. The norm
switch is pink paper. AgX Punchy is orange, less yellow than per
channel; AgX base pale.

**Skin.** The one that says something about the default rather than
the switch. Per channel renders the shaded skin a deep tanned orange,
and the switch is the same; ACES 2.0 and AgX Punchy render it paler and
more neutral, closer to how skin in shade reads. Our judgment on
seeing them side by side: ours, per channel, is over-saturated. That is
not the display curve's hue drift; it is the chroma the per-channel
curve adds in the mid-tones, the slope's spread (§226), which the
camera JPEG and Lightroom temper and the ACES and AgX designs do not
have. A line on the feel track asks for the measurement.

**The sky and the backlight.** The headland, the jets and the backlit
couple tell the transforms apart far less: ACES 2.0 and AgX a little
lower in contrast with the haze more visible, the jets' sky a shade
bluer under ACES, otherwise alike. The hue work is a highlights
question; most frames never reach it.

**What it settles for the roadmap.** The switch's problem is not
holding the hue, it is the mean norm with the geometric step taking
chroma a stop and a half early (§226); ACES 2.0 holds hue on the same
frame and keeps the color, because its chroma compression is shaped to
keep a saturated light colorful until near white. So part (1b)'s
control has a measured target: ACES 2.0's output on the gates and the
lanterns, and the question is which of its three options gets nearest
with the least machinery. AgX Punchy being within reach of per channel
says the chosen yellow skew is a modest change, not a different look.
And the skin frame moves the default's saturation from a feeling to an
item.

**The tool and its limits.** The renders are not pixel-aligned:
greycard's export and the CLI's linear develop frame the picture a
little differently (the export applies the lens profile and the
camera's crop), so the comparison is by eye and by median, not by
pixel. The exposure match by median luminance is fair for a look and
rough for a measurement; the ACES and AgX renders took 1.7 to 2.2
stops and 0.1 to 1.2 stops over the linear develop, respectively,
which says where each puts mid grey. The script's docstring has the
venv and the configs; the AgX config is cloned under `target/`, not
checked in.
