# 176. Nikon High Efficiency: the codec exists, and the patents (2026-09-25)

The roadmap's Upstream list said the Z6 III's High Efficiency raws,
64 of the 87 on hand, were "a codec to contribute to rawler". Checked
today, that is no longer the shape of the work.

**The format.** Nikon's HE and HE* are intoPIX's TicoRAW, and TicoRAW
is JPEG XS (ISO/IEC 21122) applied to a Bayer mosaic: a 5/3 wavelet,
bit-plane entropy coding in precincts, and the Star-Tetrix transform
that decorrelates the four CFA sites before the wavelet, which the
standard's second edition took in from intoPIX. HE and HE* are two
rate settings, and differ in the bit-plane regime a precinct signals.
Around the standard's core intoPIX adds the tuned encoder, the
hardware cores and a raw front end, a nonlinear curve before
quantization so the bits follow the sensor's noise.

**The decoder.** On 2026-08-30 Nicolai Buchwitz opened
dnglab/dnglab#835: a pure-Rust JPEG XS decoder for rawler, about
2,760 lines in a `decompressors/jpegxs` module (bit reader, DWT,
entropy, header, the component transform, precinct iteration) and the
hook in the NEF decoder. He started from Intel's BSD SVT-JPEG-XS,
wrote the Rust largely with an AI, and tested on his own Z6 III
against a lossless NEF of the same tripod scene; one tester reports a
Z50 II HE* file working; Z8 and Z9 are untried. The curve back to
linear sensor values reportedly uses fitted constants, so exactness
is unproven, and a month on the maintainer has not reviewed it.
LibRaw has a decoder of its own since 2024 but keeps it out of the
public snapshot; a community PR there, LibRaw#826, decodes HE and
reverted its HE* attempt for artifacts.

So the contribution is testing and review, not a codec: decode our
64 HE frames through the PR's branch (the rawler `[patch]` entry
already carries a branch), compare level, noise and color with the 23
lossless frames from the same body, and put the result on the PR,
which is the test its author asked for; then read the 2,700 lines
against the standard, a week of care. HE* on the Z8 and Z9 waits on
files.

**The patents, which are the real question.** The code's licenses
are no obstacle: rawler is LGPL-2.1 and SVT-JPEG-XS is BSD, both fine
under our GPL-3.0-or-later. But JPEG XS is standard-essential
patented by Fraunhofer IIS and intoPIX, pooled through Vectis IP as
the JPEG XS Patent Portfolio License. The terms overview of April
2025 (jpegxspool.com) has no free tier and no exemption for open
source, non-commercial or low-volume use: a software product that
decodes pays per copy, 3.00 USD per HD instance, 5.00 at 4K and 9.00
at 8K in the lowest volume tier, capped at the greater of two percent
of the product's price or 1.5 times the 4K fee; quarterly royalty
statements and patent marking on the product; terms to the end of
2030. A free download has no price, so the cap is the second term.
RapidRAW's author read the same and declined to ship the decoder in
a binary. Source in a repository is the usual position, and that is
where the PR sits.

What that leaves for greycard, none of it a codec: the upstream test
and review, which help rawler whatever we ship; a build feature off
by default, so someone compiling greycard themselves gets HE and
nothing shipped carries the decoder; and on macOS the system decoder,
if Apple's RAW engine reads HE files as one commenter on the dnglab
thread claims, which is unverified and helps one platform. The
roadmap line moves from Upstream to the Engine backlog with that
shape; the shipping call is the user's.
