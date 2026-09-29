# 142. Vignetting held at the edge of its calibration (2026-09-22)

**What lensfun's radius is.** lensfun's `pa` vignetting model,
1 + k1 r² + k2 r⁴ + k3 r⁶, puts r = 1 at the corner of the sensor the
calibration was made on, half its diagonal whatever its shape:
`modifier.cpp` says so ("For the vignetting model "pa", r = 1 is the
corner of the image"), and `mod-color.cpp` rescales by
`hypot(36, 24) / cropfactor / 2`. Distortion and TCA use Hugin's unit,
half the shorter side, instead. `vignetting_scale`, the lens's crop
factor over the body's, is that ratio of half diagonals, so a
picture's corner lands at r = `vignetting_scale`.

**Past the corner the fit is not a fit.** The polynomial is fitted to
what the calibration sensor saw, which stops at r = 1. A full-frame
lens on a GFX (crop 0.79 against 1.005) reaches r = 1.27, and the Sigma
50mm f/1.4 Art at f/5.6 falls to 0.71 at r = 1, climbs back to 0.79 at
1.2 and to 1.10 at 1.37: the correction brightened the GFX's extreme
corners less than the full-frame corner, and would darken them further
out, where they are the darkest part of the frame. Not rare: 10,198 of
the database's 29,594 `pa` entries, on 599 lenses, rise somewhere
between r = 1 and 1.4. lensfun itself evaluates the polynomial at any
r.

**Held, not extrapolated.** `Vignetting::falloff` takes min(r, 1), so
past the calibration's corner the gain is the gain at it. That still
under-corrects a real lens, whose light keeps falling, but it never
corrects less towards the corner and never invents a brightening the
data does not hold; extrapolating the slope would be a guess that grows
with every step past the data. There is no GPU lens path to match: the
correction runs on the CPU for the CLI and the editor alike. `greycard
lenses` prints a `covers` line when the calibration's sensor is more
than a percent smaller than the body's (the margin so a full-frame lens
calibrated at 1.005 is not flagged on a 1.0 body), the develop logs it,
and the LENS panel puts "measured on a smaller sensor" after the
profile's name.

**Distortion and CA are left alone.** Their calibrated range ends at
the calibration's corner too, 1.80 half shorter sides for 3:2, and a
full-frame calibration on a 44 by 33 sensor reaches 2.28. But holding
a displacement at a radius puts a kink in the geometry, and the
polynomials carry on smoothly: of 6,450 `poly3`, `poly5` and `ptlens`
entries, 7 turn back before 2.28, fisheyes, phones and two wide zooms
whose circle would not cover a bigger sensor anyway, and no TCA
channel turns back before 3. The frame that raised this is a different
matter: lensfun's calibration of this Sigma, made on a Canon 6D, has a
distortion `k1` of exactly zero, so greycard corrects none where
Adobe's profile straightens enough to crop the export by 6 percent.
That is lensfun's data to fix, not a reason to guess here.

**Measured.** On the GFX 100S II frame at f/5 the corner gain goes from
1.24 to 1.44, +0.21 EV at the tip; before and after agree out to 79
percent of the way to the corner, where the old gain peaked at 82
percent and fell 14 percent by the tip. At patches 168 px in from each
corner, about r = 1.23, the correction's own gain went from 1.31 to
1.44, which is the model's 1.309 there before and its held 1.437 after.
