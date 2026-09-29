# 26. Color grading (2026-09-07)

The second color item: three wheels, for the shadows, the mid-tones
and the highlights, in a COLOR GRADING section after the mixer.
`crates/greycard-edit/src/grading.rs` holds the edit and its meaning.

**What it does.** Each wheel is an Oklab hue and a strength, 0 to 1.
A pixel at lightness `L` is split among the three by weights that
sum to one, `(1-t)²`, `2t(1-t)` and `t²` with `t = L` (the lift,
gamma and gain partition, in effect: black is all the shadows',
white all the highlights', the middle half the mid-tones'), and each
wheel adds its weight times its strength times `RANGE` (0.2, a
color curve's edge, §25) in its hue's direction to `a` and `b`.
Balance, -1 to 1, is a power on `L` before the split, `t = L^(2^-b)`,
so positive balance hands more of the picture to the highlights
wheel and negative to the shadows', with no bend in the weights.
Lightness is held, as with the curves.

**One table.** The grading composes with the color curves by
adding into the same 256-entry color table: `Curves::bake_with(&
Grading)` sums the two shifts at each `L`, and `bake()` is the same
with no grading. Nothing else changed downstream: the shader, the
CPU finish, the histogram's pass and the export all read the table
they already read, and the curves' on/off switch leaves the grading
in. `Edit` has a `grading` field with serde's default, no schema
bump.

**The wheels.** Three pictures drawn on the CPU, 96 pixels square
with an alpha edge: Oklab's hues around, chroma outward to 0.13 at
lightness 0.72, so the wheel shows the direction a point sends
things, and the point itself as a white dot with a dark rim. They are
`CurveEditor`s with a transparent ground: the control already maps
the pointer to 0..1 with y up, and the window turns that into an
angle and a distance. A press or drag sets the wheel's hue and
strength from the pointer, with a dead zone of four percent of the
radius where the strength is nothing and the hue is left as it was,
so a wheel put back to the center does not forget its hue; release
records one history entry. The wheel last touched is the one the
Hue and Strength sliders show and act on, its label brightened;
Balance is a slider of its own. `GREYCARD_UI_WHEEL=FILE` dumps the
last wheel drawn.

**Checked.** With blue shadows and yellow-orange highlights in a
sidecar, balance a fifth toward the highlights, the viewport at 1:1
and the export's crop differ by 0.05 percent RMSE (the odd-height
crop of §25 in the sidecar), by 7 percent from the ungraded picture.
The wheel's picture looked at; the point sits where the hue and
strength say.

**Not yet.** A luminance per wheel, as Lightroom has, is a gain by
the same weights and would be a line each in `shift_at`'s
neighbor and the finish; not built until someone misses it, since
the tone sliders (§19) cover lightness by range. Grading through a
mask is the mask infrastructure's business, next.
