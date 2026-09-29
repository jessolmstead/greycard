# 150. The feel: where it stands, and how to pick it up (2026-09-22)

§141 to §149 were one afternoon's work against Lightroom, and v0.2.0
starts from here. This section is the state of it and the way back in.

**The reference set** is `E:\Photos\greycard_comparison` on the Windows
machine, not in the repo: nine raws and Lightroom's exports under
`lightroom_export/<stem>_<variant>.jpg`, one slider at its limit per
export, every other setting at Lightroom's default (Adobe Color, as
shot), lens corrections on.

| stem | body | frame | Lightroom exports |
|---|---|---|---|
| 5M0A0021 | R6 II | white watch dial, low contrast | base, exposure +0.8, highlights -100, shadows +100 |
| DSCF0195 | GFX 100S II | ferry interior, window | base, highlights -100, shadows +100 |
| 4Z4A2623 | R5 II | sunset over a city, 9% clipped | base, highlights -100, whites ±100 |
| 4Z4A3521 | R5 II | lighthouse, under-exposed | all at +1 EV: base, highlights -100, shadows +100 |
| 5M0A6341 | R6 II | wedding, white dress, dark suit | base, highlights -100, shadows +100 |
| IMG_3509 | R8 | backlit portrait, mountain | base, highlights -100, shadows +100 |
| 5M0A4134 | R6 II | dim interior under lamps, ISO 800 | base, shadows +100, blacks ±100 |
| 4Z4A2465, 4Z4A2520, 5M0A3202, DSCF0011 | R5 II, R5 II, R6 II, GFX | forest, torii, pagoda, flowers | none yet: bases wanted |

The variant names are the files' own (`highlights_down`,
`plusone_shadows_up`, `highlights_pulled_down` on the watch,
`shadows_pushed` on the ferry, `base_plusone` on the lighthouse).
Lightroom's XMP could not be saved from the catalog, so the values are
as given above, not read from a file.

**The tools** are `tools/reference/`. `render.sh` exports a variant
through the editor with a sidecar it writes on a copy of the raw, so no
hand is on a slider and the set's own sidecars are never touched;
`measure.py base` prints each frame's brightness against the camera's
JPEG and Lightroom's, `measure.py sliders` the median change in stops
by percentile band of each editor's own base with a side-by-side
sheet, `measure.py view` the viewport's shader against the export.
Bands of each editor's own picture because the two editors' lens
geometry differs (Lightroom corrects distortion lensfun has as zero,
§142) and a frame's pixels do not line up. A release build: a debug one
takes minutes a frame, the release twelve seconds.

**What the Lightroom side taught about itself.** Check every export's
size against the raw's before measuring: a crop or a mask left from an
earlier edit is invisible in the numbers until it is not (the wedding
came in 4:3 the first time, the lighthouse's variants a stop over its
base). The camera's JPEG is a poor target: it scatters ±0.8 stops from
Lightroom by frame, the R8's a stop over both editors, which reads as
its scene-adaptive rendering. Lightroom's highlights and whites adapt
to the frame's range; its shadows and blacks behave as fixed curves.

**Where each control stands**, greycard at its limit against
Lightroom's ±100, measured on the set:

- Brightness: the Canons within about a tenth of a stop of Lightroom at
  the median (§141, §143); the GFX half a stop over.
- The top: what the raw clipped is white (§145); a top that is not the
  clip sits 0.1 to 0.2 stops under Lightroom's, which is the shape of
  Adobe's curve between grey and white.
- Highlights: within a tenth or two of a stop on four frames of six
  from the 55th percentile up (§146); nothing under the median where
  Lightroom takes -0.1 to -0.2; two thirds of Lightroom's on the watch.
- Shadows: within about 0.2 stops on three frames; over-lifts the
  mid-tones of the low-key interior (+3 against +1.3 to +2.3) and gives
  the darkest tenth less (+2.3 against +4.0) (§143).
- Whites: aimed right (§149); moves only the top thirty percent where
  Lightroom's moves the 25th to 85th percentiles, one frame measured.
- Blacks: the crush band for band; the lift within about 0.3 stops from
  the 25th percentile up (§144).
- Exposure: the same response as Lightroom's once the start is the
  same.

**Open, in the order they want deciding.**

1. Fixed stops or the frame's range for highlights and whites (§146).
   Fixed for now; ask testers. The frame's range is what would give the
   watch its full pull, reach under the median and bring a clipped sky
   under white as Lightroom does. It would read the guide plane's
   percentiles, which the tone equalizer already makes.
2. The baseline per camera. Adobe's `BaselineExposure` is what
   Lightroom uses per body; one DNG exported from Lightroom per body
   (R6 II, R5 II, R8, GFX 100S II) gives the four numbers. Until then
   0.8 for everything, right for the Canons.
3. Whites' shape, with two more frames of whites exports; and whether
   shadows should give the deepest tones more. Both wait on 1.
4. The curve between grey and white, if the tops still sit short once
   the rest is settled: a steeper upper mid-tone, which is a change to
   every picture's look.
5. The watch's `exposure_pluspoint8` export and §141's +1.15 are before
   the baseline; re-measure against it rather than read them.

**Re-measuring after a change.** Build the release, render every
variant with `render.sh` into one work folder, and run `measure.py base`
on all nine and `measure.py sliders` on the five slider frames; compare
against the numbers in §141 to §149, and run `measure.py view` on one
screenshot for any change to the shader.
