# §719. A learned highlight fill: the design

We port the trial's learned highlight fill (§677's benchmark, the
trial's rounds one to ten) into the develop. It is an option on each
edit, off by default.

Citation conventions:
- "Trial" cites the trial branch's scripts as `hl6.py:N`, under
  `scripts/highlight-learn/`.
- Master is cited as `path:line`.
- **A** marks an assumption not yet checked in code or by a run.
- Each rejected alternative keeps one line, with why it was rejected.

## What ships

Checkpoint r6e-s1, run with e_tied's settings.

**Training.** r6e's recipe: `--model struct6 --core-rise --core-shape
log --core-color core --mix-sigma 6`. It is not scale-free.

**Run settings:**
- whitening 0.5;
- core boost 1.4;
- harmonic from bright unclipped pixels;
- depth tie 12 to 48 px;
- no band-only hand-over.

**Untrained settings.** Whitening, boost, depth tie, and harmonic from
bright are look constants set at inference (`infer4.py:63-68`). r6e
was trained with the harmonic from all unclipped pixels. These
constants belong to the model's version.

**Measured:**
- bench: pooled regime-3 dE 0.0631, against opposed's 0.0934;
- blind round nine: mean place 1.62;
- preview drift: 0.13 stop median at 4×, as trained.

**Rejected: r6f, the scale-free retrain, with the band-only
hand-over.** In the next two blind rounds it was ranked last on five
of the six trials with a preference, and last on the quarter-size
previews. The band-only hand-over goes with it.

### Inputs
- `x`: the classical fill, in linear Rec.2020 divided by the ceiling
  (the largest gain), so the clip sits near 1. It is opposed on the
  white-balanced mosaic, then the demosaic (RCD on Bayer, Markesteijn
  on X-Trans), then the matrix. There is no profile map, no CA, and no
  denoise. On the trial branch:
  `crates/greycard-bench/src/highlight/learn.rs:29-41`.
- `codes`: the count of colors clipped in each pixel's CFA cell, 0 to
  3. The cell is 2x2 on Bayer and 3x3 on X-Trans. Counted from the
  mosaic before opposed, at 0.987 times each gain.
- `clips`: each channel's clip level over the ceiling.
- The thumbnail: the whole frame box-averaged to 384 px on its long
  edge, in 5 channels. They are log2 of the mean of `x`, the share of
  pixels with any channel clipped, and the share fully clipped
  (`highlight_data.rs:506-548`, trial branch).
- The box in the frame, and the frame's size.

### Region analysis

It runs at half size, on 2x2 means (`hl6.py:463-486`).

**Masks:** "any clipped" and "fully clipped", each where at least half
of the 2x2 block is.

**Regions:** connected runs of any clipped cell.
- They are **8-connected**: the labelling propagates the largest flat
  index by max-pool (`hl_struct.py:41-55`). Its docstring says
  4-connected; the code is what was trained.
- A region's id is the largest flat index in it.
- Where two regions tie, the ring grows to the larger id
  (`hl_struct.py:58-64`).

**Distances:** chessboard, quantized to 2 half-size steps, and capped
(`hl3.py:45-54`). They are not Euclidean.

**Ring:** the cells within 12 half-size cells outside each region.

**Harmonic continuation:** Jacobi at 1/8 size, a fixed 300 iterations.
It is seeded with the mean of the known cells over the input
(`hl_struct.py:67-79`).

**Gaussian blurs**, at half size:
- σ 3 for depth;
- σ 2 for the color hand-over;
- σ 6 for the gain hand-over, applied on both sides of the core's edge.

**Every constant is in pixels.** r6e runs at S = 1 with `FEAT_FULL`
off (`hl6.py:98-105`, `:230-236`, `:289`), so each constant is the
same number of pixels on every frame. The full table is in the trial
report's section 9. At a 6000-px long edge (24 MP):

| constant | pixels | share of the long edge |
|---|---|---|
| distance cap | 512 | 8.5% |
| rim ring | 24 | 0.4% |
| depth blur | σ 6 | – |
| gain hand-over | σ 12 | – |
| depth tie | 12–48 | 0.2–0.8% |
| harmonic reach | about 140 | 2% |

At 11648 px (100 MP) every share is about half that. The network's
size features (`log_area`, `log_depth`) are absolute pixel counts too.
Training saw full-resolution frames from 24 to 100 MP, so every size
in that range is inside training. But every blind judgment, and so
the choice of the depth tie and the look constants, was made on 24 MP
frames. **Open:** how the tie and the constants look above 24 MP.

### Network (`hl6.py:145-167`)
- **Context encoder** (`hl2.py:81-97`): five 3x3 convolutions over the
  thumbnail, down to a quarter of its size, with 32 channels. It is
  sampled bilinearly at each region's centroid.
- **MLP:** (20 region features + 32) → 128 → 128 → 12, with GELU.
- **Size:** 57 964 parameters, about 230 KB at f32.

### Synthesis (`hl6.py:306-402`)
- Each region gets 12 numbers: peak H, falloff L, β, γ, κ, C, the
  band's gain share, γ_r, κ_r, the core rise H2, and L2.
- They become two half-size fields: a gain in stops, zero at the
  unclipped rim, and an Oklab ab offset.
- Both fields are upsampled bilinearly and applied only to clipped
  pixels (`hl_struct.py:112-121`): the ab is shifted, the luminance
  restored, then multiplied by 2^gain.

## 1. Where in the pipeline

Today's order (`stages.rs:1086-1131`):
- Prepare: the gains, opposed, and the cut at `ceiling·headroom`
  (`develop/mod.rs:726-733`);
- then CA;
- then Demosaic, Learned, or Bin;
- then Denoise and Warp;
- then IntoWorking: the matrix, the map, the look, and the orientation
  (`develop/mod.rs:939-970`).

**The fill runs between `apply_matrix` and the profile's map**
(`develop/mod.rs:947` / `:952`). That is where the trial assumed it.
- `Prepare` computes the cell codes beside opposed and carries them in
  `Prepared`.
- Core's `finish` takes an optional fill step after the matrix.
- The engine passes it through `Stage::IntoWorking`
  (`stages.rs:512`).
- The default profile is the file's own matrices, with no map
  (`greycard-edit/src/camera.rs:53-56`), so most edits give the
  network its training domain.
- Under a DCP, the map treats the invented light as scene light.
- Rejected: a stage after IntoWorking. Under a DCP the network would
  see mapped colors it never trained on.

**The learned denoiser's pair.** The fields are computed once, from the
plain demosaic, and applied to both halves before the Blend.
- Rejected: running the analysis on each half. That doubles the cost
  for offsets that are smooth in the core.
- **A:** the band under the denoised half is unmeasured.

**After the white balance**, as `CLAUDE.md` has it.
- The codes do not depend on the white balance: a photosite clips at
  0.987·gain whatever the gain.
- The picture does, so the fill follows the white balance.

**Headroom (§716).** The trial never trained or scored with headroom
above 1. `x` is read as `x / (ceiling·headroom)`, and a photosite
clips at `gain·headroom·0.987`, so the clips carry no headroom. Run
that way on 18 Canon frames with headroom up to 0.17 stop, the fill
rises with the clip level and shows no new failure.

**The clip level is unchanged.** `Developed::clip_level` stays
`ceiling·0.98` (`develop/mod.rs:971`).
- The filled light lies above it.
- The sharpen's guard, the local contrast's fade, and the auto white
  balance keep treating it as clipped, which it is: invented, not
  measured.
- Rejected: making `clip_level` a mask. It is a wider change for no
  seen gain.

**Highlights and Whites read the frame's range from before the fill.**
Their span hangs from the guide plane's 99th percentile
(`greycard-render/src/finish.rs:571`, `:1422`).
- A sky clipped over 1% of the frame would otherwise lift that top and
  move pixels the fill never touched.
- The fill changes clipped pixels and nothing else.
- Rejected: a range that sees the fill. It moves the whole picture
  when the fill is turned on.

**The schema.** No edit field chooses a highlight mode today
(`develop/mod.rs:123-137`, §677).
- A new optional field names the model version the edit was made with
  (say `"highlight_fill": "hl-1"`). An absent field means off, so
  every sidecar written so far renders unchanged with no migration.
- Skipped when absent, it needs no `VERSION` bump
  (`greycard-edit/src/lib.rs:96`), as `midtones` (`sections.rs:181`)
  did not. **A:** this holds for an enum-valued field.
- The develop key takes a tag naming the version, only for edits with
  the fill on, as `WHITES_SHAPE` (`key.rs:85`) does. Every other
  picture keeps its key, and `DEVELOP_RECIPE` (`key.rs:54`) stays.
- Rejected: a third `HighlightMode` in core. It would make core's
  settings name a model.

## 1a. Corrected by the engine's plan

Reading the engine for step 6 changed four things above:
- **The fill is its own stage, in camera space, before the Warp**
  (a DNG's rectilinear warp, the only one before `finish`). The codes
  are in the unwarped mosaic frame, so the fill must be too. The stage
  forms `x = M·rgb/(ceiling·headroom)`, fills, and writes back through
  `M⁻¹` only where the fields reached. With no warp this equals the
  fill after the matrix to rounding, and `finish` stays untouched.
  Rejected: warping the code map (a per-plane warp has no one map, and
  the network never saw warped shapes), and moving the matrix before
  the warp (every warped picture's bits would move).
- **The edit's field is a string**, not an enum, so a version name a
  build lacks still parses, renders without the fill, and is kept.
  The key's tag is the implementation's version, taken only when the
  build has the named model; the name itself is already in the key
  through the edit.
- **The reduced develops bin by 2**, not 4, so they run look-scaled at
  f = 2, where a binned pixel is exactly one Bayer cell.
- **The range** is carried as a shift: the fill stage measures the
  guide's range before and after, and the guide's tap adds the
  difference back, so Highlights and Whites read the frame as it was.

The masks see the filled base, so the mask cache's print takes the
field.

## 2. Everything in Rust

The procedural part and the network's forward pass are both plain
Rust. The forward is five convolutions, three linears, and GELU, on
the CPU.

**Rejected:**
- the network as ONNX through greycard-ai: a 58 k-parameter net does
  not repay ORT's session setup, provider variance between preview and
  export, the card policy, or its absence on some machines;
- one ONNX graph for everything: connected components, chessboard
  distances, and Jacobi iterations are poor ONNX, and it leaves no CPU
  reference of our own.

**Where each part lives:**
- **greycard-core, `develop::highlight_fill`:** the op, its region
  analysis, the forward pass, and the synthesis. It takes a
  `FillWeights` struct and never names a model. It is the op's CPU
  reference, testable with random weights, and core stays free of
  models and schemas.
- **greycard-ai, `highlight` module:** the weights, built into the
  binary with `include_bytes!` (about 230 KB, f32). Each version is its
  own constant, with its version name, its sha256 checked in a test,
  its look constants, and its license. greycard-ai is where models and
  their licenses live, and the engine already depends on it and on
  core. This module touches no ORT.
- **greycard-engine:** looks up the edit's version in greycard-ai and
  hands the weights to core.

**What building them in means:**
- There is no fetch and no missing-model state.
- A retired version stays in the binary, so an edit keeps the picture
  it was made with.
- A newer version is taken only by an explicit choice.

**Determinism.** The op is deterministic on every machine, with or
without a GPU, and gives the same bits at any thread count. GELU uses
`libm`'s erf; it is within 1.4e-6 of torch's, the gap being torch's
vectorized erf in the negative tail.

**The oracle.** The Python oracle is the trial's inference path alone,
cut down. Synthetic fixtures are made from it and committed beside it.
No raw and no trained weight is a test input.

**The oracle is in** `scripts/highlight-fill-oracle/`, with the stage
spec the port works from (`SPEC.md`) and its fixtures under
`crates/greycard-core/tests/fixtures/highlight_fill/`. It matches the
trial at zero on every stage, and departs from it in three stated
places:
- a pixel is filled only where its luminance is above 1e-6 and finite
  (the trial sent a clipped pixel of zero or negative luminance to
  about a million);
- a clipped pixel that no field reaches stays as it was, to the bit;
- a reduced develop builds its thumbnail from its own reduced input.
- each region's counts are integers and its sums are f64, rounded
  once. The trial's f32 sums drift from about 20,000 cells and are
  0.02 stop out at a 4 M-cell region, a 16 MP blown sky; at 25 M cells
  the region's center, where the context is read, is wrong.

**The fixtures** are small scenes of a few hundred pixels:
- a disk lamp with a colored band;
- a thin specular line;
- two regions touching diagonally (8-connectivity and the ring's tie);
- a region at the edge;
- a broad gradient "sky";
- an X-Trans-cell code pattern;
- a frame with headroom.

**What the fixtures check.** Each carries the inputs and every
intermediate at f32: labels as partitions, the distances, the ring,
the harmonic, the 20 features, the encoder map, the 12 outputs, the
fields, and the picture.
- They use a random-seeded checkpoint.
- Integer stages compare to the bit, float stages to stated
  tolerances.
- One ignored test runs the built-in r6e weights against the oracle's
  output for the same scenes.

## 3. Preview and export

The open frame's base is full size; only the reduced purposes bin
(`stages.rs:1018-1023`). The fill runs at full size in every base
(viewport, prefetch, and export), and the viewport shows that result.
**Preview and export agree by construction.**

**While it computes.**
- The base is delivered first with today's opposed. The filled base
  follows under the same generation, as pending fills do
  (`worker/develop.rs:99`, `base.rs:432-540`, §569).
- A white balance drag remakes the base, so it reruns the fill. Until
  the new fill lands, the last fields are applied to the new base. The
  fields are offsets in stops and Oklab ab, so the clipped areas do not
  flash to opposed and back.
- Export runs the fill inline and waits (`worker/export.rs:185-202`).

**Caching.** The fill joins the in-memory base (`Keep::Base`,
`stages.rs:765-776`). There is no disk cache until timing shows the
fill rivalling the base's own develop.

**The reduced develops, a known limit.** The 2048-px pictures and the
models' input run the fill on their binned base. Because the constants
and size features are in pixels, a 4× bin asks the network about
regions it saw at full size.
- The deepest fifth of a core moves 0.13 stop median, with outliers to
  about −0.4.
- By eye, a lamp goes duller and bokeh more yellow.
- We accept this for the small pictures.
- Rejected: scale-free training (r6f). It cut the drift to 0.10 and the
  outliers, but lost by eye at full size and in previews.
- Rejected: reading full-size fields from disk (as `Fills::Read`,
  `stages.rs:688-698`). It is exact, but it costs a cache, keys, and a
  refusal path for a 2048-px picture.

## 4. Cost and memory

**Trial timings, measured.** PyTorch on the GPU, with the card shared.
This is the e_tied bench run (r6etieds1), per clipped box:

| box | median time | per MP |
|---|---|---|
| 5–30 MP (88 cases) | 2.4 s | 0.15 s |
| 30–60 MP | 8.8 s | 0.21 s |
| 60–200 MP | 23.7 s | 0.28 s |

The cost grows faster than area because the labelling's max-pool
propagation runs until the labels settle, which grows with region
diameter. Union-find removes that.

**The harmonic's cost is linear in area.** r6e's harmonic is a fixed
300 iterations at 1/8 size, about 140 px of diffusion on every frame.
- 24 MP: about 375 k cells, about 1 G operations.
- 100 MP: about 1.5 M cells, about 4.5 G operations.

**Rust on the CPU, estimated.** **A: estimates from operation counts,
unmeasured.**

| step | estimate |
|---|---|
| labels (union-find, O(N)) | about 50 ms at 24 MP |
| chessboard distances | two passes, exact for the quantized form |
| blurs | about 0.3 s at 24 MP |
| harmonic | about 0.1 s at 24 MP, 0.4 s at 100 MP (rayon) |
| network | under 50 ms |

In total: about 0.5–1 s at 24 MP, 1–1.5 s at 45 MP, and 2–4 s at
100 MP. Region analysis dominates; the network never does.

**Measured in Rust, the region analysis alone:** 0.5 s on one thread
at 24 MP and 0.14 s on all; 2.0 s and 0.5 s at 100 MP. The network
adds 0.05 s on four threads.

**Measured in Rust, the whole fill** (analysis, network, synthesis,
and apply; all threads / one thread; peak over the input):

| frame | clipping | time | peak |
|---|---|---|---|
| 24 MP | one small spot | 0.10 s / 0.66 s | 0.15 GB |
| 24 MP | 401 lamps | 0.34 s / 1.9 s | 0.26 GB |
| 24 MP | a sky across the frame | 0.25 s / 1.4 s | 0.18 GB |
| 100 MP | one small spot | 0.35 s / 2.2 s | 0.58 GB |
| 100 MP | 401 lamps | 1.1 s / 7.9 s | 1.1 GB |
| 100 MP | a sky across the frame | 0.85 s / 5.4 s | 0.62 GB |

The synthesis runs window by window, a window being regions within a
blur's reach of each other, so scattered small highlights cost what
they cover. The fields are kept sparse, in tiles.

**Card:** none. The fill never touches the GPU, so the denoiser's card
policy (`greycard-engine/src/ai.rs:1687`, `:2050`; §712) is not
involved.

## 5. Model and versions

**The weights** are about 230 KB, in the binary (section 2).

**Version name.** Each version has a name (`hl-1` for r6e with
e_tied's constants). It is written in the sidecar and in the key's
tag. A retrain or a new look constant is a new name. Old names stay
in the binary while any edit may name them.

**License:** GPL-3.0-or-later, as the denoisers' weights. We state
that they were trained only on the author's own photographs, with no
third-party data, and that no training photograph is published or
recoverable from them.

**Rejected:** the denoiser's practice of a tier name with the model
free to change under it (`sections.rs:456`). A retrain would silently
change old edits.

## 6. UI: wording still open

1. Where the switch sits:
   - a choice in an existing highlights control (beside Highlights and
     Whites in Light);
   - a row in the Detail or Camera section;
   - a Settings default for new edits.
2. Whether it has two states, or is tiered like the denoiser.
3. Whether there is a preference to turn it on for new edits. The
   default stays off.
4. What shows while the fill is pending: nothing new (the log only),
   or the sign pending fills use.
5. How an X-Trans, linear, or monochrome frame shows it: greyed with a
   reason, or hidden.
6. What, if anything, tells an edit that a newer fill version exists,
   and the word for updating it.

## 7. Risks and what we do not know

**Being checked now in the trial:**
- whole-frame fill against the boxed fill it was scored on (the
  harmonic's seed and the edge regions change);
- Canon headroom (§716, which postdates the trial; about 15% of
  former full clips now keep a channel);
- the harmonic's cost at 45 and 100 MP.

**Above 24 MP.**
- The constants are in pixels, so a 100 MP frame's ring, tie, and
  harmonic reach are half the share of a 24 MP frame's.
- This is within training, but never judged by eye.

**Real clipping.**
- Training and the bench are synthetic (lowered white), and the real
  blind frames are few.
- Sensor nonlinearity near saturation and blooming are not modeled.

**Frames we do not offer it on:**
- X-Trans, until it is scored on its own. The trial pools 2x2 blocks
  over 3x3 cells.
- Monochrome and linear DNGs: no pattern, so neither opposed nor the
  codes run (`develop/mod.rs:670`).
- Merges, which keep their own path (`stages.rs:1285`).

**Huge clipped areas (sky).**
- The distance cap (512 px) plateaus a broad sky.
- The chessboard distance leaves faint wedges.
- One region spanning the frame gets one set of numbers.

**Failures seen in the blind rounds.** All are reduced, none gone:
- a grey ring by deep cores (on round nine's 24, "a grey halo still
  but better blended");
- a slow rise in a small core's first pixels, from the σ 12 hand-over
  (06, 09);
- a lifted sheen on thin cores, guarded only by the untrained tie;
- a saturated spot on skin;
- a tint on neutral whites;
- pastel colored light.

**Judged by one eye.** The look constants were chosen by one person's
eye on 24 frames.

**Pipeline stages the trial never saw:**
- the CA correction (the codes come before its shift);
- a DNG's warps;
- the dual demosaic and AMaZE;
- the denoised half.
**A:** they move the band little.

**The codes' frame.** The codes are read in the full-size, unwarped,
unoriented mosaic frame (`develop/clipcodes.rs`), and the fill runs
after the Warp. Step 6 either carries the codes through the warp or
runs the fill's analysis before it; the trial had no warp, so neither
is measured.

**Crops.** A crop must give its frame's fill. A frame cropped at decode
is its own frame, and its pixels match.

## 8. Steps (each one squashed commit with its tests)

1. **A minimal Python oracle, inference-only, with its synthetic
   fixtures**, committed together. It also writes the random-seeded
   checkpoint's intermediates. Small.
2. **Codes in core.** The per-cell clip bits in `Prepared`, tested
   against the bench's `ClipMap` on synthetic mosaics. Small.
3. **Region analysis in core**, against the fixtures:
   - labels (8-connected, max-index ids);
   - the quantized chessboard distance;
   - the ring, the blurs, and the harmonic;
   - the features.
   Medium.
4. **The network's forward in Rust**, `FillWeights`, and greycard-ai's
   built-in r6e weights with their hash test. Small.
5. **Synthesis and apply in core**: the fields, the upsample, and
   applying them to a picture or a pair. End-to-end against the oracle.
   Medium. Needs 3 and 4.
6. **The engine and the edit.** Medium to large. Needs 2 to 5.
   - the edit field and the key tag;
   - the fill inside IntoWorking;
   - the pair;
   - the reduced develop on the binned base;
   - the range read before the fill;
   - export inline.
7. **Progressive delivery**, and the last fields as a stand-in under a
   white balance drag. Medium. Needs 6.
8. **UI**, once the words are chosen. Small to medium.
9. **Measurement on master.** Small.
   - timing at 24, 45, and 100 MP on the CPU;
   - the bench through the engine's own path;
   - real CR3s with headroom;
   - an X-Trans score, which gates offering it there.
10. **Later, if wanted:** a GPU apply in WGSL checked against 5, a
    fields disk cache, and a faster harmonic (a new model version).

Steps 2, 3, and 4 run in parallel. Step 8 starts once 6's shape is
known.
