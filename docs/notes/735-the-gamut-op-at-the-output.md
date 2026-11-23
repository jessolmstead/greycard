# §735. The gamut op at the output: process 2 (2026-11-22)

A color the output cannot hold was clipped per channel, which turns its
hue and flattens a bright light's shape. The finish now brings such a
color into the output at its Oklab hue and lightness. It is process 2
(§734) and applies to every edit.

**What it replaces.**

- Went: the clamp per channel at the output, at the look table's entry
  (the table reads the color at its cube), under zero after a camera
  profile's tone curve, and the output sharpen's hold at zero.
- Stays: the clamp to the encodable range after the op, which takes
  rounding under zero (3e-8 at most) and up to 0.012 over one; the
  clamp into [0, 1] before the point curves, which is the curves'
  index; a picture already rendered (a JPEG or TIFF) clipped into
  [0, 1] in the display curve's place; and every clamp under AgX's
  rail (below).

**The op** (`greycard_render::gamut`, the shader's `gamut_unclamped`).
Three parts, each the identity under its threshold, then the output's
matrix and the final clamp:

- **The rail:** each channel's rise over the color's luminance, as a
  share of the headroom to white, rolled from a knee at 0.9 to white
  at 2.4, in sRGB's primaries for every output, so every export shows
  the same lights. A channel under zero takes the least scale any
  channel got, so lightness does not turn back. The luminance is held
  a ten-thousandth under one.
- **The bound:** a color past 1.1 of the output's edge has its reach
  rolled toward 1.2, its channels under zero moved toward a soft clamp
  by one ratio: toward the output's nearest color.
- **The roll:** chroma at held Oklab hue and lightness, the identity to
  0.85 of the edge, to the edge at 1.6. ACES's curve (Reference Gamut
  Compression), power 1.2; the reach along the Oklab ray, as Ottosson's
  gamut clipping takes it. Written from the published math.
- **The early out:** a color whose smallest channel keeps 0.15 of its
  brightest, under every knee, goes straight to the output, to the bit
  of the full path.
- **Thresholds:** a scene takes the above. A picture already rendered
  (a JPEG or TIFF) takes thresholds of one in its own output's
  primaries: nothing inside the output moves, and what an edit pushes
  out comes to the edge at its hue.

**The edge.** Along the ray each output channel is a cubic in the share
of chroma. Any crossing of a face chosen by a threshold slides by a
graze's length where the ray runs along a face, which drew one-pixel
seams by sRGB's blue corner. So the edge has no threshold:

- `chord`: where the straight line from grey through the color leaves
  the lower faces.
- `depth(e)`, a share of the brightest channel at the chord: past the
  chord, the deepest any channel has been under zero since; short of
  it, the least any one channel has been under zero all the way. Held
  rising.
- The edge is where the depth meets a spring to the chord,
  `SPRING (e - chord)`: one root, which moves with the color at a
  bounded slope. 20 halvings; within 2.9e-5 of the definition worked
  in f64. An exit behind an inside color does not count, so nothing
  inside moves under thresholds of one.
- `SPRING` = 0.1. Against the clamp's steepest change over three
  pixels, per family of ramps: at most 1.57 times at the test's
  sampling (1.41 P3, 1.50 Adobe RGB and Rec.2020 under thresholds of
  one; 1.00 or under under a scene's), and about 2.9 converged under
  thresholds of one at sRGB's blue corner (measured on an earlier build). Rec.2020 under a
  scene's thresholds is 2.67, the rail on a bright cyan, which the
  clamp into the working space does not have.
- The cost of the bound: a color is brought to the edge within 2.1
  degrees of its hue (the roll within 1.0, the working cube within
  3.0), most at sRGB's blue corner. Stiffer is flatter and costs more
  hue (1.4 times and 3.4 degrees at 0.25; 2.4 times and 1.5 degrees at
  0.05); the edge at the chord itself is the stiff limit.
- What the spring leaves past a face is taken in along the straight
  line to grey, not by the final clamp's turn of hue.

**The floor** (before the per channel curve, the scene's): each channel
taken toward the others by 0.0166, eased in from a stop under white to
two over, so a far-out light whitens. A neutral is the curve's to the
bit; blue's share goes 0.9 to red and 1.1 to green, which keeps a pure
blue's hue on its way to white. Every light ramp is white within 9.1
stops of mid grey (green at 9.08).

**The look table** reads the color brought to its cube with no knee and
adds its change to the color as it came, `c + s (T(g) - g)`: inside the
table's gamut the old arithmetic, to rounding; outside, the color keeps
what the table cannot see, and the op brings it in once.

**AgX.** AgX's rail takes only what is under zero, so a table read at
its cube would leave its over-one to the final clamp (125 levels in
sRGB under a strong fade). Under AgX's rail (a scene under AgX, no
camera profile's tone curve) the finish is process 1's: the floor, the
look table's form, and the op do not apply; the table clips at its
entry and the output sharpen holds channels under zero at zero (from
1% of its amount), as before. AgX under a profile's tone curve, and a
picture already rendered, were never AgX's: they change with every
other picture.

**Every edit, no field.** By the owner's ruling before 1.0 (§734's
Process), a look change applies to every edit, old and new: no edit
records which finish it takes, and nothing renders by `Edit.process`.

**Process 2 and the keys.**

- `PROCESS` and `key::PROCESS_KEYED` are 2. States read keep their
  process (1); states the user makes take 2.
- Re-keyed: every edit's kept pictures (grid, strip, lightbox, full and
  reduced), by the engine's `EDITED_RECIPE` 0x8005 and `REDUCED_RECIPE`
  0x8006 (0x8001 to 0x8004 retired). `RECIPE_FOR_PROCESS` holds them
  to `PROCESS`.
- Not re-keyed: `DEVELOP_RECIPE`, which also names the masks' rasters,
  whose print leaves the finish out (the models' preview, made through
  the finish, is the case of a change of global look); the highlight
  fills (the develop's); the camera's previews (`THUMB_RECIPE`).
- Look tables: a fit develops through the op (`FIT_VERSION` 3). A
  fit-2 table read at its cube moves pictures by nothing measurable
  inside it (four tables, 1.8e-7 at most) (measured on an earlier build), so `STALE_FIT`
  stays 2 and it is not asked for a refit; a refit replaces it.

**The defaults under any process.** Once new states are at 2, an
untouched state made at 1 differs from the default in its process
alone. Every "is this the default" check (the migration's untouched
edit, a 9 line's root, the edited mark, the index, Reset to Original,
the history's Original row, the Lightroom reader) now uses
`Edit::develops_as`: equal but for the process. A state at the defaults
is no develop under any process. `PartialEq` still sees the process, so
a step compares it.

**Schema 10's golden ids** stay at process 1: the golden edits are
pinned there, as the fixture was written, so a state made at 1 keeps
its text and id in every later build; one made at 2 differs in the
process alone, and its id moves.

**The render gate** (§733) at process 2, before the bless: 53 of 59
fixtures moved. Every moved sample is in a saturated or bright pixel of
the kept picture (spread over the brightest of 0.35 or more, or a
channel at 235); none in a plain one. AgX, black and white, chromatic
aberration, the version 1 denoise, noise, and the patches did not move.

| fixtures | max | mean |
|---|---|---|
| most | 75 to 89 | 3.9 to 5.2 |
| mixer, tint, mask by color | 50 to 52 | 2.5 to 3.6 |
| smaller | 61 | 2.94 |
| mask, linear | 39 | 1.45 |
| highlights, and the two fills | 18 to 19 | 0.008 to 0.011 |

Blessed for process 2; the gate then passes.

**The gates it misses**, as numbers:

- Hue drift to a reach of 1.2 inside sRGB's top: 1.07 degrees sRGB,
  1.16 P3, against 0.1. The bound moves a color toward the output's
  nearest color by design.
- sRGB against P3 on the exposure ramps: the neon pink 1.60 degrees
  against 0.5 (P3 holds its far part, sRGB bounds it); LED blue 7.61
  and green 6.04, not gated.
- The largest move inside sRGB under a scene's thresholds: 2.32 Oklab
  ΔE x100 (a near-pure blue), against the design's 1.9.
- Hue reversals on kept frames' blue stage lights: 0.382 per 100 path
  pixels over 87 frames in sRGB, the clamp's 0.350 (measured on an earlier build); the rise
  is four frames of one stage's blue tubes.
- Cost: 3.3 to 3.9 times the clamp's finish on saturated frames (45 MP
  saturated: 1.67 s against 0.51 s; with a look table 2.41 against
  0.68), 1.35 on a natural one; AgX unchanged (measured on an earlier build).
- The camera match's developer runners (`fit_lightroom`,
  `measure_baseline`) keep constants fitted under the clamp; the op
  moves 27% of random scene colors by half a level or more, 12% by 3
  or more, so a refit may move them (measured on an earlier build).

**Tests.** `gamut/tests.rs`: `the_edge_is_its_definition`,
`no_seam_by_any_corner`, `the_blue_light_gradient_exports_without_a_seam`,
`the_op_is_no_steeper_than_the_clamp`, `the_final_clamp_moves_little`,
`the_rail_puts_every_output_under_white`,
`the_rail_leaves_a_color_under_its_knee`,
`a_color_under_the_knees_comes_back_to_the_bit`,
`the_roll_holds_lightness_and_hue`, `thresholds_of_one_move_nothing_inside`,
`a_display_pictures_colors_inside_hold_to_a_sixteen_bit_level`,
`the_largest_move_inside_srgb`,
`thresholds_of_one_bring_a_color_to_the_edge_at_its_hue`,
`the_floor_leaves_a_neutral_and_the_mid_tones`,
`the_look_table_inside_its_gamut_is_todays`,
`into_working_takes_the_lower_faces_alone`,
`the_chroma_ramps_hold_round_fours_numbers`,
`the_exposure_ramps_hold_round_fours_numbers`,
`srgb_and_p3_agree_under_the_top`, `the_floor_takes_every_ramp_to_white`,
`a_color_past_every_number_is_the_clamps`. The finish:
`an_agx_picture_with_a_look_is_what_it_was`,
`an_agx_picture_with_an_output_sharpen_is_what_it_was`. The edit:
`process_written_is_the_states_own`. The GPU twin:
`the_gamut_finish_on_the_gpu_is_the_cpus` (every display curve),
`the_gamut_op_on_the_gpu_is_the_cpus_on_any_color` (under one level;
the bound's onset moved by 0.05 shows by 6 to 8),
`the_output_sharpen_on_the_gpu_is_the_cpus` and
`a_masks_output_sharpen_on_the_gpu_is_the_cpus` (AgX cases).

**Where measured.** The CPU numbers above, at 27db4bd8. GPU against
CPU on lavapipe at 0.50 levels worst, the readback's floor; the whole
viewport suite on the RTX 5070 Ti at 27db4bd8 (126) and the render
crate (222). Costs in a release build under other load, on an
earlier build of the same op (measured on an earlier build).
