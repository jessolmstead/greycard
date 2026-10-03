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

**Where we left it.** Two finalists, read two ways. ACES 2.0 won the
frames where correctness shows: the gates, the lanterns, skin in
shade. AgX Punchy won some frames on look: its chosen skew and its
extra contrast flatter a warm lit scene. The difference is that a look
can be laid over ACES 2.0's output, by the Look section and the camera
match tables, while AgX's hue behavior is in its primaries and cannot
be taken off. So the shape of the decision is likely ACES 2.0 as the
transform and a look on top that reads like AgX Punchy where it won,
and the call waits on a run of the tool over the whole test set and a
shoot or two, frame by frame, with an eye on skin across the range.
Not called tonight. The sigmoid shape item is a fallback to this, not
the plan, and part (1b) would become a port of ACES 2.0's chroma
compression rather than a control of our own.

**The cost, measured.** OpenColorIO's own CPU path on this machine,
one thread, 24 megapixels of scene-linear: ACES 2.0 SDR with the
matrix and the encoding, 243 ns a pixel, 5.8 s; the same path with no
tone mapping, 9 ns; our norm switch 68 ns (§226's review); the
per-channel fit 7 ns. An export is parallel over every core, so a 100
megapixel frame takes about a second more on 32 threads over roughly
seven today. The viewport is the shader's: three to four hundred
operations a pixel by the count of the model, a matrix, three
fractional powers each way, an arctangent, a sine and cosine, and the
gamut cusp from a table per hue, which OpenColorIO precomputes and a
port would too. Under a millisecond at 4K on a discrete GPU; a few
milliseconds on an integrated one, which is the machine to time before
any default.
