# §659. A mask's Opacity (2026-11-12)

Asked on issue 141: one slider that pulls a whole mask back, 70 % of
its look in one move, the sliders left as they are, as Nik Color
Efex's per-filter opacity does.

**On the adjustment, not the mask or the look.** `Adjustment::opacity`,
0 to 1. The mask says where and the look says what; the opacity is
how much of the one over the other. On the mask, it would turn with
the shapes and be read by the overlay, which must not change. On the
look, it would ride along when a look is applied to a mask, and the
picture's global look, which shares the type, has no opacity. On the
adjustment, a copied, pasted, synced, preset, or duplicated mask
carries it, and a look laid onto a mask keeps it. Read through
`Adjustment::strength`, held to 0 to 1, whole for a value that is
not a number.

**What it scales.** The weight the finish blends a mask's look in by:
the mask's value after the invert, the feather, and Refine edge, times
the opacity, on the CPU (`finish_reaching`) and in the shader (packed
into the local's spare `tone.y`, read where the weight is). The mask's
own value is untouched, so the overlay, the scopes' Selection, and
Refine edge's raster are as they were, and an inverted mask at half is
the inverted region at half strength. Every op a look carries already
goes on as a blend of its parameters by that weight, so each is
scaled the same way: exposure and the tone sliders, contrast's excess
over one, the white balance's matrix, the point and color curves (and
the grading baked into them), the mixer's bands, saturation and
vibrance, the tint's vector, and the Detail amounts (Texture, Clarity,
Structure, Dehaze, Sharpen), which pass through their soft limit after
the sum. So 50 % is exactly what the feather's half-way already is:
half of each slider, not the mean of the two pictures. A true mix of
the two outputs would take the finish twice per pixel; nothing in the
pipeline reads a mask's weight in a way that scaling it does not
scale, so we kept the one rule.

At 1 the weight is multiplied by one, so the output is the bit it
was, CPU and GPU. At 0 nothing is pushed for the mask at any pixel,
and the two places a mask's look reaches past the finish, the Detail
bands it asks the develop for (`Edit::detail_wants`) and the guide
plane it asks the export for (`export::reads_guide`), treat it as
switched off, so 0 is the mask switched off to the bit. The mask is
still made, refined, and shown at 0, as a switched-off one is.

**Compatibility.** Whole is left out of the sidecar, so every sidecar
and preset written before it reads and writes as it did, and its
develop key is unchanged; any other value is written and moves the
key, as a look slider does. No schema bump: a new field with a default
(§117's rule).

**The panel.** One slider, Opacity, at the foot of the mask's card
under Refine edge, 0 to 100 %, reset to 100 % on a double-click. It
counts as a look control: it hides the overlay as a look slider does,
records "Mask: Opacity" in the history, and coalesces as the other
sliders do. The msgid is the watermark's and the brush's "Opacity".
The row adds 36 px to the MASK section (28 px and the layout's 8 px
spacing), so at a 1000 px window its shape buttons are reached by
scrolling: there the panel's view is 399 px and the section overruns
it by 50 px. Moving Opacity hides the overlay as a look slider does,
where Refine edge leaves it on; the overlay and the scopes' Selection
show the mask's full coverage at any opacity. A mask is no longer
"fresh" (taken back with an abandoned first shape) once its Opacity
or its Refine edge is set.

What 50 % is in numbers: against a true 50/50 mix of the full and the
switched-off pictures, the weight at half differs by at most 9.8 of
255 (mean 2.3) on a mask of contrast 1.8 and a curve, 29.4 (mean 7.3)
with exposure +1.5 added, and 7.6 (mean 2.5) on exposure alone, since
the display curve is not linear.

**Left alone.** The brush's own Opacity (its flow). No importer maps
a local amount: Lightroom's masks are not imported at all, and pp3
has no locals.
