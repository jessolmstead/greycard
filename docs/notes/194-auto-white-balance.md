# 194. Auto white balance (2026-09-27)

The WHITE BALANCE section has an Auto button beside Neutral. Neutral asks
the user for a pixel that should be grey; Auto finds one for the whole
frame, and from there the two share a path (`viewport::white_from_neutral`):
the pixel goes to `color::neutral_gains` with the white the picture was
developed at, the gains through `resolve_white_balance` with the develop's
profile to a temperature and tint, the sliders take them (clamped to
2000-12000 K and ±0.05), As shot goes off, and one develop and one history
step follow. There is no second path from gains to Kelvin.

**What it reads.** The developed picture on the GPU, the texture the
droppers sample: linear working space at the white it was developed at,
before the look. The scopes' analysis picture would not do: it is 8-bit,
display-encoded and after the look. `Renderer::sample_grid` copies only the
rows on a grid about 512 points across and keeps one pixel at each point (a
point sample, not a box mean). On a 45 MP frame held upright (5464 wide,
step 10, 819 rows of 43.8 KB) that is about 36 MB copied, synchronously on
the window's thread, once per press: measured at 4 ms on this machine's GPU
(3.8-4.6 ms over five runs; 1.8 ms for the same frame on its side, 341
rows). The estimate over the ~450 000 points is CPU work on top.

**Both guards.** Between choosing a frame and its develop landing,
`st.frame` and `raw-input` are the new frame's (they change at decode)
while the texture and `base_white` are still the last frame's. Auto and the
Neutral dropper both check that the picture on the GPU is the open frame's
own develop (`own_develop`, as the viewport's own drawing does) and
otherwise say "auto white balance: wait for the develop" / "white balance:
wait for the develop". A pixel that names no white says "nothing to read
there" for Neutral and "auto white balance: nothing neutral to read" for
Auto; a frame with no profile says so; too few usable pixels says "auto
white balance: not enough of the picture to read".

**The estimate** is `color::auto_neutral(pixels, gains, matrix, clip,
as_shot)`: grey-world in the sense of RawTherapee's "automatic, RGB grey",
made in camera space so it does not depend on the white the picture is
showing.

1. Every sampled pixel is taken back to what the sensor saw: through the
   inverse of the develop's camera-to-working matrix and out of its gains
   (what `neutral_gains` does for one pixel).
2. Usable pixels only: every channel above 0.5% of the sensor's white, and
   none above 95% of where that channel clipped, which is the sensor's white
   (1.0) or the develop's clip level over the channel's gain, whichever is
   lower (the second binds only when highlights were not rebuilt). Fewer
   than 300 usable pixels and it gives up.
3. A first mean in which each pixel weighs `1 / (1 + (d / 0.25)^2)`, `d` its
   distance from the camera's as-shot white in log ratios of red and blue to
   green. The as-shot white is a prior that does not move: the camera's own
   gains for the frame. Where every pixel shares one cast all weights are
   equal and this is plain grey-world; a saturated sky or wall counts for
   less. Without this prior a sky that is most of the frame drags the first
   estimate past the point where the second pass can recover.
4. One re-weighting: the mean again with weights `exp(-d^2 / (2 * 0.1^2))`,
   `d` now the distance from the first estimate, so the answer settles on
   what that estimate calls grey. If nothing is near enough to weigh
   anything, the first estimate stands.
5. The light found is sent forward through the develop's gains and matrix to
   the working-space pixel it would have made, and that pixel goes down the
   shared path as the dropper's does.

Means are of the pixels, not their chromaticities, so a bright pixel counts
for more than a dark one, as grey-world has it.

**Why camera space.** The first version ran in working space with the prior
centered on the white the picture was developed at, and each press moved
one step further: the candles went 3127 → 2469 → 2135 → 2000 K (the clamp),
a first press from 8000 K gave 4279 K, and the teal wall gave 9369 K from as
shot but 5778 K from 3000 K. In camera space against the as-shot prior, a
press from any starting white reads the same sensor values and the same
prior, so it finds the same light. What is left is the develop's own
dependence on its white (clipping, highlight reconstruction, the demosaic
run on balanced samples), which moves the first press by under 1%. The
constants (0.25, 0.1) were kept; they still separate the sky in the tests
and on the frames.

**Five presses from three starts** (CPU develop at the defaults with the
file's own profile, the same 512-point grid, each press developing at the
white the last one chose, clamped as the sliders clamp):

| Frame | Start | Presses 1 → 5 |
|---|---|---|
| 5M0A3976.CR3, candles (as shot 3127 K) | as shot | 2265 → 2254 → 2254 → 2254 → 2254 K |
| | 3000 K | 2265 → 2254 → 2254 → 2254 → 2254 K |
| | 8000 K | 2274 → 2254 → 2254 → 2254 → 2254 K |
| 123A6932.CR3, overcast (5408 K) | as shot | 6157 → 6153 → 6153 → 6153 → 6153 K |
| | 3000 K | 6196 → 6153 → 6153 → 6153 → 6153 K |
| | 8000 K | 6151 → 6153 → 6153 → 6153 → 6153 K |
| 4Z4A2764.CR3, bridge (5268 K) | as shot | 4642 → 4624 → 4623 → 4622 → 4622 K |
| | 3000 K | 4624 → 4623 → 4622 → 4622 → 4622 K |
| | 8000 K | 4608 → 4621 → 4622 → 4622 → 4622 K |
| P1000247.RW2, portrait (5356 K) | as shot | 4043 → 4014 → 4013 → 4012 → 4012 K |
| | 3000 K | 4040 → 4014 → 4013 → 4012 → 4012 K |
| | 8000 K | 4050 → 4015 → 4013 → 4012 → 4012 K |
| 5M0A2279.CR3, teal wall (6124 K) | as shot | 9220 → 9266 → 9268 → 9268 → 9268 K |
| | 3000 K | 9212 → 9266 → 9268 → 9268 → 9268 K |
| | 8000 K | 9240 → 9267 → 9268 → 9268 → 9268 K |

Tints settle the same way (candles -0.0009, overcast -0.0011, bridge
+0.0007, portrait -0.0035, teal wall +0.0170). Every frame settles by the
second press, and the first press is within 1% of where it settles from
any start.

What the answers look like: the candle frame's stone goes neutral and the
dark walls go slightly blue-grey; the overcast marble goes neutral; the
portrait's warm backdrop goes grey, which may or may not be what was meant.
The teal wall is grey-world's known failure: almost nothing in the frame is
neutral and the wall is most of it, and the answer (9268 K, a green white
point) is wrong from every start. Auto is not the tool for that frame.

**Left out.** Auto reads the whole developed frame, not the crop.
