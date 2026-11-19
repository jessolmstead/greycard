# The learned highlight fill, hl-1: the port's spec

This is what the Rust port must compute, stage by stage, as `oracle.py`
computes it. The oracle is the trial's inference path (r6e-s1 with e_tied's
look) with its numerics unchanged, apart from the departures below. The
fixtures are in `crates/greycard-core/tests/fixtures/highlight_fill/`, and
`make_fixtures.py`'s docstring lists every array in them.

Everything is float32 unless we say otherwise. A "cell" is a half-size
cell, and "px" means full-size pixels of the input.

## Where hl-1 differs from the trial

hl-1 is the trial's fill plus three guards, one definition for reduced
develops, and three definitions where the trial's code was only
approximately what it meant (per-region sums, the labels' iteration
limit, and frames under 8 px). These are the only places the oracle
differs from the trial.

- **`apply`, luminance:** a pixel is filled only where 1e-6 < Y(x) < +inf.
  Every other pixel stays x, bit for bit. Y(x) is the same `x · LUMA` the
  clamp takes, and a NaN Y fails both comparisons.
  - The trial divided by max(Y, 1e-6). A clipped pixel with Y ≤ 1e-6
    (demosaic undershoot) came out near 1e6, and a NaN or infinite pixel
    came out NaN.
- **`apply`, no field:** a pixel is filled only where its upsampled gain or
  either upsampled offset is nonzero. A value of −0 counts as zero.
  - The trial sent a clipped pixel with no field (no region near it)
    through the Oklab round trip with zero gain and offset, which moved it
    by up to about 2e-6. It now stays x.
  - A pixel any field reaches is filled exactly as the trial fills it.
    The fields are exactly 0 only where none of the four half-size cells
    under the bilinear upsample is in a region.
- **`fill`, non-finite input:** the analysis reads a non-finite channel as
  0. This covers the half-size picture and the thumbnail. One NaN or
  infinity cannot reach a region's means, and through them every pixel of
  the region. The final apply still reads the original x.
- **The thumbnail of a reduced develop (f > 1)** is built from the reduced
  input itself.
  - The trial's look-scaled runs (`scale9.py`, `preview9.py`) gave the
    reduced input the full frame's thumbnail. On the small synthetic
    scenes the two differ by 0.018 to 0.075 stop (measured in review).
  - We keep the reduced input's thumbnail. A reduced develop in the
    editor has only its binned base, so this is the thumbnail it can
    build. It is part of hl-1's definition.
- **Per-region sums (stage 9):** counts are integers, and every
  per-region sum (the means of ly and ab, the centroid, the thin share)
  accumulates in f64 and is rounded to f32 once, after the division.
  - The trial summed in f32 (`index_add_`). Its error grows with the
    region: on a region of constant ly = log2(0.9), the mean is off by
    more than the features' 2e-5 from about 20 000 cells (80 k px), by
    9e-4 at 1 M cells, 1.8e-3 at 4 M, and 1.6e-2 at 25 M. In review a
    4 M-cell region's mean ly read 0.924 for a true 0.900.
  - Its counts stopped at 2²⁴ = 16 777 216, so a region of 25 M cells
    (a blown sky on a 100 MP frame) read a short area and every mean
    over it was off by half; its centroid landed at (0.758, 0.765).
- **Labels (stage 3)** run until nothing changes. The trial stopped at
  20000 iterations, which a region more than about 20000 cells across
  would reach.
- **Frames under 8 px on a side (stage 6):** the quarter grid is empty,
  and the harmonic is 0 everywhere, the "nothing known" result. The
  trial's pool raised.

On the fixtures, the guards change only five pixels of the `mixed` scene.
Four are its negative-luminance pixels, where the trial gives about
(1.1e6, −4.8e5, −2.4e5) and hl-1 gives x. The fifth is its lone clipped
pixel, where the trial moves x by about 1e-6.

Every stage up to the features matches the trial at 0. The f64 sums move
the features by at most 3.5e-6 (sky), the outputs by 6.3e-7, gain_h by
6.0e-7, dab_h by 2.8e-8, and the final picture by 3.6e-6, with the
random checkpoint at f = 1, 2.5, and 3 (`compare_trial.py`, run with the
trial's thumbnail convention). The `nonfinite`, `tiny`, and `tiles` scenes
have no trial counterpart.

## Inputs

- `x`: the classical fill, linear Rec.2020 divided by the ceiling, as
  (H, W, 3).
  - With headroom, x and the clips are divided by the headroom factor too.
    The `headroom` scene uses 1.123.
- `codes`: u8 (H, W). The low 7 bits are the count of colors clipped in
  the pixel's CFA cell (2×2 on Bayer, 3×3 on X-Trans), 0 to 3. Bit 7
  (untrusted) is ignored: k = codes & 0x7F.
- `clips`: each channel's clip level over the ceiling, three numbers.
- `box`, `frame`, `thumb`.
  - The editor passes none of them: the box is (0, 0), the frame is
    (W, H), and the thumbnail comes from `thumbnail` on the input.
  - The trial passed a crop's box in its frame, with the frame's
    thumbnail.
- `f`, the look scale of a reduced develop: the frame's full long edge
  over the input's long edge, a float, never rounded. 1 is the full-size
  develop.

## A reduced develop's input (`down`, `down_codes`)

This is part of hl-1's definition for reduced develops. The f = 3 and
f = 2.5 fixtures were made this way, with an integer factor n.

- **`down(x, n)`:** drop the ragged edge (keep the first ⌊H/n⌋·n rows and
  ⌊W/n⌋·n columns), then take the numpy float32 mean of each n×n block.
- **`down_codes(codes, n)`:**
  - first k = codes & 0x7F, and drop the ragged edge;
  - 3 where more than half of the block's pixels have k = 3;
  - otherwise the block's largest k capped at 2, which is 0 when nothing
    in it is clipped;
  - bit 7 is not carried.
- `depth_tie_f2_5` is `depth_tie` reduced with n = 2 and run at f = 2.5,
  standing for a frame 2.5× its input's long edge.

## Constants (`oracle.py`'s top block)

- FLOOR 2⁻¹² and ABMAX 0.5.
- REACH_HALF 256 cells, RING 12 cells, and CORE_REACH 8 cells.
- THIN 4 px and OFF 4 px.
- HARM_ITERS 300 and HARM_BRIGHT 0.25.
- Blur σ: SIGMA_DEPTH 3, SIGMA_MIX 2, and MIX_SIGMA 6 (cells).
- CORE_WHITEN 0.5 and CORE_BOOST 1.4.
- DEPTH_TIE (12, 48) px.
- THUMB_LONG 384.
- S = 1 throughout: the model is not scale-free.
- **Look scale:** σ_g = max(6/f, 1) for the gain's hand-over only, and the
  tie is (12/f, 48/f). The color hand-over (σ 2) and every other constant
  stay as trained.
  - When σ_g == 2.0 exactly (f = 3), the wide branch is skipped, so
    mixg = mix. The result is the same either way.

### Precision with an arbitrary f

These points are pinned by the f = 2.5 fixture.

- **σ_g:** computed as max(6/f, 1) in f64, then sg(σ) = max(0.5, σ) in f64.
  - The kernel's radius is r = int(3σ), truncated, in f64.
  - The kernel takes σ rounded to f32: k(t) = exp(−0.5·(t/σ₃₂)²) in f32.
- **The tie:**
  - tie_lo = 12/f in f64, rounded to f32 for the subtraction;
  - the denominator, 48/f − 12/f, is computed in f64 and then rounded to
    f32;
  - tie = clamp((cdepth − tie_lo₃₂) / den₃₂, 0, 1) in f32.
- **`where`:** each entry is an f64 division (box_x / fw, and so on),
  rounded to f32.
- **The look's boost and whitening** (`oracle.py` lines 403 and 422):
  - the boost enters as 1 + (boost − 1)·tie, with boost − 1 computed in
    f64 (0.3999999999999999 for hl-1's 1.4) and rounded to f32, which is
    f32(0.4); then (boost − 1)₃₂·tie, plus 1, in f32;
  - the whitening is rounded to f32 (0.5, exact) and multiplied left to
    right in f32: keep = 1 − (((whiten₃₂·mix)·s2)·on)·tie, where `on` is
    the region's core indicator.

## Arithmetic order

These are torch's CPU orders, checked bit for bit against explicit numpy
float32.

- **Y** = (r·0.2627 + g·0.678) + b·0.0593 in f32, with no fused
  multiply-add. `oracle.luma_np` is this order, and the fixtures' checks
  use it.
- **Each 3×3 matrix product row** (TO_LMS, LMS_TO_LAB, FROM_LAB, FROM_LMS)
  = (v₀·m₀ + v₁·m₁) + v₂·m₂, with no FMA.
- **The cube root:** sign(l)·(|l| + 1e-9)^p with p = f32(1/3) =
  0.33333334, a power and not cbrt.
  - torch's pow lies within 2 ulp of pow(x, p) correctly rounded.
  - The port should compute it as an f64 pow, rounded to f32, and expect
    2 ulp against the fixtures.
- **The cube in `rgb()`** is (x·x)·x.
- **exp, log, erf, tanh, and sigmoid** are torch's vector forms, within an
  ulp or two of correctly rounded.

## Stages, in order

### 1. Half size (`fill`)

- Read non-finite channels as 0 (hl-1).
- If H or W is odd, pad x and k by one row or column, replicating the
  edge. Then Hh = ceil(H/2) and Wh = ceil(W/2).
- xh: the mean of each 2×2 block, numpy float32 (sum of four, then ÷ 4).
  The summation order is numpy's, so expect ±1 ulp.
- any_share = mean(k > 0) and core_share = mean(k == 3), both exact
  quarters. kh = max(k) over the block.
  - kh is only ever compared with 0, 1, and 2, so a port may cap it at
    3. Codes with low bits above 3 are not produced by the app.
- In `numbers`: fa = any_share > 0.5, fc = (core_share > 0.5) · fa, and
  band = fa · (1 − fc).

### 2. Oklab (`Oklab`)

- Y(rgb) = rgb · LUMA, with LUMA (0.2627, 0.678, 0.0593) as f32.
- of(rgb):
  - lms = rgb · TO_LMSᵀ;
  - lms = sign(lms) · (|lms| + 1e-9)^p (sign(0) = 0);
  - lab = lms · LMS_TO_LABᵀ.
- rgb(lab): ((lab · FROM_LABᵀ)³) · FROM_LMSᵀ.
- FROM_* are torch's float32 inverses, in `oklab/`. They are not core's
  `LAB_TO_LMS`.
- unit_ab(xh): y = max(Y, 1e-6), and abh = of(xh / y)[a, b]. yh is the
  unclamped Y.
- abh is clamped to ±0.5 (at half size only).
- ly = log2(max(yh, FLOOR)).

### 3. Labels (`labels`)

- Each fa cell starts with id = row-major flat index + 1, on the padded
  half grid.
- Repeat: new = 3×3 max (padding −inf) × fa, until unchanged. The check
  runs every 16 iterations, with no limit (hl-1).
- So regions are the converged **8-connected** components, and a
  region's id is its largest flat index + 1. Any O(N) method giving that
  partition and those ids is the same (core uses union-find).
- The port keeps ids as integers (u32). The oracle propagates in f64 and
  returns f32 below 2²⁴ half-size cells, where f32 holds every integer
  exactly. At or above 2²⁴ cells (a frame of about 67 MP or more) it
  returns the f64 ids, which are still exact. An f32 id there would merge
  regions.

### 4. Distances (`distance`, step 2)

- d = 0 on mask cells and reach elsewhere.
- For k = 1 … reach/2, dilate by a 5×5 max (padding −inf). A cell first
  reached at step k takes 2k.
- Equivalently, d = 2·ceil(chebyshev / 2), or reach when beyond it.
- The frame edge is never on the mask.
- The three distances, in px (multiples of 4):
  - d0 = distance(1 − fa, 256) · 2, capped at 512;
  - dcore = distance(1 − fc, 8) · 2, capped at 16;
  - dc0 = distance(1 − fc, 256) · 2, capped at 512.

### 5. Ring (`grow`)

- 12 steps of: out = out > 0 ? out : 3×3 max(out) · (1 − fa). Then zero
  the regions' own cells.
- The ring grows only through cells outside every region, 8-connected: a
  geodesic chessboard ring.
- A cell reached in the same step by two regions takes the **larger id**.

### 6. Harmonic (`harmonic`)

- Under 8 px on a side (Hh or Wh under 4) the quarter grid is empty and
  harm is 0 everywhere (hl-1; the `tiny` scene).
- fixed = (1 − fa) · (yh > 0.25): raw yh, not exp2(ly).
  - With correctly rounded log2 and exp2, and with torch's and numpy's,
    the two readings agree on every f32 within 2²⁰ ulp of 0.25, so the
    choice moves no cell there; it is kept as written.
- q = avgpool4(abh · fixed) and fq = avgpool4(fixed).
  - Kernel 4, stride 4, floor: the last Hh % 4 rows and Wh % 4 columns
    are dropped.
- known = fq > 0.25, and val = q / max(fq, 1e-6).
- Start: cur = val where known. Elsewhere it is the mean of val over the
  known cells, Σ(val · known) / max(count, 1), which is 0 when nothing is
  known.
- 300 iterations of: cur = known ? val : (Σ of the 3×3 of the
  replicate-padded cur) / 9.
- Then interpolate back to (Hh, Wh): bilinear, align_corners False,
  size-based.
  - src = (dst + 0.5) · (n_in / n_out) − 0.5, clamped at ≥ 0.
  - i0 = floor(src), and i1 = min(i0 + 1, n_in − 1).

### 7. Thumbnail (`thumbnail`)

- f_t = max(ceil(long edge / 384), 1). The size is ceil(W / f_t) ×
  ceil(H / f_t); the last row and column of blocks may hold fewer pixels.
- Sums are f64 over max(x, 0), in raster order within each block.
- Channels 0–2: log2(max(f32(sum / n), 1/4096)).
- Channels 3 and 4: f32(count(k > 0) / n) and f32(count(k == 3) / n).
- At f > 1, use the reduced input (hl-1).

### 8. Encoder (`Context`)

- Divide channels 0–2 by 4.
- Then, in order:
  - conv 5→16 k3 p1;
  - conv 16→32 k3 s2 p1;
  - conv 32→32 k3 p2 dilation 2;
  - conv 32→32 k3 s2 p1;
  - conv 32→32 k3 p2 dilation 2.
- Each conv is followed by exact GELU, 0.5x(1 + erf(x/√2)), and pads with
  zeros. A stride of 2 gives ceil(n/2).

### 9. Per region (`numbers`)

- Rows are the sorted unique values of the label map. Row 0 is id 0, the
  outside, when any cell is outside.
- inv maps each cell to its row. For ring cells, the ring id's row is
  found by searchsorted; non-ring cells map to row 0 and are masked.
- mean(v, sel) = f32(Σ(v · sel) / max(Σ sel, 1)), the sum in f64 over
  the f32 values and the count an integer, rounded to f32 once (hl-1).
  The order of an f64 sum no longer matters at f32; core sums in raster
  order.
- Counts (area, cbr, cb1, cb2, ccore, and cbn) are integers. Each enters
  a feature as f32(count); tot is summed as an integer, then read as
  f32.
- Masks:
  - inside: the region's cells;
  - bright = ring · (kh == 0) · (exp2(ly) > 0.25);
  - b1 = inside · (kh == 1) and b2 = inside · (kh == 2);
  - bandm = band · inside and corem = fc · inside.
- Per region:
  - area = count; depth0 = max(d0);
  - ybr, yb1, yb2 = means of ly over bright (by ring owner), b1, and b2,
    with counts cbr, cb1, and cb2; ccore = count of corem;
  - tot = max(cbr + cb1 + cb2 + ccore, 1);
  - top = yb2 if cb2 > 0, else yb1 if cb1 > 0, else ybr;
  - rise = top − ybr if cbr > 0 and cb1 + cb2 > 0, else 0;
  - ar, br = means of abh over bright (by ring owner);
  - an, bn = means over bandm, with count cbn;
  - thin = mean over corem of (dcore ≤ 4).
- The 20 features, in order:
  - log2(max(area, 1)) / 16 and log2(max(depth0, 1)) / 8;
  - ybr / 4, yb1 / 4, yb2 / 4, and rise;
  - cbr / tot, cb1 / tot, cb2 / tot, and ccore / tot;
  - 10·ar, 10·br, 10·an, 10·bn, 10·√(an² + bn²), and 10·√(ar² + br²);
  - thin, then clip_r, clip_g, and clip_b.
- **Context:**
  - The centroid is cx = mean over inside of (x + 0.5) / Wh (that f32
    value, summed in f64 as every mean), and likewise cy.
  - where = [box_x / fw, box_y / fh, 2·Wh / fw, 2·Hh / fh]. This uses the
    padded size.
  - g = (where₀ + where₂ · cx) · 2 − 1, and likewise for y.
  - Sample the encoder's map with grid_sample: bilinear, border padding,
    align_corners False. ix = ((g + 1) · w − 1) / 2, clamped to
    [0, w − 1].
- **MLP** (`RegionNet6.outputs`): [20 features, 32 context] → Linear 128
  → GELU → Linear 128 → GELU → Linear 12. Each linear is x·Wᵀ + b.
- If row 0 is id 0, its 12 outputs are zeroed before use.

### 10. Synthesis (`numbers`)

- blur(v, σ) (`blur`, `gauss`):
  - kernel exp(−0.5·(t/σ)²) for t in [−r, r], with r = int(3σ),
    normalized in f32 (see "Precision with an arbitrary f");
  - horizontal pass, then vertical, with replicate padding;
  - sg(σ) = max(0.5, σ).
- blurN(v, m) = blur(v · m, 3) / max(blur(m, 3), 1e-3) · m.
- off = min(4, 0.5 · depth0) per region.
- dd = blurN((d0 − off)₊ · fa, fa); depth = max(max(dd), 1).
- Outputs r0 … r11 to parameters:
  - H = 5·tanh(r0/5) and L = clamp(depth · exp(clamp(r1, ±20)), 1, 4096);
  - β = σ(r2), γ = σ(r3), and κ = exp(0.7·tanh r4);
  - C = 0.03·tanh(r5, r6);
  - β_b = 1.5·σ(r7), γ_r = σ(r8), and κ_r = exp(0.7·tanh r9);
  - H2 = 5·tanh(r10/5) and L2 = clamp(cdepth · exp(clamp(r11, ±20)), 1, 4096).
- s = (1 − exp(−dd / L)) · fa.
- mix = clamp(blur(fc, 2) · fa, 0, 1), raised to at least 0.5 where fc.
- mixg: the same with σ_g. It is skipped (mixg = mix) when σ_g == 2.0.
- g = H · s · (mixg + (1 − mixg) · β_b).
- dc = blurN((dc0 − off)₊ · fc, fc); cdepth = max(max(dc), 1).
- s2 = log1p(dc / L2) / log1p(cdepth / L2).
- tie = clamp((cdepth − tie_lo) / (tie_hi − tie_lo), 0, 1).
- **gain_h** = g + mix · H2 · s2 · (1 + (boost − 1) · tie), with boost 1.4
  (see "Precision with an arbitrary f").
- Color, with A = abh:
  - core = β·(harm − A) + [cbn > 0]·γ·(κ·(an, bn) − A) + C·s;
  - bandd = [cbr > 0]·γ_r·(κ_r·(ar, br) − A)·s;
  - cmix = 1 where fc, else mix;
  - d = (core·cmix + bandd·(1 − cmix))·fa;
  - keep = 1 − 0.5·mix·s2·tie;
  - **dab_h** = ((A + d)·keep − A)·fa.

### 11. Upsample and apply (`fill`, `apply`)

- Upsample gain_h and dab_h with interpolate(scale_factor 2, bilinear,
  align_corners False), then crop to H × W.
  - src = (dst + 0.5) / 2 − 0.5, clamped at ≥ 0; neighbor min(i0 + 1,
    n − 1).
- The oracle runs strips of 512 rows with a one-cell margin. Strips agree
  with one pass to an ulp, not bit for bit.
  - Every fixture is under 512 rows, so it is one pass. The port may fill
    in one pass.
- m = (k > 0). gain and dab are multiplied by m.
- y = max(Y(x), 1e-6) and u = of(x / y). Add dab to u's a and b, then
  rgb = Oklab.rgb(u).
- out = rgb / max(Y(rgb), 1e-6) · y · 2^gain.
- out is taken only where (`filled`): m, 1e-6 < Y(x) < inf, and the gain
  or either offset is nonzero. Every other pixel is x, bit for bit (hl-1).

## Fixture format and layouts

- Each directory has a `manifest.txt` with one line per array:
  `name dtype dim …`.
  - dtype is f32, i32, or u8, and a scalar is stored as shape 1.
  - The Rust reader splits each line on whitespace.
- Each array is `name.bin`: raw little-endian, C (row-major) order.
- `.gitattributes` marks the `.bin` files binary and the `.txt` files LF.
- Layouts:
  - Pictures and the thumbnail are channels-last: x (H, W, 3), xh
    (Hh, Wh, 3), and thumb and thumb_long40 (h, w, 5).
  - Every other multi-channel field is channels-first. (2, ·, ·) holds
    abh, harm, harm_val, harm_start, harm_eighth, core_dab, band_dab,
    dab_mixed, dab_h, and dab_up; cmap is (32, h, w).
  - Half-size planes are (Hh, Wh). The harmonic's eighth-size planes are
    (⌊Hh/4⌋, ⌊Wh/4⌋). gain_up is (H, W) and dab_up (2, H, W).
  - Per-region arrays are (n, k), rows in ascending id order.
  - out_clipped is (N, 3): the pixels with k > 0, in raster order.
- Weights are PyTorch state-dict tensors, in PyTorch's layouts:
  - conv weights (out, in, 3, 3);
  - linear weights (out, in).
- The built-in weights are one blob (`export_weights.py`, and core's
  `FillWeights::to_bytes` and `from_bytes`), all little-endian:
  - `GCHLFILL`, the format version (u32, 1), and the tensor count (u32);
  - per tensor, in the state dict's order: the name's length (u16) and
    its UTF-8 bytes, the number of dimensions (u8), each dimension
    (u32), then the values (f32, C order).
  - A reader refuses an unknown, missing, or repeated name, a tensor out
    of that order, a wrong shape, a value that is not finite, and bytes
    after the last tensor, so every blob it accepts is the one bytes its
    writer would write.

## Tolerances

These are about 3–5× the oracle's own float32-against-float64 maxima,
taken over all scenes with both checkpoints (`compare_trial.py`).

- **Exact:**
  - fa, fc, kh, any_share, and core_share;
  - regions, ring, ring owner, d0, dcore, and dc0;
  - harm_known, ids, and inv;
  - tie where it is 0 or 1;
  - every pixel hl-1 does not fill, bit for bit.
- **2e-6 abs:** xh, abh, ly, and harm. Measured up to 4.5e-7.
- **5e-6:** s, mix, mixg, s2, tie, keep, and dab_h. Measured up to 8e-7.
- **2e-5:** feats, cmap, and ctx. Measured up to 7e-6.
- **outputs and params:** 5e-6 with the random weights and 5e-5 with
  r6e-s1. Measured 1.1e-6 and 1.5e-5.
  - L and L2 are relative to max(|v|, 1), as the depths they scale
    (they reach 4096, where one ulp is 2.4e-4). Every other parameter is
    absolute.
- **1e-6 relative to max(|v|, 1):** dd, dc, depth, and cdepth (values
  reach 512). Measured up to 3e-5 abs on the fixtures, and 1.5e-4 abs at
  values near 469 (3.2e-7 relative) on a 2400×1600 case in review.
- **1e-5:** gain_h. Measured 2.6e-6.
- **Final picture: 5e-6 relative to max(|v|, 1).** Measured 7.7e-6 abs
  on values up to about 4 (1.9e-6 relative), and 1.4e-6 relative in the
  port; the error is flat in the value.
- **The thumbnail:** 2 ulp. Its f64 sums reproduce exactly, but numpy's
  float32 log2 is up to 2 ulp from the correctly rounded one (log2 of
  0.92012054 is −0.12010523; numpy 2.5 gives −0.120105214), and a
  platform libm may add one more.
- **Threshold flips.** yh > 0.25, exp2(ly) > 0.25, fq > 0.25, Y(x) > 1e-6,
  and a field's exact zero can flip on a 1-ulp difference. None of the
  fixtures sits within an ulp of one of them.
  - A flip of the zero test moves a pixel by at most the round trip's
    2e-6, which is within the picture's tolerance.

## Branch coverage

- **Regions:**
  - 8- against 4-connectivity (3 regions against 4): diagonal.
  - Ring ties going to the larger id: diagonal (46 cells, the larger id
    on the left), depth_tie, mixed, and depth_tie_f3.
  - No region at all, so the fill returns x: no_clip. That scene has no
    clipped pixel.
  - A clipped pixel no field reaches stays x: mixed, its lone pixel
    (hl-1).
  - No outside row (ids[0] ≠ 0): all_clipped.
- **Distances and the harmonic:**
  - d0 at the 512 cap, and a harmonic with nothing known: all_clipped.
  - No bright rim: all_clipped and mixed.
  - Odd padding: edge_cut and depth_tie_f3.
  - The harmonic's quarter grid dropping rows or columns: edge_cut,
    lamp_f3, and depth_tie_f3.
  - An empty quarter grid (under 8 px a side), harm 0: tiny (hl-1).
- **Features:**
  - top from two-color cells: lamp, line, sky, and others.
  - top from one-color cells: edge_cut, depth_tie, and mixed.
  - top from the rim, with has_band 0: diagonal and mixed.
  - Core share exactly 0.5 with kh 3: mixed, xtrans, lamp_f3, and
    depth_tie_f3.
  - The thin cue: every scene with a region.
- **Synthesis:**
  - off = 0.5·depth0: line and mixed.
  - L and L2 at their lower clamp: most scenes.
  - tie 0: lamp and others.
  - tie between 0 and 1: sky (0.27), depth_tie (0.48), and
    depth_tie_f2_5 (0.31).
  - tie 1: edge_cut and all_clipped.
  - Core depths: 4.3, 12.3, and 29.2 px in depth_tie; 61 px in edge_cut;
    9.2 px in depth_tie_f2_5.
  - mixg at σ 6: every f = 1 scene. At σ 2.4: depth_tie_f2_5. Skipped
    at σ 2: lamp_f3 and depth_tie_f3.
- **Color and apply:**
  - The ab clamp, negative LMS, the log floor, and the Y ≤ 1e-6 guard:
    mixed.
  - NaN and infinite x: nonfinite.
  - Bit 7 ignored: xtrans.
  - Headroom: headroom.
- **The port's tiles** (core keeps the fields in tiles of 16 half-size
  cells): a region on four tiles, and filled pixels reading an empty tile
  beside a stored one: tiles. It is no departure from the trial, but is
  written after the departures so every other fixture keeps its bytes.
- **Not covered:**
  - regions large enough for the f64 sums to differ from f32 ones by
    more than the tolerances (core tests a 3000 × 2000-cell region of its
    own);
  - L or L2 at 4096;
  - r1 or r11 at ±20;
  - H or H2 in tanh's saturation;
  - tie strictly between 0 and 1 at f = 3 (depth_tie_f3's deepest core
    sits exactly at its lower bound, 4); f = 2.5 covers it;
  - d0 at the cap in a region that also has an unclipped rim. With S = 1
    that needs a region over 1000 px across;
  - ids at or above 2²⁴ half-size cells.

## What the fixtures leave out

- Boxed runs. Only whole frames are stored; `compare_trial.py` runs the
  boxed equality.
- The final picture at unclipped pixels: `out_clipped` holds only the
  clipped pixels, and the rest are x.
- Half-size stages are stored by group, per scene (`GROUPS` in
  `make_fixtures.py`).
  - Every stage is stored only in `lamp_f3`: the encoder's map, the
    harmonic's internals at an eighth, the upsampled fields, and the
    per-pixel synthesis.
  - `depth_tie` stores no half-size plane; its reduced versions do.
- Thumbnails. Only `lamp_f3` and `lamp`'s `thumb_long40` (a ragged box)
  store one. Elsewhere the test builds it from x and the codes.
- `headroom`'s x and codes. They are lamp's, with x ÷ 1.123 in f32.
- Trained weights. An ignored test can write r6e-s1 with
  `checkpoint.save` and compare it against the oracle.

## Trial behavior the port reproduces

Each of these is the trial's, and hl-1 keeps it.

1. **Brightness is judged two ways.** The harmonic takes yh > 0.25, and the
   features take exp2(log2(max(yh, 2⁻¹²))) > 0.25.
2. **Two definitions of the band.**
   - Color uses the shares: band = fa · (1 − fc).
   - The features use kh, the block's largest code.
   - A cell with core share exactly 0.5 and kh 3 is a band cell for color
     but counts in none of b1, b2, or core. Its region's shares then do
     not sum to 1.
3. **`where` uses the padded size 2·Wh / fw.** For an odd size this
   overstates the extent by a pixel; a whole frame gets (W + 1) / W.
4. **The thumbnail's blocks do not fit the frame.** They span f_t·tw ≥ W
   px, but the sampling treats them as exactly the frame. The trial's
   thumbnails were stored at f16, ours are f32.
5. **The harmonic depends on the crop.**
   - Its quarter grid drops trailing cells and is stretched back with
     n_in / n_out, not ¼.
   - It starts from the crop's mean of known cells and stops after 300
     iterations, unconverged on large regions. It starts at 0 when
     nothing is known.
   - This is most of the difference between the whole frame and the
     trial's box: on these scenes, at most 0.08 stop and 0.014 in ab.
6. **The frame edge is never unclipped.** A region cut by the frame reads
   deeper than it is. Distances are quantized to 4 px and capped at 512;
   cells beyond reach share the cap's value.
7. **Ids depend on the grid.** An id is the largest flat index on the
   padded half grid, and ring ties go to the larger id. This is
   deterministic but depends on raster position.
8. **The outside is a row of its own.** Its features and outputs are
   computed, then the outputs are zeroed. Its parameters (L = depth,
   β = 0.5, and so on) are masked by fa.
9. **Look scale at f = 3 gives σ_g = 2.0 exactly**, which skips the wide
   branch. The result is the same.
10. **The ab clamp is at half size only.** The full-size apply has none.

The trial's f32 per-region sums, its labels' iteration limit, and its
pool's refusal of frames under 8 px are not kept; see "Where hl-1 differs
from the trial".
