# §661. Midtones, lower and narrower (2026-11-12)

**Found in use.** §644's bell was too broad upward: it reached to the
frame's top, so on a typical frame a stop over the middle kept 71% of
its peak and it overlapped Highlights, which moved the same tones.

**Tried.** A trial build offered six shapes beside §644's. The least
slope ties reach to travel: a smoothstep's steepest slope is 1.5 over
its reach, so at the slider's limit `1 − 2 × 1.5 × peak / reach` must
stay at 0.25, and a narrower bell moves the middle less.

- Two stops each way, not stretched to the top: no Highlights overlap
  at its peak, but still half the peak a stop over the middle.
- 1.5 stops each way at 0.375: the overlap gone, the travel down to
  ±0.75 stop.
- 1.5 stops each way at 0.5: the travel kept, the least slope zero, so
  the curve can fold. Out.
- Each centered half a stop lower: Highlights' overlap traded for some
  of Shadows', most on low-key frames, where Shadows is full near the
  middle (0.86 of the peak there at two stops' reach, against 0.52).

**Chosen.** Full half a stop under the highlights span's foot
(`MIDTONES_UNDER_FOOT`), falling by a smoothstep to nothing 1.75 stops
either way (`MIDTONES_REACH`), peak 0.4375 (`MIDTONES_PEAK`): ±2 moves
the bell's center ±0.875 stop, and the least slope is
`1 − 2 × 1.5 × 0.4375 / 1.75` = 0.25, as before. It is nothing from a
stop and a quarter over the foot, so the frame's top is never moved
(the span is never under 1.6 stops). One shader serves the viewport,
GPU exports, and masks' Midtones; the CPU's `midtones_weight` is its
reference.

**Figures at +2** (stops of shift; "foot" is the span's):

| frame | center (foot −0.5) | foot | median | grey | foot −1 | foot +1 | frame top |
|---|---|---|---|---|---|---|---|
| typical (median −0.9, top +1.9) | +0.875 | +0.70 | +0.70 | +0.09 | +0.70 | +0.05 | 0 |
| low-key interior (median −4.67, top +0.97) | +0.875 | +0.70 | +0.01 | 0 | +0.70 | +0.05 | 0 |
| high-key (median +0.4, top +1.2) | +0.875 | +0.70 | +0.14 | +0.42 | +0.70 | +0.05 | 0 |

On the typical frame, the share of the peak from two stops under the
foot to two over, by half stops: .06, .39, .80, 1, .80, .39, .06, 0,
0; nothing where Highlights peaks or where Shadows is full. Where the
bell is full, Highlights takes at most 6% of its own (the widest span);
where Highlights peaks, the bell is nothing but for 0.2% on the least
span. On the low-key frame Shadows' full point takes 0.82 of the peak.

**Proofs re-run.** Each region alone keeps its least slope (shadows
0.14, midtones 0.25, highlights 0.125); composed over the eight
corners the least is now 0.017 (0.0052 before; shadows and midtones at
+2, highlights at −2, on the least span), 0.020 without midtones. The
32-corner finish sweep under both curves at two display whites, the 16
over every span, and the 8 along a merge's pull pass unchanged. The
CPU and the shader agree on lavapipe, the random parity run on seeds 0
to 10.

**Kept pictures.** Midtones is unreleased, so no schema change. An
edit that sets it, the picture's or a mask's, takes `MIDTONES_SHAPE`
into its develop key (as `SHARPEN_GUIDE` does), so its kept pictures
are made again and every other frame's stay; the index's keys are
remade once from the sidecars.

**Open.** A feel decision; the constants may be tuned again.
