# §365. The grading wheels at two thirds, until fitted (2026-10-18)

The first catalog brought in from a shoot graded in Lightroom showed
its warm highlights well past Lightroom's: the top of the histogram
went flat yellow, and each frame had to be dialed back by hand to
match. The mapper read the wheels one to one, a saturation of 100
onto a wheel at 1, which is a shift of 0.2 in Oklab's a and b, about
the chroma of a saturated yellow; and our weights hand the highlights
wheel all of a pixel at white, where Lightroom fades. Every Basic
slider, the mixer's bands and the curve had a scale fitted from
exports (§342); the wheels had none, so their units were a guess.

The wheels get a scale of their own in the mapper's table, `grading`,
shipped at two thirds by eye. It is a scale and not a multiplier in
the code so that the fit, when it comes, replaces a number. The fit
wants exports with only grading set, one with the highlights at a
known hue and saturation, one with the shadows, and a flat frame or
a ramp to pin the lightness curve against Lightroom's Blending and
per-wheel Luminance, which we do not read; it is on the roadmap with
the other scales that wait on exports. Frames imported before this
keep the sidecars they got, and take the new scale only on a forced
import.
