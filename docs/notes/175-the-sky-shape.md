# 175. The Sky shape (2026-09-25)

Roadmap v0.5.0's AI masks line: "Sky first, and it must never paint where
there is no sky." Built from the trial of the section before last. The
Python was the spec: `run_panoptic.py`, `run_sam_sky.py` and
`edges_matte.py`, with the growth passes of `tree_gaps.py`. A solo
item after wave C, an opus author and an opus reviewer, one round.

**What it is.** `Shape::Sky { picks }` in greycard-edit's mask module,
beside `Subject {}`. The tag is `sky`, and `picks` defaults to empty, so
`{"kind":"sky"}` loads. It is a learned raster shape, like Subject and
Object.

The Masks tab has a Sky button (a Lucide cloud icon) beside Subject, in
both the new-adjustment row and the add-shape row. On a chosen sky the
add-shape button reads Pick. Pick puts a tool in hand the way Object's
does: a click adds a positive pick, a right click a negative one, and
the picks show as dots. The raster is made on the worker through the
same `Job::Mask` path as Subject.

The pipeline lives in `greycard-ai::sky` and `greycard-ai::matte`. Each
stage is a pure function with a CPU reference and unit tests.

1. **The prior.** EoMT-S, COCO panoptic at 640 (`registry::SKY`, id
   `eomt-s-coco-panoptic-640`). The license is MIT, and the notice
   carries the TU/e copyright. `modified` says the file is an ONNX export
   of the published weights at revision 10f5326 and that the weights are
   untouched. The file is 96,025,182 bytes, sha256 `805ed0fb…372d`, and
   its URL is `huggingface.co/jessolmstead/greycard-sky`, published
   there the next morning with the MIT notice and a card, the served
   file's size and hash checked against the registry's. `tools/ai/eomt_export.py` reproduces those bytes exactly
   with the versions now in `tools/ai/requirements.txt` (torch 2.14.0,
   transformers 5.17.0, onnx 1.23.0, protobuf 7.36.2). One detail matters
   for the hash: the wrapper's attribute is named `m`, because that name
   is in every node and weight name.
   - *Input.* The picture is letterboxed to 640 on its long side with an
     antialiased bilinear resize (a triangle filter widened by the
     scale, as torchvision's is), padded with black at the bottom or
     right, and ImageNet-normalized.
   - *Labeling rule* (the trial's, not the transformers post-processor).
     A query is kept at a class probability of 0.5 or more. Each pixel
     goes to the kept query with the highest class times mask
     probability. A pixel whose winning mask is under a half is
     unlabeled. Labels are made at 1024 on the long side, then brought
     to the preview: the soft map bilinearly, the labels to the nearest
     pixel.
   - *Only a sure sky is sky* (a departure from the trial, after the
     review). A kept sky query the model is less than 98% sure of
     labels its pixels a doubt (`DOUBT`), not sky: a class the matte
     treats as neither sky nor one sky shows through. The soft map's
     numerator counts only the sure sky queries: Σ p(sky|q)·m_q over
     them, over Σ p(any|q)·m_q over all the queries. Without this, the
     gate asked only whether *some* sky query was sure, while the
     labels, the core, the edge test and SAM's clip took every sky
     query. The reviewer's probe had one 99.5% query over a strip at
     the top and one 90% query over a large block. That passed the gate
     and labeled 54% of the frame sky against the sure query's 7.5%:
     a 0957-type slope painted whenever a real sky elsewhere is sure.
     It now labels 7.5%, and none of the block.
   - *Exactness.* On the trial's `eomt.in.f32`, the Rust class logits
     match `eomt.cpu.f32` to 0.0. The mask logits match Python ORT 1.30's
     to 3.0e-4; the Rust side runs ORT 1.28.
2. **The gate** (`sky::gate`). It is stricter than the trial's; see
   "The gate" below. In order:
   - a kept sky query's class probability is at least 0.98;
   - the soft map over 0.8, after an erosion of 1% of the long side,
     covers at least 0.25% of the frame;
   - at least 0.5% of the frame is labeled sky;
   - the sky comes within 1% of the long side of the top, the left or
     the right edge.

   Fail any, and the raster is empty and the status line says "no sky
   found in FILE". A cached empty raster says the same. SAM is loaded
   and the preview embedded only after the gate passes, so a frame with
   no sky pays only for the prior.
3. **The outline** (`sky::seeds`, `sky::outline`).
   - *Seeds.* Up to eight positives over the confident sky (over 0.8)
     and eight negatives over the confident not-sky (under 0.1). Both
     grounds are eroded 20 px, scaled from 2048 to the preview.
   - *Decodes.* One SAM decode per positive, with every negative. The
     union is clipped to the prior's sky (over 0.5) grown by 12 px.
   - *Fallback.* With no positive seed, or nothing left after the clip,
     the prior's own labels are the outline.
   - *Picks.* A person's positive picks each decode and are not clipped.
     Their negatives join every decode.
   - *Departure from the Python.* The trial picked a random pixel in
     each grid cell and a random eight of the cells. Here each cell's
     point is the mask pixel nearest the cell's mask centroid, and the
     cells are chosen farthest-first from the fullest. The spread is the
     same, and a picture always gets the same mask.
   - *SAM is optional.* When SAM is not in the store, or its offer was
     declined or failed, the shape runs on the prior alone.
     `ai::model_with` offers the prior first, then SAM. SAM failing at
     run time (to load, to embed or to decode) does the same: `find`
     catches it, logs it, and the status line says the sky came from
     the model's labels alone. Before the review, that failed the whole
     mask.
   - *Picks and what they cannot do.* A positive pick needs SAM. A pick
     on a frame the gate refused adds nothing: a click is not allowed
     to make a sky the model does not see, since the gate is the rule
     that must never fail. The panel says so rather than leaving
     clicks to do nothing:
     - Pick on a sky the gate found none in does not take the tool,
       and says "no sky to correct: the model found none in this
       picture";
     - without SAM, Pick takes the tool for negative picks and says a
       click that adds sky needs the Object model;
     - a made raster says the same on the status line.
4. **The edge** (`sky::refine_sky`). This is the one interface between
   the outline and the raster. Behind it is the color-line matte at the
   frame's own resolution in linear working-space values
   (`matte::color_line`), averaged down to the 2048-wide raster. When the
   matte has nothing to go on (no known sky after the shrink, or no
   linear frame of the preview's shape), the fallback is the guided
   filter at the Subject rule (`sky::feathered`).

**The gate.** On the editor's own preview, the trial's gate let through
5M0A0957, the defocused bluish snow slope: 9% of the frame painted. The
trial's rule was a core over 0.8 surviving the erosion. There the core
survived the 20 px erosion that saved the trial on the CLI's preview.

The first fix added two guards: a sure query (0.98) and a core over
0.9. The review showed the second was wrong.

- 6U3A7205 is a pale hazy sky over a coast. Its query is 99.8% sure
  and 30.6% of the frame is labeled sky, but its core over 0.9 was
  0.06% of the frame.
- The cause is the soft map's denominator, summed over all 200 queries.
  The leftover low-probability queries dilute it, and a pale sky's map
  sits between 0.8 and 0.9.
- On crops with sky at 3% of the frame, the 0.9 core refused 3 of 12;
  at 1%, 10 of 12.

The core is back over 0.8, with a floor of 0.25% of the frame, and the
sure query alone refuses the slope. `what_the_gate_sees` over both sets
(the 41 and the reviewer's 30) gives these margins:

- every frame the gate passes has a sky query at least 99.1% sure
  (4Z4A2978; all but four are 99.6% or more);
- every sky query refused is at most 97.5% sure: DSCF0029, a real sky
  behind blossoms. Next come 5M0A0957 (the slope) at 95.4% and
  DSCF0737 at 94.6%;
- so the bar at 98% sits in a gap of 97.5 to 99.1 on 71 frames. That is
  narrow on DSCF0029's side, and DSCF0029 is a real sky, so that side
  can only cost a miss, never a false sky;
- the smallest core over 0.8 on a frame that passes is 3.3% of the
  frame (DSCF0012), against the 0.25% floor. With the sure query
  required first, the core has not decided any frame in either set.

**The matte.** The matting trial's decision, with its two changes: the
growth passes from `tree_gaps.py`, and the prior's things kept out.

- *Trimap, at the preview.*
  - Known sky is the outline shrunk by 0.5% of the long side, less the
    things.
  - Known not-sky is everything outside the prior's sky and the outline
    grown by 2%, less the tree, flower and unlabeled pixels within 20%
    of the known sky.
  - The things themselves (COCO's first 80 classes), shrunk by 0.5%, are
    known not-sky.
  - Everything else is in question.
- *Local colors.* The local sky and not-sky colors are normalized
  Gaussian blurs of the known regions, made at 512 pixels (σ 2%, or 15%
  where the near weight is thin), and sampled bilinearly.
- *Growth.* In three passes, a pixel in question joins the known sky
  when all of these hold: it is treeish or the prior gives it at least
  0.35; its chromaticity is within 0.04 of the local sky's; it projects
  between 0.95 and 1.3 on the line; and it lies within a quarter of the
  line's length of it.
- *Full size.* At the frame's full size, one output block of 128 pixels
  at a time (a block with nothing in question is a lookup), each pixel
  in question is projected onto the line. Then comes the guided filter:
  radius 8, ε 1e-4, with the guide the frame's own luminance over the
  sky's.

Four guards on a pixel's share were not in the brief. Each was added
after looking at the real frames.

- *Off the line.* A pixel more than 0.25 to 0.5 of the line's length off
  the line takes no share. This was a white facade against the blue sky
  on 5M0A7661.
- *Past the sky.* A pixel projecting beyond 1.3 to 1.8 of the line takes
  no share. This was street lamps as specks on 5M0A7753.
- *Hue.* A pixel as bright as the sky must have the sky's hue: its
  chromaticity within 0.04 to 0.10 of the sky's. This is the unmix
  stage's gate. It took out a pale facade seen through a canopy on
  5M0A7661.
- *The prior's word.* This applies to a pixel whose class sky does not
  show through: mountain, building, sea, a person's edge, a doubtful
  sky. Outside the outline its share is capped at ramp(p; 0.05, 0.35).
  It is floored at ramp(p; 0.7, 0.95), but only where the pixel is
  darker than the local sky (a dark cloud) and not on the region of a
  person's negative pick.
  - The cap: on DSCF0121, the snowy ridge lies on the line between the
    grey sky and the dark water and took a mottled 2% band. Taipei 101's
    glass, reflecting the sky, took half the tower.
  - The floor: dark clouds that SAM left out of the outline became
    holes.
  - Tree, flower and unlabeled pixels are the projection's alone.
- *What SAM and a negative pick left out stays out* (after the review).
  - The trimap's band is the outline grown by 2%. It used to be the
    outline, the prior's sky and the map over a half, grown.
  - Before, with the floor applying anywhere, the reviewer's wall came
    back at full sky. The prior called the wall sky at 0.97, and SAM
    and a negative pick both removed it. It now comes back at 0.00
    either way.
  - A negative pick covers its region: the pixels outside the outline
    of the prior's class at the pick, connected to it, within 20% of
    the long side. There the share outside the outline is nothing and
    there is no floor.

**Linear data.** The matte reads the base develop, which is the frame's
own linear working-space image, not the display-encoded preview. On a
frame whose preview clips the sky to white (DSCF0012, DSCF0016), the
sky's color and brightness are still there. So the line from not-sky to
sky exists, and the "past the sky" and hue guards can tell a lamp or a
white sign from the sky.

On a clipped preview, every clipped pixel matches a clipped sky. The
trial saw white gaps go in whole with hard edges.

Unclipped data changes little in which pixels are found. It matters
most in the darkening: the sky's own values are there to take down.

**Measured.** All of the following comes from
`worker::tests::the_sky_over_real_frames`, which is the product's own
path. Each frame is opened and developed as the worker does, then
`Ai::raster` makes the raster once on the machine's providers (WebGPU)
and once on the CPU alone. Release build, 9950X3D, RTX 5070 Ti, the
user's 41 frames copied under the worktree.

- **The five no-sky frames: zero pixels, on both providers**, before
  and after the review's fixes. The reviewer's 30 frames of its own
  (interiors, a product, a close portrait, a night street, a moon)
  were rerun on the final code: no false sky. 6U3A7205 is now found
  (30.1%). P1000247 (an RW2, taken now that the test takes the
  decoder's own list of raw extensions) is a portrait with no sky and
  gets none.
  - 5M0A0952, 5M0A5135, DSCF0153 and DSCF0186: the model calls nothing
    sky.
  - 5M0A0957: the model is 95.2% sure.
  - The test fails if any of the five gets a single pixel.
- **Counts on the 36 with sky** (by eye on the sheet, and at 100% on
  four frames):
  - 31 found;
  - DSCF0012 partial: the branch gaps against sunset glare, patchy;
  - 4 missed: the two windows, 5M0A7680 (85% sure) and DSCF0195 (73%);
    DSCF0025 (93%); and DSCF0029 (97.7%).
  - Against the trial's EoMT ∩ SAM (31 found, 0 partial, 5 missed),
    DSCF0012 moves from missed to partial. Against DETR's (31 found, 2
    partial, 3 missed), DSCF0029 stays a miss.
- **CPU against WebGPU.**
  - On every frame where sky was found, the rasters differ by at most
    1/255, with a mean under 2.4e-7, and no pixel crosses one half. The
    empty rasters of the nine refused frames are identical.
  - On a made-up frame, the prior's class logits differ by 4.1e-5, the
    mask logits by 7.7e-4, and no label changes.
  - The ignored test asserts under 0.1% of labels changed and a sky map
    within 0.05 at most and 1e-3 on average.
- **Times per stage** (warm, 24 MP and 100 MP frames, previews of
  2048):

  | stage | WebGPU | CPU |
  |---|---|---|
  | preview render (the models' picture) | 0.18 s (24 MP), 0.57–0.78 s (100 MP) | the same |
  | prior (letterbox, model, labels at 1024) | 0.08–0.33 s | 0.18–0.25 s |
  | gate | 0.004 s | 0.004 s |
  | seeds | 0.015 s | 0.015 s |
  | outline: SAM embed + up to 8 decodes + clip | 0.22–0.24 s (embed 0.06–0.08) | 0.85–0.93 s (embed 0.65–0.72) |
  | edge (the matte at full size, averaged to 2048) | 0.12–0.23 s (24 MP), 0.21–0.33 s (100 MP) | the same |
  | raster | 0.01–0.03 s | 0.01–0.03 s |
  | **total, ask to raster** | **0.56–0.82 s (24 MP), 0.94–1.06 s (4Z4A, 45 MP), 1.26–1.61 s (100 MP)** | **1.25–1.46 s (24 MP), 1.84–2.2 s (100 MP)** |

  The totals are the reviewer's rerun, which agrees with mine.

  - The model alone is 0.035 s on WebGPU and 0.124–0.141 s on the CPU
    (the reviewer's figure, on a quiet machine; mine, 0.28–0.42 s, was
    taken while the machine was shared), against the trial's 0.028 s and
    0.145 s.
  - Inside the real path, the WebGPU prior is bimodal: about 0.08 s, or
    0.2 to 0.35 s. The slow mode is no faster than the CPU's 0.18 to
    0.19 s. It was not chased.
  - The same matte from the frame brought down to the preview's size
    takes 0.11–0.13 s. At full size it takes 0.12–0.33 s. So the full
    size costs up to 0.2 s more on a 100 MP frame. The trial's Python
    took 1.3 to 6 s.
  - The one outlier is 5M0A7909 (sky among strings of lanterns): 0.77 s
    at full size, since nearly every block holds pixels in question.
- **In the editor.** The capture was DSCF0151 with a Sky adjustment at
  −1.5 EV, from a hand-written sidecar, run with `--tab Masks
  --show-mask 1 --snapshot` on a headless mutter, with the mask cache
  emptied first.
  - On a first run, from "developed" to the raster was 3.05 s. Of that,
    0.63 s was the Sky model's WebGPU session and 1.23 s SAM's two
    sessions.
  - The status line said "sky found on WebGPU in 2.57 s". The snapshot
    was written 0.35 s later.
  - On the final build the sessions were warmer: model load 0.22 s,
    SAM load 0.33 s.
  - 5M0A0957 showed "no sky found in 5M0A0957.CR3" with nothing
    painted.
  - A second Sky mask in the same session costs the warm figures above.
  - DSCF0153 showed "no sky found in DSCF0153.RAF" after 0.3 s of prior,
    with nothing painted.
- **At 100%**, with the sky taken down 1.5 EV and warmed through the
  matte, in linear light:
  - DSCF0151's hair is clean: no rim, and the sunglass lens is
    untouched.
  - 5M0A4761's bare branches: the gaps darken evenly, with a faint light
    line along the twigs. That is the 2048 raster brought up to 4000,
    not the matte.
  - DSCF0016's defocused tree top is a partial.
    - The edge has a soft light halo where alpha falls before the blur
      does.
    - The pale blossom interior takes patchy 0 to 0.7 alpha.
  - DSCF0012: twigs against sunset glare, patchy.

**Tests.**

- *From the review, one test for each fix.*
  `a_sure_sky_diluted_under_nine_tenths_passes_the_gate`,
  `a_sure_sky_elsewhere_does_not_make_an_unsure_one_sky` (the probe),
  `what_sam_and_a_negative_pick_leave_out_stays_out` (the wall, and the
  wall right under the outline), `a_sam_failure_falls_back_to_the_priors_labels`,
  and `pick_on_a_sky_says_what_it_can_do` with the panel driven to a
  no-sky raster in `a_sky_comes_from_its_button_and_takes_picks`. The
  reviewer's own probe (`target/review/probe`) now prints: A, 7.5%
  labeled sky, none in the block; B, the wall at 0.00 with and without
  the pick.

- *greycard-ai, `sky`, 18 unit tests on synthetic data.* The letterbox
  and its resize; the labeling rule from made-up logits; an unsure query;
  each of the gate's four rules, including a snow-slope prior and a
  lake's reflection away from the edges; the seeds' place and spread;
  the clip; the no-seed fallback; a person's picks; a canopy of thin
  lines over a sky; `find` on a frame with no sky, which never calls
  SAM; and the matte through `find`.
- *greycard-ai, `matte`, 8 tests.* Sky between thin dark lines over a
  gradient sky is found and the lines are not; a thing in the band takes
  no alpha; the matte at the raster's size is the full-size one
  averaged; no known sky gives no matte; the projection; the share
  guards; and a bright ridge and a lamp take no sky.
- *greycard-ai, `tests/sky.rs`, 4 ignored tests.* The reference check;
  CPU against WebGPU; a made-up sky; and `what_the_gate_sees`, the gate
  diagnostic over a folder of previews. They need `GREYCARD_MODELS` and
  pass at once without it, as the other model tests do. The reference
  check also needs `GREYCARD_SKY_REFERENCE`, a directory holding the
  trial's `eomt.in.f32` and `eomt.cpu.f32`. Those are a real frame and
  stay out of the repo.
- *greycard-edit.* A sky round-trips, loads without picks, and turns its
  picks.
- *greycard-ui.* The Sky offer order (prior, SAM, prior alone); the
  button and Pick; the no-sky note and the cache slot following SAM; and
  the ignored `the_sky_over_real_frames`, which needs
  `GREYCARD_SAMPLES` and `GREYCARD_MODELS`.
  - `GREYCARD_SKY_OUT` writes each frame's preview, raster, trimap and
    labels.

**A fix on the way.** `panel::mask::step` used to fall back to the
Subject original whenever the offer sheet was busy and the store held
that file. It did this for any shape, so an Object waiting on SAM would
be asked for with the Subject model. The fallback is now for a Subject
want alone, and a test holds it.

**Left.**

- *The branch and blossom frames.* DSCF0012 (twigs against glare) and
  the blossom frames DSCF0025 and DSCF0029 are known failures that want
  a learned sky matte.
- *Canopies whose sky changes hue.* The hue gate costs gaps deep in a
  canopy whose sky changes hue quickly (a sunset), where the growth
  cannot follow.
- *Mid-tones near the sky.* A mid-grey object among the trees near an
  overcast sky takes about 20% alpha, since it lies on the line. This is
  the sign on 5M0A4761. The pale defocused blossoms of DSCF0016 take
  patchy partial alpha.
- *Raster resolution.* The raster is 2048 wide, so a matte made at 100
  MP is averaged down fourfold and brought up again: the faint light
  line along 5M0A4761's twigs.
  - A 4096-wide raster for Sky would be about 22 MB at 100 MP.
  - A full-size raster is past wgpu's 8192 texture limit.
  - Not a one-line change: the raster width is one constant shared
    with the brushes and the shader.
- *Windows.* Sky seen through a window is refused (73–85% sure). That
  is arguably right.
- *The bimodal WebGPU prior* above: not chased.
- *The gate's margin* on the unsure side is 97.5% (DSCF0029, a real sky
  among blossoms) against the 98% bar. More hard negatives are wanted:
  sky-blue walls, a lake reflecting sky with none above it, a snowfield
  filling the frame.

**The review.** Land after fixes, and the rule held everywhere it was
tried: zero sky pixels on the five no-sky frames on both providers,
and on 30 more frames the reviewer chose from the sample folder
(interiors, a dark product, a close portrait, a lantern-lit street, a
moon on a black sky), every raster byte-identical to the author's.
What it found was the gate's shape, not its rule: the core over 0.9
threw away a pale hazy sky the model was 99.8 percent sure of, and
most small skies with it; the sureness test was per frame while the
labels took every sky query, shown with a synthetic probe; the
matte's floor undid SAM and a negative pick on a wall the prior
called sky; a SAM failure failed the whole shape; and picks did
nothing silently without SAM or on a refused frame. It also measured
the model alone at 0.13 s on the CPU where the author's shared-machine
figure said 0.3, found the WebGPU prior bimodal inside the real path,
ran the export script to the registry's exact bytes, and checked the
license text against the model card. One round fixed all five with a
test each, the reviewer's probes among them.
