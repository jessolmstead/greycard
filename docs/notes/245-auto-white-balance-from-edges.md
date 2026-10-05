# 245. Auto white balance from edges (2026-10-04)

The roadmap had "Auto white balance seems to lean way too warm". We
measured before changing anything.

**How we measured.** `cargo run --release -p greycard-core --example
auto_white -- FILE...` develops each file at its as-shot white with the
file's own profile (RCD and no CA correction, for now; the estimate
is made in camera space and hardly sees the demosaic). It reads a grid
512 points across, as `Renderer::sample_grid` reads the GPU's texture,
rounded to half floats the same way, and runs `color::auto_neutral` on
that grid, then prints the as-shot and Auto temperature and tint. The
column "warmer" is the as-shot white's mireds less Auto's, so a positive
number means Auto chose a higher white and the picture comes out warmer
than the camera rendered it. We ran it on the 31 sample frames that
decode (the two HE* NEFs do not). The camera's embedded previews served
as the visual reference: on every daylight frame the as-shot rendering
looks right, so for those frames as shot is the target. Both columns
below come from this example: "before" on master, "after" on this
change.

**It is not one warm bias. It is grey-world reading a frame's area.**
Over all 31 frames the mean was +2.0 mired, which looks like no bias at
all. The mean hid two opposite errors:

- Daylight frames (26): +14.9 mired mean, 25.9 mean absolute. Frames
  where pale sky or sea is 30% or more of the usable picture (10):
  +38.6 mean. Seascape and sky with sea 5733 → 9156 K; two jets on sky
  5689 → 7844 K and 5698 → 7726 K; two lighthouse frames 5268 → 8221 K
  and 8210 K; hazy ridge under blue sky 5899 → 7850 K. All of them come
  out visibly amber. The teal wall (6124 → 9216 K) is the same failure
  with a wall in place of a sky, though too little of it reads as
  sky-blue to fall in that set.
- Tungsten and night frames (5): −65.1 mean. Candles 3127 → 2264 K,
  lanterns 2986 → 2293 K. Grey-world neutralizes a warm light the
  camera chose to keep.

This is how grey-world fails, and the cause is not a slip in the code.
We checked the suspects. The averaging is in linear camera space
before the matrix. Clipped and black pixels are excluded. The Kelvin
conversion is rawcolor's, the same path the Neutral dropper takes. A
sky is bright and big. Under the as-shot white a pale sky sits about
0.3 to 0.5 from grey in the log ratios. The as-shot prior weighs it at
0.2 to 0.4, and its brightness and area are still enough to pull the
first estimate toward blue. The re-weighting then settles on what that
estimate calls grey: the horizon haze and the pale water. When the
light is taken for bluer than it is, the white is set higher and the
picture comes out warm. §194 put the teal wall down as grey-world's
known failure, but the same failure covers every frame dominated by
sky or water, and those are the frames people notice.

**What we changed.** `auto_neutral` is now grey-edge (van de Weijer,
Gevers and Gijsenij, "Edge-based color constancy", 2007; darktable's
color calibration has a detection from image edges built on the same
idea). The hypothesis is that the mean difference between neighboring
surfaces is grey, where grey-world assumes the mean surface is. We
take the grid's usable points back to camera space as before. At each
point whose right and lower neighbors are usable too, we take each
channel's gradient, the length of its two differences, and these
gradients are the samples. The as-shot prior (0.25) and the
re-weighting toward grey (0.1) run on the samples unchanged, and the
means stay magnitude-weighted, Minkowski p = 1 in the paper's terms.
A smooth sky or wall is many pixels but has few and faint edges, so
it stops winning by area. When a picture has fewer than 300 edges
(flat, or a single row) the samples are the usable pixels and the
estimate is grey-world as before. The usable-pixel minimum and its
message are unchanged. `sample_grid` now also returns the grid's
width, which the gradients need. The readback costs no more: only the
rows on the grid come back from the GPU, as before. The work is still
done in camera space against the fixed as-shot prior, so §194's
property holds: a press from any starting white reads the same.

**Before and after**, in mireds warmer than as shot:

| Frame | As shot | Before | After |
|---|---|---|---|
| seascape, sea and sky | 5733 K | 9156 K (+65) | 7806 K (+46) |
| jets on sky | 5689 K | 7844 K (+48) | 5783 K (+3) |
| jets on sky | 5698 K | 7726 K (+46) | 5342 K (−12) |
| lighthouse, sea | 5268 K | 8221 K (+68) | 5298 K (+1) |
| lighthouse, sea | 5268 K | 8210 K (+68) | 5329 K (+2) |
| hazy ridge, blue sky | 5899 K | 7850 K (+42) | 7148 K (+30) |
| teal wall | 6124 K | 9216 K (+55) | 7274 K (+26) |
| sea cliff in sun | 5561 K | 6736 K (+31) | 5193 K (−13) |
| overcast marble | 5408 K | 6163 K (+23) | 5889 K (+15) |
| portrait, warm backdrop | 5356 K | 4069 K (−59) | 4950 K (−15) |
| candles | 3127 K | 2264 K (−122) | 2226 K (−130) |
| lanterns | 2986 K | 2293 K (−101) | 2452 K (−73) |
| amp dial, warm light | 5475 K | 5035 K (−16) | 6220 K (+22) |
| overcast cliffs, brown grass | 6011 K | 5994 K (−1) | 5013 K (−33) |
| street at dusk | 6255 K | 6422 K (+4) | 5131 K (−35) |

The amp dial's tint sits at or near the ±0.05 clamp in both columns
(+0.0445 before, +0.0500 after), so its temperature means little; it
is in the night set below.

| Set | Before mean / mean abs | After mean / mean abs |
|---|---|---|
| all 31 | +2.0 / 32.2 | −7.4 / 19.9 |
| daylight 26 | +14.9 / 25.9 | −0.6 / 13.5 |
| sky or sea ≥ 30% (10) | +38.6 / 39.6 | +6.6 / 10.9 |
| tungsten and night 5 | −65.1 / 65.1 | −42.6 / 53.1 |

The warm lean on daylight frames is gone: −0.6 mired mean against as
shot, half the error per frame. The sky frames go from nearly 40 mired
warm to within 7.

**Two frames got worse.** The overcast cliffs and the dusk street each
move about 35 mired cooler than as shot: brown grass and warm street
light have strong edges of their own color, and grey-edge neutralizes
them. Grey-world kept them only because the grass and the street were
averaged against everything else. The candle frame went slightly
cooler (−122 to −130 mired), and Auto still takes the warmth out of
tungsten light. Whether Auto should
keep some of a warm light's warmth, as Lightroom and cameras'
"ambience priority" modes do, is a separate decision and is not made
here.

**Alternatives we measured and did not take.** Grey-world with each
pixel normalized to its chromaticity reached +32 mired on the sky
frames, still warm. Grey-edge with p = 2 did worse (daylight mean abs
18.5). Grey-edge with each edge normalized to its chromaticity scored
best on these frames (daylight 10.7, night 35.4). On frames that are
nearly all smooth sky, though, every faint gradient counts as much as
an object's edge, so the noise decides the answer. That is a weak
foundation, so we kept the magnitude weighting the paper uses. A blend
of grey-world and grey-edge kept half of grey-world's sky error.
Narrower or wider priors (0.15, 0.4) moved little.

**Tests.** `auto_neutral_reads_the_light_off_edges_not_a_big_sky`: a
frame of textured grey ground under a bright, smooth, pale sky that
fills most of it. On the grid, Auto finds the ground's light within
2%. Read as pixels alone it lands more than 5% off, toward blue, which
is the warm picture the roadmap reported.
`auto_neutral_leaves_out_edges_at_clipped_and_black_pixels`: glints
dotted through the ground with green clipped and red and blue not, and
black noise beside them. Each glint makes a strong edge near grey but
off it, close enough to pass both passes, so the test fails when the
gradients are taken over every point instead of the usable ones. The
older tests now run on a grid (the white-independence and
own-answer tests) or on a single column, the pixel path (ramp, clip,
too-few). The GPU test checks the width `sample_grid` returns.
