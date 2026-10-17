# §325. Lightroom's tint came in backwards (2026-10-16)

The review of the RawTherapee import (in build) checked both
importers' white balance against their sources and found the Lightroom
mapper (§52) wrong on master: `lightroom.rs` negated Lightroom's Tint
and scaled it by 0.0002, on the belief that a positive Tint there means
magenta where our positive Duv means green.

Both name the light, not the picture. Our Tint slider is Duv, positive
above the locus, a green white point that renders the picture magenta,
and the panel's track says so. Lightroom's is the same: the DNG SDK's
`dng_temperature.cpp` computes `fTint` with `kTintScale = -3000` along
a direction below the locus, so a green white point gives a positive
Tint, and Adobe's Daylight and Fluorescent presets carry positive tints
for that reason. So Tint ≈ 3000 × Duv, and the mapping is `tint =
Tint / 3000`: the old one had the wrong sign and about 0.6 of the size.

A preset with a Tint of +15 lands at a Duv of +0.005 now, where it
landed at −0.003 since 0.1.0. The fit of §323 will check the scale
against the archive's exports; the sign is settled by the SDK.
