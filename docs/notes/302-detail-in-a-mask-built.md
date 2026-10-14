# §302. Detail in a mask, built (2026-10-11)

The design is the section before this one. This is what we built, where
it departs from that design, and what we measured.

## What landed

- `Look` gains `detail: Detail`. A sidecar leaves it out at its default.
  The global look never reads it: `Edit::look` gives the default and
  `set_look` ignores it. `Edit::detail_wants` names the planes the
  enabled, non-empty masks ask for, and `same_develop` compares it, so
  undo, history and presets develop again when the set of planes
  changes. A change of amount does not develop again. History rows read
  "Mask: Texture -50", "Mask: Detail off" and so on, in the global's
  words.
- The engine:
  - `local_contrast::bands` and `local_contrast_with_bands` give the
    texture band and the clarity band (its mid-tone weight in it). They
    come off the same filters the op runs.
  - `clip_fade` is the op's fade, made public.
  - `Reduced::fit(strength)` fits the dehaze model at a strength past
    the slider's end. `Model::haze` gives the haze at full size.
  - `local_clearing` bounds the result as the global's apply does.
- `greycard-ui/src/detail.rs`: `DetailPlanes` is texture, clarity and
  haze as f16 RGBA, 8 bytes a pixel, plus the airlight and the clip.
  `amounts` sums the masks by weight. `apply` is the per-pixel
  arithmetic, and the shader's `detail_at` mirrors it.
- The worker makes the planes on the CPU from the patched picture when a
  mask asks for them. It makes them on a thread of their own (`PlanesJob`)
  while the develop goes on, and takes them up where the develop is
  delivered.
- They are kept in `Base`. The texture and clarity bands are keyed on
  the patched picture. The haze is keyed on the patched picture and the
  global's local contrast it was read after. The rules for reusing and
  uploading them:
  - A develop reuses the kept planes when they were made from the same
    patched picture, hold every plane asked for, and (when the haze is
    asked for) had their haze read after the same global Texture and
    Clarity. A superset is enough, so a slider moved back to zero
    remakes nothing.
  - A move of the global's Texture or Clarity under a mask asking for
    the haze makes the haze again and takes the bands from the kept
    planes (`DetailPlanes::make_with`).
  - The export and the kept pictures use the same rule
    (`KeptPlanes::serves_edit`).
  - A develop that ends without delivering, because it failed or was
    given up, sets the planes' cancel flag as its job is dropped. The
    thread checks the flag between the bands and the haze and sends
    nothing; the dropped result is logged at debug.
  - They travel in `Outcome::Developed` and `Landed` to
    `Renderer::set_detail` (binding 12). That call skips the upload when
    the same kept `Arc` lands again; a re-upload is 360 MB at 45 MP.
- The export geometry-applies and fits the planes with the picture.
- The kept pictures (`edited::make`) average the planes straight from
  the half floats by the picture's own factor (`to_image_reduced`), with
  no full-size f32 copy in between.
- The reduced develop makes the planes at its own size.
- A size mismatch between the planes and the picture leaves the mask's
  Detail off. The finish, the export and the viewport each log it with
  `tracing::warn`, and a debug build stops on a `debug_assert`
  (`detail::fits`).
- The panel: a DETAIL section on the Masks tab after TINT, bound to the
  chosen mask (`local-detail-enabled`, `local-texture`, `local-clarity`,
  `local-dehaze`). It is a look control, so it hides the overlay.
  Moving it calls for a develop only when it asks for a plane the shown
  develop did not come with and no develop is already on its way with
  (`panel::mask::detail_asks`). The planes there are `State::planes_wants`,
  set on each landing. The planes on the way are `State::planes_asked`,
  recorded on the ask. They are cleared on the landing, on a failed
  develop of the current generation, and on a develop taken by culling,
  so a slider moved after any of those asks again.

## Departures from the design

1. **The mask's Detail goes on before the masks' white, not after.**
   A test guards this order: the whole-frame equality also runs under a
   white balance of the mask's own, with the same white on both sides.
   The design put it "after the locals' white balance". We put it first
   in `finish_pixel_with`, on the developed picture in the develop's
   white. In the shader it is on `t`, before `wm`.
   - The dehaze's airlight, and the bound `t >= 1 - px_c / A_c`, are of
     the picture the model was fitted on. Under a mask's own white the
     pixel is in another white, so the bound would read the wrong
     ratios.
   - Texture and Clarity are gains, which commute with the white
     matrix, except for the fade toward the clip. The global reads that
     fade in the develop's white too.
   - So a whole-frame mask with a single op equals the global under a local white as
     well.
   - In the random parity test the after-white order also put a lifted
     black through a mask's white matrix, whose negative terms took a
     channel to zero.
2. **Texture and Clarity go on one after the other, with the fade read
   again before Clarity**, as the global op does. The design already
   says this. The code confirms it: the log is taken once, and Clarity's
   fade reads the pixel after Texture.
3. **Dehaze is put on as what it takes off the transmission**: `u = 1 -
   t` and `px' = (px - A u) / (1 - u)`. The global's form is `(px - A) /
   t + A`. The two are the same arithmetic. The global's form subtracts
   two numbers near the airlight, which loses precision near black under
   a mask's exposure. `local_clearing` keeps `u` and bounds it exactly
   as the apply bounds `t`.
4. **A summed amount is put through a soft threshold**
   (`detail::soft`, mirrored in the shader).
   - The rule: nothing under a thousandth of the slider's travel
     (`NEGLIGIBLE`), a straight ramp from there to twice it, and the
     amount as set from twice it up.
   - Why a threshold at all: where a mask has all but faded out, the GPU
     and the CPU read its weight about a millionth apart. A Dehaze that
     puts haze in lifts a black pixel, unlike everything else in a look,
     and that lift was 10 levels of disagreement on a pure-black pixel
     (parity seed 9, edit 158).
   - Why not a hard cut: a cut at 1e-3 is a step of about
     `A · 1e-3 · 0.8 · haze ≈ 5.8e-4` linear over near black, about 2
     sRGB levels, which can show as a contour along a feather. The ramp
     removes the step.
   - Why the ramp ends at 2e-3: the review suggested
     `sign(k) · max(|k| − N, 0)`. That shifts every amount by N, so a
     whole-frame mask would no longer equal the global at the same
     amount. Ending the ramp at 2e-3 keeps every amount a slider is set
     to as it is.
5. **The planes are made on the CPU in both develop paths, on a thread
   of their own, and always of the patched picture.**
   - The design expected a few milliseconds on the GPU when the GPU runs
     the detail stage. We make them on the CPU and upload them as
     `Rgba16Float`, as the guide plane is uploaded.
   - On the GPU's path the CPU is mostly idle while the device works, so
     the thread hides part of the cost (timings below). A GPU-side maker
     is left for later.
   - The haze is fitted, as the global's dehaze is, on the picture
     after the global's Texture and Clarity when they are on.
     - That picture is made on the planes' thread: a copy of the patched
       picture with the global's local contrast run on it, the bands
       still to make coming off the same filters. It overlaps the GPU's
       work as the bands do.
     - An earlier version fitted the haze on the patched picture alone,
       to save that second local contrast. That broke a case that was
       exact: a global Texture or Clarity with a mask's Dehaze. In
       levels of 65535, before and after that version:

       | Case | Before | That version |
       |---|---|---|
       | Global Texture −0.5 + mask Dehaze 0.6 | 8 | 658 |
       | Global Clarity 0.8 + mask Dehaze 0.6 | 9 | 2640 |
       | All three at their ends | 106 | 17714 |

       The airlight also moved, from (0.70, 0.67, 0.58) to (0.94, 0.82,
       0.57). A global Clarity with a far ridge dehazed under a mask is
       a common edit, so we restored the fit after the local contrast.
     - The model and the airlight could come from the reduced copy the
       worker keeps for the global's dehaze (`b.haze`). The haze plane
       cannot: it is the model's line in each full-size pixel's
       luminance, and on the GPU's path the contrasted full-size picture
       exists only on the device.
   - What it costs: a move of the global's Texture or Clarity while a
     mask asks for the haze makes the haze again. That is a copy of the
     picture (540 MB at 45 MP, freed once the haze is read), one CPU
     local contrast and a refit, on the planes' thread. The bands are
     kept.
     - On the GPU's path that move develops in 0.31 s at 45 MP (0.12 s
       when the planes were kept whole) and 0.18 s at 24 MP (0.06 s).
     - On the CPU's path it takes 1.26 s and 0.62 s (1.03 s and
       0.48 s).
     - A move with no mask asking for the haze remakes nothing.
6. **The viewport reads the planes by texel and interpolates in f32**
   (`planes_at`), as `guide_at` and the rasters do. The source picture
   still goes through the hardware sampler. A plane changes by a stop
   from one pixel to the next, so the sampler's rounding of the weights
   would show. On cubic views the planes go through `sample_cubic`, now
   generalized to take a texture.
7. **The planes are f16 RGBA, 8 bytes a pixel**, not the design's 6. The
   viewport's texture holds four lanes and the CPU keeps the same
   buffer: 190 MB at 24 MP and 360 MB at 45 MP.

## Tests

- **CPU reference, single ops**
  (`detail::tests::a_whole_frame_mask_is_the_global`):
  - Set-up: the global Detail is run on the picture and exported,
    against a whole-frame mask with the same amounts through
    `export::render`. Sharpen off, identity geometry, TIFF 16. The
    picture is a synthetic hazy frame with some pixels toward and past
    the clip.
  - Each case runs twice: with no white of the mask's own, and with a
    warm white (3800 K) on the mask on both sides.
  - Worst difference, in levels of 65535:

    | Case | No mask white | Mask white |
    |---|---|---|
    | Texture +0.7 | 1 | 1 |
    | Texture −0.5 | 1 | 1 |
    | Clarity +0.8 | 2 | 2 |
    | Clarity −0.6 | 2 | 2 |
    | Texture −0.5 with Clarity +0.8 | 2 | 3 |
    | Dehaze +0.6 | 8 | 8 |
    | Dehaze −0.4 | 2 | 2 |

  - The test holds every case to 10. Off by its switch, the mask's
    Detail with planes is the same bytes as without.
  - **"A whole-frame mask equals the global" holds only when the mask
    uses one op, or Texture with Clarity.** It does not hold once a
    mask's Dehaze is combined with its own Texture or Clarity.
- **The design's limit: a mask's Dehaze combined with its own Texture or
  Clarity** (`detail::tests::a_masks_dehaze_with_its_bands_is_near_the_global`).
  - Why: the global's dehaze model is fitted on the picture after its
    Texture and Clarity. The mask's haze plane is made before any
    amount is known, and its bands go on at the finish, so it cannot
    see them.
  - Measured on the same frame, TIFF 16, and held to bounds just over
    these numbers so that a change to the order shows:

    | Case | Worst (of 65535) | Mean (of 65535) | ≈ sRGB levels, worst / mean |
    |---|---|---|---|
    | Texture −0.5 + Dehaze 0.6 | 657 | 46.7 | 2.6 / 0.18 |
    | Clarity 0.8 + Dehaze 0.6 | 2641 | 714 | 10.3 / 2.8 |
    | Texture −1, Clarity −1, Dehaze 1 | 17715 | 5656 | 69 / 22 |

  - The reviewer's numbers on the core dehaze tests' hazy scene at
    768×512, 8-bit sRGB:

    | Case | Mean | Worst |
    |---|---|---|
    | Texture −0.5 + Dehaze 0.6 | 0.09 | 6 |
    | Clarity 0.8 + Dehaze 0.6 | 3.7 | 13 |
    | Texture −1, Clarity −1, Dehaze 1 | 1.6 | 38 |

  - Clarity moves the luminance the haze model reads far more than
    Texture does, which is why its cases are the large ones.
  - A mask's Dehaze over the global's Texture or Clarity does **not**
    have this limit. Its haze is read after the global's local contrast
    (`a_masks_dehaze_over_the_globals_bands_is_the_global`):
    - Global Texture −0.5 + mask Dehaze 0.6: at most 8 levels of 65535.
    - Global Clarity 0.8 + mask Dehaze 0.6: at most 9.
    - All three at their ends: at most 106. The output's sensitivity to
      the stored haze u is (px − A)/(1 − u)², and a Dehaze of one takes
      the transmission to its floor, where that is up to 25 |px − A|,
      so the plane's half-float rounding is magnified that much. Storing
      the transmission at full strength (1 − u) in the lane instead,
      where f16's steps are finer for a large haze, would likely bring
      this to 30 to 50; left open.
  - We accept this: a mask that takes the haze off a far ridge and also
    puts Clarity on it is cleared from the haze as shot. The cost is a
    few levels in ordinary use and about 20 on average with every
    slider at its end.
- **Half weight and two halves**
  (`detail::tests::half_a_mask_is_half_the_amount`):
  - A mask at weight 0.5 equals the global at half the amounts. Measured
    in linear output: 5e-6 for Texture, 2.2e-5 for Clarity, 4.7e-5 for
    Dehaze. The test holds them under 1e-4, about 6.5 levels of 65535,
    in line with the whole-frame bound.
  - Two masks at 0.5 equal one at 1 within 1e-6.
- **Smaller tests:**
  - Planes made again for another global local contrast, with the bands
    taken from the last, equal planes made fresh, and a cancelled job
    gives none (`planes_made_again_keep_their_bands_and_a_cancel_stops_them`).
  - The threshold has no step (`the_threshold_has_no_step`).
  - The planes averaged from the half floats are bit-equal to the picture
    of them averaged as the picture is
    (`the_planes_reduced_are_the_picture_of_them_reduced`).
- **GPU against CPU on lavapipe**:
  - `render::tests::a_masks_detail_on_both_sides` holds to 2 levels.
  - The random parity test draws each local's Detail after every
    earlier draw (part "local detail"). The planes are present in 80% of
    edits, with random bands of ±1.5 and ±1 stops and a haze of 0–0.9.
  - After the soft threshold, the default seed passes, and so do seeds
    1, 2, 3 and 9 (0.83–0.90 levels at most). Before it, seeds 4–7 and
    10 passed at 0.79–0.89.
  - **Seed 8, edit 98: an accepted lavapipe-only corner.** It is 2.53
    levels out against a 2.5 tolerance. The coordinator's run reports it
    passes on NVIDIA. The pixel is near black in the frame's bottom rows,
    (3e-5, 1.8e-5, 6.6e-7).
  - Replaying it with one part left out at a time:

    | Part left out | Worst (levels) |
    |---|---|
    | nothing | 2.53 |
    | local detail | 1.44 |
    | local white | 1.47 |
    | tint | 1.93 |
    | output space | 2.26 |
    | grading | 0.50 |
    | color curves | 0.50 |
    | look table | 0.50 |

  - The CPU's answer is stable at 54.5 under every nudge. What
    amplifies the difference is the near-black Oklab color chain
    (grading, the color curves, the look table), which is §239's
    territory. The mask's Detail, its white and the tint only feed that
    chain a pixel at a slightly different place.
  - Our first reading, that Clarity steepened a mask-weight
    disagreement, was wrong.
  - We leave the corner as it is. The fix belongs to the Oklab chain
    near black, not to the Detail.
- **Reduced-develop check**: the edits drawn there now give the first
  mask a Texture (60%) and a Dehaze (50%), from a generator of their
  own. Run on two sample raws (24 and 45 MP), 12 cases each, it passed
  within its limits: worst mean 0.86, p99 4.27, median of the means at
  2048 is 0.156.
- **Schema**:
  - A look with `detail` reads in an old build as the look without it.
    We checked this against a struct without the field and with no
    `deny_unknown_fields`.
  - A sidecar without the field reads as no detail.
  - The global look never carries one.
- **Panel** (`panel::mask::tests::a_masks_detail_is_its_own_and_a_look_control`):
  - the overlay is hidden;
  - the history rows;
  - the develop is called for only for a missing plane, not a second
    time while one is on its way, and again after that develop fails;
  - the JSON round trip;
  - `show_edit` puts the values back.

## Measured

- **The order against the sharpen.**
  - Method: on the sample raws, a whole-frame mask with Texture -0.5
    (global Detail off) is applied after the sharpen at its default.
    It is compared with the global Texture -0.5 applied before the
    sharpen. Both go through the export's finish to full-size 8-bit
    sRGB with no output sharpen. We report Oklab ΔE ×100 over the
    whole frame.
  - The frames are not chosen for skin. A whole frame of mixed subjects
    is the harder case for the sharpen, since skin has less fine detail
    than foliage.

  | Frame | Size | Mean ΔE | p99 ΔE |
  |---|---|---|---|
  | 1 | 24 MP | 0.047 | 0.46 |
  | 2 | 45 MP | 0.065 | 0.63 |
  | 3 | 45 MP | 0.050 | 0.39 |
  | 4 | 45 MP | 0.095 | 1.02 |
  | 5 | 45 MP | 0.052 | 0.40 |

  At a p99 at or under about 1 ΔE, it is not visible.
- **CPU time to make the planes** (texture, clarity and haze, from the
  developed picture, release build, nice 19, 8 rayon threads, the user
  at the machine):
  - 0.27 s at 24 MP
  - 0.36–0.59 s at 45 MP

  Whole develops under the same load spread too widely to separate this
  from (2.1–4.7 s; with a mask asking, within the spread of without).
  The planes' own time is the number to read.
- **The develop's latency with a mask asking**, from a white-balance
  move, which makes the patched picture again. Release build, best of
  three, `GREYCARD_SAMPLES`:

  | Path | Frame | No mask asking | Mask asking for all three planes | Global Texture moved (planes kept) |
  |---|---|---|---|---|
  | GPU (the machine's own adapter) | 24 MP | 0.90 s | 1.05 s | 0.18 s |
  | GPU (the machine's own adapter) | 45 MP | 1.68 s | 1.96 s | 0.31 s |
  | CPU | 24 MP | 1.36 s | 1.55 s | 0.62 s |
  | CPU | 45 MP | 2.67 s | 2.87 s | 1.26 s |

  - On the GPU's path the thread hides about half of the planes' own
    time, which is 0.27 s at 24 MP and 0.36–0.59 s at 45 MP.
  - In the last column the mask asks for all three planes. The global's
    Texture moving remakes the haze from a contrasted copy and keeps the
    bands.
  - What remains is about 0.16–0.27 s on the GPU's path. Delivering the
    picture first and the planes after would remove it, and is left
    for later.
- **GPU (lavapipe)**, a 2560×1440 draw of the whole frame, read back,
  best of 5:

  | Frame | Planes upload | Draw without | Draw with a mask's three |
  |---|---|---|---|
  | 24 MP | 79 ms | 16.7 ms | 20.3 ms |
  | 45 MP | 123 ms | 16.1 ms | 19.6 ms |

- Commands:
  - The measurement tests are
    `worker::reduced_check::a_masks_detail_measured` and
    `worker::reduced_check::a_masks_detail_on_the_gpu_path_timed` (with
    `GREYCARD_SAMPLES`; `GREYCARD_PLANES_CPU=1` for the CPU's path), and
    `render::tests::a_masks_detail_timed_on_the_gpu`.
  - All three are ignored and run with `--release`.
- **Export memory:**
  - At 45 MP an export with a mask asking holds the planes as a
    three-channel f32 picture (540 MB), its geometry copy (540 MB),
    then its fit. Each copy is freed as the next is made.
  - Sampling the f16 planes directly at the frame's source positions
    would save that memory. On a downsized export, though, it would
    point-sample the texture band where the picture is area-averaged.
    So the planes go through the picture's own filters, and the cost is
    what the picture itself already pays.

## Left open

- A GPU maker for the planes, or delivering the picture before them.
- Storing the transmission rather than the haze in the f16 lane, for
  the precision at the transmission's floor (above).
- ~~A planes job that panics lands the develop without planes and is
  asked again on every slider move; a warning on the failed receive
  would make it visible.~~ Said since 2026-10-13: the receive tells a
  dead sender from the maker's own None and warns on the first.
- The near-black Oklab chain behind seed 8, edit 98 on lavapipe.
- A mask's Dehaze with its own Texture or Clarity reads the haze from
  before them (the design's limit, measured above). Over the global's
  Texture or Clarity it reads the haze from after them, as the global's
  dehaze does.
- The droppers' `pick` ignores a mask's Detail, as the design says.
