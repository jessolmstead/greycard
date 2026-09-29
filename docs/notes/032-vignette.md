# 32. Vignette (2026-09-07)

Our list had it. A vignette is an exposure falloff over the
frame as shown, in stops, so darkening keeps its hues as the exposure
slider does and the same op brightens the corners when asked; it sits
on the crop and any size of export, since its positions are fractions
of the frame. `Vignette { amount, midpoint, feather, roundness }` in
greycard-edit, global (Lightroom's post-crop vignette is too), with
`Vignette::at(u, v, aspect)` the weight, 0 inside the midpoint and 1
past the feather: the frame's ellipse at roundness 0, stretched by
the aspect's power towards a circle at 1, a superellipse towards a
rounded rectangle at −1, the distance scaled so the corner is 1 and a
smoothstep from the midpoint over the feather. `finish_pixel_with`
takes the stops for a pixel and adds them to the exposure before
everything else; the shader's `vignette_at` is the same maths on the
frame position it already had. The check of §18 with the falloff
inside the 1:1 view agrees to 0.06%. Four sliders in their own
section; the histogram sees it, since the analysis draw is the same
shader over the frame. Not in masks: a vignette is a place in the
frame, not a look.

### The wheels, enlarged, and put back by a double click

Two things we asked for on §26's wheels. A wheel's name is now a
button: it enlarges that wheel to the panel's width above the row,
for a finer hand on hue and strength, and the name again puts it
back; the three pictures live in one model and the enlarged one is
drawn at its size (320 pixels across, the dot scaled with it), not
scaled up. A double click on any wheel, enlarged or not, sets it to
nothing, as a double click on a slider does, and records the edit.
