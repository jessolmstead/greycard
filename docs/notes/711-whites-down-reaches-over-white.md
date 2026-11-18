# §711. Whites down reaches over white (2026-11-17)

Issue 132: Whites down greyed the picture without bringing back the
lights over white. It now does both: the white point moves as it did,
and a roll brings what is still over white under it. "The earlier
build" below is the white point alone, as 0.11 shipped it.

## The rule

Whites down is the earlier build's white point (a power on the luminance about the foot of the frame's span), then a roll that brings what the white point leaves over white back under it, in order. The roll makes its room in a band under white, so that band darkens on any frame, with lights over white or none.

On the brightest channel, `t` stops from white, with `d = -whites`:

    reach = d^3 / (d^2 + 0.1^2)            ~d from a half; no value or slope at zero
    knee  = -s * reach / (1 - s)           s = 0.2: a quarter of the reach under white
    pull  = (1 - s) (ramp(t - knee + w) - ramp(t - reach + w))      w = 0.1
    ramp(z) = 0, z^2 / 4w, z - w           for z <= 0, < 2w, past it

One gain, `2^-pull`, goes on all three channels.

- Nothing changes under `knee - w`. There the picture is the white point's to the bit.
- Over the knee, under white, the roll darkens what the earlier build left there. A grey the earlier build showed at 254 comes out 0.13, 0.48, 1.8, 5, and 10.9 levels darker at Whites −0.25, −0.5, −1, −2, and −3.5 (Channels; AgX 0.64, 1.6, 4.2, 9.4, and 17.8). The change starts where the earlier build showed 253, 252, 250, 246, and 237 (AgX 251, 250, 246, 239, and 228). Measured on a grey pixel through `shape_rolled`, roll off and on, then the display curve, solved for the pixel the earlier build shows at 254. On 27 real frame and setting pairs with no pixel at white, the render still changed.
- Between the knee and the reach the slope is `s`, and the light at the reach lands on white (within 0.02 stop, at the rounded corner).
- Past the reach the slope is one again and the pull is its most, the reach. A light the roll cannot bring under white stays over white, and the pull falls to nothing with `d`.

In the scene, Whites brings these stops over white under it:

| Whites | the white point alone (the earlier build) | with the roll |
|---|---|---|
| −0.5 | 0.5 | 0.99 |
| −1 | 1 | 2.18 |
| −2 | 2 | 4.89 |

The function is `whites_roll_pull` in `auto.rs`. The finish (`whites_roll`), the shader, and Auto's model all read it.

## Why the frame-top shoulder was dropped

Three trial builds hung a shoulder from the frame's top. Each failed on one number:

- **A rolled the pixel but read its region unrolled.** More Whites down brightened pixels, by up to a stop on a merge (67,215 of 1.33 M sweep steps).
- **B slid the shoulder with the top.** A hotspot's lead over its skin came back up to 4.9 L* over A on the synthetic face.
- **C hung the shoulder from the top, held three stops under white.** The top was flattened: bright textured cells kept a median 0.85 of the earlier build's spread at Whites −1 (10th percentile 0.60), and the moon kept 42%.

A shoulder from the top compresses whatever sits there, so it tames a hotspot and flattens a subject in one act. The roll compresses only a band under white, a quarter of its reach deep: the least room that lets lights over white land in order. Pushing the knee to white would leave them no room at all.

## Slope 0.2

The knee's place is the whole trade. A lower slope puts the knee nearer white, so fewer real cells sit in its band, but the lights it brings under white land closer together and flatter.

At 0.2, on the 54 cells that sit at white at the default, the median keeps 0.97 of the earlier build's spread at −1 (10th percentile 0.91). Cells under white keep all of it.

## Auto

Auto's sliders are the earlier build's. It still sets Whites by the white point alone: the frame's top a tenth under white, and never more at white than the default. The roll then takes the top further under white, by 0.02, 0.10, 0.28, and 0.31 stops at Whites −0.45, −0.92, −1.84, and −2, so the top lands the earlier build to 0.42 under white. It also brings the lights over the top under white. `whites_lands` is the model of where the top lands, and a test holds it to the render to a thousandth of a stop.

We tried two other aims:

- **Through the roll, a tenth under white.** Whites came out shallower: −0.56 against −0.74 on the sun frame. But the top sat in the roll's band, and a bright subject showed flat white: on a merge whose subject was a stop over white, 3% of the frame sat at white, where the earlier build held it.
- **Under the roll's band.** This aim showed the least white: the 99.5th percentile sat at white on 10 of 84 synthetic tops, against 39. But it pulled Whites deeper than the earlier build, and on the engine's merge test it took 1.09 stops off the exposure.

Kept at the earlier build's, Auto leaves less at white than the earlier build:

| frame | at white, the earlier build | at white, with the roll |
|---|---|---|
| the sun frame | 3.6% | 3.4% |
| the lanterns | 6.0% | 5.75% |
| the moon | 0.19% | 0.15% |

On the 84 synthetic tops, the 99.5th percentile sits at white in 11 cases against 39.

## The costs

- **The upper midtones stay the earlier build's.** Whites −2 still pulls the 75th percentile 0.40 and the 90th 0.54. C's relief there came from the flattening.
- **The knee's band.** Over the knee's band, up to the reach, light rolls at slope 0.2, then through the display curve's shoulder. The lights it brings back have little spread: about 0.05 to 0.06 eight-bit levels on a textured synthetic patch, against none clipped. Cells straddling white keep 0.97 of their spread at −1.
- **Saturated lights.** They are rolled by their brightest channel, so a red at white luminance comes back darker and more saturated than the earlier build's (Oklab C 0.21 against 0.13 at −1; hue 27° against 35°). A saturated red or orange outside sRGB keeps a clipped channel in every build, from the gamut, not the roll.
- **The band under white.** It darkens on every Whites-down edit, by the numbers in the rule.
- **The cache.** Every Whites-down edit is made again once (`whites-3`). The key cannot read pixels, so it cannot tell the frames whose picture changes only in the band from those with lights over white.
- **A built-in preset.** `warm-negative` sets Whites −0.1, so it now rolls slightly: about 0.03 stops at white. We do not exempt it, since a preset is an edit like any other.

## Tests

- `whites_zero_and_up_is_the_white_point_alone_to_the_bit`
- `what_lands_under_the_rolls_band_is_the_white_points_to_the_bit`
- `lights_come_under_white_in_order_to_the_reach` (reach, order, saturated channels, and Auto's model against the render to 1e-3)
- `a_textured_patch_keeps_its_spread_under_white_and_gains_it_over`
- `the_roll_holds_the_hue_and_never_brightens_a_color`
- `more_whites_down_never_brightens_a_pixel_through_the_finish`, and its sibling under Blacks, Contrast, Exposure, and a mask
- `a_brighter_flat_region_never_renders_darker`, `no_pixel_folds_within_a_region_under_the_whites`
- `every_slider_and_the_exposure_move_the_shape_continuously`
- `the_roll_has_no_kink_in_the_pixel_or_in_whites` (one-sided slopes within 2%)
- `no_plane_is_the_planed_path_on_a_flat_region`, `the_dropper_reads_what_the_render_shows_on_a_flat_region`
- `a_masks_whites_down_rolls_as_the_pictures_does`
- `a_merge_never_steps_as_the_whites_and_the_exposure_cross_its_white`
- Auto: `whites_down_lands_the_top_through_the_roll`, `the_whites_roll_holds_its_shape`
- The key: `the_whites_shape_is_keyed_only_where_whites_is_down`
- The GPU, on the card and lavapipe: `the_whites_roll_on_the_gpu_is_the_cpus`, `a_merges_pull_on_the_gpu_is_the_cpus`, `the_sliders_in_either_order_draw_what_a_fresh_render_draws`

Every rule was taken out on a scratch copy, one at a time, and a test failed for each.

## Limits

- The reach is a scene reach. In eight-bit output the lights it brings in land within a few hundredths of a stop of white, about one to three levels under 255.
- Whites at zero and up is the earlier build's.
- Highlights is the earlier build's, ease and all.

## What remains of issue 132

Highlights at Whites zero: a hotspot's lead over its skin still grows as Highlights comes down, by +3.4 L* at the median on the 108-face grid. Measured, it splits: +2.6 is the display shoulder (one gain moves both down to where the curve is steeper), +0.1 the ramp's ease (up to +5 on large faces). A region reach over white for Highlights was measured and not built. Hotspots are Whites' to tame, not Highlights': with Whites down the roll shrinks the lead on 18 to 37 of the 108 faces and grows it on at most 2.

## The Lightroom importer

Whites maps ±100 onto ±0.75, a scale fitted with the white point alone. Up is unchanged.

Down, every frame imports a little darker in the band under white, and a frame with lights over white now brings them under white, as Lightroom's own Whites down recovers them. The scale was fitted on the earlier build's render and is not refitted: a known limit. The negative side is worth a refit with the fit tool on frames with clipped highlights, perhaps as a scale of its own.
