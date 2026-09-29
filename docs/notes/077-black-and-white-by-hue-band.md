# 77. Black and white, by hue band (2026-09-18)

Asked for on the roadmap: strong black-and-white support, not just
desaturate. A BLACK & WHITE section between COLOR MIXER and COLOR
GRADING, off by default, whose conversion weighs each hue band's
contribution to the grey as a contrast filter over the lens does.
`crates/greycard-edit/src/bw.rs` holds the edit and the one function
`finish.rs` and the shader both hold to, `BlackWhite::light`.

**What it is.** Eight weights, -1 to 1, in the mixer's eight bands
(§23), and a row of the classic filters above them: None, Red,
Orange, Yellow, Green, Blue, each a set of weights, so a red filter
lightens skin and darkens a blue sky in one click. The panel says
which filter the weights are and shows nothing chosen when they are
no filter's, so a hand-moved band is visibly custom without a
"Custom" entry of its own.

**The maths, which is the mixer's.** A weight is a gain on that
band's grey in stops, one stop at a weight of one, the same stop per
unit as the mixer's luminance slider, so the two read alike. The two
bands the hue lies between share the weight linearly as they share a
mixer slider, and the whole thing is scaled by `mixer::confidence` of
the chroma (§63), so a neutral pixel keeps exactly the grey it had
whatever the weights say. The hue and the confidence are read from
the same local mean of a and b the mixer reads them from
(`local_ab`), for the same reason: read from the pixel, a dark
near-neutral surface hands the conversion its noise and the weights
print it as speckle. The mean is the source's, before the mixer's own
hue shift; the band a pixel belongs to is the scene's color, not
what the mixer made of it. The output is Oklab's lightness times that
gain with a and b at zero, which is a neutral in the working space
exactly (the first column of Oklab's inverse is 1, 1, 1), not a
weighted sum of the channels: the base grey is the pixel's perceptual
lightness.

**Where, and why there.** In the same Oklab pass as the mixer and the
global color, last in it: `mix_with` does the mixer's shift, scale
and gain, then vibrance and saturation's scale of chroma, then, when
the section is on, throws the chroma away and applies the band's gain
to the lightness. One Oklab round trip for all three, and the order
is the guarantee we asked for — nothing that scales chroma runs
after the conversion, so neither the mixer's saturation nor the
global Saturation and Vibrance can bring color back to a mono
picture, however hard they are wound. That guarantee is about the
controls that scale chroma, and only those: the per-channel point
curves and the grading wheels do put color back, on purpose and by
addition rather than by scale, which is what toning is. The section
sits before the curves and the grading for exactly that reason: the
point curves, the color curves and the three grading wheels still
act on the mono picture, so split toning is toning a black and white,
which is what it is for.

**One place for a switch.** `Mixer`, `Color` and `BlackWhite` gained
an `effective()` beside `Light`'s (§48): the section as it acts, every
slider at nothing when the switch is off. `finish_pixel_with` had that
zeroing written out for the mixer and the color; `pick`, which the
droppers read, did not, so a mixer with its switch off and its sliders
still set moved the dropper's answer. Both go through `effective()`
now, and a test asks that all three switched off with their sliders
wound right up read exactly as none of them set.

**Global, not per mask.** §27 put a whole look inside a mask, the
mixer and the curves included, but this one is the edit's and not the
look's: half a mono picture is not a mono picture, and a mask that
carried it would have to blend its weight in, which has no meaning
between color and grey. `Baked::of` leaves it off and `Baked::global`
is the only thing that sets it, so a local adjustment cannot turn it
on by accident; the section is not offered on the Masks tab.

**The schema.** A new `bw` field with `serde(default)`, the section
off and the weights flat, which is what every older sidecar meant, so
no version bump: §16's rule is that a field with a default is added
freely and only a field that *changes meaning* bumps `VERSION` and
gets a case in `migrate`. `Section::BlackWhite` for presets,
serialized `black-and-white`. Lightroom's `ConvertToGrayscale` (or a
Black & White treatment) now turns this section on and
`GrayMixerRed..GrayMixerMagenta` are its eight weights, band for
band, ±100 to ±1; the mixer keeps its own HSL bands and is no longer
greyed out to fake the conversion, which is what §52 had it do. The
grey mix is read *only* when the conversion is asked for: Lightroom
leaves those keys in a color preset's file, and a preset that
carried this section with its switch off would turn a mono picture
back to color when laid over it.

**Checked.** The viewport at 1:1 and the export's crop of
`5M0A3976.CR3` with the mixer, the global color and a Red filter all
on differ by 0.062 percent RMSE (0.16 of 255), 0.010 percent mean,
against §60's 0.06 and §63's 0.19: the shader agrees with
`BlackWhite::light`. Both are neutral to the eye and to the numbers,
mean HSL saturation 3e-7 on the export and 4e-4 on the viewport,
which is the odd rounded pixel. The Red filter's export and the Blue
filter's differ by 7.0 percent and the mono from the color by 12.2;
the church's gilt altar and candles are plainly lighter under the red
one. Tests: the filters' weights and their reach; a neutral grey
unchanged under every filter and under a set of hand weights; zero
chroma out for any color in; a saturated red lighter under Red than
under None and than under Blue, by exactly the stops `bw.rs` says at
the pixel's own hue, through the whole Oklab round trip; a blue sky
the other way about; the mixer at full saturation with Saturation and
Vibrance at +1 still coming out neutral, through `mix_with` and
through the whole finish. The Lightroom import round trip on a made-up
mono preset, by hand through the CLI as well as in the test: its eight
GrayMixer values land on the weights and the preset carries Light and
Black and white; the same file without the conversion asked for
carries Light alone and leaves `bw` at its default.

**Cost.** A 24 MP export with the section the only thing on is 1.56 s
against 1.45 s with nothing on: the 5x5 mean of a and b, which any
mixer already pays for (§63), and the pass itself. Nothing is paid
when the section is off.

**Not.** No weighting proportional to chroma through the whole range:
the fade is `confidence`'s, full from an Oklab chroma of 0.03 up, so
a pale color takes a band's weight as fully as a saturated one, as
the mixer's own sliders do. A real filter's effect is proportional to
saturation; matching the mixer was judged worth more than matching
the glass, and the mixer's saturation slider is there for anyone who
wants to pull a band down first. No infrared or orthochromatic
presets, no per-band luminance curve, and no tint of its own: the
grading wheels are the toning.
