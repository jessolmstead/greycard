# 76. The VNG4 half cannot be skipped, so it got cheaper instead (2026-09-18)

§56 left the dual demosaic's VNG4 as the obvious waste: 394 ms of the
841, run under every pixel when "a third of the frame is blended away".
The blend weight is known before VNG4 runs, so the plan was to compute
the mask first and run VNG4 only in the bands that carry any weight.

The premise is wrong, and the mask says so plainly. The weight VNG4
gets at a pixel is `1 - blend`, and on four real frames it is **never**
zero: not once in 45 million pixels. Measured on the developed frames
with their own noise-model thresholds (§13h), the share of pixels where
`blend` reaches exactly 1.0 is 0.000% on all of them, and even allowing
a tolerance — treating a weight below a hundredth as zero, which is not
bit-identical and would change the picture — not one tile is free of
VNG4, at 64x64, 64x256, 32x64 or a whole 64-row band. The numbers per
frame, as "share of pixels whose VNG4 weight is under 1e-2 / under
1e-1", are 1.98 / 6.07 percent (4Z4A3525, R5 II, 45 MP), 1.03 / 8.75
(4Z4A2764, ISO 250, 45 MP), 0.63 / 2.52 (5M0A8354, 24 MP) and 0.01 /
1.02 (5M0A4160). Detail is everywhere in a photograph and a 64x64 tile
is small.

Two things make it so. The sigmoid reaches exactly 1.0 in f32 only
around x = 3000, which at the noise model's threshold of 0.12 means a
local contrast of about 22 — an L\* step of 90 across two pixels, a
specular edge. Only 0.12 to 0.17 percent of pixels clear that before
the blur, and the blur with sigma 2 then needs a 13x13 neighborhood
of them to leave a one standing; none does. And the half that really
is blended away is the other one: `flat_fraction` is 0.64 to 0.90 on
these frames, so it is AMaZE that contributes nothing over half the
picture (`blend` under a hundredth on 23 to 66 percent of pixels).
AMaZE cannot be skipped there, because the mask is built on the L\* of
AMaZE's own output; that is the shape of the reference's algorithm, not
an accident of ours.

So VNG4 got faster rather than rarer, in three rearrangements that
preserve the arithmetic exactly:

- **The gradient accumulation.** Each of dcraw's terms adds its
  weighted difference to some of the eight directions, chosen by a bit
  mask, which was a branch per direction per term — around 300 branchy
  iterations a pixel. The mask is now eight floats, ones and zeros, and
  the loop is `gval[i] += diff * grads[i]` over a `[f32; 8]`: `x * 1.0`
  and `x + 0.0` are exact for the finite samples the pipeline hands
  the demosaic (`normalize_levels` clamps to 0 to 1; a NaN or an
  infinity times zero is NaN where the branch left the direction
  alone, and a debug assertion says so), so the sums are the bits the
  branch gave, and the compiler vectorizes them.
- **The interpolated greens.** At each of up to eight low-gradient
  neighbors, `green` asked for the other green plane's value there,
  which is a 3x3 weighted mean of the raw — and every pixel within one
  step asked for the same value again, so a red site paid for up to
  twelve of them. Both green planes are now filled a row at a time into
  a three-row rolling cache and read from it: two per pixel instead of
  eight to twelve, and the values are `plane`'s own, so nothing moves.
  Three rows, not a band's worth, because the band belongs to a thread:
  six rows of floats is 200 KB where a band's would be megabytes, and
  they sit in the tail of the band's existing buffer so a band is still
  one allocation.
- **The ceiling.** `Vng4::new` folded the largest sample out of the
  whole mosaic on one thread. In parallel now; the maximum is exact and
  does not care in what order it is taken.

The mask's blur got the §56 sharpen treatment while we were here: the
row pass runs its interior over windows with no clamp and the edges as
before, and the column pass walks whole rows for each tap instead of
gathering a column per output pixel. The taps are taken in the same
order a clamped index gave, so the sums are the same bits. `sharpen`
uses the same blur for its own mask, so it gains too.

**Checked.** Bit-identity first, three ways. Hashes of the whole VNG4
frame, of AMaZE and of the dual result on both sample frames are
unchanged; a full 24 MP develop to 16-bit TIFF differs from the old
binary's in six bytes, which are the two copies of the EXIF write time.
Two tests pin it in the repo: `every_phase_is_bit_for_bit_what_the_port_first_gave`
hashes VNG4 over a synthetic mosaic in all four CFA phases against the
values the first version of the port gave, and
`the_blur_is_bit_for_bit_the_clamped_loop` does the same for the blur,
on one image wider than the kernel and one narrower.

Times, best of five, 16-core desktop, and the machine had another
agent's benchmark on it, so AMaZE — untouched — is left in as a control
and the runs were interleaved. On the 45 MP R5 Mark II frame
(4Z4A3525, 8192x5464): VNG4 alone 422 to 199 ms, the dual demosaic 895
to 625, the blur over a 45 MP plane 54 to 22, AMaZE 335 against 311.
On the 24 MP frame (5M0A8354): VNG4 209 to 103 ms, the dual 441 to 331,
the blur 17.8 to 11.5, AMaZE 166 against 169. End to end, a develop
from the command line, best of three: 1912 to 1669 ms at 45 MP, 1074 to
969 at 24 MP.

Memory grew a little: peak resident over the 24 MP develop of §13p is
925 MB against 897, three percent. Six rows of cache per worker is 140
KB, 4.5 MB over the pool; the rest did not trace to a buffer — folding
the cache into the band's allocation, keeping one scratch buffer per
worker instead of one per band, and matching the old buffer's rounded
capacity each left it where it is, so it is the allocator's arenas
behaving differently for a differently sized band, not a frame held
twice.

What was tried and did not pay: the skip itself, at every granularity
from a 32x64 tile to a whole band and at tolerances from exact to a
tenth — nothing is skippable, so no code was written for it; and
per-worker scratch buffers, which removed 86 allocations a frame and
moved neither the clock nor the peak.
