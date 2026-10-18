# §345. The Sky matte over a diluted map: the sure sky held, and its pockets (2026-10-17)

The first follow-up the landscape build left (§335, "Left, and
follow-ups"): on XT5, a peak above a sea of cloud, the found sky shows
a row of square holes at 0.7–0.8 alpha in flat cloud and stair-stepped
plateaus in the fog, and the seeds, the clip and the matte read the
soft map over all the queries, diluted on that frame, where the gate
reads the map over the kept ones. §335 left the matte alone because
that map feeds every found sky's floor and cap, so all 32 skies of the
sky set would move. This section measures that, decides what an
unlabeled pixel inside a found sky means to the matte, and changes the
matte by the least that fixes XT5.

### The measurement first

Two pieces, kept in the tree:

- `greycard-ui`'s ignored `dump_the_models_inputs` (worker/samples.rs):
  each raw of the folders `GREYCARD_SAMPLES` names (the store in
  `GREYCARD_MODELS`, which it requires) opened and
  developed as the worker does under the default edit, then
  `<stem>.png`, the models' preview (`ai::preview`), and `<stem>.lin`,
  the linear base at the frame's own size (or `GREYCARD_DUMP_LONG`),
  in the GCLN format `tests/landscape.rs` already reads. The trial of
  §335 wrote the same from a test it did not keep; this is that test,
  kept, so the next measurement does not write it again. 42 frames
  (the sky set's 41 and XT5) took 289 s and 14 GB, full size.
- `greycard-ai`'s example `sky_matte`: the Sky shape over a folder of
  those, as the editor makes it with no picks (the prior, the gate,
  SAM 2.1's outline when SAM is in the store and `SKY_NO_SAM` is not
  set, the color-line matte from the full-size base, the raster at the
  preview's size), on the CPU or WebGPU (`SKY_PROVIDER`). A line a
  frame: the gate's verdict and core, the matte's recall and spill
  against the labels (as §335 measures its groups), the share of the
  labeled sky held between 0.05 and 0.95 ("partial", where a plateau
  shows), the unlabeled pixels whose region borders only labeled sky,
  how many of those are holes (under 0.85 in a region held at 0.95 or
  less, so surrounded by alpha over 0.95, that touches no edge of the
  frame or never falls under a half), and the stages' times; and the
  alpha, the alpha over the preview and the holes as PNGs.
  `SKY_COMPARE=A,B` compares two runs' alphas. `SKY_NONE` fails a run
  that paints a frame named in it; the nine without a sky were named in
  every run.

Release, 9950X3D, the machine shared with two other agents (load
average 9 to 13 throughout). Commands, from the worktree, through a
nice-19 wrapper with the XDG directories under `target/scratch`:

```text
GREYCARD_SAMPLES=target/scratch/frames GREYCARD_DUMP=target/scratch/dump \
  cargo test --release -p greycard-ui --bin greycard-ui -- --ignored --exact \
  worker::samples::dump_the_models_inputs --nocapture --test-threads=1
GREYCARD_MODELS=<store> GREYCARD_REQUIRE_ORT=1 SKY_IN=target/scratch/dump \
  SKY_OUT=<run> SKY_NONE=<the nine> cargo run --release -p greycard-ai --example sky_matte
```

**The baseline** (master's matte, CPU, SAM): the nine without a sky
get no pixel; 33 frames get a sky (the sky set's 32 and XT5), all but
XT5 from SAM's outline; recall 73.2–99.7% (DSCF0012's twigs against
glare the low one, the rest 93.3–99.7%), spill 0.0–24.5%. Of the 33,
seven have unlabeled pixels whose region borders only sky (101,296
pixels in all, 64,800 of them on 5M0A7900), and two have holes among
them: XT5, 8,093 of its 18,544, and 5M0A7795, 1,969 of 1,984 — a sky
lantern the labels did not name, which is rightly a hole. CPU against
WebGPU: at most 1/255 on every frame but XT5 (8/255, mean 2e-4/255),
no pixel across a half anywhere.

**What XT5's holes are.** A probe of the matte at preview pixels in and
beside the row of squares:

- the soft map over all the queries is 0.24–0.27 across the whole sea
  of cloud, the map over the kept queries 0.99–1.00; the dilution of
  §335 is not only under 0.8 there but at a quarter;
- the squares are not mostly the unlabeled pixels: they are the
  labeled sky around each pocket, the half-percent band the outline's
  shrink leaves in question, since the outline is the labels (no seed
  survives a map that peaks at 0.761) and has a hole at every pocket;
- at the preview's scale those pixels are plainly sky: they project at
  0.97–1.06 on the line, a chromaticity 0.002–0.02 from the local sky,
  0.003–0.03 off the line;
- at full size the same pixels average 0.67–0.93: ISO 3200 noise on a
  projection the share clamps at one, so its mean falls under one,
  and the guided filter, whose guide carries the same noise, keeps it;
- nothing held them up. The growth would have joined them to the known
  sky, but it asks the prior for 0.35 or more and read the diluted map
  (0.26); the floor would have held them at 0.8, as it holds the band
  on every other frame, but ramp(p; 0.7, 0.95) of 0.26 is nothing.

On every other frame the diluted map over a sure sky is 0.85 or more,
so the growth's test passes and the floor holds the band at 0.6–1.0.
XT5 differs by the dilution alone.

### The candidates and their numbers

Each over all 42 frames, CPU, SAM, against the baseline. "Others" are
the 32 skies of the set; the nine without stayed empty in every run.

| Candidate | XT5 holes / partial | Others | Why not, or why |
|---|---|---|---|
| (a) unlabeled pixels enclosed by sky labels are a class sky does not show through: the floor and cap from the diluted map | 2,234 / 9.21% | only 5M0A7900 and 7909 move | XT5 unchanged to the eye (a cap of 0.67, no floor, from a map of 0.25); and 5M0A7900 loses the sky through a balcony's railing, an unlabeled region of fine bars the cap at 0.05 closes |
| (b1) the floor and cap read the kept map everywhere | 7,605 / 5.62%, spill +0.97 | recall up to +2.07 (DSCF0012), spill up to +0.30, partial −25 points summed | 5M0A4761: a dark lamp post the labels' coarse step calls sky is painted sky, since the floor overrides color |
| (b2) the same within the labeled sky | 7,733 / 5.78% | recall up to +2.05, partial −24.7 summed | the same post |
| (c) (a) and (b2) | 31 / 5.66% | as (b2) | the post; and 5M0A7795's lantern fills a quarter (1,969 hole pixels to 1,472): the kept map over an unlabeled pixel is a faint mask divided by others, as §335 says |
| (c1) (a) and (b1) | 31 / 5.54% | as (b1) | as (c) |
| (s) the seeds read the kept map within the labels | 0 / 9.54% | none move but DSCF0012 (spill +0.77) | XT5 gets SAM's outline, which leaves out the sea of cloud: recall 98.5% to 57.8% |
| (g) the growth reads the kept map within the labeled sky | 7,662 / 7.85% | recall at most +0.03, partial at most −0.23 | right, and not enough: the band pixels that project under 0.95 at the preview stay noisy |
| (k) (g) and a floor for the sure sky from the kept map, gated by the place on the line, ramp(t; 0.6, 0.85) | 7,602 / 7.71% | as (g) | the gate too narrow: the band's median t is 0.88, noisy at full size |
| (k3) (g), the floor ramp(t; 0.4, 0.7), pockets, and a cap on unlabeled regions by their dip in the soft map | 531 / 7.41% | 5M0A7900 spill −0.90 | the railing again: its region's soft map is a ninth of the sky's |
| (k5) (g) and the floor at ramp(t; 0.4, 0.7) on the labeled sky alone | 7,553 / 7.42% | as shipped | the pockets themselves stay holes |
| (k4) (g), the floor at ramp(t; 0.4, 0.7) on the labeled sky inside the outline and its pockets, its hue tested from t = 0.7 on the frame's own pixel | 531 / 7.41% | as below | the review: a rose or magenta square, a pocket darker than the sky, came out at 1.000 against master's 0.014; the floor read no distance off the line, and the hue gate only near the sky's end |
| **shipped**: (k4) with the floor's color tests on the preview's pixel, off the line and of the sky's hue at any t; pockets no larger than 0.5% of the frame | **518 / 6.52%** | below | |
| the same with the color tests on the frame's own pixel | 3,680 / – | | the noise the floor is there for fails them |

### The rule

The matte reads the soft map over the kept queries, as the gate does,
in two places and only within the labeled sky; everywhere else it reads
the map over all the queries as before.

- **The growth.** A pixel in question may join the known sky when it is
  treeish or the prior gives it 0.35, and for a labeled-sky pixel that
  is now the kept map. The growth's color tests are unchanged (on the
  sky's chromaticity within 0.04, projecting between 0.95 and 1.3,
  within a quarter of the line's length of it), so a pixel joins only
  where its color is the sky's.
- **The sure sky's floor.** A pixel in question that is labeled sky
  inside the outline, or lies in a pocket, and is darker than the local
  sky and not near a person's negative pick, is held up to

  ramp(kept; 0.7, 0.95) · ramp(t; 0.4, 0.7) · hue(q; 1) · (1 − ramp(off_q; 0.25, 0.5))

  where t is the frame's own pixel's place on the line, and q the
  preview's pixel there, with off_q its distance off the line. That is
  a floor for the sky's own color a little darker: in the last 60% of
  the line, near it, and of the sky's hue at any place on it, tested on
  the preview's pixel because the frame's noise is what the floor is
  for (the same tests on the frame's own pixel leave 3,680 of XT5's
  holes). Anything else is the projection's, as before: a post in the
  first 40% of the line (t about 0.1) within 0.02 of master's alpha,
  and a rose or magenta pocket at t about 0.7 too (tests hold both).
  The floor of §175 stays as it was beside it, on the diluted map, for
  the dark cloud SAM left out; that one does override color, and the
  diluted map's doubt is what keeps it off a post or a wall.
- **Pockets** (`matte::pockets`). What an unlabeled pixel inside a
  found sky means: a pocket of that sky when its region (four ways)
  borders only labeled sky and the frame's edge, all of that border
  inside the outline, it covers no more than 0.5% of the frame, and
  its soft map keeps at least 0.8 of its border's on average. Then the
  sure sky's floor holds it as it holds the sky around it; otherwise
  it stays what it was, a pixel sky shows through, the projection's
  alone. The conditions are each a frame, or the review's probe:
  - touching a tree or any other label: not the sky's pocket (a test
    holds the same square, touching a trunk, at the projection's 0.8);
  - the soft map's dip: 5M0A7795's lantern keeps 0.32 of the sky's map
    around it, 5M0A7909's lanterns 0.09 and 0.62, 5M0A7900's railing
    0.11; XT5's pockets 0.92–1.00. None of those things is held, and
    their regions keep the projection, so the railing keeps its sky;
  - the border inside the outline: what SAM left out stays out, as
    §175 has it;
  - the size: the frame's edge counts as border, so without a cap an
    unlabeled foreground from the bottom of the frame to both sides,
    under a labeled sky, is a pocket (the review's on-line shaded snow
    went from 0.857 to 0.986). A pocket of the logits' grid is a few
    dozen cells; XT5's largest is 11,102 pixels, 0.40% of the frame.
  - The soft map is not what keeps every thing out. Without SAM (the
    labels the outline), 5M0A4761's lamp post is four pockets keeping
    0.89–0.99 of the sky's map, and 5M0A7661's dark facade one keeping
    0.95; with SAM both are out only because their border lies outside
    SAM's outline. What keeps them out of the sky is the floor's color
    tests: both lie near the not-sky end of the line. Without SAM their
    alpha is master's (crops `out/4761-post-nosam.png`,
    `out/7661-facade-nosam.png`).
- **Scope.** `matte::color_line` takes `Hold::Sure` for the Sky and
  `Hold::None` for the landscape's groups. Their soft map is over the
  kept queries already, so the growth reads the same map either way,
  and their mattes, measured in §335, do not move.

### What moved

All 42 frames, CPU, SAM, before (master) and after:

- **The nine without a sky: no pixel, before and after**, on the CPU
  and on WebGPU, with SAM and without.
- **XT5**: holes 8,093 to 518 of the 18,544 unlabeled pixels inside its
  sky; partial 9.20% to 6.52% of the labeled sky; recall 98.48% to
  98.51%, spill 10.30% to 10.38%. In the crop of the row of squares
  (560 × 150 preview pixels) the least alpha goes from 184/255 to
  209/255, the share under 0.92 from 8.4% to 0.09% and under 0.85
  from 3.6% to 0.001%. The blobs at the bottom are pale; the plateaus
  where the fog runs between cloud and forest are as they were
  (below).
- **The other 32**: recall moves on three frames, by +0.01 to +0.03
  points; spill on two, by +0.01 (5M0A7909) and +0.04 (5M0A0943, the
  soft halo above a treeline brought a few pixels down); partial falls
  on 27, by 0.01 to 1.5 points (DSCF0012 the most, then 5M0A5229
  0.53, DSCF0203 0.47, 5M0A4761 0.43). Over a whole frame the alpha
  moves by at most 0.11/255 on average and 0.026% of the pixels cross
  a half (5M0A0943); no hole appears or closes on any of them. Against
  the first build of the rule (the review's k4), the color tests moved
  ten of them back by 0.01 to 0.28 points of partial and nothing else.
- Where the partial fell, by eye at 2–4×: DSCF0074 and DSCF0203 had the
  same stepped plateaus as XT5 in the overcast and haze just above the
  horizon, at 0.8–0.9 with edges at the labels' grid, and they fill;
  DSCF0151's sky meets a hazy sea a few pixels lower; 5M0A0943's halo
  above the larches narrows; 5M0A4761's lamp post is as it was, with
  the sky between its two uprights lighter.
- **Without SAM** (`SKY_NO_SAM`, the prior's labels every outline, as
  the editor runs without SAM): the same, recall at most +0.03 (XT5),
  spill at most +0.08 (XT5), partial −4.8 points summed; the post and
  the facade as above.
- **CPU against WebGPU**, after: at most 1/255 on every frame but
  DSCF0012 (6/255, mean 1e-4/255), no pixel across a half; before, at
  most 1/255 but XT5 (8/255). The empty rasters are identical.

### Times

The matte's stage (`Times::edge`, the full-size matte averaged to the
raster), the least of four runs a side, alternating before and after,
on a machine whose run-to-run spread on one build was 4% to 15% in
total and up to 35% on a frame: **7.39 s before, 7.33 s after over the
33 skies (−0.7%)**; on a frame from −9.8% (XT5, 0.297 s to 0.268 s) to
+10.5% (5M0A7900, 0.153 s to 0.169 s). Each frame's least, 0.11–0.44
s, is in the table below. The preview's projection the color tests add
is taken only for a pixel the floor may hold. A first build of the rule, which computed the
pockets and the negative picks' lookup for every pixel in question,
measured +3.3% and was trimmed: the pockets are found only when the
frame has an unlabeled pixel, and a pixel sky shows through looks up
nothing more unless it is in one.

### The step at the logits' grid

The roadmap's line asked whether the labels are upsampled from 1024
with no interpolation. They are not: `label_at` reads each query's
mask logits bilinearly from the 160 grid (half-pixel centers, as
`F.interpolate`) before the sigmoid and the argmax, at 1024, and only
the finished labels go to the preview by the nearest pixel, a step of
two pixels at 2048. The steps at the grid are the logits' own: in flat
cloud the classes' interpolated logits differ by little, so the argmax
and the claim-over-a-half trace the 160 grid's cells (a cell is 12.8
preview pixels on XT5), and so do the pockets.

The plateaus are the same cause as the holes in part. Inside the
outline's shrink the known sky is one, with an edge at the labels'
steps; just outside it the projection decides, and on XT5 nothing held
it up. The sure sky's floor now holds what lies in the last 60% of the
line toward the sky, which fills the plateaus in flat cloud on XT5,
DSCF0074 and DSCF0203. Where the fog itself runs from cloud to forest,
the line's middle, the known sky's stepped edge is still a step, and
the bilinear read is already in place, so it is not the fix. That part
is the outline's: the labels as the outline when SAM has no seed.

### Cache keys

The Sky raster's key moved, `sky-2` to `sky-3`, so a cached sky is
made again. Foliage's slot hashes the sky's key with its own, so a
Foliage cut by an older sky is made again too. `landscape-2` stays:
the groups' own mattes read `Hold::None` and do not move, and Mountain
and Building are the labels feathered.

The pictures kept of an edit (the thumbnails and the larger picture,
keyed by `Edit::develop_key`) did not follow a mask's raster: since
§335 an edited frame with a Sky kept the picture its old sky made
after `sky-2`. The masks' versions now live in greycard-edit's `key`
module (`SKY_PIPELINE`, `LANDSCAPE_PIPELINE`, `PART_PIPELINE`,
`PERSON_EDGE`), the editor's mask cache reads them from there, and the
develop key takes in, once each, the versions of the stages its masks
are made by (`key::mask_pipelines`: a Sky the sky's, a Foliage the
landscape's and the sky's, a Water the landscape's, a People mask the
part's and the whole person's edge). So a raise remakes the pictures
of the frames with such a mask and of no other; `DEVELOP_RECIPE`
stands. `keys_made_under` names the four too, so the index knows its
stored keys may be another's. With this commit every edit with a Sky,
Landscape or People mask gets a new key once and its pictures are made
again; an edit without one keeps its key.

### Tests

- *greycard-ai, `matte`, 5 new*:
  `an_unlabeled_pocket_of_a_sure_sky_fills` (a pale overcast whose
  diluted map is a quarter and kept map sure, the labels the outline,
  a square of cloud a fifth of the way down the line left unlabeled:
  over 0.95 held, under 0.9 under `Hold::None`, master's rule);
  `the_same_square_touching_a_tree_does_not_fill`;
  `a_lantern_and_what_the_outline_left_out_are_no_pockets`;
  `a_dark_pole_the_labels_call_sky_is_not_held_up` (in the band,
  within 0.02 of master's rule and under a half);
  `an_off_hue_pocket_stays_out` (a rose and a magenta pocket at t about
  0.7 within 0.02 of master's rule; 0.93 under the first build); the
  lantern test also holds an unlabeled foreground to the frame's edges
  out by its size.
- *greycard-edit, `key`, 1 new*:
  `a_learned_mask_s_stages_raised_change_the_keys_of_its_edits_alone`
  (a Sky's and a Foliage's key move with the sky's stages, a Water's
  and an edit's without a mask do not).
- `cargo test -p greycard-ai`: 137 unit tests and the rest, with and
  without `GREYCARD_MODELS`; with it and `--ignored`, `tests/sky.rs`'s
  four (the reference check prints that it has no
  `GREYCARD_SKY_REFERENCE`; the made-up sky on WebGPU; the prior on the
  CPU against WebGPU, labels the same, the map within 4.5e-6).
- greycard-ui's suite in groups, each under the 12 GB address-space
  limit (the whole suite in one process runs out of it there, in
  render and roots tests that build GPU contexts).

### Left

- **The fog's own plateaus on XT5**: where the cloud thins over forest
  the known sky's edge is the labels' step, a hard edge across a soft
  transition. Wants an outline there that is not the labels: SAM, if a
  seed could be had without SAM losing the sea of cloud (seeding from
  the kept map gave 57.8% recall), or a feathered known-sky edge when
  the outline is the labels.
- **Noise at full size**: the projection of a flat noisy sky still
  dips under one wherever no floor holds it, a frame of ISO 3200 or
  more; the faint specks left in XT5's flat cloud are that. Reading
  the projection from a lightly smoothed pixel would touch every
  edge's detail and was not tried.
- **The seeds and the clip** still read the diluted map. On this set
  only XT5 has no seed for it, and the seeds that the kept map gives it
  make a worse outline; left as it is.
- The harness's labels-relative measures cannot see a wrong label
  (§335 said so of Foliage); every move above was also looked at.

### All 42 frames, before and after (CPU, SAM)

| frame | verdict | outline | labeled | core | recall | spill | partial | unlabeled inside (px) | holes (px) | matte (s) |
|---|---|---|---|---|---|---|---|---|---|---|
| 4Z4A2455 | sky | sam | 38.9% | 37.0% | 99.72 | 0.06 | 0.74 | 0 | 0 | 0.209 → 0.212 |
| 4Z4A2615 | sky | sam | 26.8% | 24.9% | 99.62 | 0.06 | 0.96 → 0.95 | 0 | 0 | 0.153 → 0.151 |
| 4Z4A2621 | sky | sam | 27.8% | 25.9% | 99.43 | 0.02 | 1.34 → 1.32 | 0 | 0 | 0.142 → 0.144 |
| 5M0A0933 | sky | sam | 31.3% | 27.3% | 98.52 | 2.96 | 2.98 | 0 | 0 | 0.177 → 0.183 |
| 5M0A0943 | sky | sam | 36.3% | 33.2% | 98.47 → 98.50 | 0.35 → 0.39 | 2.91 → 2.68 | 0 | 0 | 0.167 → 0.162 |
| 5M0A0952 | no sky (the model calls nothing sky); after: same, 0 px | | | | | | | | | |
| 5M0A0957 | no sky (the model is only 95.2% sure of its sky); after: same, 0 px | | | | | | | | | |
| 5M0A0963 | sky | sam | 22.8% | 18.1% | 97.50 | 2.21 | 7.23 → 7.07 | 0 | 0 | 0.196 → 0.203 |
| 5M0A4761 | sky | sam | 23.4% | 19.8% | 96.88 → 96.89 | 3.08 | 6.47 → 6.04 | 1344 | 0 | 0.164 → 0.168 |
| 5M0A5058 | sky | sam | 24.4% | 20.4% | 98.48 | 0.35 | 2.38 | 0 | 0 | 0.131 → 0.132 |
| 5M0A5062 | sky | sam | 27.1% | 23.3% | 98.07 | 0.37 | 4.12 → 4.03 | 0 | 0 | 0.144 → 0.134 |
| 5M0A5074 | sky | sam | 36.3% | 32.5% | 98.66 | 0.20 | 3.07 → 3.01 | 0 | 0 | 0.151 → 0.141 |
| 5M0A5135 | no sky (the model calls nothing sky); after: same, 0 px | | | | | | | | | |
| 5M0A5229 | sky | sam | 6.4% | 4.5% | 93.27 | 0.42 | 16.35 → 15.82 | 0 | 0 | 0.125 → 0.118 |
| 5M0A7658 | sky | sam | 56.4% | 52.1% | 99.48 | 5.20 | 0.66 | 0 | 0 | 0.140 → 0.140 |
| 5M0A7661 | sky | sam | 42.2% | 36.8% | 99.16 | 2.14 | 2.16 | 1052 | 0 | 0.187 → 0.192 |
| 5M0A7680 | no sky (the model is only 85.1% sure of its sky); after: same, 0 px | | | | | | | | | |
| 5M0A7705 | sky | sam | 36.5% | 34.0% | 99.48 | 0.92 | 1.49 → 1.48 | 0 | 0 | 0.155 → 0.169 |
| 5M0A7737 | sky | sam | 44.3% | 40.0% | 99.71 | 3.56 | 1.92 → 1.83 | 0 | 0 | 0.189 → 0.196 |
| 5M0A7753 | sky | sam | 16.9% | 11.6% | 97.36 | 15.31 | 6.14 → 6.13 | 0 | 0 | 0.200 → 0.217 |
| 5M0A7795 | sky | sam | 28.8% | 24.9% | 98.58 | 0.31 | 2.24 → 2.23 | 1984 | 1969 | 0.133 → 0.136 |
| 5M0A7880 | sky | sam | 12.6% | 10.1% | 96.95 → 96.97 | 0.43 | 6.51 → 6.19 | 0 | 0 | 0.128 → 0.141 |
| 5M0A7900 | sky | sam | 38.5% | 35.5% | 99.50 | 1.53 | 1.60 → 1.50 | 64800 | 0 | 0.153 → 0.169 |
| 5M0A7909 | sky | sam | 45.6% | 36.3% | 98.89 | 1.00 → 1.01 | 3.54 → 3.43 | 13560 | 0 | 0.172 → 0.169 |
| 5M0A7950 | sky | sam | 11.9% | 9.8% | 98.93 | 0.62 | 2.28 → 2.26 | 0 | 0 | 0.112 → 0.117 |
| DSCF0012 | sky | sam | 7.6% | 3.6% | 73.23 | 24.48 | 27.85 → 26.31 | 0 | 0 | 0.442 → 0.438 |
| DSCF0015 | sky | sam | 25.5% | 23.1% | 98.55 | 0.33 | 2.50 → 2.45 | 12 | 0 | 0.352 → 0.337 |
| DSCF0016 | sky | sam | 22.9% | 20.2% | 98.16 | 1.99 | 4.67 → 4.61 | 0 | 0 | 0.327 → 0.324 |
| DSCF0025 | no sky (the model is only 92.9% sure of its sky); after: same, 0 px | | | | | | | | | |
| DSCF0029 | no sky (the model is only 97.7% sure of its sky); after: same, 0 px | | | | | | | | | |
| DSCF0035 | sky | sam | 31.6% | 29.1% | 98.84 | 1.18 | 2.46 → 2.32 | 0 | 0 | 0.412 → 0.398 |
| DSCF0074 | sky | sam | 61.8% | 59.8% | 98.83 | 0.13 | 2.37 → 2.02 | 0 | 0 | 0.298 → 0.294 |
| DSCF0121 | sky | sam | 35.8% | 33.8% | 99.16 | 0.52 | 3.11 → 3.03 | 0 | 0 | 0.314 → 0.297 |
| DSCF0137 | sky | sam | 23.8% | 21.9% | 99.13 | 0.34 | 3.05 → 2.92 | 0 | 0 | 0.292 → 0.299 |
| DSCF0151 | sky | sam | 30.6% | 27.9% | 98.85 | 0.53 | 2.56 → 2.36 | 0 | 0 | 0.355 → 0.347 |
| DSCF0153 | no sky (the model calls nothing sky); after: same, 0 px | | | | | | | | | |
| DSCF0186 | no sky (the model calls nothing sky); after: same, 0 px | | | | | | | | | |
| DSCF0195 | no sky (the model is only 72.9% sure of its sky); after: same, 0 px | | | | | | | | | |
| DSCF0203 | sky | sam | 40.6% | 38.5% | 99.52 | 0.43 | 3.12 → 2.65 | 0 | 0 | 0.301 → 0.288 |
| DSCF0741 | sky | sam | 16.0% | 14.6% | 97.43 | 0.00 | 5.60 → 5.30 | 0 | 0 | 0.248 → 0.244 |
| DSCF0766 | sky | sam | 34.5% | 32.7% | 98.52 | 0.00 | 2.55 → 2.51 | 0 | 0 | 0.419 → 0.406 |
| XT5 | sky | labels | 54.7% | 44.7% | 98.48 → 98.51 | 10.30 → 10.38 | 9.20 → 6.52 | 18544 | 8093 → 518 | 0.297 → 0.268 |
