# §348. The baseline per body (2026-10-17)

§141 put every raw 0.8 stops over the scene before its own exposure,
from two reference frames, and said that if the makers' JPEGs kept
disagreeing by camera the number would be a body's, as DNG's
`BaselineExposure` is. §150 left it open as item 2, §333 and §342
measured the GFX 100S II about a stop bright against Lightroom on a
catalog's exports and gave the import an offset for it, and §342 left
the default develop alone. This section measures the default develop
against each body's own JPEG over the library and moves one body.

## The numbers

`greycard-ui --measure-baseline OUT --baseline-before` over a copy of
the library index (three roots, the archive read-only): per body 32
frames, the frames without an adaptive setting alone where a body has
twenty of them, spread evenly over four ISO bands (to 200, 800, 3200
and over) and within a band over folders and dates as the camera
match spreads a group. Each frame is developed at the default as the
match develops it (per channel, lens corrections on, no look), its
camera's JPEG laid over it by the match's registration, and the
Exposure slider moved by the match's iteration until the median of the
flat, unclipped blocks' lightness sits on the JPEG's. That slider is
the number: negative where the default is brighter than the camera.
288 frames in 2436 seconds, one process, eight threads at nice 19 with
two other agents' builds on the machine; 34 dropped (18 of the Nikon's
for its High Efficiency raws, which this build does not read, the rest
under the match's 0.9 correlation).

A session is a folder, its frames' median. The anchors are the R5
Mark II and the R6 Mark II, the bodies Lightroom's exports put at 0.8
(§141, §143, §342): pooled, +0.49 as baseline plus slider over 64
frames and +0.50 over 18 sessions.

| body | frames (sessions) | by frame: median (25th to 75th) | by session | ISO 200 and under | against the anchors: frame, session |
|---|---|---|---|---|---|
| R5 | 31 (11) | -0.27 (-0.32 to -0.03) | -0.04 | +0.02 | +0.05, +0.26 |
| R5 Mark II | 32 (12) | -0.24 (-0.31 to -0.10) | -0.26 | -0.26 | anchor |
| R6 | 32 (5) | -0.26 (-0.29 to -0.16) | -0.29 | -0.26 | +0.05, +0.01 |
| R6 Mark II | 32 (6) | -0.43 (-0.52 to -0.31) | -0.47 | -0.31 | anchor |
| R6 Mark III | 31 (2) | -0.48 (-0.51 to -0.31) | -0.38 | -0.29 | -0.16, -0.08 |
| GFX 100S II | 32 (6) | -0.50 (-0.56 to -0.24) | -0.58 | -0.56 | **-0.18, -0.28** |
| Z 6 III | 14 (3) | -0.14 (-0.29 to +0.02) | -0.14 | -0.29 | +0.17, +0.16 |
| R8 | 28, 3 fixed | -0.45 (-0.56 to -0.21), adaptive | | | too few fixed |
| R7, S5 II, A7 IV, the drone | 3 to 7 | | | | too few |

**Sessions, not frames.** Frames of one session are one scene in one
light, and they are not independent readings. The R5's sample took an
ISO series of one scene, seventeen frames from ISO 400 to 51200 at
-0.18 to -0.44, so its frame median is -0.27 where its eleven sessions
put it at -0.04; the two anchors' frame medians sit 0.19 apart, their
session medians 0.21 apart on six and twelve sessions. A body moves
only when its frames and its sessions both sit past 0.15 stops of the
anchors on one side, from five sessions or more, and it moves by its
sessions'. The GFX is the one body that does: -0.18 by frame, -0.28
by session, five of its six sessions between -0.47 and -0.69 and the
sixth, thirteen frames from ISO 80 to 2500, at -0.23. The R6 Mark III (-0.16 by frame) has
two sessions; the Z 6 III (+0.17, +0.16) three, and its base-ISO
frames sit with the Canons at -0.29.

**ISO moves the camera's JPEG.** On the R6 Mark II and III the JPEG
darkens against the raw as the ISO rises (base ISO -0.31 and -0.29,
over all bands -0.43 and -0.48); on the Z 6 III it brightens. Spreading
the sample over the bands, as asked, weighs the high ISOs the bands
hold rather than the ones a photographer shoots most; that is one
more reason the verdict reads sessions. A baseline is one number a
body and cannot follow it.

**Under the fitted looks** the slider moves by the look's own
brightness, the median over the same frames: GFX -0.16 (Provia and
Reala Ace), R5 and R5 Mark II -0.08 and -0.09, R6 Mark III -0.08, R6
+0.08, R6 Mark II 0.00. §342 found the refitted GFX tables darkening by
0.15 to 0.3; this is the same thing on the camera's side. The baseline
is set without a look, since the look is a choice the user makes on
top of the default.

**What the other evidence says about the GFX.** Lightroom's own
panorama DNGs in the archive carry Adobe's `BaselineExposure`: +0.36
for the R6 Mark II's, -0.01 for both GFX 100S II merges, 0.37 apart.
§342 put the GFX half a stop over Lightroom in the bright blocks and
0.9 over the whole frame. The camera's JPEG says 0.28 by session. All
three point the same way and the JPEG's is the smallest, so 0.52 is a
move toward Lightroom's GFX and short of it.

## What ships

- `greycard_edit::baseline`: `DEFAULT` 0.8, `BEFORE_BODIES` 0.8,
  `BODIES` with the GFX 100S II at 0.52, `for_body` and
  `for_make_model`. The table is in the edit crate because the edit's
  numbers are now from it and the import needs it; the finish's
  `BASELINE_EXPOSURE` is its default.
- The finish: `Source::Scene` carries the body's baseline (in
  hundredths, so a source still compares whole), made from the raw's
  make and model where the develop opens it (the worker's base, the
  reduced develop, the open frame's source). Everything finished from a
  develop reads it there: the viewport's uniform, the export, `pick`,
  the camera match and the Lightroom measurement. `fit_lightroom`'s
  emulation reads the body from the record.
- **The display white follows the baseline.** §145 put the curve's
  white where the sensor's clip lands at 0.8, so a channel the raw
  clipped is white. Left there, a GFX frame at 0.52 put its clip 0.28
  stops short of it: the blown cores still reached 255 (they are past
  the clip), but a grey rim of 249 to 255 ringed every blown area, and
  a channel at the clip itself no longer reached white; a review export
  kept 88 to 90 percent of master's white pixels white, the rest at a
  median of about 253 (251 under AgX). `finish::White` is the curve's
  white for the stops a frame renders at, `2.47 + b` over mid grey with
  the shoulder's gain `1 / aces(0.18 * 2^(2.47 + b))`, and AgX with its
  white relative exposure bisected to it, each made once a value and
  kept; the shader takes the three and AgX's five fitted numbers as
  uniforms, where they were literals. At 0.8, a body at the default and
  an older edit that keeps 0.8, it is the old constants to the bit, and
  a test renders both ways and compares bits. The same two GFX frames
  exported at the default and pinned at 0.8 (master's picture): of
  199,729 and 59,728 white pixels under per channel, 199,723 and 59,719
  are white at the default, the other fifteen within a level; under
  AgX 199,720 of 199,723 and 59,758 of 59,765. With the white nearer
  grey the Whites slider's pole is nearer too: at a baseline `b` the
  slider's top is `0.47 + b` stops short of it, 0.99 at 0.52 (1.27 at
  the default).
- **Edits written before keep their brightness.** The number in a
  sidecar is stops from the default, and the default is what moved, so
  without more a GFX edit would come up 0.28 stops darker. Schema 6:
  an edit written before it that develops anything keeps
  `light.baseline` 0.8, the zero it was made against, on every state
  (current, history, snapshots, versions); one that develops nothing
  (a frame only rated, a seeded learned blend) is the default and takes
  its body's, as a frame with no sidecar does. A build from before
  refuses a schema 6 sidecar rather than drop the field and move the
  picture. The panel's sliders never touch the field and the Light
  section's switch keeps it. So an older GFX edit with every slider
  brought back to zero still renders at 0.8, and the panel shows
  nothing of it; only Reset, or a preset or a paste that lays the Light
  section, takes it to the body's.
- **Between frames the zero goes with the exposure; in a file it does
  not.** A paste and a sync (`Preset::between_frames`, under the
  clipboard and `sync_onto`) carry the source's `light.baseline` when
  the Light section or its Exposure part goes, so an older GFX edit at
  +0.3 synced onto another GFX frame lands where the source sits and not
  0.28 stops under it. A sync that shifts the exposure rather than
  setting it moves the target's own exposure, so the target keeps its
  own zero. A preset saved to a file, and one read from a
  file, carries none, so a preset that sets the exposure to zero lands
  on the body's default. A new edit has none to carry: its exposure
  pasted onto an R5 frame counts from the R5's default. The Light
  section's "applied" test compares without the field, so a preset
  whose sliders match an older edit reads as applied.
- The import: `BODY_OFFSETS` keeps the measured -0.90 for the GFX,
  measured against 0.8, and `body_offset` takes off the change of the
  body's baseline, so the GFX's import offset is -0.62 and an imported
  frame renders at the same stops over the scene (0.8 - 0.90 and
  0.52 - 0.62 are one number); above about 160/255 the GFX renders up
  to 4.5 levels brighter, which placing white per body implies, and
  that nudges its bright blocks slightly further from Lightroom. The R5
  Mark II stays at 0. The GFX's 0.52 is
  one number in `baseline::BODIES`; the import, the finish and the
  tests follow it.
- `DEVELOP_RECIPE` 3 to 4: the baseline is a default the edit does not
  store, so every kept picture of an edited frame is made again once.
  A frame nobody edited shows the camera's preview and is not touched.
- `--measure-baseline` with `--baseline-frames`, `--baseline-body`,
  `--baseline-before` and a folder as the path; `report.md`,
  `report.json`, `frames.jsonl`. A frame is its file name and a folder a
  number. `camera_match::pairs_under` takes an edit of the caller's.

**Before and after, from the measurement.** Over the library's 32 GFX
frames at 0.52 (`--measure-baseline OUT --baseline-body GFX`): the
slider by session -0.58 to -0.30, by frame -0.50 to -0.22, the
anchors' -0.30; the default develop's own lightness against the JPEG
at the median -0.65 to -0.28 stops. The captured frame (a test-folder
GFX portrait at ISO 80): slider -0.63 to -0.35, the default's lightness
against its JPEG -0.96 to -0.54. With the display white placed from
the baseline the medians read the same to the hundredth (the blocks the
match keeps are flat and unclipped, under the shoulder). Every other
body's default develop is the one it was.

`fit_lightroom`'s test of the body's offset holds the matched error
under 0.005 where it was 0.003: the match lands the blocks' median, and
under Highlights at -30 the slider's stop is not a stop at every block,
so at the GFX's offset since its baseline moved it stops a few
hundredths of a stop short and leaves about 0.0034 of ΔE.

**Tests.** The table and its fallback; the migration keeping 0.8 on an
older edit that develops something (with no light key at all, too),
not on one that develops nothing or a picture's, on every state of an
older sidecar, and not over one already there; the panel's light and
the switch keeping it; presets carrying none, a resetting preset
landing on the body's, a paste (through `Clipboard::applied`) and a
sync (through `sync_onto`) carrying it with the Light section and with
the Exposure part alone, a saved preset not, and the section's
"applied" ignoring it; the
import landing a GFX frame at Lightroom's zero at 0.8 - 0.90 stops
and every measured body's offset plus baseline equal to before; the
finish (the GFX's default is the default body's at the difference on
the slider under the shoulder, an older edit renders as before, a
picture takes none; a GFX channel at the clip is white at the default
under both curves where the default's white left it short; a body at
0.8 and an older edit at 0.8 render to the bit as the constants do);
the GPU's per-channel and AgX curves against the CPU's at 0.8 and at
the GFX's baseline (lavapipe and RADV);
the worker's base, the reduced develop and the headless export of an
unedited frame, each from a linear DNG of our own writing named for the
GFX against one named for another body (the export compared under the
shoulder); an older sidecar beside each
exporting the same pixels; the measurement's sample, quartiles and
verdict.

## Open

- **Which anchor.** The pooled sessions weight the R5 Mark II's twelve
  against the R6 Mark II's six. Against the R6 Mark II alone the GFX
  would not move (-0.07 by frame, -0.11 by session); against the R5
  Mark II alone it is -0.26 and -0.32; with the two bodies weighted
  equally -0.16 and -0.21, a baseline of 0.59. The anchors sit 0.21
  apart by session. Anchoring every body to its camera's JPEG instead
  (each body's sessions on the JPEG) would give the R5 0.76, the R5
  Mark II 0.54, the R6 0.51, the R6 Mark II 0.33, the GFX 0.22 and the
  Z 6 III 0.66. The choice is the user's; 0.52 stands until it is made.
- The R6 Mark III and the Z 6 III sit past the line by frame and want
  sessions: more folders of each, and the Z 6 III's High Efficiency
  raws once the reader lands.
- The anchors themselves read 0.21 apart by session (R6 Mark II the
  brighter), which §143's wedding and interior frames (+0.17, +0.26
  over Lightroom) also say. Whether the R6 Mark II's 0.8 is its right
  number is Lightroom's to say, from history pairs of that body.
- `BaselineExposure` from an Adobe DNG of each body, the R5 Mark II's
  above all, to put the JPEG's yardstick and Adobe's on one scale.
- The looks' own brightness, the GFX's 0.16 most: a fit that leaves the
  median where the exposure match put it.
- Whether the Lightroom import's offset belongs on unedited frames too
  (§342's open question) is unchanged; with this baseline the gap
  between an imported GFX frame and an untouched neighbor is 0.62
  stops, not 0.9.
