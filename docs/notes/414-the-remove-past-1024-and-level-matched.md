# §414. The remove past 1024, and level-matched

*2026-10-24*

Two things the user found on the first day with the generative
remove: a darker band around a removed object, and that most objects
of any size on a 45 MP frame never reached the remove at all.

## The band

§399 measured the fp16 decoder returning the window a little darker
than it went in. The composite blended that decode into the original
across the hole's edge, so the hole sat low and the edge showed it.
The decoded window covers far more than the hole, so the decode is
matched to the original where both are known: a ring 24 px wide
outside the hole, both sides box-blurred over known pixels only so
the removed object and the model's texture stay out of the fit. The
match is an offset per channel by default, the mean difference over
the ring, which is what §399's error is. A gain is fitted as well
only when the ring's low-passed spread reaches 0.1 and it holds at
least sixteen blur boxes; narrower rings fit their slope to texture
(two halves of a 0.07 ring disagreed by 0.03, of a 0.11 ring by
0.01). A fitted gain outside 0.9 to 1.1 drops that channel to the
offset with a log line rather than keep a clamped value. On wide
rings the red gain came out 1.03 to 1.04 on every window tried, each
half agreeing: the decoder flattens red a little, so the gain earns
its place there.

On the 1024 reference case the ring's error went from -0.49, -0.21,
+0.03 /255 to -0.01, -0.01, -0.03; against the fp32 reference the
hole went from -0.84, -0.56, -0.81 to -0.21, -0.22, -0.82. Blue's
remainder does not show in the ring and is the UNet's, not the
decoder's. The parity floors of §405 and §408 hold after the match
(38.58 dB at 1024, 33.96 at 512). A smooth correction field was
tried and bought nothing over the single fit: what remains at the
edge is texture, not level. Where the ring is dominated by one thing
(a tree against sky) the single fit cannot follow the sky locally,
and a few /255 of step remains there; the field is the answer if it
ever matters.

## Past 1024

A window over 1024 a side is scaled so its longer side is 1024, run
as any window, scaled back, and composited under the full-size hole
only, so every pixel outside is the original bit for bit. Down by an
exact area filter, which keeps the mean and averages grain rather
than aliasing it; up by Lanczos 3, clamped at zero. Both in linear
light: the window arrives sRGB-encoded at the grey gain, so it is
decoded for the resampling and re-encoded after. A small pixel is in
the hole when any pixel it covers is, so a one-pixel stroke survives
4x. The factor is capped at 4 (a 4096 window, a third of a 100 MP
frame's width); past that the fill is LaMa's as before, with a log
line. The scaling converts rows as it goes and grows the fill back
over the hole's bounding box only, so a 4096 window costs the output
buffer and a 1024-wide intermediate, not four full-size copies.

The seed and "Another" are untouched, since the noise is at the
latent size; the drift and correlation checks run at the small size;
a refusal is remembered by the full-size window as before.

Measured: a scaled run is a different fill, about as far from the
native one as another seed is (21.9 dB in the hole at a pretend 2x,
against 24.0 between seeds). Three real windows, at 1.7x, 2.7x, and
4.0x, all beat LaMa: a hillside continued cleanly where LaMa grew a
blob; water and rock carried through where LaMa smeared; at 4x on a
corner of trees the model drew a softer tree rather than none, which
is what a hole that is mostly tree asks of it. The cost is
resolution: by about 2.5x the fill's fine texture reads softer than a
sharp, glinting surround at 1:1, though the grain pass keeps it from
reading smooth. The time is the native run's plus under a second for
the resampling, since the UNet runs at 1024 regardless.

## Not done

- A detail pass for the fill's texture at 2.5x and up, if the
  removal harness says the softness matters.
- The smooth level field for a ring dominated by one subject.
