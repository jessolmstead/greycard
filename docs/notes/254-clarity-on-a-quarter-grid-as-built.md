# 254. Clarity on a quarter grid, as built (2026-10-06)

§251's first item, built: from a Clarity radius of 64 pixels, the
guided filter finds its slope and intercept on a grid of four-pixel
blocks and applies them at full size. Clarity alone takes half the
time it did, and below that radius the filter is the exact one, bit
for bit.

**What it is.** `coarse_guided_filter` in `local_contrast.rs`
reduces the log luminance and its square to block sums, takes each
window's mean and mean square from them, finds the slope and
intercept per block, averages them over the windows, and takes them
back to full size bilinearly, each block's value at its center. As
§251 asked, the square is reduced, not squared after reducing, so a
textured window keeps the variance finer than a block. Texture's
filter, the low-pass, the mid-tone weight and the clip fade stay at
full size. `LocalContrastStats` reports the full-size radius, so the
status line still says "3 px / 150 px".

**Where it departs from §251.**

- The window is `round(((2r + 1) / s − 1) / 2)` blocks, not
  `round(r / s)`: the radius whose `(2r' + 1) s` pixels is nearest
  the full-size `2r + 1`. At 150 pixels that is 37 blocks, 300 pixels
  against 301, where `round(r / s)` gives 38 and 308. Against the
  exact filter at r = 150 the worst difference is 0.0011 stops this
  way and 0.0079 the other.
- It starts at a long edge of 2540, not 2560: Clarity's radius is the
  long edge over 40, rounded, and 2540 / 40 = 63.5 rounds to 64.
- A last row or column of blocks cut short by the picture's edge
  counts for the pixels it holds, in the windows' means and in the
  averages of the slope and intercept alike. Counted as whole blocks
  they put up to 0.019 stops of error in the last row of a side not a
  multiple of four (2603 × 1001); weighted, 0.003, the same as
  everywhere else. Every raw we have has sides that are multiples of
  four; an odd-sized TIFF or JPEG would have shown it.

**Measured.** Release, 32 threads, five runs, medians, one-minute
load 8 to 10 (the reviewer's run):

| Frame | Master | Grid |
|---|---|---|
| 24 MP, Clarity alone | 0.111 s | 0.058 s |
| 24 MP, with Texture | 0.180 s | 0.141 s |
| 45 MP, Clarity alone | 0.229 s | 0.110 s |
| 45 MP, with Texture | 0.376 s | 0.246 s |

The exact path's digests in `ops_alone` (`develop/timing.rs`, which
now has the op, Clarity alone and with Texture) are master's: 24 MP
`928d4d50a96e56aa` / `ac38f970c93141d3`, 45 MP `476b18e274c196af` /
`e9dbf7bfaddd7354`.

The coarse filter against the exact one over whole planes, edges
included, is at most 0.003 stops at any size. Exports at Clarity +50
from master and from the grid differ by at most one level in 255
(8-bit RMSE 0.017% on the church frame, 0.016% on the portrait, 0.023%
at Clarity −100 with Texture +50), with no pixel off by more than one.
§103's behaviors run through both the exact filter and the grid
forced on small pictures: the soft bump's lift, the four-stop edge's
overshoot, the period-six grating, the mid-tone fade and the clip
guard. The grey standard deviation of the altar and the face crops
is unchanged to four decimals.

**The viewport and the export agree.** The worker develops the
full-size patched picture, not a reduced one (the proxy develop is
still deferred, §115), and the export goes through the same
`develop_job` (§236). Both take the grid on the same frames, and both
take the exact filter below 2540.

**Next** is §251's second item, local contrast on the GPU, which
this makes possible: nothing in the op is a large box at full size
any more.
