# 335. The Landscape masks, as built (2026-10-17)

The build of the landscape trial's recommendation (the trial report of
2026-10-16, 135 frames; its §7 and §8 were the spec): four shapes from
the Sky prior's own scene labels, Water, Foliage, Mountain and
Building, offered by a Landscape button that runs the model once and
names only what it found. With it, the bounded second part the trial
asked for: the Sky gate's core re-measured over the kept queries, and
changed.

### The groups and their gates

`greycard-ai::landscape`. The groups are the trial's table 1, in the
model's ids (detectron2's contiguous COCO panoptic order):

| Group | Classes | Bar | Edge |
|---|---|---|---|
| Water | river 99, sea 103, water-other 113 | 0.85 | color-line matte |
| Foliage | tree-merged 116 | 0.93 | color-line matte |
| Mountain | mountain-merged 124 | 0.93 | labels feathered |
| Building | building-other-merged 129, house 91, roof 101 | 0.90 | labels feathered |

A group is offered when the mean winning class probability over its
labeled pixels is at or above its bar, and its core covers 0.25% of the
frame: the soft map over 0.8, eroded by 1% of the long side. The soft
map is Σ p(group|q)·m_q over the kept queries of the group, over
Σ p(any|q)·m_q over the kept queries, as the trial made it.

One departure from the trial: the core is taken **within the group's
own labeled pixels**. Over the kept queries alone, a pixel no kept
query claims divides one faint mask by others, and the group's share
of that ratio can be anything; a synthetic sliver of roof, the only
kept query on its frame, read a core over the whole frame. On the 34
frames of the test folder this moves no verdict: the trial's core and
ours agree to 0.1 of a point on every frame but five, and those five
(4Z4A2764 39.4% against 45.4, 5M0A3021 22.5 against 23.0, DSCF0599 42.0
against 45.9, 5M0A3976 0.8 against 2.1, an interior refused on its mean
either way) are far from the 0.25% floor or refused by the mean.

### One model run, one labeling

The prior's labeling (`sky::label_at`) now makes every map in its one
pass over the 200 queries: the sky's soft map over all queries (the
seeds, the clip and the matte read it, unchanged), the sky's soft map
over the kept queries (the gate reads it, below), each pixel's winning
query's class probability, and the four groups' soft maps. `Prior::label`
is that pass's prior; `Prior::labeled` the whole of it; and
`landscape::Scene` holds the prior, the four measures, and the groups'
soft maps at the labels' working size (1024 on the long side),
brought to the preview's only when a shape is made.

In the editor, `Ai` keeps one `Scene` a develop where it kept the sky's
prior. The Landscape button, a Sky shape and every Landscape shape read
it, whichever asks first makes it: `the_landscape_over_real_frames`
asserts one model run a frame for the button, every shape it offered,
and a second press, on all 34 frames. A Sky made after the button runs
no model.

Measured on the editor's previews (release, WebGPU, an RTX 5070 Ti,
the machine shared): the model 0.032–0.045 s, the one labeling pass
0.125–0.199 s for the sky and all four groups (the trial's separate
labeling of fifteen groups was 0.34 s median, beside Sky's own).

### The outline and the edge

The outline is the prior's labels of the group, never SAM 2 seeded as
the sky's is (the trial: a point on a canopy gets one tree).

- **Water and Foliage**: the color-line matte (`matte::color_line`) at
  the frame's own resolution, the group standing as the matte's "sky"
  (`Scene::as_sky`): its classes labeled sky, the real sky, tree and
  flower moved to stuff ids nothing treats specially, the things left
  as known not-group. No class is one the group shows through. Where
  the matte has nothing to go on it falls back to the labels feathered.
- **Foliage is then cut by the found sky** (`landscape::cap`): the Sky
  shape with no picks, made as the Sky button makes it (SAM's outline
  when SAM is in the store), kept for the develop, so a Sky and a
  Foliage on the same picture share one sky. The cut is one minus the
  sky's share brought up by a ramp, from 0.2 to 0.55: never more than
  one minus the sky, and nothing where the sky holds a pixel at 0.55
  or more.
- **Mountain and Building**: the labels feathered by the guided filter
  at the Subject rule (`sky::feathered`), never the matte.

**Foliage, as first built, painted open sky.** It took the sky as the
class foliage shows through, after the trial's mapping, so every
labeled-sky pixel within the matte's reach of the foliage was left to
the color line, whose guards were tuned for a sky against a tree and
do not hold the other way round. And the known foliage is the labels
shrunk by only 0.5%, so the sky between sparse branches, which the
labels call tree, was known foliage at full alpha. The review measured
it on the editor's rasters, as the share of the pixels Sky holds over a
half where Foliage is over a quarter: 5M0A8082 20.3% (the gaps between
branches at full alpha, and a 0.6 blob in open sky), DSCF0438 6.3% (a
30–50% gradient across open blue sky with a straight diagonal edge),
5M0A1023 5.4% (a band of cloud up to 116/255 with a hard horizontal
edge, labeled sky beneath it). Our own recall and spill could not see
it: they measure the matte against the labels, and the labels were
wrong the same way.

Two changes, each needed:

- The sky is no longer something Foliage shows through. Labeled sky
  beside foliage is known not-foliage past the matte's 2% growth band,
  and within it held down by Foliage's own soft map. This alone took
  the band of cloud out (5M0A1023 to 0.00%).
- The cut by the found sky takes the gaps between the branches, which
  the labels call tree and only the sky's matte, made from the sky's
  side, tells from the leaves. Plain one minus the sky left Foliage at
  0.3–0.4 over a corner of open blue the Sky shape holds at only 0.6–0.7
  (its outline left the corner out; on 5M0A8082 that was 7.4% of the
  sky); the ramp clears it.

After, on the editor's own path (`the_landscape_over_real_frames`,
release, WebGPU, rasters at 2048), on all eight frames of the 34 where
Sky and Foliage are both offered: Foliage over **0.00%** of the sky on
every one (5M0A8082 from 20.3%, DSCF0438 from 6.3%, 5M0A1023 from
5.4%), and no pixel where the two sum past 1.25. On the 4096 dumps
(`tests/landscape.rs`, which also gives the share uncut): 5M0A8082 from
19.4% uncut to 0.00%, DSCF0438 4.5% to 0.00%, 4Z4A4547 2.7% to 0.00%.
Both tests now fail a frame over 2%.

What the sky cannot tell, Foliage keeps. Against the labeled sky rather
than the Sky shape, Foliage is over a quarter on at most 1.3% of it on
seven of the eight frames, and 7.0% on DSCF0599, whose defocused canopy
lets bright bokeh disks through that the prior labels sky and the Sky
shape (rightly, we think) leaves out; Foliage takes them with the
leaves.

What the cut costs: Foliage now waits on the sky when it is made first,
0.3–1.6 s on these frames in the editor's path (the editor's capture
of the coast: 1.98 s, SAM's load among it), and nothing when a Sky was
made first; a Sky with no picks made after a Foliage reads the same
kept sky and costs nothing either. Its disk slot follows SAM's presence
and the sky's stages (`SKY_PIPELINE`), as the sky's own does.
Recall against the labels falls where the labels were wrong: 5M0A8082
from 94.5% to 78.7%, the gaps the labels called foliage.

**Where Mountain meets a found sky** we did not take one minus Sky's
matte. Sky's matte is the costly part of Sky (SAM's embedding and up to
eight decodes and a full-size matte, 0.4–1.2 s on these frames); a
Mountain at 0.05–0.09 s would wait on it, and on SAM being in the
store. Foliage pays it because without it Foliage is wrong; Mountain's
only cost is the ridge. On 5M0A1023 (peaks against a blue sky) the
review measured the Sky and Mountain rasters' sum dipping to about 52%
over some ten raster pixels at the peak: Mountain's feather ramps over
about 30 pixels, the sky's matte over 10. And the spires the labels'
step under-covers stay out of Mountain and Building both (the logits'
160 grid; a cathedral's pinnacles are thinner than a cell). Both are
follow-ups below.

Edge numbers from `tests/landscape.rs` over the 34 frames' 4096 dumps
(recall: the share of the labels the matte keeps over a half; spill:
what it adds outside them, of the labels' area):

| Group | Frames made | Recall | Spill | Time |
|---|---|---|---|---|
| Water | 10 | 94.1–98.0% | 0.2–1.9% | 0.11–0.19 s (matte) |
| Foliage, cut | 12 | 78.7–99.0% | 0.3–9.0% | 0.13–0.21 s (matte, the sky made apart) |
| Mountain | 5 | 99.4–99.7% | 0.4–0.9% | 0.05–0.08 s (feathered) |
| Building | 9 | 98.0–99.8% | 0.3–1.7% | 0.05–0.08 s (feathered) |

The worst Foliage, 5M0A1023 (83%, 9.5% spill), is larches against
snow, where the matte's line runs between yellow needles and white
snow. In the editor's path, at the frame's full size, the matte took
0.18–1.01 s and the feathering 0.05–0.09 s.

### The button

The Masks tab's Sky row is now Sky and Landscape (a Lucide mountain,
drawn from its published path); Object and People moved to the row
under it.

- Pressed on the chosen mask, it sends `Job::Landscape`; the worker
  answers `Outcome::Landscape` from `Ai::landscape`, the offers largest
  first. Its model is the sky's prior alone (`model_with` names SKY for
  a Landscape shape, whatever SAM's state), offered by the model sheet
  as any shape's model is when it is not in the store. Pressed while a
  download runs or another sheet is up, the press waits and the status
  line says "the landscape is found once the download in hand is
  done"; when a download ends, or fails, the press goes on as the
  model's state then allows (asked, offered, or let go with why).
- What came opens a menu under the button, one line an offer, "Foliage
  33%", "Sky 25%", "Water 17%"; the status line says "3 shapes found on
  WebGPU in 0.70 s". The sky is offered when the sky's own gate passes.
- Nothing found: no menu, and the status line says "no landscape found
  in 5M0A2279.CR3".
- A pick makes that shape in the mask the button was pressed on, joined
  by the mode it was pressed with (a first shape is Add), named for it
  as a first shape is ("Water 1"), and its raster asked for as Sky's
  is ("finding the water"). A pick from the menu of a press since
  spent makes nothing. Escape, another mask, another tab or another
  picture let a waiting press go.
- A Landscape shape on a frame its gate refuses (pasted there, synced
  there) is an empty raster, and says so as an empty sky does: "no
  water found in FILE", from the caches too.
- The warning a sidecar with shapes this build does not know gives
  said they are dropped at the next save. A Landscape of a later group
  and a Part of a later route are written back as they came; the
  warning now counts the two kinds apart.
- `--sheet landscape` presses the button on the mask `--show-mask`
  chooses, and the snapshot waits for the answer, for captures.

Captures, release build, headless weston with the GL renderer
(`target/scratch/shots/` in the worktree, the panel scrolled 250 px
so the button shows):

- `offer-coast.png`, a coast with a lighthouse rock: "Foliage 33%, Sky
  25%, Water 17%", "3 shapes found on WebGPU in 0.89 s". The fog bank
  the labels call mountain is held back (90.4%), as in the trial.
- `offer-cloud.png`, the peak above a sea of cloud: "Sky 55%, Mountain
  20%"; the foliage below is held back (84.0%). Before the gate change
  this frame offered Mountain alone.
- `offer-interior.png`, a portrait against a fur wall: no menu, "no
  landscape found in 5M0A2279.CR3".
- `made-water.png`, `made-foliage.png` (the coast), `made-mountain.png`
  (the peak; the cloud bands across it are left out), `made-building.png`
  (a cathedral front, the person before it left out), and
  `made-sky-cloud.png`, the Sky this frame now has. Each from a
  hand-written sidecar, `--show-mask 1`. In those runs the prior took
  0.20–0.24 s on WebGPU, the matte 0.15–0.32 s and the feathering
  0.04–0.06 s; the Foliage capture, retaken after the cut, made its sky
  first (1.98 s, SAM's load among it).

**CPU against WebGPU.** `the_group_measures_are_the_same_on_the_cpu_and_on_webgpu`
over the 34 previews: the four groups' area, mean and core agree to
1.4e-6, 4.1e-6 and 1.4e-6 at worst, and every gate's verdict is the
same. The nearest means to a bar are the coast's fog-bank Mountain,
0.0264 under 0.93, and 5M0A1023's Foliage, 0.032 over it: a hundred
times the providers' difference and more.

### The schema

`Shape::Landscape { group: LandscapeGroup }`, tag `landscape`, the
group by name: `{"kind":"landscape","group":"water"}`. One kind with a
group, as a Part has its route, not four kinds: a group a later build
adds (Grass, Rock) loads here as `LandscapeGroup::Other`, counts as a
shape this build does not know, and is written back as it came, where
a new kind of its own would be dropped at the next save. A build from
before the kind reads `landscape` as `Shape::Unknown` (`serde(other)`)
and keeps the rest of the mask, as it does any later kind; a test reads
a mask with one through a stand-in of the old enum. `VERSION` stands at
5: no field changed meaning, and the unknown kind is the case the
schema already has.

**No picks.** Sky's picks steer SAM's outline and a negative one holds
in the matte. These shapes have no SAM outline: their outline is the
model's own labels, which a click cannot move, and the trial showed
seeding SAM from a click is the wrong outline for a canopy or a
hillside. A correction is another shape in the mask: a brush
subtracted, or an intersection. Should picks come, `#[serde(default)]
picks` adds them without a version.

### The Sky gate's core, over the kept queries

The trial found a true sky refused: XT5, a peak above a sea of cloud,
54.7% of the frame labeled sky at 0.997, refused for "too little the
model is sure is sky (0.00%)", the soft map over all 200 queries never
reaching 0.8. The leftover queries dilute it, the dilution §175 met at
0.9.

The gate's core now reads `Prior::sky_kept`, the soft map over the
kept queries, within the labeled sky (for the reason the groups' core
is). Every other rule stands, and the sure query (0.98) is still first.
The seeds, the clip and the matte read the map over all the queries, as
before.

Measured with `what_the_gate_sees` over the 41 frames of the sky set
and the 34 of the test folder, before and after:

- **One verdict changes in 75: XT5**, refused to found. Its core over
  the kept queries is 55.9% of the frame, the trial's 56%; within the
  labeled sky, 44.7%.
- **Every negative stays out with the same margin**, since the sure
  query refuses each of them first and is unchanged:
  - the five no-sky frames: 5M0A0952, 5M0A5135, DSCF0153 and DSCF0186
    have no sure sky query; the snow slope, 5M0A0957, is refused at
    95.2%;
  - the real skies refused: DSCF0029 97.7%, DSCF0025 92.9%, 5M0A7680
    85.1%, DSCF0195 72.9%; in the test folder 066A3439 91.0%,
    5M0A5391 93.0%, DSCF0737 94.7%.
  - The gap on the sure query is as §175 left it: refused at most 97.7%,
    passed at least 99.2% (4Z4A2978).
- **The core's own margin is better**: on every frame that passes, the
  core within the labeled sky over the kept queries is at least the old
  one; the smallest, DSCF0012, from 3.28% to 3.58%. Before, as after,
  the core decided no frame of the 75 but XT5.
- Of the trial's 135, XT5 was the only frame its core refused
  (`table.txt`); the 60 frames from the archive were not re-run here.
- Through the product's path, `the_sky_over_real_frames` on the 41
  (release, WebGPU and the CPU): the five no-sky frames get no pixel on
  either provider, the four refused real skies stay refused, and 32
  frames get a sky, as §175 counted them (31 found and one partial).
  The rasters differ between the providers by at most 1/255, no pixel
  across a half.

The Sky raster's cache key moved (`sky-2`), so an empty sky cached for
XT5 is made again.

What XT5's sky looks like now: the open sky above is clean, and the sea
of cloud below the peak is sky as the labels say. But the gate reads
the map over the kept queries while the seeds, the clip and the matte
still read the diluted map over all of them (its peak on this frame is
0.761): no seed, so the labels are the outline. The review found
stair-stepped plateaus in the fog, the labels' step at the logits'
grid, and a row of square holes at 0.7–0.8 alpha in flat cloud, which
are unlabeled pixels on that grid, which the matte treats as a class the
sky shows through and leaves to the projection. We did not change the
matte to read the kept map: the holes are the unlabeled pixels' rule,
not the map, and the map feeds every found sky's floor and cap, so the
32 skies of the sky set would all move. It is not a regression (the
frame had no sky before) and it is the first follow-up below.

### Tests

- *greycard-ai, `landscape`, 10 tests on synthetic logits*: Foliage
  over a canopy against a sky, its gaps foliage uncut and cut by the
  found sky, the branches kept, no open sky, and the two summing to one
  at most; every group
  measured from the one labeling, and the sky's prior the same as Sky's
  own labeling; the offer is what passes, largest first; each group's
  own bar; the soft map over the kept queries (a sure sea the leftover
  queries would dilute keeps its core); no core, no offer; the mean
  and not the best query (the trial's lavender field); the matte's view
  of a group; `find` with each edge, and nothing on a refused group;
  the reasons in words.
- *greycard-ai, `sky`, 2 more*: a sure sky diluted under 0.8 over all
  the queries passes on the kept ones; the kept core is of the labeled
  sky.
- *greycard-ai, ignored*: `tests/landscape.rs`
  `the_landscape_over_real_frames` (offers, measures beside the trial's
  core, each shape made with its recall and spill, frames that must be
  offered nothing, and Foliage's reach into the Sky shape and the
  labeled sky, failing a frame over 2% of the Sky); the groups'
  measures on the CPU against WebGPU; and `what_the_gate_sees` now
  printing the core over the kept queries. Run with `GREYCARD_MODELS`
  set.
- *greycard-edit, 3*: each group round-trips by name; a later group is
  kept and counts as nothing; the kind reads as unknown where it is not
  known.
- *greycard-ui, 11*: a press during a download waits for it and goes on
  when it ends; the button beside Sky and its menu in the Slint
  source; the menu's words and kinds; what is found is offered
  and a pick makes it, with the first shape's Add and a spent menu;
  nothing found; an answer for another picture or none waiting; the
  worker's answer through `deliver`; a press without the model; the
  shape's model is the prior alone and a later group is never asked
  for; the button and its shapes read a kept scene and run no model; a
  slot a group in the disk cache. Ignored, run with `GREYCARD_MODELS`
  and `GREYCARD_SAMPLES`: `the_landscape_over_real_frames`, the
  editor's own path, one model run a frame, Foliage over at most 2% of
  the sky and nowhere both. The People menu's test
  read the first `Menu` of the masks panel as its own; it now reads
  the one after its button.
- Totals: greycard-ai 123 unit tests, greycard-edit 399, greycard-ui
  1287, the rest of the workspace as it was; clippy and fmt clean.
  Ignored, run with `GREYCARD_MODELS` set: both landscape real-frame
  tests, `what_the_gate_sees` over both sets, `the_sky_over_real_frames`
  over the 41, the sky prior on the CPU against WebGPU (on the peak
  above cloud: labels differ on 0.0001% of pixels, the sky map by at
  most 3e-6) and the made-up sky; after the Foliage fix, both landscape
  real-frame tests and the measures on both providers again.

### Left, and follow-ups this build shapes

- **XT5's sky** (a peak above a sea of cloud): stair-stepped plateaus in
  the fog and a row of square holes at 0.7–0.8 alpha in flat cloud, the
  unlabeled pixels at the logits' grid; the seeds, the clip and the
  matte read the diluted map over all the queries (peak 0.761) where
  the gate reads the kept one. Wants the matte's rule for unlabeled
  pixels inside a found sky, measured over all 32 skies.
- **Mountain at a found sky**: one minus Sky's matte on the ridge, as
  Foliage now has, rather than the feathered labels (the sum dips to
  about 52% over ten raster pixels at a peak). It would cost Mountain
  the sky's making, as it costs Foliage.
- **DSCF0438's top edge**: the Sky shape is nothing in the top ten or so
  raster rows above the tree, so the cut leaves Foliage at full over a
  hard-edged block of open blue about 170 by 12 raster pixels. The
  Sky's own miss, which Foliage inherits; the sky's outline at the
  frame's top edge wants a look.
- **Spires and pinnacles** the labels' step under-covers, for Mountain
  and Building both.
- **Grass and Rock** next, as the trial ranked them, each a line in
  `Group` and its bar, and a group the schema already carries by name;
  they want their hard negatives first (a wheat field, bokeh, a
  rockface's plants, a concrete ledge). Ground and Road after.
- **Cloud, Cliff, Hedge, a field crop** have no COCO class; they are
  SAM 3 phrases, after the labels have offered theirs.
- Sand, Snow, Flower, Wall and Bridge stay out, as the trial said.
- **Overlap.** The labels tile the frame, but each matte grows past its
  labels by a few percent; nothing caps one group by its neighbors.
- **More hard negatives for the bars**: fog, snowfields, beaches,
  interiors with plants. Each bar sits in a gap as narrow as Sky's was.
